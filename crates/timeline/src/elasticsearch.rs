//! Elasticsearch projection with fixed fields and external-version fencing.
use crate::{
    Error, Result, TimelineEvent, TimelinePage, TimelineQuery, TimelineScope, TimelineStore,
    predicate::{Comparison, Predicate},
    search::{PreparedQuery, SearchFields},
    store::{Projection, identity},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
pub struct ElasticsearchTimelineStore {
    client: reqwest::Client,
    base: String,
}
impl ElasticsearchTimelineStore {
    pub async fn new(client: reqwest::Client, url: &str, index: &str) -> Result<Self> {
        if index.is_empty()
            || !index
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
        {
            return Err(Error::Invalid("invalid timeline index".into()));
        }
        let base = format!("{}/{}", url.trim_end_matches('/'), index);
        let store = Self { client, base };
        let response = store.client.put(&store.base).json(&json!({"mappings":{"dynamic":"strict","properties":{
            "namespace":{"type":"keyword"},"tenant":{"type":"keyword"},"source":{"type":"keyword"},"event_id":{"type":"keyword"},
            "revision":{"type":"long"},"expires_at":{"type":"date_nanos"},"deleted":{"type":"boolean"},
            "occurred_at":{"type":"date_nanos"},"event_group":{"type":"keyword"},"event_type":{"type":"keyword"},
            "exact":{"type":"keyword"},"paths":{"type":"keyword"},"tokens":{"type":"keyword"},"keyed_tokens":{"type":"keyword"},
            "event":{"type":"object","enabled":false}
        }}})).send().await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if !status.is_success()
            && value.pointer("/error/type").and_then(Value::as_str)
                != Some("resource_already_exists_exception")
        {
            return Err(Error::Storage(format!(
                "timeline index creation returned {status}"
            )));
        }
        Ok(store)
    }
    async fn search(&self, body: Value) -> Result<Vec<TimelineEvent>> {
        let value: Value = self
            .client
            .post(format!("{}/_search", self.base))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        value
            .pointer("/hits/hits")
            .and_then(Value::as_array)
            .ok_or_else(|| Error::Storage("invalid search response".into()))?
            .iter()
            .map(|h| {
                serde_json::from_value(
                    h.pointer("/_source/event")
                        .cloned()
                        .ok_or_else(|| Error::Storage("missing event in search result".into()))?,
                )
                .map_err(Error::from)
            })
            .collect()
    }
}
fn term(field: &str, value: impl Into<Value>) -> Value {
    json!({"term":{field:value.into()}})
}
fn compile_predicate(predicate: &Predicate) -> Value {
    match predicate {
        Predicate::Constant(true) => json!({"match_all": {}}),
        Predicate::Constant(false) => json!({"match_none": {}}),
        Predicate::All(nodes) => {
            json!({"bool": {"filter": nodes.iter().map(compile_predicate).collect::<Vec<_>>()}})
        }
        Predicate::Any(nodes) if nodes.is_empty() => json!({"match_none": {}}),
        Predicate::Any(nodes) => {
            json!({"bool": {"should": nodes.iter().map(compile_predicate).collect::<Vec<_>>(), "minimum_should_match": 1}})
        }
        Predicate::Not(node) => json!({"bool": {"must_not": [compile_predicate(node)]}}),
        Predicate::Term(field, value) => term(field.column(), value.clone()),
        Predicate::Time(Comparison::Eq, value) => term("occurred_at", value.to_rfc3339()),
        Predicate::Time(op, value) => json!({"range": {"occurred_at": {op.elastic(): value}}}),
    }
}
#[async_trait]
impl TimelineStore for ElasticsearchTimelineStore {
    async fn apply(&self, p: &Projection) -> Result<()> {
        let fields = p
            .event
            .as_ref()
            .map(|e| SearchFields::from_event(&e.event))
            .transpose()?
            .unwrap_or_default();
        let body = json!({"namespace":p.scope.namespace,"tenant":p.scope.tenant,"source":p.source,"event_id":p.event_id,
            "revision":p.revision,"expires_at":p.expires_at,"deleted":p.event.is_none(),"event":p.event,
            "occurred_at":p.event.as_ref().map(|e|e.event.occurred_at),"event_group":p.event.as_ref().map(|e|&e.event.event_group),"event_type":p.event.as_ref().map(|e|&e.event.event_type),
            "exact":fields.exact,"paths":fields.paths,"tokens":fields.tokens,"keyed_tokens":fields.keyed_tokens});
        let response = self
            .client
            .put(format!("{}/_doc/{}", self.base, p.id()))
            .query(&[
                ("version", p.revision.to_string()),
                ("version_type", "external".into()),
            ])
            .json(&body)
            .send()
            .await?;
        // Equal/newer versions already persisted: the service guarantees unique content per revision.
        if response.status() != reqwest::StatusCode::CONFLICT {
            response.error_for_status()?;
        }
        Ok(())
    }
    async fn get(
        &self,
        scope: &TimelineScope,
        source: &str,
        event_id: &str,
    ) -> Result<Option<TimelineEvent>> {
        let response = self
            .client
            .get(format!(
                "{}/_doc/{}",
                self.base,
                identity(scope, source, event_id)
            ))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value: Value = response.error_for_status()?.json().await?;
        let event = value
            .pointer("/_source/event")
            .cloned()
            .unwrap_or(Value::Null);
        let event: Option<TimelineEvent> = serde_json::from_value(event)?;
        Ok(event.filter(|e| &e.scope == scope && e.expires_at > Utc::now()))
    }
    async fn query(&self, scope: &TimelineScope, q: &TimelineQuery) -> Result<TimelinePage> {
        let p = PreparedQuery::new(scope, q)?;
        let mut filters = vec![
            term("namespace", scope.namespace.clone()),
            term("tenant", scope.tenant.clone()),
            term("deleted", false),
            json!({"range":{"expires_at":{"gt":Utc::now()}}}),
        ];
        filters.push(compile_predicate(&p.predicate));
        let direction = if p.ascending { "asc" } else { "desc" };
        let mut body = json!({"size":p.limit+1,"track_total_hits":false,"query":{"bool":{"filter":filters}},"sort":[{"occurred_at":direction},{"source":direction},{"event_id":direction}]});
        if let Some(cursor) = &p.cursor {
            body["search_after"] = json!([cursor.occurred_at, cursor.source, cursor.event_id]);
        }
        Ok(p.page(self.search(body).await?))
    }
    async fn cleanup(&self, now: DateTime<Utc>) -> Result<u64> {
        let result: Value = self.client.post(format!("{}/_update_by_query",self.base)).query(&[("conflicts","proceed")]).json(&json!({
            "query":{"bool":{"filter":[{"range":{"expires_at":{"lte":now}}},term("deleted",false)]}},
            "script":{"lang":"painless","source":"ctx._source.event = null; ctx._source.deleted = true; ctx._source.exact = []; ctx._source.paths = []; ctx._source.tokens = []; ctx._source.keyed_tokens = []; ctx._source.event_group = null; ctx._source.event_type = null; ctx._source.occurred_at = null;"}
        })).send().await?.error_for_status()?.json().await?;
        Ok(result["updated"].as_u64().unwrap_or(0))
    }
}
