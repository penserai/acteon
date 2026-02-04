use crate::config::ClickHouseConfig;

/// Run database migrations, creating required tables if they do not exist.
///
/// This creates the state and locks tables using `ReplacingMergeTree` engines
/// appropriate for `ClickHouse`'s append-only storage model:
///
/// - The **state table** uses `ReplacingMergeTree(version)` ordered by `key`,
///   with a soft-delete `is_deleted` flag and optional `expires_at` timestamp.
/// - The **locks table** uses `ReplacingMergeTree(version)` ordered by `name`,
///   tracking ownership and expiration per lock.
///
/// # Errors
///
/// Returns a [`clickhouse::error::Error`] if any DDL statement fails.
pub async fn run_migrations(
    client: &clickhouse::Client,
    config: &ClickHouseConfig,
) -> Result<(), clickhouse::error::Error> {
    let state_table = config.state_table();
    let locks_table = config.locks_table();
    let timeout_index_table = config.timeout_index_table();

    let create_state = format!(
        "CREATE TABLE IF NOT EXISTS {state_table} (
            key String,
            value String,
            version UInt64,
            is_deleted UInt8 DEFAULT 0,
            expires_at Nullable(DateTime64(3, 'UTC'))
        ) ENGINE = ReplacingMergeTree(version)
        ORDER BY key"
    );

    let create_locks = format!(
        "CREATE TABLE IF NOT EXISTS {locks_table} (
            name String,
            owner String,
            expires_at DateTime64(3, 'UTC'),
            version UInt64
        ) ENGINE = ReplacingMergeTree(version)
        ORDER BY name"
    );

    // Timeout index table for efficient O(log N) queries on expired timeouts.
    // Uses ReplacingMergeTree with version for upsert semantics.
    // ORDER BY expires_at_ms allows efficient range queries.
    let create_timeout_index = format!(
        "CREATE TABLE IF NOT EXISTS {timeout_index_table} (
            key String,
            expires_at_ms Int64,
            version UInt64 DEFAULT 1,
            is_deleted UInt8 DEFAULT 0
        ) ENGINE = ReplacingMergeTree(version)
        ORDER BY (expires_at_ms, key)"
    );

    client.query(&create_state).execute().await?;
    client.query(&create_locks).execute().await?;
    client.query(&create_timeout_index).execute().await?;

    Ok(())
}
