use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::AttributeValue;
use tokio::time::Instant;

use acteon_state::error::StateError;
use acteon_state::lock::{DistributedLock, LockGuard};

use crate::config::DynamoConfig;
use crate::store::build_client;
use crate::table::build_lock_sk;

/// Retry interval when polling for lock acquisition.
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Partition key used for all lock entries.
const LOCK_PK_SUFFIX: &str = "_locks";

/// DynamoDB-backed implementation of [`DistributedLock`].
///
/// Lock entries are stored in the same table as state entries using a dedicated
/// partition key format `{prefix}:_locks` and a sort key of `_lock:{name}`.
/// Lock expiry is enforced via an `expires_at` attribute (epoch seconds).
pub struct DynamoDistributedLock {
    client: Client,
    table_name: String,
    prefix: String,
}

impl DynamoDistributedLock {
    /// Create a new `DynamoDistributedLock` from the provided configuration.
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

    /// Create a new `DynamoDistributedLock` from an existing `DynamoDB` client.
    ///
    /// Useful for sharing a client across the store and lock backends.
    pub fn from_client(client: Client, config: &DynamoConfig) -> Self {
        Self {
            client,
            table_name: config.table_name.clone(),
            prefix: config.key_prefix.clone(),
        }
    }

    /// Build the partition key for lock entries.
    fn lock_pk(&self) -> String {
        format!("{}:{LOCK_PK_SUFFIX}", self.prefix)
    }

    /// Return the current epoch seconds.
    fn now_epoch() -> i64 {
        chrono::Utc::now().timestamp()
    }
}

#[async_trait]
impl DistributedLock for DynamoDistributedLock {
    async fn try_acquire(
        &self,
        name: &str,
        ttl: Duration,
    ) -> Result<Option<Box<dyn LockGuard>>, StateError> {
        let pk = self.lock_pk();
        let sk = build_lock_sk(name);
        let owner = uuid::Uuid::new_v4().to_string();
        let expires_at_secs =
            Self::now_epoch().saturating_add(i64::try_from(ttl.as_secs()).unwrap_or(i64::MAX));

        // Conditional put: only succeed if the item does not exist or is expired.
        let result = self
            .client
            .put_item()
            .table_name(&self.table_name)
            .item("pk", AttributeValue::S(pk.clone()))
            .item("sk", AttributeValue::S(sk.clone()))
            .item("owner", AttributeValue::S(owner.clone()))
            .item("expires_at", AttributeValue::N(expires_at_secs.to_string()))
            .condition_expression("attribute_not_exists(pk) OR expires_at < :now")
            .expression_attribute_values(":now", AttributeValue::N(Self::now_epoch().to_string()))
            .send()
            .await;

        match result {
            Ok(_) => Ok(Some(Box::new(DynamoLockGuard {
                client: self.client.clone(),
                table_name: self.table_name.clone(),
                pk,
                sk,
                owner,
            }))),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_conditional_check_failed_exception() {
                    // Lock is held by another owner and not expired.
                    Ok(None)
                } else {
                    Err(StateError::Backend(service_err.to_string()))
                }
            }
        }
    }

    async fn acquire(
        &self,
        name: &str,
        ttl: Duration,
        timeout: Duration,
    ) -> Result<Box<dyn LockGuard>, StateError> {
        let deadline = Instant::now() + timeout;

        loop {
            if let Some(guard) = self.try_acquire(name, ttl).await? {
                return Ok(guard);
            }

            if Instant::now() >= deadline {
                return Err(StateError::Timeout(timeout));
            }

            let remaining = deadline - Instant::now();
            let sleep_dur = LOCK_POLL_INTERVAL.min(remaining);
            tokio::time::sleep(sleep_dur).await;
        }
    }
}

/// A held distributed lock backed by `DynamoDB`.
///
/// Dropping the guard without calling [`release`](LockGuard::release) is safe;
/// the lock will expire after its TTL. Explicit release is preferred for prompt
/// cleanup.
pub struct DynamoLockGuard {
    client: Client,
    table_name: String,
    pk: String,
    sk: String,
    owner: String,
}

#[async_trait]
impl LockGuard for DynamoLockGuard {
    async fn extend(&self, duration: Duration) -> Result<(), StateError> {
        let new_expires = DynamoDistributedLock::now_epoch()
            .saturating_add(i64::try_from(duration.as_secs()).unwrap_or(i64::MAX));

        let result = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(self.pk.clone()))
            .key("sk", AttributeValue::S(self.sk.clone()))
            .update_expression("SET expires_at = :new_exp")
            .condition_expression("owner = :owner AND expires_at > :now")
            .expression_attribute_values(":owner", AttributeValue::S(self.owner.clone()))
            .expression_attribute_values(
                ":now",
                AttributeValue::N(DynamoDistributedLock::now_epoch().to_string()),
            )
            .expression_attribute_values(":new_exp", AttributeValue::N(new_expires.to_string()))
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_conditional_check_failed_exception() {
                    Err(StateError::LockExpired(format!(
                        "lock {} is no longer held by this owner",
                        self.sk
                    )))
                } else {
                    Err(StateError::Backend(service_err.to_string()))
                }
            }
        }
    }

    async fn release(self: Box<Self>) -> Result<(), StateError> {
        let result = self
            .client
            .delete_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(self.pk.clone()))
            .key("sk", AttributeValue::S(self.sk.clone()))
            .condition_expression("owner = :owner")
            .expression_attribute_values(":owner", AttributeValue::S(self.owner.clone()))
            .send()
            .await;

        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                let service_err = err.into_service_error();
                if service_err.is_conditional_check_failed_exception() {
                    Err(StateError::LockExpired(format!(
                        "lock {} was not held by this owner at release time",
                        self.sk
                    )))
                } else {
                    Err(StateError::Backend(service_err.to_string()))
                }
            }
        }
    }

    async fn is_held(&self) -> Result<bool, StateError> {
        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .key("pk", AttributeValue::S(self.pk.clone()))
            .key("sk", AttributeValue::S(self.sk.clone()))
            .send()
            .await
            .map_err(|e| StateError::Backend(e.to_string()))?;

        let Some(item) = result.item() else {
            return Ok(false);
        };

        let owner_matches = match item.get("owner") {
            Some(AttributeValue::S(o)) => o == &self.owner,
            _ => false,
        };

        let not_expired = match item.get("expires_at") {
            Some(AttributeValue::N(n)) => {
                n.parse::<i64>().unwrap_or(0) > DynamoDistributedLock::now_epoch()
            }
            _ => false,
        };

        Ok(owner_matches && not_expired)
    }
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
    async fn lock_conformance() {
        let config = test_config();
        let lock = DynamoDistributedLock::new(&config)
            .await
            .expect("client creation should succeed");
        create_table(&lock.client, &lock.table_name)
            .await
            .expect("table creation should succeed");
        acteon_state::testing::run_lock_conformance_tests(&lock)
            .await
            .expect("lock conformance tests should pass");
    }
}
