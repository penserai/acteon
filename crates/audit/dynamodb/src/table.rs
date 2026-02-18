use aws_sdk_dynamodb::Client;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType, TimeToLiveSpecification,
};

/// Create the `DynamoDB` audit table programmatically.
///
/// The table uses a single partition key `id` (String) with three Global
/// Secondary Indexes for efficient query patterns:
///
/// - **`ns_tenant_dispatched`** — PK: `ns_tenant`, SK: `dispatched_at_ms`
/// - **`ns_tenant_sequence`** — PK: `ns_tenant`, SK: `sequence_number`
/// - **`action_id_index`** — PK: `action_id`, SK: `dispatched_at_ms`
///
/// Uses `PAY_PER_REQUEST` billing mode and enables native TTL on `expires_at_ttl`.
///
/// This is intended for tests and local development. In production you would
/// typically provision the table via Infrastructure-as-Code tooling.
///
/// # Errors
///
/// Returns an error if the `CreateTable` call fails for reasons other than
/// the table already existing.
#[allow(clippy::too_many_lines)]
pub async fn create_audit_table(
    client: &Client,
    table_name: &str,
) -> Result<(), aws_sdk_dynamodb::Error> {
    let result = client
        .create_table()
        .table_name(table_name)
        // Primary key: id (String)
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("id")
                .key_type(KeyType::Hash)
                .build()
                .expect("valid key schema"),
        )
        // Attribute definitions for all keys used in table + GSIs
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("valid attribute definition"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("ns_tenant")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("valid attribute definition"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("dispatched_at_ms")
                .attribute_type(ScalarAttributeType::N)
                .build()
                .expect("valid attribute definition"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("sequence_number")
                .attribute_type(ScalarAttributeType::N)
                .build()
                .expect("valid attribute definition"),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("action_id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .expect("valid attribute definition"),
        )
        // GSI 1: ns_tenant_dispatched (query by namespace+tenant sorted by time)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("ns_tenant_dispatched")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("ns_tenant")
                        .key_type(KeyType::Hash)
                        .build()
                        .expect("valid key schema"),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("dispatched_at_ms")
                        .key_type(KeyType::Range)
                        .build()
                        .expect("valid key schema"),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .expect("valid GSI"),
        )
        // GSI 2: ns_tenant_sequence (hash chain tip + ascending seq queries)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("ns_tenant_sequence")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("ns_tenant")
                        .key_type(KeyType::Hash)
                        .build()
                        .expect("valid key schema"),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("sequence_number")
                        .key_type(KeyType::Range)
                        .build()
                        .expect("valid key schema"),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .expect("valid GSI"),
        )
        // GSI 3: action_id_index (get_by_action_id lookups)
        .global_secondary_indexes(
            GlobalSecondaryIndex::builder()
                .index_name("action_id_index")
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("action_id")
                        .key_type(KeyType::Hash)
                        .build()
                        .expect("valid key schema"),
                )
                .key_schema(
                    KeySchemaElement::builder()
                        .attribute_name("dispatched_at_ms")
                        .key_type(KeyType::Range)
                        .build()
                        .expect("valid key schema"),
                )
                .projection(
                    Projection::builder()
                        .projection_type(ProjectionType::All)
                        .build(),
                )
                .build()
                .expect("valid GSI"),
        )
        .billing_mode(BillingMode::PayPerRequest)
        .send()
        .await;

    match result {
        Ok(_) => {}
        Err(err) => {
            let service_err = err.into_service_error();
            if !service_err.is_resource_in_use_exception() {
                return Err(service_err.into());
            }
        }
    }

    // Enable TTL on expires_at_ttl attribute.
    let ttl_result = client
        .update_time_to_live()
        .table_name(table_name)
        .time_to_live_specification(
            TimeToLiveSpecification::builder()
                .enabled(true)
                .attribute_name("expires_at_ttl")
                .build()
                .expect("valid TTL spec"),
        )
        .send()
        .await;

    // Tolerate "TTL already enabled" errors.
    if let Err(err) = ttl_result {
        let msg = err.to_string();
        if !msg.contains("already enabled") && !msg.contains("TimeToLive is already enabled") {
            tracing::warn!(error = %msg, "failed to enable TTL on audit table (non-fatal)");
        }
    }

    Ok(())
}
