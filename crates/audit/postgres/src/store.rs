use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;

use acteon_audit::analytics::AnalyticsStore;
use acteon_audit::cursor::{AuditCursor, CursorKind};
use acteon_audit::error::AuditError;
use acteon_audit::record::{AuditPage, AuditQuery, AuditRecord};
use acteon_audit::store::AuditStore;

use crate::analytics::PostgresAnalyticsStore;
use crate::config::PostgresAuditConfig;
use crate::migrations;

/// Build `PgConnectOptions` from a [`PostgresAuditConfig`], applying SSL
/// settings when configured.
fn build_audit_connect_options(
    config: &PostgresAuditConfig,
) -> Result<sqlx::postgres::PgConnectOptions, AuditError> {
    let mut options: sqlx::postgres::PgConnectOptions = config
        .url
        .parse()
        .map_err(|e: sqlx::Error| AuditError::Storage(e.to_string()))?;

    if let Some(ref mode) = config.ssl_mode {
        let ssl_mode = match mode.as_str() {
            "disable" => sqlx::postgres::PgSslMode::Disable,
            "prefer" => sqlx::postgres::PgSslMode::Prefer,
            "require" => sqlx::postgres::PgSslMode::Require,
            "verify-ca" => sqlx::postgres::PgSslMode::VerifyCa,
            "verify-full" => sqlx::postgres::PgSslMode::VerifyFull,
            other => {
                return Err(AuditError::Storage(format!("unknown ssl_mode: {other}")));
            }
        };
        options = options.ssl_mode(ssl_mode);
    }

    if let Some(ref path) = config.ssl_root_cert {
        options = options.ssl_root_cert(path);
    }

    if let Some(ref path) = config.ssl_cert {
        options = options.ssl_client_cert(path);
    }

    if let Some(ref path) = config.ssl_key {
        options = options.ssl_client_key(path);
    }

    Ok(options)
}

/// Postgres-backed audit store using `sqlx`.
pub struct PostgresAuditStore {
    pool: PgPool,
    table: String,
}

impl PostgresAuditStore {
    /// Create a new store, connecting to Postgres and running migrations.
    pub async fn new(config: &PostgresAuditConfig) -> Result<Self, AuditError> {
        let connect_options = build_audit_connect_options(config)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_with(connect_options)
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

    /// Access the connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Access the table name.
    pub fn table_name(&self) -> &str {
        &self.table
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
                attachment_metadata,
                signature, signer_id, kid, canonical_hash
            ) VALUES (
                $1, $2, $3, $4, $5, $6, $7,
                $8, $9, $10,
                $11, $12, $13, $14,
                $15, $16, $17, $18,
                $19, $20,
                $21, $22, $23,
                $24,
                $25, $26, $27, $28
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
            .bind(&entry.signature)
            .bind(&entry.signer_id)
            .bind(&entry.kid)
            .bind(&entry.canonical_hash)
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

    #[allow(clippy::too_many_lines, clippy::similar_names)]
    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();
        let (mut where_clause, binds, from_idx, to_idx, mut bind_idx) = build_where_clause(query);

        // Decode the cursor up-front so we can fail fast on bad input.
        let cursor = query
            .cursor
            .as_deref()
            .map(AuditCursor::decode)
            .transpose()?;

        // Append the keyset condition for the cursor, if any.
        let cursor_dispatched_idx;
        let cursor_id_idx;
        let cursor_seq_idx;
        if let Some(ref cursor) = cursor {
            let prefix = if where_clause.is_empty() {
                "WHERE"
            } else {
                "AND"
            };
            match cursor.kind {
                CursorKind::Ts => {
                    if query.sort_by_sequence_asc {
                        return Err(AuditError::Serialization(
                            "cursor kind 'ts' does not match sort_by_sequence_asc=true".into(),
                        ));
                    }
                    let ts_idx = bind_idx;
                    let id_idx = bind_idx + 1;
                    where_clause = format!(
                        "{where_clause} {prefix} (dispatched_at, id) < (${ts_idx}, ${id_idx})"
                    );
                    cursor_dispatched_idx = Some(ts_idx);
                    cursor_id_idx = Some(id_idx);
                    cursor_seq_idx = None;
                    bind_idx += 2;
                }
                CursorKind::Seq => {
                    if !query.sort_by_sequence_asc {
                        return Err(AuditError::Serialization(
                            "cursor kind 'seq' requires sort_by_sequence_asc=true".into(),
                        ));
                    }
                    let seq_idx = bind_idx;
                    where_clause = format!("{where_clause} {prefix} sequence_number > ${seq_idx}");
                    cursor_dispatched_idx = None;
                    cursor_id_idx = None;
                    cursor_seq_idx = Some(seq_idx);
                    bind_idx += 1;
                }
            }
        } else {
            cursor_dispatched_idx = None;
            cursor_id_idx = None;
            cursor_seq_idx = None;
        }

        // Count query — only run when no cursor is present (offset path).
        // Cursor pagination intentionally skips the count to keep page
        // latency O(limit).
        let total = if cursor.is_none() {
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
            let count = count_q
                .fetch_one(&self.pool)
                .await
                .map_err(|e| AuditError::Storage(e.to_string()))?;
            #[allow(clippy::cast_sign_loss)]
            let count = count as u64;
            Some(count)
        } else {
            None
        };

        // Data query.
        let order_clause = if query.sort_by_sequence_asc {
            "ORDER BY sequence_number ASC NULLS LAST, id ASC"
        } else if query.sort_by_sequence_desc {
            // Hash-chain tip selection: greatest sequence number first.
            "ORDER BY sequence_number DESC NULLS LAST, id DESC"
        } else {
            "ORDER BY dispatched_at DESC, id DESC"
        };
        let limit_idx = bind_idx;
        let offset_idx = bind_idx + 1;
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
        if let Some(ref cursor) = cursor {
            match cursor.kind {
                CursorKind::Ts => {
                    let ts = chrono::DateTime::<chrono::Utc>::from_timestamp_millis(
                        cursor.dispatched_at_ms.unwrap_or(0),
                    )
                    .unwrap_or_default();
                    data_q = data_q.bind(ts);
                    data_q = data_q.bind(cursor.id.clone().unwrap_or_default());
                }
                CursorKind::Seq => {
                    #[allow(clippy::cast_possible_wrap)]
                    let seq = cursor.sequence_number.unwrap_or(0) as i64;
                    data_q = data_q.bind(seq);
                }
            }
        }
        let _ = (cursor_dispatched_idx, cursor_id_idx, cursor_seq_idx);
        // Fetch limit + 1 so we can detect whether another page exists
        // without returning an empty trailing cursor to the caller.
        data_q = data_q.bind(i64::from(limit) + 1);
        // Cursor pagination always uses offset 0; offset is only used in
        // the legacy non-cursor path.
        let offset_value = if cursor.is_some() {
            0
        } else {
            query.effective_offset()
        };
        data_q = data_q.bind(i64::from(offset_value));

        let rows: Vec<AuditRow> = data_q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let mut records: Vec<AuditRecord> = rows.into_iter().map(Into::into).collect();
        let has_more = records.len() > limit as usize;
        if has_more {
            records.truncate(limit as usize);
        }

        let next_cursor = if has_more {
            records
                .last()
                .map(|rec| {
                    if query.sort_by_sequence_asc {
                        AuditCursor::from_sequence(rec.sequence_number.unwrap_or(0), rec.id.clone())
                    } else {
                        AuditCursor::from_timestamp(
                            rec.dispatched_at.timestamp_millis(),
                            rec.id.clone(),
                        )
                    }
                })
                .map(|c| c.encode())
                .transpose()?
        } else {
            None
        };

        Ok(AuditPage {
            records,
            total,
            limit,
            offset: offset_value,
            next_cursor,
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

    fn analytics(&self) -> Option<Arc<dyn AnalyticsStore>> {
        Some(Arc::new(PostgresAnalyticsStore::new(
            self.pool.clone(),
            self.table.clone(),
        )))
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
        (&query.signer_id, "signer_id"),
        (&query.kid, "kid"),
    ];

    for (value, col) in fields {
        if let Some(v) = value {
            conditions.push(format!("{col} = ${bind_idx}"));
            binds.push(v.clone());
            bind_idx += 1;
        }
    }

    // Tenant-scope OR-group: restrict to tenants covered hierarchically by any
    // pattern in the scope. Empty scope adds nothing (preserves prior behavior).
    if !query.tenant_scope.is_empty() {
        let mut scope_terms = Vec::with_capacity(query.tenant_scope.len());
        for p in &query.tenant_scope {
            let exact_idx = bind_idx;
            binds.push(p.clone());
            bind_idx += 1;
            let like_idx = bind_idx;
            binds.push(acteon_core::tenant_scope::like_descendants_pattern(p));
            bind_idx += 1;
            scope_terms.push(format!(
                "(tenant = ${exact_idx} OR tenant LIKE ${like_idx})"
            ));
        }
        conditions.push(format!("({})", scope_terms.join(" OR ")));
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
    #[sqlx(default)]
    signature: Option<String>,
    #[sqlx(default)]
    signer_id: Option<String>,
    #[sqlx(default)]
    kid: Option<String>,
    #[sqlx(default)]
    canonical_hash: Option<String>,
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
            signature: row.signature,
            signer_id: row.signer_id,
            kid: row.kid,
            canonical_hash: row.canonical_hash,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_scope_adds_no_tenant_scope_predicate() {
        // A fully-empty query (no filters, empty scope) must produce exactly
        // the same WHERE clause and binds as today — byte-for-byte unchanged.
        let query = AuditQuery::default();
        let (where_clause, binds, from_idx, to_idx, bind_idx) = build_where_clause(&query);

        assert_eq!(where_clause, "");
        assert!(binds.is_empty());
        assert_eq!(from_idx, None);
        assert_eq!(to_idx, None);
        assert_eq!(bind_idx, 1);
    }

    #[test]
    fn non_empty_scope_produces_or_group() {
        let mut query = AuditQuery::default();
        query.tenant_scope = vec!["acme".to_string(), "globex.eu".to_string()];

        let (where_clause, binds, from_idx, to_idx, bind_idx) = build_where_clause(&query);

        // One parenthesized OR-group, two terms (one per pattern).
        assert_eq!(
            where_clause,
            "WHERE ((tenant = $1 OR tenant LIKE $2) OR (tenant = $3 OR tenant LIKE $4))"
        );
        // Two binds per pattern: exact value + descendant LIKE pattern.
        assert_eq!(
            binds,
            vec![
                "acme".to_string(),
                "acme.%".to_string(),
                "globex.eu".to_string(),
                "globex.eu.%".to_string(),
            ]
        );
        assert_eq!(from_idx, None);
        assert_eq!(to_idx, None);
        assert_eq!(bind_idx, 5);
    }

    #[test]
    fn scope_anded_with_exact_tenant_and_time_range() {
        let now = chrono::Utc::now();
        let mut query = AuditQuery::default();
        query.tenant = Some("acme.team".to_string());
        query.tenant_scope = vec!["acme".to_string()];
        query.from = Some(now);
        query.to = Some(now);

        let (where_clause, binds, from_idx, to_idx, bind_idx) = build_where_clause(&query);

        assert_eq!(
            where_clause,
            "WHERE tenant = $1 AND ((tenant = $2 OR tenant LIKE $3)) \
             AND dispatched_at >= $4 AND dispatched_at <= $5"
        );
        assert_eq!(
            binds,
            vec![
                "acme.team".to_string(),
                "acme".to_string(),
                "acme.%".to_string(),
            ]
        );
        assert_eq!(from_idx, Some(4));
        assert_eq!(to_idx, Some(5));
        assert_eq!(bind_idx, 6);
    }
}
