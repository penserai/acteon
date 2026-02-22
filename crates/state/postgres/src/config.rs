/// Configuration for the `PostgreSQL` state store and distributed lock backends.
#[derive(Debug, Clone)]
pub struct PostgresConfig {
    /// `PostgreSQL` connection URL (e.g. `postgres://user:pass@localhost:5432/acteon`).
    pub url: String,

    /// Maximum number of connections in the `sqlx` connection pool.
    pub pool_size: u32,

    /// Database schema to use for tables (e.g. `"public"`).
    pub schema: String,

    /// Prefix applied to table names to avoid collisions (e.g. `"acteon_"`).
    pub table_prefix: String,

    /// SSL mode for the connection (`disable`, `prefer`, `require`, `verify-ca`, `verify-full`).
    pub ssl_mode: Option<String>,

    /// Path to the CA certificate for SSL server verification.
    pub ssl_root_cert: Option<String>,

    /// Path to the client certificate for mTLS.
    pub ssl_cert: Option<String>,

    /// Path to the client private key for mTLS.
    pub ssl_key: Option<String>,
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            url: String::from("postgres://localhost:5432/acteon"),
            pool_size: 5,
            schema: String::from("public"),
            table_prefix: String::from("acteon_"),
            ssl_mode: None,
            ssl_root_cert: None,
            ssl_cert: None,
            ssl_key: None,
        }
    }
}

impl PostgresConfig {
    /// Return the fully-qualified state table name (`schema.prefix_state`).
    pub(crate) fn state_table(&self) -> String {
        format!("{}.{}state", self.schema, self.table_prefix)
    }

    /// Return the fully-qualified locks table name (`schema.prefix_locks`).
    pub(crate) fn locks_table(&self) -> String {
        format!("{}.{}locks", self.schema, self.table_prefix)
    }

    /// Return the fully-qualified timeout index table name (`schema.prefix_timeout_index`).
    pub(crate) fn timeout_index_table(&self) -> String {
        format!("{}.{}timeout_index", self.schema, self.table_prefix)
    }

    /// Return the fully-qualified chain ready index table name (`schema.prefix_chain_ready_index`).
    pub(crate) fn chain_ready_index_table(&self) -> String {
        format!("{}.{}chain_ready_index", self.schema, self.table_prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = PostgresConfig::default();
        assert_eq!(cfg.url, "postgres://localhost:5432/acteon");
        assert_eq!(cfg.pool_size, 5);
        assert_eq!(cfg.schema, "public");
        assert_eq!(cfg.table_prefix, "acteon_");
    }

    #[test]
    fn table_names() {
        let cfg = PostgresConfig::default();
        assert_eq!(cfg.state_table(), "public.acteon_state");
        assert_eq!(cfg.locks_table(), "public.acteon_locks");
    }

    #[test]
    fn custom_table_names() {
        let cfg = PostgresConfig {
            schema: "myschema".into(),
            table_prefix: "app_".into(),
            ..PostgresConfig::default()
        };
        assert_eq!(cfg.state_table(), "myschema.app_state");
        assert_eq!(cfg.locks_table(), "myschema.app_locks");
    }
}
