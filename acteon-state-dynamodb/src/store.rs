use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;

use acteon_state::error::StateError;
use acteon_state::key::{KeyKind, StateKey};
use acteon_state::store::{CasResult, StateStore};

use crate::config::DynamoConfig;
use crate::table::{build_pk, build_sk};

/// DynamoDB-backed implementation of [`StateStore`].
///
/// Uses a single `DynamoDB` table with composite primary key (`pk`, `sk`).
/// Versioning is tracked via a `version` attribute (numeric). TTL is stored
/// in `expires_at` as epoch seconds and checked on read; `DynamoDB`'s native
/// TTL feature can also be configured on the table for background cleanup.
pub struct DynamoStateStore {
    client: Client,
    table_name: String,
    prefix: String,
}

impl DynamoStateStore {
    /// Create a new `DynamoStateStore` from the provided configuration.
    ///
    /// Loads AWS credentials and configuration from the environment and
    /// optionally overrides the endpoint URL for local development.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Connection`] if the AWS SDK configuration fails.
    pub async fn new(config: &DynamoConfig) -> Result<Self, StateError> {
        let client = build_client(config).await;
        Ok(Self {
            client,
            table_name: config.table_name.clone(),
            prefix: config.key_prefix.clone(),
        })
    }

    /// Create a new `DynamoStateStore` from an existing `DynamoDB` client.
    ///
    /// Useful for sharing a client across the store and lock backends.
    pub fn from_client(client: Client, config: &DynamoConfig) -> Self {
        Self {
            client,
            table_name: config.table_name.clone(),
            prefix: config.key_prefix.clone(),
        }
    }

    /// Return the current epoch seconds.
    fn now_epoch() -> i64 {
        chrono::Utc::now().timestamp()
    }

    /// Compute the `expires_at` epoch seconds from an optional TTL.
    fn expires_at(ttl: Option<Duration>) -> Option<i64> {
        ttl.map(|d| {
            let secs = i64::try_from(d.as_secs()).unwrap_or(i64::MAX);
            Self::now_epoch().saturating_add(secs)
        })
    }

    /// Check if an item is expired based on its `expires_at` attribute.
    fn is_expired(item: &std::collections::HashMap<String, AttributeValue>) -> bool {
        if let Some(AttributeValue::N(expires_str)) = item.get("expires_at")
            && let Ok(expires_at) = expires_str.parse::<i64>()
        {
            return expires_at <= Self::now_epoch();
        }
        false
    }

    /// Delete an item by its pk and sk.
    async fn delete_item(&self, pk: &str, sk: &str) -> Result<(), StateError> {
        self.client
            .delete_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk.to_owned()))
            .key("sk", AttributeValue::S(sk.to_owned()))
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;
        Ok(())
    }
}

#[async_trait]
impl StateStore for DynamoStateStore {
    async fn check_and_set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<bool, StateError> {
        let pk = build_pk(&self.prefix, key);
        let sk = build_sk(key);

        // First try: conditional put with attribute_not_exists.
        let mut put = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item("pk", AttributeValue::S(pk.clone()))
            .item("sk", AttributeValue::S(sk.clone()))
            .item("value", AttributeValue::S(value.to_owned()))
            .item("version", AttributeValue::N("1".to_owned()))
            .condition_expression("attribute_not_exists(pk)");

        if let Some(exp) = Self::expires_at(ttl) {
            put = put.item("expires_at", AttributeValue::N(exp.to_string()));
        }

        let result = put.send().await;

        match result {
            Ok(_) => Ok(true),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_conditional_check_failed_exception() {
                    // Item exists. Check if it's expired.
                    let get_result = self
                        .client
                        .get_item()
                        .table_name(&self.table_name)
                        .key("pk", AttributeValue::S(pk.clone()))
                        .key("sk", AttributeValue::S(sk.clone()))
                        .send()
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;

                    if let Some(item) = get_result.item()
                        && Self::is_expired(item)
                    {
                        // Item is expired, delete it and retry.
                        self.delete_item(&pk, &sk).await?;
                        return self.check_and_set(key, value, ttl).await;
                    }

                    // Item exists and is not expired.
                    Ok(false)
                } else {
                    Err(StateError::Backend(service_err.to_string()))
                }
            }
        }
    }

    async fn get(&self, key: &StateKey) -> Result<Option<String>, StateError> {
        let pk = build_pk(&self.prefix, key);
        let sk = build_sk(key);

        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some(item) = result.item() else {
            return Ok(None);
        };

        // Check expiry: treat expired items as missing.
        if Self::is_expired(item) {
            return Ok(None);
        }

        match item.get("value") {
            Some(AttributeValue::S(v)) => Ok(Some(v.clone())),
            _ => Ok(None),
        }
    }

    async fn set(
        &self,
        key: &StateKey,
        value: &str,
        ttl: Option<Duration>,
    ) -> Result<(), StateError> {
        let pk = build_pk(&self.prefix, key);
        let sk = build_sk(key);

        // Use UpdateItem with SET to upsert and atomically increment the version.
        let mut update = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .update_expression("SET #val = :val, version = if_not_exists(version, :zero) + :one")
            .expression_attribute_names("#val", "value")
            .expression_attribute_values(":val", AttributeValue::S(value.to_owned()))
            .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
            .expression_attribute_values(":one", AttributeValue::N("1".to_owned()));

        if let Some(exp) = Self::expires_at(ttl) {
            update = update
                .update_expression(
                    "SET #val = :val, version = if_not_exists(version, :zero) + :one, expires_at = :exp",
                )
                .expression_attribute_values(":exp", AttributeValue::N(exp.to_string()));
        } else {
            update = update.update_expression(
                "SET #val = :val, version = if_not_exists(version, :zero) + :one REMOVE expires_at",
            );
        }

        update
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        Ok(())
    }

    async fn delete(&self, key: &StateKey) -> Result<bool, StateError> {
        let pk = build_pk(&self.prefix, key);
        let sk = build_sk(key);

        // First check if the item exists and is not expired.
        let get_result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk.clone()))
            .key("sk", AttributeValue::S(sk.clone()))
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some(item) = get_result.item() else {
            return Ok(false);
        };

        if Self::is_expired(item) {
            // Clean up the expired item but report as not existing.
            self.delete_item(&pk, &sk).await?;
            return Ok(false);
        }

        // Item exists and is not expired, delete it.
        self.delete_item(&pk, &sk).await?;
        Ok(true)
    }

    async fn increment(
        &self,
        key: &StateKey,
        delta: i64,
        ttl: Option<Duration>,
    ) -> Result<i64, StateError> {
        let pk = build_pk(&self.prefix, key);
        let sk = build_sk(key);

        // Check if there is an existing expired item and delete it first.
        let get_result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk.clone()))
            .key("sk", AttributeValue::S(sk.clone()))
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        if let Some(item) = get_result.item()
            && Self::is_expired(item)
        {
            self.delete_item(&pk, &sk).await?;
        }

        // Use ADD to atomically increment (or create from 0).
        // The value attribute stores the counter as a number.
        let mut update = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk))
            .key("sk", AttributeValue::S(sk))
            .expression_attribute_names("#val", "value")
            .expression_attribute_values(":delta", AttributeValue::N(delta.to_string()))
            .expression_attribute_values(":zero", AttributeValue::N("0".to_owned()))
            .expression_attribute_values(":one", AttributeValue::N("1".to_owned()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllNew);

        if let Some(exp) = Self::expires_at(ttl) {
            update = update
                .update_expression(
                    "SET version = if_not_exists(version, :zero) + :one, \
                     expires_at = :exp ADD #val :delta",
                )
                .expression_attribute_values(":exp", AttributeValue::N(exp.to_string()));
        } else {
            update = update.update_expression(
                "SET version = if_not_exists(version, :zero) + :one ADD #val :delta",
            );
        }

        let result = update
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        parse_counter_value(result.attributes())
    }

    async fn compare_and_swap(
        &self,
        key: &StateKey,
        expected_version: u64,
        new_value: &str,
        ttl: Option<Duration>,
    ) -> Result<CasResult, StateError> {
        let pk = build_pk(&self.prefix, key);
        let sk = build_sk(key);

        // Conditional update: version must match expected.
        let mut update = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(pk.clone()))
            .key("sk", AttributeValue::S(sk.clone()))
            .condition_expression("version = :expected")
            .expression_attribute_names("#val", "value")
            .expression_attribute_values(
                ":expected",
                AttributeValue::N(expected_version.to_string()),
            )
            .expression_attribute_values(":new_val", AttributeValue::S(new_value.to_owned()))
            .expression_attribute_values(":one", AttributeValue::N("1".to_owned()));

        if let Some(exp) = Self::expires_at(ttl) {
            update = update
                .update_expression(
                    "SET #val = :new_val, version = version + :one, expires_at = :exp",
                )
                .expression_attribute_values(":exp", AttributeValue::N(exp.to_string()));
        } else {
            update = update.update_expression(
                "SET #val = :new_val, version = version + :one REMOVE expires_at",
            );
        }

        let result = update.send().await;

        match result {
            Ok(_) => Ok(CasResult::Ok),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_conditional_check_failed_exception() {
                    // Read current state for the conflict response.
                    let get_result = self
                        .client
                        .get_item()
                        .table_name(&self.table_name)
                        .key("pk", AttributeValue::S(pk))
                        .key("sk", AttributeValue::S(sk))
                        .send()
                        .await
                        .map_err(|e| StateError::Backend(e.to_string()))?;

                    let (current_value, current_version) = match get_result.item() {
                        Some(item) => {
                            let val = match item.get("value") {
                                Some(AttributeValue::S(v)) => Some(v.clone()),
                                _ => None,
                            };
                            let ver = match item.get("version") {
                                Some(AttributeValue::N(n)) => n.parse::<u64>().unwrap_or(0),
                                _ => 0,
                            };
                            (val, ver)
                        }
                        None => (None, 0),
                    };

                    Ok(CasResult::Conflict {
                        current_value,
                        current_version,
                    })
                } else {
                    Err(StateError::Backend(service_err.to_string()))
                }
            }
        }
    }

    async fn scan_keys(
        &self,
        namespace: &str,
        tenant: &str,
        kind: KeyKind,
        prefix: Option<&str>,
    ) -> Result<Vec<(String, String)>, StateError> {
        let pk = format!("{}:{}:{}", self.prefix, namespace, tenant);
        let sk_prefix = match prefix {
            Some(p) => format!("{kind}:{p}"),
            None => format!("{kind}:"),
        };

        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut results = Vec::new();
        let mut exclusive_start_key = None;

        loop {
            let mut query = self
                .client
                .query()
                .table_name(&self.table_name)
                .key_condition_expression("pk = :pk AND begins_with(sk, :sk_prefix)")
                .expression_attribute_values(":pk", AttributeValue::S(pk.clone()))
                .expression_attribute_values(":sk_prefix", AttributeValue::S(sk_prefix.clone()))
                .expression_attribute_values(":now", AttributeValue::N(now_epoch.to_string()))
                .filter_expression("attribute_not_exists(expires_at) OR expires_at > :now");

            if let Some(key) = exclusive_start_key {
                query = query.set_exclusive_start_key(Some(key));
            }

            let response = query
                .send()
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;

            for item in response.items() {
                let key = match item.get("sk") {
                    Some(AttributeValue::S(s)) => format!("{namespace}:{tenant}:{s}"),
                    _ => continue,
                };
                let value = match item.get("value") {
                    Some(AttributeValue::S(v)) => v.clone(),
                    _ => continue,
                };
                results.push((key, value));
            }

            exclusive_start_key = response.last_evaluated_key().cloned();
            if exclusive_start_key.is_none() {
                break;
            }
        }

        Ok(results)
    }

    async fn scan_keys_by_kind(&self, kind: KeyKind) -> Result<Vec<(String, String)>, StateError> {
        // For DynamoDB, we need to scan the entire table and filter by kind.
        // This is expensive but necessary for global scans.
        let sk_prefix = format!("{kind}:");

        let now_epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let mut results = Vec::new();
        let mut exclusive_start_key = None;

        loop {
            let mut scan = self
                .client
                .scan()
                .table_name(&self.table_name)
                .filter_expression(
                    "begins_with(sk, :sk_prefix) AND \
                     (attribute_not_exists(expires_at) OR expires_at > :now)",
                )
                .expression_attribute_values(":sk_prefix", AttributeValue::S(sk_prefix.clone()))
                .expression_attribute_values(":now", AttributeValue::N(now_epoch.to_string()));

            if let Some(key) = exclusive_start_key {
                scan = scan.set_exclusive_start_key(Some(key));
            }

            let response = scan
                .send()
                .await
                .map_err(|e| StateError::Backend(e.to_string()))?;

            for item in response.items() {
                // Extract pk to get namespace:tenant
                let pk = match item.get("pk") {
                    Some(AttributeValue::S(s)) => s.clone(),
                    _ => continue,
                };
                // pk format: {prefix}:{namespace}:{tenant}
                // Strip the prefix to get namespace:tenant
                let ns_tenant = pk.strip_prefix(&format!("{}:", self.prefix)).unwrap_or(&pk);

                let sk = match item.get("sk") {
                    Some(AttributeValue::S(s)) => s.clone(),
                    _ => continue,
                };
                let value = match item.get("value") {
                    Some(AttributeValue::S(v)) => v.clone(),
                    _ => continue,
                };

                // Reconstruct the key as namespace:tenant:kind:id
                let key = format!("{ns_tenant}:{sk}");
                results.push((key, value));
            }

            exclusive_start_key = response.last_evaluated_key().cloned();
            if exclusive_start_key.is_none() {
                break;
            }
        }

        Ok(results)
    }
}

/// Parse the counter value from an `UpdateItem` response.
fn parse_counter_value(
    attrs: Option<&std::collections::HashMap<String, AttributeValue>>,
) -> Result<i64, StateError> {
    let attrs = attrs
        .ok_or_else(|| StateError::Backend("UpdateItem did not return attributes".to_owned()))?;

    match attrs.get("value") {
        Some(AttributeValue::N(n)) => n
            .parse::<i64>()
            .map_err(|e| StateError::Serialization(e.to_string())),
        _ => Err(StateError::Backend(
            "counter value attribute missing or wrong type".to_owned(),
        )),
    }
}

/// Build an AWS `DynamoDB` [`Client`] from the provided configuration.
///
/// Uses the standard AWS SDK environment credential chain and optionally
/// overrides the endpoint URL for local development.
pub async fn build_client(config: &DynamoConfig) -> Client {
    let mut aws_config =
        aws_config::from_env().region(aws_config::Region::new(config.region.clone()));

    if let Some(endpoint) = &config.endpoint_url {
        aws_config = aws_config.endpoint_url(endpoint);
    }

    let sdk_config = aws_config.load().await;
    Client::new(&sdk_config)
}

#[cfg(all(test, feature = "integration"))]
mod integration_tests {
    use super::*;
    use crate::config::DynamoConfig;
    use crate::table::create_table;

    fn test_config() -> DynamoConfig {
        DynamoConfig {
            table_name: std::env::var("DYNAMODB_TABLE")
                .unwrap_or_else(|_| "acteon_state_test".to_owned()),
            endpoint_url: Some(
                std::env::var("DYNAMODB_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:8000".to_owned()),
            ),
            key_prefix: format!("test-{}", uuid::Uuid::new_v4()),
            ..DynamoConfig::default()
        }
    }

    #[tokio::test]
    async fn store_conformance() {
        let config = test_config();
        let store = DynamoStateStore::new(&config)
            .await
            .expect("client creation should succeed");
        create_table(&store.client, &store.table_name)
            .await
            .expect("table creation should succeed");
        acteon_state::testing::run_store_conformance_tests(&store)
            .await
            .expect("conformance tests should pass");
    }
}
