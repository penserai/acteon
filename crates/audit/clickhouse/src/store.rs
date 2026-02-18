use async_trait::async_trait;
use chrono::{DateTime, Utc};

use acteon_audit::error::AuditError;
use acteon_audit::record::{AuditPage, AuditQuery, AuditRecord};
use acteon_audit::store::AuditStore;

use crate::config::ClickHouseAuditConfig;
use crate::migrations;

// ---------------------------------------------------------------------------
// Row types for ClickHouse serde
// ---------------------------------------------------------------------------

/// Row layout used when inserting audit records into `ClickHouse`.
#[derive(clickhouse::Row, serde::Serialize)]
struct AuditInsertRow {
    id: String,
    action_id: String,
    chain_id: Option<String>,
    namespace: String,
    tenant: String,
    provider: String,
    action_type: String,
    verdict: String,
    matched_rule: Option<String>,
    outcome: String,
    /// JSON-serialised `serde_json::Value`.
    action_payload: Option<String>,
    /// JSON-serialised `serde_json::Value`.
    verdict_details: String,
    /// JSON-serialised `serde_json::Value`.
    outcome_details: String,
    /// JSON-serialised `serde_json::Value`.
    metadata: String,
    /// Milliseconds since the Unix epoch.
    dispatched_at: i64,
    /// Milliseconds since the Unix epoch.
    completed_at: i64,
    duration_ms: u64,
    /// Milliseconds since the Unix epoch, if set.
    expires_at: Option<i64>,
    caller_id: String,
    auth_method: String,
    record_hash: Option<String>,
    previous_hash: Option<String>,
    sequence_number: Option<u64>,
}

/// Row layout used when reading audit records from `ClickHouse`.
#[derive(clickhouse::Row, serde::Deserialize)]
struct AuditSelectRow {
    id: String,
    action_id: String,
    chain_id: Option<String>,
    namespace: String,
    tenant: String,
    provider: String,
    action_type: String,
    verdict: String,
    matched_rule: Option<String>,
    outcome: String,
    action_payload: Option<String>,
    verdict_details: String,
    outcome_details: String,
    metadata: String,
    dispatched_at: i64,
    completed_at: i64,
    duration_ms: u64,
    expires_at: Option<i64>,
    caller_id: String,
    auth_method: String,
    record_hash: Option<String>,
    previous_hash: Option<String>,
    sequence_number: Option<u64>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

impl From<AuditRecord> for AuditInsertRow {
    fn from(r: AuditRecord) -> Self {
        Self {
            id: r.id,
            action_id: r.action_id,
            chain_id: r.chain_id,
            namespace: r.namespace,
            tenant: r.tenant,
            provider: r.provider,
            action_type: r.action_type,
            verdict: r.verdict,
            matched_rule: r.matched_rule,
            outcome: r.outcome,
            action_payload: r
                .action_payload
                .map(|v| serde_json::to_string(&v).unwrap_or_default()),
            verdict_details: serde_json::to_string(&r.verdict_details).unwrap_or_default(),
            outcome_details: serde_json::to_string(&r.outcome_details).unwrap_or_default(),
            metadata: serde_json::to_string(&r.metadata).unwrap_or_default(),
            dispatched_at: r.dispatched_at.timestamp_millis(),
            completed_at: r.completed_at.timestamp_millis(),
            duration_ms: r.duration_ms,
            expires_at: r.expires_at.map(|dt| dt.timestamp_millis()),
            caller_id: r.caller_id,
            auth_method: r.auth_method,
            record_hash: r.record_hash,
            previous_hash: r.previous_hash,
            sequence_number: r.sequence_number,
        }
    }
}

impl From<AuditSelectRow> for AuditRecord {
    fn from(row: AuditSelectRow) -> Self {
        Self {
            id: row.id,
            action_id: row.action_id,
            chain_id: row.chain_id,
            namespace: row.namespace,
            tenant: row.tenant,
            provider: row.provider,
            action_type: row.action_type,
            verdict: row.verdict,
            matched_rule: row.matched_rule,
            outcome: row.outcome,
            action_payload: row
                .action_payload
                .and_then(|s| serde_json::from_str(&s).ok()),
            verdict_details: serde_json::from_str(&row.verdict_details)
                .unwrap_or(serde_json::Value::Null),
            outcome_details: serde_json::from_str(&row.outcome_details)
                .unwrap_or(serde_json::Value::Null),
            metadata: serde_json::from_str(&row.metadata).unwrap_or(serde_json::Value::Null),
            dispatched_at: millis_to_datetime(row.dispatched_at),
            completed_at: millis_to_datetime(row.completed_at),
            duration_ms: row.duration_ms,
            expires_at: row.expires_at.map(millis_to_datetime),
            caller_id: row.caller_id,
            auth_method: row.auth_method,
            record_hash: row.record_hash,
            previous_hash: row.previous_hash,
            sequence_number: row.sequence_number,
        }
    }
}

/// Convert milliseconds since the Unix epoch to a `DateTime<Utc>`.
fn millis_to_datetime(ms: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(ms).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// SQL helpers
// ---------------------------------------------------------------------------

/// The explicit column list used in SELECT statements so that column ordering
/// is deterministic and matches the `AuditSelectRow` field order.
const SELECT_COLUMNS: &str = "\
    id, action_id, chain_id, namespace, tenant, provider, action_type, verdict, \
    matched_rule, outcome, action_payload, verdict_details, outcome_details, \
    metadata, dispatched_at, completed_at, duration_ms, expires_at, \
    caller_id, auth_method, record_hash, previous_hash, sequence_number";

/// Escape a string value for safe interpolation inside a `ClickHouse` SQL
/// single-quoted literal.  `ClickHouse` uses backslash escaping by default.
fn escape_ch(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// Build a `WHERE` clause and its corresponding SQL fragment from an
/// [`AuditQuery`].  Returns a string that is either empty or starts with
/// `WHERE `.
fn build_where_clause(query: &AuditQuery) -> String {
    let mut conditions: Vec<String> = Vec::new();

    let string_filters: &[(&Option<String>, &str)] = &[
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

    for (value, col) in string_filters {
        if let Some(v) = value {
            conditions.push(format!("{col} = '{}'", escape_ch(v)));
        }
    }

    if let Some(from) = query.from {
        conditions.push(format!("dispatched_at >= {}", from.timestamp_millis()));
    }

    if let Some(to) = query.to {
        conditions.push(format!("dispatched_at <= {}", to.timestamp_millis()));
    }

    if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    }
}

// ---------------------------------------------------------------------------
// Store
// ---------------------------------------------------------------------------

/// `ClickHouse`-backed audit store.
///
/// Stores audit records in a `MergeTree` table ordered by
/// `(namespace, tenant, dispatched_at)`.  JSON fields are serialised to
/// `String` columns because `ClickHouse`'s native JSON type is experimental.
///
/// # Cleanup behaviour
///
/// [`cleanup_expired`](AuditStore::cleanup_expired) issues an `ALTER TABLE
/// ... DELETE` mutation.  `ClickHouse` mutations are asynchronous, so the
/// returned count is an *estimate* obtained by counting matching rows
/// immediately before the mutation is submitted.
pub struct ClickHouseAuditStore {
    client: clickhouse::Client,
    table: String,
}

impl ClickHouseAuditStore {
    /// Create a new store, connecting to `ClickHouse` and running migrations.
    pub async fn new(config: &ClickHouseAuditConfig) -> Result<Self, AuditError> {
        let client = clickhouse::Client::default()
            .with_url(&config.url)
            .with_database(&config.database);

        migrations::run_migrations(&client, &config.prefix)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(Self {
            client,
            table: format!("{}audit", config.prefix),
        })
    }

    /// Create from an existing `clickhouse::Client` (useful for testing).
    pub async fn from_client(client: clickhouse::Client, prefix: &str) -> Result<Self, AuditError> {
        migrations::run_migrations(&client, prefix)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(Self {
            client,
            table: format!("{prefix}audit"),
        })
    }
}

#[async_trait]
impl AuditStore for ClickHouseAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let row = AuditInsertRow::from(entry);

        let mut insert = self
            .client
            .insert(&self.table)
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        insert
            .write(&row)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        insert
            .end()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(())
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let sql = format!("SELECT {SELECT_COLUMNS} FROM {} WHERE id = ?", self.table,);

        let rows = self
            .client
            .query(&sql)
            .bind(id)
            .fetch_all::<AuditSelectRow>()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(rows.into_iter().next().map(Into::into))
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let sql = format!(
            "SELECT {SELECT_COLUMNS} FROM {} WHERE action_id = ? ORDER BY dispatched_at DESC LIMIT 1",
            self.table,
        );

        let rows = self
            .client
            .query(&sql)
            .bind(action_id)
            .fetch_all::<AuditSelectRow>()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(rows.into_iter().next().map(Into::into))
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();
        let offset = query.effective_offset();
        let where_clause = build_where_clause(query);

        // Count query.
        let count_sql = format!("SELECT count() FROM {} {where_clause}", self.table,);

        let total = self
            .client
            .query(&count_sql)
            .fetch_one::<u64>()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        // Data query.
        let data_sql = format!(
            "SELECT {SELECT_COLUMNS} FROM {} {where_clause} ORDER BY dispatched_at DESC LIMIT {limit} OFFSET {offset}",
            self.table,
        );

        let rows = self
            .client
            .query(&data_sql)
            .fetch_all::<AuditSelectRow>()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let records = rows.into_iter().map(Into::into).collect();

        Ok(AuditPage {
            records,
            total,
            limit,
            offset,
        })
    }

    /// Remove expired audit records.
    ///
    /// `ClickHouse` mutations (`ALTER TABLE ... DELETE`) are asynchronous, so the
    /// returned count is only an *estimate* -- it reflects the number of
    /// matching rows at the instant immediately before the mutation is issued.
    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        // Count the rows that will be affected so we can return an estimate.
        let count_sql = format!(
            "SELECT count() FROM {} WHERE expires_at IS NOT NULL AND expires_at <= now64(3)",
            self.table,
        );

        let count = self
            .client
            .query(&count_sql)
            .fetch_one::<u64>()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        if count > 0 {
            let delete_sql = format!(
                "ALTER TABLE {} DELETE WHERE expires_at IS NOT NULL AND expires_at <= now64(3)",
                self.table,
            );

            self.client
                .query(&delete_sql)
                .execute()
                .await
                .map_err(|e| AuditError::Storage(e.to_string()))?;
        }

        Ok(count)
    }
}
