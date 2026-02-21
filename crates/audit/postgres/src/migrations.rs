use sqlx::PgPool;

/// Run the audit table migration, creating the table and indexes if they do
/// not already exist.
pub async fn run_migrations(pool: &PgPool, prefix: &str) -> Result<(), sqlx::Error> {
    let table = format!("{prefix}audit");

    let create_table = format!(
        "
        CREATE TABLE IF NOT EXISTS {table} (
            id              TEXT PRIMARY KEY,
            action_id       TEXT NOT NULL,
            namespace       TEXT NOT NULL,
            tenant          TEXT NOT NULL,
            provider        TEXT NOT NULL,
            action_type     TEXT NOT NULL,
            verdict         TEXT NOT NULL,
            matched_rule    TEXT,
            outcome         TEXT NOT NULL,
            action_payload  JSONB,
            verdict_details JSONB NOT NULL,
            outcome_details JSONB NOT NULL,
            metadata        JSONB NOT NULL DEFAULT '{{}}'::jsonb,
            dispatched_at   TIMESTAMPTZ NOT NULL,
            completed_at    TIMESTAMPTZ NOT NULL,
            duration_ms     BIGINT NOT NULL,
            expires_at      TIMESTAMPTZ,
            caller_id       TEXT NOT NULL DEFAULT '',
            auth_method     TEXT NOT NULL DEFAULT ''
        )
        "
    );

    sqlx::query(&create_table).execute(pool).await?;

    let indexes = [
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{prefix}audit_ns_tenant_time ON {table} (namespace, tenant, dispatched_at DESC)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{prefix}audit_outcome ON {table} (outcome, dispatched_at DESC)"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{prefix}audit_provider ON {table} (provider, dispatched_at DESC)"
        ),
        format!("CREATE INDEX IF NOT EXISTS idx_{prefix}audit_action_id ON {table} (action_id)"),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{prefix}audit_expires ON {table} (expires_at) WHERE expires_at IS NOT NULL"
        ),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{prefix}audit_metadata ON {table} USING GIN (metadata)"
        ),
    ];

    for idx in &indexes {
        sqlx::query(idx).execute(pool).await?;
    }

    // Add caller columns to existing tables (idempotent).
    let add_columns = [
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS caller_id TEXT NOT NULL DEFAULT ''"),
        format!(
            "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS auth_method TEXT NOT NULL DEFAULT ''"
        ),
    ];
    for stmt in &add_columns {
        sqlx::query(stmt).execute(pool).await?;
    }

    // Add chain_id column (idempotent).
    let chain_id_stmts = [
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS chain_id TEXT"),
        format!(
            "CREATE INDEX IF NOT EXISTS idx_{prefix}audit_chain_id ON {table} (chain_id) WHERE chain_id IS NOT NULL"
        ),
    ];
    for stmt in &chain_id_stmts {
        sqlx::query(stmt).execute(pool).await?;
    }

    // Add hash chain columns for compliance mode (idempotent).
    // The UNIQUE index on (namespace, tenant, sequence_number) enforces
    // optimistic concurrency: if two gateway replicas race for the same
    // sequence number, one will get a unique constraint violation and retry.
    let hash_chain_stmts = [
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS record_hash TEXT"),
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS previous_hash TEXT"),
        format!("ALTER TABLE {table} ADD COLUMN IF NOT EXISTS sequence_number BIGINT"),
        format!(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_{prefix}audit_hash_chain ON {table} (namespace, tenant, sequence_number) WHERE sequence_number IS NOT NULL"
        ),
    ];
    for stmt in &hash_chain_stmts {
        sqlx::query(stmt).execute(pool).await?;
    }

    // Add attachment_metadata column (idempotent).
    let attachment_stmt = format!(
        "ALTER TABLE {table} ADD COLUMN IF NOT EXISTS attachment_metadata JSONB NOT NULL DEFAULT '[]'::jsonb"
    );
    sqlx::query(&attachment_stmt).execute(pool).await?;

    Ok(())
}
