use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, KeySchemaElement, KeyType, ProvisionedThroughput, ScalarAttributeType,
};

use acteon_state::key::StateKey;

/// Build the partition key (PK) from a prefix and state key.
///
/// Format: `{prefix}:{namespace}:{tenant}`
pub fn build_pk(prefix: &str, key: &StateKey) -> String {
    format!("{}:{}:{}", prefix, key.namespace, key.tenant)
}

/// Build the sort key (SK) from a state key.
///
/// Format: `{kind}:{id}`
pub fn build_sk(key: &StateKey) -> String {
    format!("{}:{}", key.kind, key.id)
}

/// Build a sort key for a distributed lock entry.
///
/// Format: `_lock:{name}`
pub fn build_lock_sk(name: &str) -> String {
    format!("_lock:{name}")
}

/// Create the `DynamoDB` table programmatically.
///
/// The table uses a composite primary key with:
/// - `pk` (String) as the partition key
/// - `sk` (String) as the sort key
///
/// This is intended for tests and local development. In production you would
/// typically provision the table via Infrastructure-as-Code tooling.
///
/// # Errors
///
/// Returns an error if the `CreateTable` call fails for reasons other than
/// the table already existing.
pub async fn create_table(
    client: &Client,
    table_name: &str,
) -> Result<(), aws_sdk_dynamodb::Error> {
    let result = client
        .create_table()
        .table_name(table_name)
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("pk")
                .key_type(KeyType::Hash)
                .build()
                .expect("valid key schema"),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("sk")
                .key_type(KeyType::Range)
                .build()
                .expect("valid key schema"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("pk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("valid attribute definition"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sk")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("valid attribute definition"),
        )
        .provisioned_throughput(
            ProvisionedThroughput::builder()
                .read_capacity_units(5)
                .write_capacity_units(5)
                .build()
                .expect("valid throughput"),
        )
        .send()
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(err) => {
            // Tolerate "table already exists" errors so `create_table` is idempotent.
            let service_err = err.into_service_error();
            if service_err.is_resource_in_use_exception() {
                Ok(())
            } else {
                Err(service_err.into())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use acteon_state::key::{KeyKind, StateKey};

    use super::*;

    #[test]
    fn pk_format() {
        let key = StateKey::new("notif", "tenant-1", KeyKind::State, "abc");
        assert_eq!(build_pk("acteon", &key), "acteon:notif:tenant-1");
    }

    #[test]
    fn sk_format() {
        let key = StateKey::new("ns", "t", KeyKind::Dedup, "abc-123");
        assert_eq!(build_sk(&key), "dedup:abc-123");
    }

    #[test]
    fn lock_sk_format() {
        assert_eq!(build_lock_sk("my-lock"), "_lock:my-lock");
    }

    #[test]
    fn pk_with_custom_prefix() {
        let key = StateKey::new("ns", "t", KeyKind::Counter, "id");
        assert_eq!(build_pk("myapp", &key), "myapp:ns:t");
    }
}
