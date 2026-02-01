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
}

impl Default for PostgresConfig {
    fn default() -> Self {
        Self {
            url: String::from("postgres://localhost:5432/acteon"),
            pool_size: 5,
            schema: String::from("public"),
            table_prefix: String::from("acteon_"),
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
