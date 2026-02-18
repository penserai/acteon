/// Run the audit table migration, creating the table if it does not already
/// exist.
///
/// The table uses `MergeTree()` with an ordering key of
/// `(namespace, tenant, dispatched_at)` to optimise the most common query
/// patterns (filter by namespace/tenant, sort by time).
///
/// JSON fields (`action_payload`, `verdict_details`, `outcome_details`,
/// `metadata`) are stored as `String` because `ClickHouse`'s native JSON column
/// type is still experimental. Values are serialised to / deserialised from
/// JSON text at the application layer.
///
/// # Errors
///
/// Returns a [`clickhouse::error::Error`] if the DDL statement fails.
pub async fn run_migrations(
    client: &clickhouse::Client,
    prefix: &str,
) -> Result<(), clickhouse::error::Error> {
    let table = format!("{prefix}audit");

    let create_table = format!(
        "CREATE TABLE IF NOT EXISTS {table} (
            id              String,
            action_id       String,
            namespace       String,
            tenant          String,
            provider        String,
            action_type     String,
            verdict         String,
            matched_rule    Nullable(String),
            outcome         String,
            action_payload  Nullable(String),
            verdict_details String,
            outcome_details String,
            metadata        String,
            dispatched_at   DateTime64(3, 'UTC'),
            completed_at    DateTime64(3, 'UTC'),
            duration_ms     UInt64,
            expires_at      Nullable(DateTime64(3, 'UTC')),
            caller_id       String DEFAULT '',
            auth_method     String DEFAULT ''
        ) ENGINE = MergeTree()
        ORDER BY (namespace, tenant, dispatched_at)"
    );

    client.query(&create_table).execute().await?;

    // Add caller columns to existing tables (idempotent).
    let add_columns = [
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS caller_id String DEFAULT ''"),
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS auth_method String DEFAULT ''"),
    ];
    for stmt in &add_columns {
        client.query(stmt).execute().await?;
    }

    // Add chain_id column (idempotent).
    let chain_id_stmt =
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS chain_id Nullable(String)");
    client.query(&chain_id_stmt).execute().await?;

    // Add hash chain columns for compliance mode (idempotent).
    let hash_chain_stmts = [
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS record_hash Nullable(String)"),
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS previous_hash Nullable(String)"),
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS sequence_number Nullable(UInt64)"),
    ];
    for stmt in &hash_chain_stmts {
        client.query(stmt).execute().await?;
    }

    Ok(())
}
