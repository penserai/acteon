use sqlx::PgPool;

use crate::config::PostgresConfig;

/// Run database migrations, creating required tables if they do not exist.
///
/// This creates the state and locks tables in the configured schema with the
/// configured table prefix.
///
/// # Errors
///
/// Returns a [`sqlx::Error`] if any DDL statement fails.
pub async fn run_migrations(pool: &PgPool, config: &PostgresConfig) -> Result<(), sqlx::Error> {
    let state_table = config.state_table();
    let locks_table = config.locks_table();

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

    sqlx::query(&create_state).execute(pool).await?;
    sqlx::query(&create_locks).execute(pool).await?;

    Ok(())
}
