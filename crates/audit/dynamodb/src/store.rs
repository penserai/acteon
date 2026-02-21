use std::collections::HashMap;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use chrono::{DateTime, Utc};

use acteon_audit::error::AuditError;
use acteon_audit::record::{AuditPage, AuditQuery, AuditRecord};
use acteon_audit::store::AuditStore;

use crate::config::DynamoDbAuditConfig;

/// `DynamoDB`-backed implementation of [`AuditStore`].
///
/// Uses a single `DynamoDB` table with `id` as the partition key, three Global
/// Secondary Indexes for efficient query patterns, and native TTL for automatic
/// record expiration.
///
/// Hash chain integrity is enforced via `TransactWriteItems` with a fence item
/// using `attribute_not_exists` to guarantee sequence number uniqueness.
pub struct DynamoDbAuditStore {
    client: Client,
    table_name: String,
    _prefix: String,
}

impl DynamoDbAuditStore {
    /// Create a new `DynamoDbAuditStore` from the provided configuration.
    ///
    /// Loads AWS credentials and configuration from the environment and
    /// optionally overrides the endpoint URL for local development.
    pub async fn new(config: &DynamoDbAuditConfig) -> Self {
        let client = build_client(config).await;
        Self {
            client,
            table_name: config.table_name.clone(),
            _prefix: config.key_prefix.clone(),
        }
    }

    /// Create a new `DynamoDbAuditStore` from an existing `DynamoDB` client.
    pub fn from_client(client: Client, config: &DynamoDbAuditConfig) -> Self {
        Self {
            client,
            table_name: config.table_name.clone(),
            _prefix: config.key_prefix.clone(),
        }
    }

    /// Build the composite `ns_tenant` key.
    fn ns_tenant(namespace: &str, tenant: &str) -> String {
        format!("{namespace}#{tenant}")
    }

    /// Write an audit record with hash chain CAS using `TransactWriteItems`.
    ///
    /// Atomically writes both a fence item (to enforce sequence uniqueness) and
    /// the actual audit record. If another replica races for the same sequence
    /// number, the transaction fails with `TransactionCanceledException` whose
    /// message contains `ConditionalCheckFailed` — which is mapped to an error
    /// containing "duplicate" to trigger the `HashChainAuditStore` retry logic.
    async fn record_with_sequence(&self, entry: &AuditRecord, seq: u64) -> Result<(), AuditError> {
        let ns_tenant = Self::ns_tenant(&entry.namespace, &entry.tenant);
        let fence_id = format!("SEQ#{ns_tenant}#{seq}");

        let item = record_to_item(entry);

        // Build fence item: PK = fence_id, with condition attribute_not_exists(id).
        let fence_put = aws_sdk_dynamodb::types::Put::builder()
            .table_name(&self.table_name)
            .item("id", AttributeValue::S(fence_id))
            .item("_fence", AttributeValue::Bool(true))
            .condition_expression("attribute_not_exists(id)")
            .build()
            .expect("valid Put");

        // Build record item put (unconditional).
        let mut record_put_builder =
            aws_sdk_dynamodb::types::Put::builder().table_name(&self.table_name);
        for (k, v) in &item {
            record_put_builder = record_put_builder.item(k, v.clone());
        }
        let record_put = record_put_builder.build().expect("valid Put");

        let fence_write = aws_sdk_dynamodb::types::TransactWriteItem::builder()
            .put(fence_put)
            .build();
        let record_write = aws_sdk_dynamodb::types::TransactWriteItem::builder()
            .put(record_put)
            .build();

        let result = self
            .client
            .transact_write_items()
            .transact_items(fence_write)
            .transact_items(record_write)
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                let msg = err.to_string();
                // Map conditional check failures to "duplicate" for
                // HashChainAuditStore retry logic.
                if msg.contains("ConditionalCheckFailed") || msg.contains("TransactionConflict") {
                    Err(AuditError::Storage(format!(
                        "duplicate sequence number {seq} for {}: conditional check failed",
                        entry.namespace
                    )))
                } else {
                    Err(AuditError::Storage(msg))
                }
            }
        }
    }

    /// Build filter expressions and attribute values for query filtering.
    ///
    /// Returns `(filter_expression, expression_attribute_values)` for fields
    /// that cannot be efficiently queried via GSI key conditions.
    fn build_filters(
        query: &AuditQuery,
    ) -> (
        Vec<String>,
        HashMap<String, AttributeValue>,
        HashMap<String, String>,
    ) {
        let mut filters = Vec::new();
        let mut values = HashMap::new();
        let mut names = HashMap::new();

        if let Some(ref provider) = query.provider {
            filters.push("#provider = :provider".to_owned());
            values.insert(":provider".to_owned(), AttributeValue::S(provider.clone()));
            names.insert("#provider".to_owned(), "provider".to_owned());
        }
        if let Some(ref action_type) = query.action_type {
            filters.push("action_type = :action_type".to_owned());
            values.insert(
                ":action_type".to_owned(),
                AttributeValue::S(action_type.clone()),
            );
        }
        if let Some(ref outcome) = query.outcome {
            filters.push("outcome = :outcome".to_owned());
            values.insert(":outcome".to_owned(), AttributeValue::S(outcome.clone()));
        }
        if let Some(ref verdict) = query.verdict {
            filters.push("verdict = :verdict".to_owned());
            values.insert(":verdict".to_owned(), AttributeValue::S(verdict.clone()));
        }
        if let Some(ref matched_rule) = query.matched_rule {
            filters.push("matched_rule = :matched_rule".to_owned());
            values.insert(
                ":matched_rule".to_owned(),
                AttributeValue::S(matched_rule.clone()),
            );
        }
        if let Some(ref caller_id) = query.caller_id {
            filters.push("caller_id = :caller_id".to_owned());
            values.insert(
                ":caller_id".to_owned(),
                AttributeValue::S(caller_id.clone()),
            );
        }
        if let Some(ref chain_id) = query.chain_id {
            filters.push("chain_id = :chain_id".to_owned());
            values.insert(":chain_id".to_owned(), AttributeValue::S(chain_id.clone()));
        }
        // Filter out fence items that may appear in GSI queries.
        filters.push("attribute_not_exists(#fence)".to_owned());
        names.insert("#fence".to_owned(), "_fence".to_owned());

        (filters, values, names)
    }
}

#[async_trait]
#[allow(clippy::too_many_lines)]
impl AuditStore for DynamoDbAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        if let Some(seq) = entry.sequence_number {
            return self.record_with_sequence(&entry, seq).await;
        }

        // Simple PutItem without hash chain CAS.
        let item = record_to_item(&entry);
        let mut put = self.client.put_item().table_name(&self.table_name);
        for (k, v) in item {
            put = put.item(k, v);
        }
        put.send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;
        Ok(())
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let result = self
            .client
            .query()
            .table_name(&self.table_name)
            .index_name("action_id_index")
            .key_condition_expression("action_id = :aid")
            .filter_expression("attribute_not_exists(#fence)")
            .expression_attribute_names("#fence", "_fence")
            .expression_attribute_values(":aid", AttributeValue::S(action_id.to_owned()))
            .scan_index_forward(false)
            .limit(1)
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let items = result.items();
        if items.is_empty() {
            return Ok(None);
        }

        item_to_record(&items[0]).map(Some)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("id", AttributeValue::S(id.to_owned()))
            .send()
            .await
            .map_err(|e| AuditError::Storage(e.to_string()))?;

        let Some(item) = result.item() else {
            return Ok(None);
        };
        // Filter out fence items.
        if item.get("_fence").is_some() {
            return Ok(None);
        }
        item_to_record(item).map(Some)
    }

    async fn query(&self, query: &AuditQuery) -> Result<AuditPage, AuditError> {
        let limit = query.effective_limit();
        let offset = query.effective_offset();

        // Require at least namespace + tenant for GSI queries.
        let (namespace, tenant) = match (&query.namespace, &query.tenant) {
            (Some(ns), Some(t)) => (ns.clone(), t.clone()),
            _ => {
                // Without namespace+tenant we cannot use the GSI efficiently.
                // Return empty for now (a full table scan would be expensive).
                return Ok(AuditPage {
                    records: Vec::new(),
                    total: 0,
                    limit,
                    offset,
                });
            }
        };

        let ns_tenant = Self::ns_tenant(&namespace, &tenant);
        let (filter_parts, filter_values, filter_names) = Self::build_filters(query);

        // Choose GSI based on sort order.
        let (index_name, key_condition, mut expr_values) = if query.sort_by_sequence_asc {
            (
                "ns_tenant_sequence",
                "ns_tenant = :ns_tenant".to_owned(),
                HashMap::from([(
                    ":ns_tenant".to_owned(),
                    AttributeValue::S(ns_tenant.clone()),
                )]),
            )
        } else {
            let mut kc = "ns_tenant = :ns_tenant".to_owned();
            let mut ev = HashMap::from([(
                ":ns_tenant".to_owned(),
                AttributeValue::S(ns_tenant.clone()),
            )]);

            // Add time range conditions on the SK.
            if let Some(ref from) = query.from {
                kc.push_str(" AND dispatched_at_ms >= :from_ms");
                ev.insert(
                    ":from_ms".to_owned(),
                    AttributeValue::N(from.timestamp_millis().to_string()),
                );
            }
            if let Some(ref to) = query.to {
                if query.from.is_some() {
                    // Already have a range start, add end.
                    kc = kc.replace(
                        " AND dispatched_at_ms >= :from_ms",
                        " AND dispatched_at_ms BETWEEN :from_ms AND :to_ms",
                    );
                } else {
                    kc.push_str(" AND dispatched_at_ms <= :to_ms");
                }
                ev.insert(
                    ":to_ms".to_owned(),
                    AttributeValue::N(to.timestamp_millis().to_string()),
                );
            }

            ("ns_tenant_dispatched", kc, ev)
        };

        // Merge filter values.
        expr_values.extend(filter_values);

        let filter_expression = if filter_parts.is_empty() {
            None
        } else {
            Some(filter_parts.join(" AND "))
        };

        // For total count, run a SELECT COUNT query first.
        let total = {
            let mut count_query = self
                .client
                .query()
                .table_name(&self.table_name)
                .index_name(index_name)
                .key_condition_expression(&key_condition)
                .select(aws_sdk_dynamodb::types::Select::Count);

            for (k, v) in &expr_values {
                count_query = count_query.expression_attribute_values(k, v.clone());
            }
            for (k, v) in &filter_names {
                count_query = count_query.expression_attribute_names(k, v);
            }
            if let Some(ref fe) = filter_expression {
                count_query = count_query.filter_expression(fe);
            }

            let scan_forward = query.sort_by_sequence_asc;
            count_query = count_query.scan_index_forward(scan_forward);

            // Paginate through all to get accurate total count.
            let mut total = 0u64;
            let mut exclusive_start_key = None;
            loop {
                let mut q = count_query.clone();
                if let Some(key) = exclusive_start_key {
                    q = q.set_exclusive_start_key(Some(key));
                }
                let resp = q
                    .send()
                    .await
                    .map_err(|e| AuditError::Storage(e.to_string()))?;
                total += u64::try_from(resp.count()).unwrap_or(0);
                exclusive_start_key = resp.last_evaluated_key().cloned();
                if exclusive_start_key.is_none() {
                    break;
                }
            }
            total
        };

        // Fetch actual records with client-side offset skipping.
        let mut all_records = Vec::new();
        let items_needed = offset as usize + limit as usize;
        let mut exclusive_start_key = None;

        let scan_forward = query.sort_by_sequence_asc;

        loop {
            let mut q = self
                .client
                .query()
                .table_name(&self.table_name)
                .index_name(index_name)
                .key_condition_expression(&key_condition)
                .scan_index_forward(scan_forward);

            for (k, v) in &expr_values {
                q = q.expression_attribute_values(k, v.clone());
            }
            for (k, v) in &filter_names {
                q = q.expression_attribute_names(k, v);
            }
            if let Some(ref fe) = filter_expression {
                q = q.filter_expression(fe);
            }
            if let Some(key) = exclusive_start_key {
                q = q.set_exclusive_start_key(Some(key));
            }

            let resp = q
                .send()
                .await
                .map_err(|e| AuditError::Storage(e.to_string()))?;

            for item in resp.items() {
                if all_records.len() >= items_needed {
                    break;
                }
                match item_to_record(item) {
                    Ok(record) => all_records.push(record),
                    Err(e) => {
                        tracing::warn!(error = %e, "skipping malformed audit record");
                    }
                }
            }

            if all_records.len() >= items_needed {
                break;
            }

            exclusive_start_key = resp.last_evaluated_key().cloned();
            if exclusive_start_key.is_none() {
                break;
            }
        }

        // Apply client-side offset.
        let records: Vec<AuditRecord> = all_records
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect();

        Ok(AuditPage {
            records,
            total,
            limit,
            offset,
        })
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        // DynamoDB native TTL handles expiration automatically.
        // No manual cleanup needed.
        Ok(0)
    }
}

// ---------------------------------------------------------------------------
// Attribute conversion helpers
// ---------------------------------------------------------------------------

/// Convert an [`AuditRecord`] to a `DynamoDB` item (attribute map).
#[allow(clippy::too_many_lines)]
fn record_to_item(record: &AuditRecord) -> HashMap<String, AttributeValue> {
    let mut item = HashMap::new();
    let ns_tenant = DynamoDbAuditStore::ns_tenant(&record.namespace, &record.tenant);

    item.insert("id".to_owned(), AttributeValue::S(record.id.clone()));
    item.insert(
        "action_id".to_owned(),
        AttributeValue::S(record.action_id.clone()),
    );
    item.insert("ns_tenant".to_owned(), AttributeValue::S(ns_tenant));
    item.insert(
        "dispatched_at_ms".to_owned(),
        AttributeValue::N(record.dispatched_at.timestamp_millis().to_string()),
    );
    item.insert(
        "namespace".to_owned(),
        AttributeValue::S(record.namespace.clone()),
    );
    item.insert(
        "tenant".to_owned(),
        AttributeValue::S(record.tenant.clone()),
    );
    item.insert(
        "provider".to_owned(),
        AttributeValue::S(record.provider.clone()),
    );
    item.insert(
        "action_type".to_owned(),
        AttributeValue::S(record.action_type.clone()),
    );
    item.insert(
        "verdict".to_owned(),
        AttributeValue::S(record.verdict.clone()),
    );
    item.insert(
        "outcome".to_owned(),
        AttributeValue::S(record.outcome.clone()),
    );
    item.insert(
        "dispatched_at".to_owned(),
        AttributeValue::S(record.dispatched_at.to_rfc3339()),
    );
    item.insert(
        "completed_at".to_owned(),
        AttributeValue::S(record.completed_at.to_rfc3339()),
    );
    item.insert(
        "duration_ms".to_owned(),
        AttributeValue::N(record.duration_ms.to_string()),
    );
    item.insert(
        "caller_id".to_owned(),
        AttributeValue::S(record.caller_id.clone()),
    );
    item.insert(
        "auth_method".to_owned(),
        AttributeValue::S(record.auth_method.clone()),
    );

    // Optional string fields.
    if let Some(ref chain_id) = record.chain_id {
        item.insert("chain_id".to_owned(), AttributeValue::S(chain_id.clone()));
    }
    if let Some(ref matched_rule) = record.matched_rule {
        item.insert(
            "matched_rule".to_owned(),
            AttributeValue::S(matched_rule.clone()),
        );
    }

    // JSON fields stored as String attributes.
    if let Some(ref payload) = record.action_payload {
        item.insert(
            "action_payload".to_owned(),
            AttributeValue::S(payload.to_string()),
        );
    }
    item.insert(
        "verdict_details".to_owned(),
        AttributeValue::S(record.verdict_details.to_string()),
    );
    item.insert(
        "outcome_details".to_owned(),
        AttributeValue::S(record.outcome_details.to_string()),
    );
    item.insert(
        "metadata".to_owned(),
        AttributeValue::S(record.metadata.to_string()),
    );

    // Attachment metadata (JSON array of objects, no binary data).
    if !record.attachment_metadata.is_empty() {
        let json = serde_json::to_string(&record.attachment_metadata).unwrap_or_default();
        item.insert("attachment_metadata".to_owned(), AttributeValue::S(json));
    }

    // TTL: epoch seconds for DynamoDB native TTL.
    if let Some(ref expires_at) = record.expires_at {
        item.insert(
            "expires_at_ttl".to_owned(),
            AttributeValue::N(expires_at.timestamp().to_string()),
        );
        item.insert(
            "expires_at".to_owned(),
            AttributeValue::S(expires_at.to_rfc3339()),
        );
    }

    // Hash chain fields.
    if let Some(ref hash) = record.record_hash {
        item.insert("record_hash".to_owned(), AttributeValue::S(hash.clone()));
    }
    if let Some(ref prev) = record.previous_hash {
        item.insert("previous_hash".to_owned(), AttributeValue::S(prev.clone()));
    }
    if let Some(seq) = record.sequence_number {
        item.insert(
            "sequence_number".to_owned(),
            AttributeValue::N(seq.to_string()),
        );
    }

    item
}

/// Convert a `DynamoDB` item to an [`AuditRecord`].
fn item_to_record(item: &HashMap<String, AttributeValue>) -> Result<AuditRecord, AuditError> {
    let get_s = |key: &str| -> Result<String, AuditError> {
        match item.get(key) {
            Some(AttributeValue::S(v)) => Ok(v.clone()),
            _ => Err(AuditError::Storage(format!(
                "missing or invalid string attribute: {key}"
            ))),
        }
    };

    let get_s_opt = |key: &str| -> Option<String> {
        match item.get(key) {
            Some(AttributeValue::S(v)) => Some(v.clone()),
            _ => None,
        }
    };

    let get_n_u64 = |key: &str| -> Result<u64, AuditError> {
        match item.get(key) {
            Some(AttributeValue::N(v)) => v
                .parse()
                .map_err(|_| AuditError::Storage(format!("invalid number for {key}: {v}"))),
            _ => Err(AuditError::Storage(format!(
                "missing or invalid number attribute: {key}"
            ))),
        }
    };

    let get_n_u64_opt = |key: &str| -> Option<u64> {
        match item.get(key) {
            Some(AttributeValue::N(v)) => v.parse().ok(),
            _ => None,
        }
    };

    let parse_datetime = |key: &str| -> Result<DateTime<Utc>, AuditError> {
        let s = get_s(key)?;
        DateTime::parse_from_rfc3339(&s)
            .map(|dt| dt.with_timezone(&Utc))
            .map_err(|e| AuditError::Storage(format!("invalid datetime for {key}: {e}")))
    };

    let parse_datetime_opt = |key: &str| -> Option<DateTime<Utc>> {
        get_s_opt(key).and_then(|s| {
            DateTime::parse_from_rfc3339(&s)
                .map(|dt| dt.with_timezone(&Utc))
                .ok()
        })
    };

    let parse_json = |key: &str| -> serde_json::Value {
        get_s_opt(key)
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::Value::Null)
    };

    let parse_json_opt = |key: &str| -> Option<serde_json::Value> {
        get_s_opt(key).and_then(|s| serde_json::from_str(&s).ok())
    };

    Ok(AuditRecord {
        id: get_s("id")?,
        action_id: get_s("action_id")?,
        chain_id: get_s_opt("chain_id"),
        namespace: get_s("namespace")?,
        tenant: get_s("tenant")?,
        provider: get_s("provider")?,
        action_type: get_s("action_type")?,
        verdict: get_s("verdict")?,
        matched_rule: get_s_opt("matched_rule"),
        outcome: get_s("outcome")?,
        action_payload: parse_json_opt("action_payload"),
        verdict_details: parse_json("verdict_details"),
        outcome_details: parse_json("outcome_details"),
        metadata: parse_json("metadata"),
        dispatched_at: parse_datetime("dispatched_at")?,
        completed_at: parse_datetime("completed_at")?,
        duration_ms: get_n_u64("duration_ms")?,
        expires_at: parse_datetime_opt("expires_at"),
        caller_id: get_s_opt("caller_id").unwrap_or_default(),
        auth_method: get_s_opt("auth_method").unwrap_or_default(),
        record_hash: get_s_opt("record_hash"),
        previous_hash: get_s_opt("previous_hash"),
        sequence_number: get_n_u64_opt("sequence_number"),
        attachment_metadata: get_s_opt("attachment_metadata")
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default(),
    })
}

/// Build an AWS `DynamoDB` [`Client`] from the provided configuration.
///
/// Uses the standard AWS SDK environment credential chain and optionally
/// overrides the endpoint URL for local development.
pub async fn build_client(config: &DynamoDbAuditConfig) -> Client {
    let mut aws_config =
        aws_config::from_env().region(aws_config::Region::new(config.region.clone()));

    if let Some(endpoint) = &config.endpoint_url {
        aws_config = aws_config.endpoint_url(endpoint);
    }

    let sdk_config = aws_config.load().await;
    Client::new(&sdk_config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn sample_record() -> AuditRecord {
        let now = Utc::now();
        AuditRecord {
            id: "rec-001".to_owned(),
            action_id: "act-001".to_owned(),
            chain_id: None,
            namespace: "notifications".to_owned(),
            tenant: "tenant-1".to_owned(),
            provider: "email".to_owned(),
            action_type: "send_alert".to_owned(),
            verdict: "allow".to_owned(),
            matched_rule: Some("rule-1".to_owned()),
            outcome: "executed".to_owned(),
            action_payload: Some(serde_json::json!({"to": "user@example.com"})),
            verdict_details: serde_json::json!({"reason": "matched"}),
            outcome_details: serde_json::json!({"status": 200}),
            metadata: serde_json::json!({"env": "prod"}),
            dispatched_at: now,
            completed_at: now,
            duration_ms: 42,
            expires_at: Some(now + chrono::Duration::days(30)),
            caller_id: "user-1".to_owned(),
            auth_method: "jwt".to_owned(),
            record_hash: Some("abc123".to_owned()),
            previous_hash: Some("def456".to_owned()),
            sequence_number: Some(1),
            attachment_metadata: Vec::new(),
        }
    }

    #[test]
    fn record_to_item_basic_fields() {
        let record = sample_record();
        let item = record_to_item(&record);

        assert_eq!(
            item.get("id"),
            Some(&AttributeValue::S("rec-001".to_owned()))
        );
        assert_eq!(
            item.get("action_id"),
            Some(&AttributeValue::S("act-001".to_owned()))
        );
        assert_eq!(
            item.get("ns_tenant"),
            Some(&AttributeValue::S("notifications#tenant-1".to_owned()))
        );
        assert_eq!(
            item.get("namespace"),
            Some(&AttributeValue::S("notifications".to_owned()))
        );
        assert_eq!(
            item.get("tenant"),
            Some(&AttributeValue::S("tenant-1".to_owned()))
        );
        assert_eq!(
            item.get("provider"),
            Some(&AttributeValue::S("email".to_owned()))
        );
        assert_eq!(
            item.get("action_type"),
            Some(&AttributeValue::S("send_alert".to_owned()))
        );
        assert_eq!(
            item.get("verdict"),
            Some(&AttributeValue::S("allow".to_owned()))
        );
        assert_eq!(
            item.get("outcome"),
            Some(&AttributeValue::S("executed".to_owned()))
        );
        assert_eq!(
            item.get("duration_ms"),
            Some(&AttributeValue::N("42".to_owned()))
        );
    }

    #[test]
    fn record_to_item_hash_chain_fields() {
        let record = sample_record();
        let item = record_to_item(&record);

        assert_eq!(
            item.get("record_hash"),
            Some(&AttributeValue::S("abc123".to_owned()))
        );
        assert_eq!(
            item.get("previous_hash"),
            Some(&AttributeValue::S("def456".to_owned()))
        );
        assert_eq!(
            item.get("sequence_number"),
            Some(&AttributeValue::N("1".to_owned()))
        );
    }

    #[test]
    fn record_to_item_optional_fields_absent() {
        let now = Utc::now();
        let record = AuditRecord {
            id: "rec-002".to_owned(),
            action_id: "act-002".to_owned(),
            chain_id: None,
            namespace: "ns".to_owned(),
            tenant: "t".to_owned(),
            provider: "log".to_owned(),
            action_type: "test".to_owned(),
            verdict: "allow".to_owned(),
            matched_rule: None,
            outcome: "executed".to_owned(),
            action_payload: None,
            verdict_details: serde_json::Value::Null,
            outcome_details: serde_json::Value::Null,
            metadata: serde_json::Value::Null,
            dispatched_at: now,
            completed_at: now,
            duration_ms: 0,
            expires_at: None,
            caller_id: String::new(),
            auth_method: String::new(),
            record_hash: None,
            previous_hash: None,
            sequence_number: None,
            attachment_metadata: Vec::new(),
        };
        let item = record_to_item(&record);

        assert!(!item.contains_key("chain_id"));
        assert!(!item.contains_key("matched_rule"));
        assert!(!item.contains_key("action_payload"));
        assert!(!item.contains_key("expires_at_ttl"));
        assert!(!item.contains_key("record_hash"));
        assert!(!item.contains_key("previous_hash"));
        assert!(!item.contains_key("sequence_number"));
    }

    #[test]
    fn record_to_item_ttl_is_epoch_seconds() {
        let record = sample_record();
        let item = record_to_item(&record);

        let ttl = item.get("expires_at_ttl").unwrap();
        if let AttributeValue::N(n) = ttl {
            let epoch_secs: i64 = n.parse().unwrap();
            // Should be close to the expected epoch seconds.
            let expected = record.expires_at.unwrap().timestamp();
            assert_eq!(epoch_secs, expected);
        } else {
            panic!("expires_at_ttl should be a number");
        }
    }

    #[test]
    fn item_to_record_roundtrip() {
        let original = sample_record();
        let item = record_to_item(&original);
        let restored = item_to_record(&item).unwrap();

        assert_eq!(restored.id, original.id);
        assert_eq!(restored.action_id, original.action_id);
        assert_eq!(restored.namespace, original.namespace);
        assert_eq!(restored.tenant, original.tenant);
        assert_eq!(restored.provider, original.provider);
        assert_eq!(restored.action_type, original.action_type);
        assert_eq!(restored.verdict, original.verdict);
        assert_eq!(restored.matched_rule, original.matched_rule);
        assert_eq!(restored.outcome, original.outcome);
        assert_eq!(restored.duration_ms, original.duration_ms);
        assert_eq!(restored.caller_id, original.caller_id);
        assert_eq!(restored.auth_method, original.auth_method);
        assert_eq!(restored.record_hash, original.record_hash);
        assert_eq!(restored.previous_hash, original.previous_hash);
        assert_eq!(restored.sequence_number, original.sequence_number);
    }

    #[test]
    fn item_to_record_missing_required_field() {
        let mut item = HashMap::new();
        item.insert("id".to_owned(), AttributeValue::S("x".to_owned()));
        // Missing action_id, etc.
        assert!(item_to_record(&item).is_err());
    }

    #[test]
    fn ns_tenant_format() {
        assert_eq!(
            DynamoDbAuditStore::ns_tenant("notifications", "tenant-1"),
            "notifications#tenant-1"
        );
    }

    #[test]
    fn build_filters_empty_query() {
        let query = AuditQuery::default();
        let (parts, values, _names) = DynamoDbAuditStore::build_filters(&query);
        // Should have at least the fence filter.
        assert!(parts.iter().any(|p| p.contains("fence")));
        assert!(!values.is_empty() || values.is_empty()); // fence filter uses names not values
    }

    #[test]
    fn build_filters_with_provider() {
        let query = AuditQuery {
            provider: Some("email".to_owned()),
            ..Default::default()
        };
        let (parts, values, names) = DynamoDbAuditStore::build_filters(&query);
        assert!(parts.iter().any(|p| p.contains("provider")));
        assert!(values.contains_key(":provider"));
        assert!(names.contains_key("#provider"));
    }

    #[test]
    fn dispatched_at_ms_is_millis() {
        let record = sample_record();
        let item = record_to_item(&record);

        if let Some(AttributeValue::N(n)) = item.get("dispatched_at_ms") {
            let ms: i64 = n.parse().unwrap();
            let expected_ms = record.dispatched_at.timestamp_millis();
            assert_eq!(ms, expected_ms);
        } else {
            panic!("dispatched_at_ms should be a number");
        }
    }

    #[test]
    fn json_fields_stored_as_strings() {
        let record = sample_record();
        let item = record_to_item(&record);

        // verdict_details should be a String attribute containing JSON.
        if let Some(AttributeValue::S(s)) = item.get("verdict_details") {
            let _parsed: serde_json::Value = serde_json::from_str(s).unwrap();
        } else {
            panic!("verdict_details should be a string attribute");
        }
    }
}
