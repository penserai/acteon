//! Versioned `ClickHouse` projection. Mutable filters run only after FINAL resolution.
use crate::{
    Error, Result, TimelineEvent, TimelinePage, TimelineQuery, TimelineScope, TimelineStore,
    predicate::Predicate,
    search::{PreparedQuery, SearchFields},
    store::Projection,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
pub struct ClickHouseTimelineStore {
    client: reqwest::Client,
    url: String,
    table: String,
}
fn literal(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "\\'"))
}
fn compile_predicate(predicate: &Predicate) -> String {
    match predicate {
        Predicate::Constant(value) => u8::from(*value).to_string(),
        Predicate::All(nodes) if nodes.is_empty() => "1".into(),
        Predicate::Any(nodes) if nodes.is_empty() => "0".into(),
        Predicate::All(nodes) | Predicate::Any(nodes) => format!(
            "({})",
            nodes
                .iter()
                .map(compile_predicate)
                .collect::<Vec<_>>()
                .join(if matches!(predicate, Predicate::All(_)) {
                    " AND "
                } else {
                    " OR "
                })
        ),
        Predicate::Not(node) => format!("(NOT {})", compile_predicate(node)),
        Predicate::Term(field, value) => {
            let column = field.column();
            let value = literal(value);
            if field.is_array() {
                format!("has({column},{value})")
            } else {
                format!("{column}={value}")
            }
        }
        Predicate::Time(op, value) => format!(
            "occurred_at{}parseDateTime64BestEffort({},9)",
            op.sql(),
            literal(&value.to_rfc3339())
        ),
    }
}
impl ClickHouseTimelineStore {
    pub async fn new(client: reqwest::Client, url: &str, table: &str) -> Result<Self> {
        if table.is_empty()
            || !table
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || table.as_bytes()[0].is_ascii_digit()
        {
            return Err(Error::Invalid("invalid timeline table".into()));
        }
        let store = Self {
            client,
            url: url.into(),
            table: table.into(),
        };
        store.execute(&format!("CREATE TABLE IF NOT EXISTS {table} (namespace String, tenant String, source String, event_id String, revision UInt64, expires_at DateTime64(9, 'UTC'), deleted UInt8, occurred_at DateTime64(9, 'UTC'), event_group String, event_type String, exact Array(String), paths Array(String), tokens Array(String), keyed_tokens Array(String), event String) ENGINE = ReplacingMergeTree(revision) PARTITION BY cityHash64(namespace,tenant,source,event_id) % 64 ORDER BY (namespace,tenant,source,event_id)" )).await?;
        Ok(store)
    }
    async fn execute(&self, sql: &str) -> Result<String> {
        Ok(self
            .client
            .post(&self.url)
            .query(&[
                ("date_time_input_format", "best_effort"),
                ("output_format_json_quote_64bit_integers", "0"),
            ])
            .body(sql.to_owned())
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }
    async fn rows(&self, sql: &str) -> Result<Vec<TimelineEvent>> {
        let response = self.execute(&format!("{sql} FORMAT JSONEachRow")).await?;
        response
            .lines()
            .filter(|s| !s.is_empty())
            .map(|line| {
                let row: Value = serde_json::from_str(line)?;
                serde_json::from_str(
                    row["event"]
                        .as_str()
                        .ok_or_else(|| Error::Storage("invalid ClickHouse event".into()))?,
                )
                .map_err(Error::from)
            })
            .collect()
    }
}
#[async_trait]
impl TimelineStore for ClickHouseTimelineStore {
    async fn apply(&self, p: &Projection) -> Result<()> {
        let fields = p
            .event
            .as_ref()
            .map(|e| SearchFields::from_event(&e.event))
            .transpose()?
            .unwrap_or_default();
        let row = json!({"namespace":p.scope.namespace,"tenant":p.scope.tenant,"source":p.source,"event_id":p.event_id,"revision":p.revision,
            "expires_at":p.expires_at.to_rfc3339(),"deleted":u8::from(p.event.is_none()),"occurred_at":p.event.as_ref().map_or(p.expires_at,|e|e.event.occurred_at).to_rfc3339(),
            "event_group":p.event.as_ref().map_or("",|e|e.event.event_group.as_str()),"event_type":p.event.as_ref().map_or("",|e|e.event.event_type.as_str()),
            "exact":fields.exact,"paths":fields.paths,"tokens":fields.tokens,"keyed_tokens":fields.keyed_tokens,"event":serde_json::to_string(&p.event)?});
        self.execute(&format!(
            "INSERT INTO {} FORMAT JSONEachRow\n{row}",
            self.table
        ))
        .await?;
        Ok(())
    }
    async fn get(
        &self,
        scope: &TimelineScope,
        source: &str,
        event_id: &str,
    ) -> Result<Option<TimelineEvent>> {
        let sql = format!(
            "SELECT event FROM (SELECT * FROM {} FINAL WHERE namespace={} AND tenant={} AND source={} AND event_id={}) WHERE deleted=0 AND expires_at>now64(9) LIMIT 1",
            self.table,
            literal(&scope.namespace),
            literal(&scope.tenant),
            literal(source),
            literal(event_id)
        );
        Ok(self.rows(&sql).await?.pop())
    }
    async fn query(&self, scope: &TimelineScope, q: &TimelineQuery) -> Result<TimelinePage> {
        let p = PreparedQuery::new(scope, q)?;
        let mut filters = vec!["deleted=0".into(), "expires_at>now64(9)".into()];
        filters.push(compile_predicate(&p.predicate));
        if let Some(c) = &p.cursor {
            filters.push(format!(
                "(occurred_at,source,event_id) {} (parseDateTime64BestEffort({},9),{},{})",
                if p.ascending { ">" } else { "<" },
                literal(&c.occurred_at.to_rfc3339()),
                literal(&c.source),
                literal(&c.event_id)
            ));
        }
        let dir = if p.ascending { "ASC" } else { "DESC" };
        let sql = format!(
            "SELECT event FROM (SELECT * FROM {} FINAL WHERE namespace={} AND tenant={}) WHERE {} ORDER BY occurred_at {dir},source {dir},event_id {dir} LIMIT {} SETTINGS optimize_move_to_prewhere=0,optimize_move_to_prewhere_if_final=0",
            self.table,
            literal(&scope.namespace),
            literal(&scope.tenant),
            filters.join(" AND "),
            p.limit + 1
        );
        Ok(p.page(self.rows(&sql).await?))
    }
    async fn cleanup(&self, now: DateTime<Utc>) -> Result<u64> {
        // A mutation clears every expired version; retained rows still fence old writers.
        self.execute(&format!("ALTER TABLE {} UPDATE event='null',deleted=1,exact=[],paths=[],tokens=[],keyed_tokens=[],event_group='',event_type='' WHERE expires_at<=parseDateTime64BestEffort({},9) AND deleted=0",self.table,literal(&now.to_rfc3339()))).await?;
        Ok(0) // ClickHouse mutations do not report a synchronous affected-row count.
    }
}
