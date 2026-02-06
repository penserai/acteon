use sqlx::PgPool;

use crate::config::PostgresConfig;

/// Run database migrations, creating required tables if they do not exist.
///
/// This creates the state, locks, and `timeout_index` tables in the configured
/// schema with the configured table prefix.
///
/// # Errors
///
/// Returns a [`sqlx::Error`] if any DDL statement fails.
pub async fn run_migrations(pool: &PgPool, config: &PostgresConfig) -> Result<(), sqlx::Error> {
    let state_table = config.state_table();
    let locks_table = config.locks_table();
    let timeout_index_table = config.timeout_index_table();

    let create_state = format!(
        "CREATE TABLE IF NOT EXISTS {state_table} (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            version BIGINT NOT NULL DEFAULT 1,
            expires_at TIMESTAMPTZ
        )"
    );

    let create_locks = format!(
        "CREATE TABLE IF NOT EXISTS {locks_table} (
            name TEXT PRIMARY KEY,
            owner TEXT NOT NULL,
            expires_at TIMESTAMPTZ NOT NULL
        )"
    );

    // Timeout index table for efficient O(log N) queries on expired timeouts.
    // The index on expires_at allows efficient range queries.
    let create_timeout_index = format!(
        "CREATE TABLE IF NOT EXISTS {timeout_index_table} (
            key TEXT PRIMARY KEY,
            expires_at_ms BIGINT NOT NULL
        )"
    );

    let create_timeout_index_idx = format!(
        "CREATE INDEX IF NOT EXISTS {}_expires_at_idx ON {timeout_index_table} (expires_at_ms)",
        config.table_prefix
    );

    // Chain ready index table for efficient O(log N) queries on ready chains.
    let chain_ready_index_table = config.chain_ready_index_table();
    let create_chain_ready_index = format!(
        "CREATE TABLE IF NOT EXISTS {chain_ready_index_table} (
            key TEXT PRIMARY KEY,
            ready_at_ms BIGINT NOT NULL
        )"
    );

    let create_chain_ready_index_idx = format!(
        "CREATE INDEX IF NOT EXISTS {}chain_ready_at_idx ON {chain_ready_index_table} (ready_at_ms)",
        config.table_prefix
    );

    sqlx::query(&create_state).execute(pool).await?;
    sqlx::query(&create_locks).execute(pool).await?;
    sqlx::query(&create_timeout_index).execute(pool).await?;
    sqlx::query(&create_timeout_index_idx).execute(pool).await?;
    sqlx::query(&create_chain_ready_index).execute(pool).await?;
    sqlx::query(&create_chain_ready_index_idx)
        .execute(pool)
        .await?;

    Ok(())
}
