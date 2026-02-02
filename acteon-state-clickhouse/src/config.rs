/// Configuration for the `ClickHouse` state store and distributed lock backends.
#[derive(Debug, Clone)]
pub struct ClickHouseConfig {
    /// `ClickHouse` HTTP endpoint URL (e.g. `http://localhost:8123`).
    pub url: String,

    /// Database name.
    pub database: String,

    /// Prefix applied to table names to avoid collisions (e.g. `"acteon_"`).
    pub table_prefix: String,
}

impl Default for ClickHouseConfig {
    fn default() -> Self {
        Self {
            url: String::from("http://localhost:8123"),
            database: String::from("default"),
            table_prefix: String::from("acteon_"),
        }
    }
}

impl ClickHouseConfig {
    /// Return the state table name (`{prefix}state`).
    pub(crate) fn state_table(&self) -> String {
        format!("{}state", self.table_prefix)
    }

    /// Return the locks table name (`{prefix}locks`).
    pub(crate) fn locks_table(&self) -> String {
        format!("{}locks", self.table_prefix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = ClickHouseConfig::default();
        assert_eq!(cfg.url, "http://localhost:8123");
        assert_eq!(cfg.database, "default");
        assert_eq!(cfg.table_prefix, "acteon_");
    }

    #[test]
    fn table_names() {
        let cfg = ClickHouseConfig::default();
        assert_eq!(cfg.state_table(), "acteon_state");
        assert_eq!(cfg.locks_table(), "acteon_locks");
    }

    #[test]
    fn custom_table_names() {
        let cfg = ClickHouseConfig {
            table_prefix: "app_".into(),
            ..ClickHouseConfig::default()
        };
        assert_eq!(cfg.state_table(), "app_state");
        assert_eq!(cfg.locks_table(), "app_locks");
    }
}
