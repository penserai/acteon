use async_trait::async_trait;
use sqlx::PgPool;

use acteon_audit::error::AuditError;
use acteon_audit::record::{AuditPage, AuditQuery, AuditRecord};
use acteon_audit::store::AuditStore;

use crate::config::PostgresAuditConfig;
use crate::migrations;

/// Postgres-backed audit store using `sqlx`.
pub struct PostgresAuditStore {
    pool: PgPool,
    table: String,
}

impl PostgresAuditStore {
    /// Create a new store, connecting to Postgres and running migrations.
    pub async fn new(config: &PostgresAuditConfig) -> Result<Self, AuditError> {
        let pool = PgPool::connect(&config.url)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        migrations::run_migrations(&pool, &config.prefix)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(Self {
            pool,
            table: format!("{}audit", config.prefix),
        })
    }

    /// Create from an existing pool (useful for testing).
    pub async fn from_pool(pool: PgPool, prefix: &str) -> Result<Self, AuditError> {
        migrations::run_migrations(&pool, prefix)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(Self {
            pool,
            table: format!("{prefix}audit"),
        })
    }
}

#[async_trait]
impl AuditStore for PostgresAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        let sql = format!(
            r"
            INSERT INTO {} (
                id, action_id, chain_id, namespace, tenant, provider, action_type,
                verdict, matched_rule, outcome,
                action_payload, verdict_details, outcome_details, metadata,
                dispatched_at, completed_at, duration_ms, expires_at,
                caller_id, auth_method,
                record_hash, previous_hash, sequence_number,
                attachment_metadata
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                $8, $9, $10,
                $11, $12, $13, $14,
                $15, $16, $17, $18,
                $19, $20,
                $21, $22, $23,
                $24
            )
            ",
            self.table
        );

        #[allow(clippy::cast_possible_wrap)]
        let duration = entry.duration_ms as i64;
        #[allow(clippy::cast_possible_wrap)]
        let sequence_number = entry.sequence_number.map(|n| n as i64);

        sqlx::query(&sql)
            .bind(&entry.id)
            .bind(&entry.action_id)
            .bind(&entry.chain_id)
            .bind(&entry.namespace)
            .bind(&entry.tenant)
            .bind(&entry.provider)
            .bind(&entry.action_type)
            .bind(&entry.verdict)
            .bind(&entry.matched_rule)
            .bind(&entry.outcome)
            .bind(&entry.action_payload)
            .bind(&entry.verdict_details)
            .bind(&entry.outcome_details)
            .bind(&entry.metadata)
            .bind(entry.dispatched_at)
            .bind(entry.completed_at)
            .bind(duration)
            .bind(entry.expires_at)
            .bind(&entry.caller_id)
            .bind(&entry.auth_method)
            .bind(&entry.record_hash)
            .bind(&entry.previous_hash)
            .bind(sequence_number)
            .bind(serde_json::Value::Array(entry.attachment_metadata.clone()))
            .execute(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(())
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let sql = format!(
            "SELECT * FROM {} WHERE action_id = $1 ORDER BY dispatched_at DESC LIMIT 1",
            self.table
        );

        let row = sqlx::query_as::<_, AuditRow>(&sql)
            .bind(action_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(row.map(Into::into))
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let sql = format!("SELECT * FROM {} WHERE id = $1", self.table);

        let row = sqlx::query_as::<_, AuditRow>(&sql)
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(row.map(Into::into))
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();
        let offset = query.effective_offset();
        let (where_clause, binds, from_idx, to_idx, bind_idx) = build_where_clause(query);

        // Count query.
        let count_sql = format!("SELECT COUNT(*) as cnt FROM {} {where_clause}", self.table);
        let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
        for b in &binds {
            count_q = count_q.bind(b);
        }
        if from_idx.is_some() {
            count_q = count_q.bind(query.from.unwrap());
        }
        if to_idx.is_some() {
            count_q = count_q.bind(query.to.unwrap());
        }

        let total = count_q
            .fetch_one(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        // Data query.
        let limit_idx = bind_idx;
        let offset_idx = bind_idx + 1;
        let order_clause = if query.sort_by_sequence_asc {
            "ORDER BY sequence_number ASC NULLS LAST"
        } else {
            "ORDER BY dispatched_at DESC"
        };
        let data_sql = format!(
            "SELECT * FROM {} {where_clause} {order_clause} LIMIT ${limit_idx} OFFSET ${offset_idx}",
            self.table
        );

        let mut data_q = sqlx::query_as::<_, AuditRow>(&data_sql);
        for b in &binds {
            data_q = data_q.bind(b);
        }
        if from_idx.is_some() {
            data_q = data_q.bind(query.from.unwrap());
        }
        if to_idx.is_some() {
            data_q = data_q.bind(query.to.unwrap());
        }
        data_q = data_q.bind(i64::from(limit));
        data_q = data_q.bind(i64::from(offset));

        let rows: Vec<AuditRow> = data_q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let records = rows.into_iter().map(Into::into).collect();

        #[allow(clippy::cast_sign_loss)]
        let total = total as u64;

        Ok(AuditPage {
            records,
            total,
            limit,
            offset,
        })
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        let sql = format!(
            "DELETE FROM {} WHERE expires_at IS NOT NULL AND expires_at <= NOW()",
            self.table
        );

        let result = sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        Ok(result.rows_affected())
    }
}

/// Build the WHERE clause and bind values for the query.
fn build_where_clause(query: &AuditQuery) -> (String, Vec<String>, Option<u32>, Option<u32>, u32) {
    let mut conditions = Vec::new();
    let mut bind_idx = 1u32;
    let mut binds: Vec<String> = Vec::new();

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

    for (value, col) in fields {
        if let Some(v) = value {
            conditions.push(format!("{col} = ${bind_idx}"));
            binds.push(v.clone());
            bind_idx += 1;
        }
    }

    let from_idx = if query.from.is_some() {
        conditions.push(format!("dispatched_at >= ${bind_idx}"));
        let idx = bind_idx;
        bind_idx += 1;
        Some(idx)
    } else {
        None
    };

    let to_idx = if query.to.is_some() {
        conditions.push(format!("dispatched_at <= ${bind_idx}"));
        let idx = bind_idx;
        bind_idx += 1;
        Some(idx)
    } else {
        None
    };

    let where_clause = if conditions.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", conditions.join(" AND "))
    };

    (where_clause, binds, from_idx, to_idx, bind_idx)
}

/// Internal row type for mapping database rows to `AuditRecord`.
#[derive(sqlx::FromRow)]
struct AuditRow {
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
    action_payload: Option<serde_json::Value>,
    verdict_details: serde_json::Value,
    outcome_details: serde_json::Value,
    metadata: serde_json::Value,
    dispatched_at: chrono::DateTime<chrono::Utc>,
    completed_at: chrono::DateTime<chrono::Utc>,
    duration_ms: i64,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
    caller_id: String,
    auth_method: String,
    record_hash: Option<String>,
    previous_hash: Option<String>,
    sequence_number: Option<i64>,
    attachment_metadata: serde_json::Value,
}

impl From<AuditRow> for AuditRecord {
    fn from(row: AuditRow) -> Self {
        #[allow(clippy::cast_sign_loss)]
        let duration_ms = row.duration_ms as u64;

        let attachment_metadata = match row.attachment_metadata {
            serde_json::Value::Array(arr) => arr,
            _ => Vec::new(),
        };

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
            action_payload: row.action_payload,
            verdict_details: row.verdict_details,
            outcome_details: row.outcome_details,
            metadata: row.metadata,
            dispatched_at: row.dispatched_at,
            completed_at: row.completed_at,
            duration_ms,
            expires_at: row.expires_at,
            caller_id: row.caller_id,
            auth_method: row.auth_method,
            record_hash: row.record_hash,
            previous_hash: row.previous_hash,
            #[allow(clippy::cast_sign_loss)]
            sequence_number: row.sequence_number.map(|n| n as u64),
            attachment_metadata,
        }
    }
}
