use async_trait::async_trait;
use chrono::Utc;

use acteon_audit::error::AuditError;
use acteon_audit::record::{AuditPage, AuditQuery, AuditRecord};
use acteon_audit::store::AuditStore;

use crate::config::ElasticsearchAuditConfig;

/// Elasticsearch-backed audit store using the REST API via `reqwest`.
///
/// Documents are indexed into a single Elasticsearch index whose name is
/// derived from [`ElasticsearchAuditConfig::index_name`]. The index mapping
/// is created automatically on construction if it does not already exist.
pub struct ElasticsearchAuditStore {
    client: reqwest::Client,
    base_url: String,
    index: String,
    username: Option<String>,
    password: Option<String>,
}

impl ElasticsearchAuditStore {
    /// Create a new store, optionally configured with basic authentication.
    ///
    /// This constructor builds the HTTP client, resolves the index name from
    /// the provided configuration, and ensures the index exists with the
    /// correct mapping.
    pub async fn new(config: &ElasticsearchAuditConfig) -> Result<Self, AuditError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let base_url = config.url.trim_end_matches('/').to_owned();
        let index = config.index_name();

        let store = Self {
            client,
            base_url,
            index,
            username: config.username.clone(),
            password: config.password.clone(),
        };

        store.ensure_index().await?;
        Ok(store)
    }

    /// Build a [`reqwest::RequestBuilder`] for the given method and path,
    /// applying basic authentication when credentials are configured.
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}/{path}", self.base_url);
        let mut req = self.client.request(method, &url);
        if let Some(ref user) = self.username {
            req = req.basic_auth(user, self.password.as_deref());
        }
        req
    }

    /// Create the Elasticsearch index with the expected mapping if it does not
    /// already exist.
    ///
    /// A `400 Bad Request` response containing
    /// `resource_already_exists_exception` is treated as success.
    async fn ensure_index(&self) -> Result<(), AuditError> {
        let mapping = serde_json::json!({
            "mappings": {
                "properties": {
                    "id":               { "type": "keyword" },
                    "action_id":        { "type": "keyword" },
                    "chain_id":         { "type": "keyword" },
                    "namespace":        { "type": "keyword" },
                    "tenant":           { "type": "keyword" },
                    "provider":         { "type": "keyword" },
                    "action_type":      { "type": "keyword" },
                    "verdict":          { "type": "keyword" },
                    "matched_rule":     { "type": "keyword" },
                    "outcome":          { "type": "keyword" },
                    "action_payload":   { "type": "object", "enabled": false },
                    "verdict_details":  { "type": "object", "enabled": false },
                    "outcome_details":  { "type": "object", "enabled": false },
                    "metadata":         { "type": "object", "enabled": false },
                    "dispatched_at":    { "type": "date" },
                    "completed_at":     { "type": "date" },
                    "duration_ms":      { "type": "long" },
                    "expires_at":       { "type": "date" },
                    "caller_id":        { "type": "keyword" },
                    "auth_method":      { "type": "keyword" },
                    "record_hash":      { "type": "keyword" },
                    "previous_hash":    { "type": "keyword" },
                    "sequence_number":  { "type": "long" }
                }
            }
        });

        let resp = self
            .request(reqwest::Method::PUT, &self.index)
            .json(&mapping)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        // 200/201 = created, 400 with "resource_already_exists_exception" = OK
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::BAD_REQUEST {
            tracing::debug!(index = %self.index, "elasticsearch index ensured");
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(AuditError::Storage(format!(
                "failed to create index '{}': {body}",
                self.index
            )))
        }
    }
}

// ---------------------------------------------------------------------------
// Elasticsearch response types (internal)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct GetResponse {
    #[serde(rename = "_source")]
    source: AuditRecord,
    found: bool,
}

#[derive(serde::Deserialize)]
struct SearchResponse {
    hits: SearchHits,
}

#[derive(serde::Deserialize)]
struct SearchHits {
    total: HitsTotal,
    hits: Vec<SearchHit>,
}

#[derive(serde::Deserialize)]
struct HitsTotal {
    value: u64,
}

#[derive(serde::Deserialize)]
struct SearchHit {
    #[serde(rename = "_source")]
    source: AuditRecord,
}

#[derive(serde::Deserialize)]
struct DeleteByQueryResponse {
    deleted: u64,
}

// ---------------------------------------------------------------------------
// AuditStore implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl AuditStore for ElasticsearchAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let path = format!("{}/_doc/{}", self.index, entry.id);

        let resp = self
            .request(reqwest::Method::PUT, &path)
            .json(&entry)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        if resp.status().is_success() {
            tracing::debug!(id = %entry.id, "audit record indexed");
            Ok(())
        } else {
            let body = resp.text().await.unwrap_or_default();
            Err(AuditError::Storage(format!(
                "failed to index audit record: {body}"
            )))
        }
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let path = format!("{}/_search", self.index);

        let body = serde_json::json!({
            "query": {
                "term": { "action_id": action_id }
            },
            "sort": [{ "dispatched_at": "desc" }],
            "size": 1
        });

        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AuditError::Storage(format!(
                "search by action_id failed: {text}"
            )));
        }

        let search: SearchResponse = resp
            .json()
            .await
            .map_err(|e| AuditError::Serialization(e.to_string()))?;

        Ok(search.hits.hits.into_iter().next().map(|h| h.source))
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let path = format!("{}/_doc/{id}", self.index);

        let resp = self
            .request(reqwest::Method::GET, &path)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AuditError::Storage(format!("get by id failed: {text}")));
        }

        let get_resp: GetResponse = resp
            .json()
            .await
            .map_err(|e| AuditError::Serialization(e.to_string()))?;

        if get_resp.found {
            Ok(Some(get_resp.source))
        } else {
            Ok(None)
        }
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();
        let offset = query.effective_offset();

        let es_query = build_es_query(query);

        let body = serde_json::json!({
            "query": es_query,
            "sort": [{ "dispatched_at": "desc" }],
            "size": limit,
            "from": offset,
            "track_total_hits": true
        });

        let path = format!("{}/_search", self.index);

        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AuditError::Storage(format!("query failed: {text}")));
        }

        let search: SearchResponse = resp
            .json()
            .await
            .map_err(|e| AuditError::Serialization(e.to_string()))?;

        let records = search.hits.hits.into_iter().map(|h| h.source).collect();

        Ok(AuditPage {
            records,
            total: search.hits.total.value,
            limit,
            offset,
        })
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        let now = Utc::now().to_rfc3339();

        let body = serde_json::json!({
            "query": {
                "bool": {
                    "filter": [
                        { "exists": { "field": "expires_at" } },
                        { "range": { "expires_at": { "lte": now } } }
                    ]
                }
            }
        });

        let path = format!("{}/_delete_by_query", self.index);

        let resp = self
            .request(reqwest::Method::POST, &path)
            .json(&body)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(AuditError::Storage(format!(
                "cleanup_expired failed: {text}"
            )));
        }

        let result: DeleteByQueryResponse = resp
            .json()
            .await
            .map_err(|e| AuditError::Serialization(e.to_string()))?;

        tracing::info!(deleted = result.deleted, "expired audit records cleaned up");
        Ok(result.deleted)
    }
}

// ---------------------------------------------------------------------------
// Query builder helpers
// ---------------------------------------------------------------------------

/// Build an Elasticsearch query JSON value from an [`AuditQuery`].
///
/// String filters become `term` clauses inside `bool.must`, while date range
/// filters become `range` clauses inside `bool.filter`.
fn build_es_query(query: &AuditQuery) -> serde_json::Value {
    let mut must_clauses: Vec<serde_json::Value> = Vec::new();
    let mut filter_clauses: Vec<serde_json::Value> = Vec::new();

    // String equality filters.
    let fields: &[(&Option<String>, &str)] = &[
        (&query.namespace, "namespace"),
        (&query.tenant, "tenant"),
        (&query.provider, "provider"),
        (&query.action_type, "action_type"),
        (&query.outcome, "outcome"),
        (&query.verdict, "verdict"),
        (&query.matched_rule, "matched_rule"),
        (&query.caller_id, "caller_id"),
        (&query.chain_id, "chain_id"),
    ];

    for (value, field) in fields {
        if let Some(v) = value {
            must_clauses.push(serde_json::json!({ "term": { *field: v } }));
        }
    }

    // Date range filters.
    if let Some(from) = query.from {
        filter_clauses.push(serde_json::json!({
            "range": { "dispatched_at": { "gte": from.to_rfc3339() } }
        }));
    }
    if let Some(to) = query.to {
        filter_clauses.push(serde_json::json!({
            "range": { "dispatched_at": { "lte": to.to_rfc3339() } }
        }));
    }

    if must_clauses.is_empty() && filter_clauses.is_empty() {
        serde_json::json!({ "match_all": {} })
    } else {
        serde_json::json!({
            "bool": {
                "must": must_clauses,
                "filter": filter_clauses
            }
        })
    }
}
