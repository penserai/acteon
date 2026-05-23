/// Configuration for the `DynamoDB` audit store backend.
#[derive(Debug, Clone)]
pub struct DynamoDbAuditConfig {
    /// `DynamoDB` table name.
    pub table_name: String,

    /// AWS region (e.g. `"us-east-1"`).
    pub region: String,

    /// Optional endpoint URL for local development (e.g. `DynamoDB` Local).
    pub endpoint_url: Option<String>,

    /// Key prefix applied to partition keys to avoid collisions.
    pub key_prefix: String,
}

impl Default for DynamoDbAuditConfig {
    fn default() -> Self {
        Self {
            table_name: String::from("acteon_audit"),
            region: String::from("us-east-1"),
            endpoint_url: None,
            key_prefix: String::from("acteon"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let cfg = DynamoDbAuditConfig::default();
        assert_eq!(cfg.table_name, "acteon_audit");
        assert_eq!(cfg.region, "us-east-1");
        assert!(cfg.endpoint_url.is_none());
        assert_eq!(cfg.key_prefix, "acteon");
    }

    #[test]
    fn custom_values() {
        let cfg = DynamoDbAuditConfig {
            table_name: "my_audit_table".into(),
            region: "eu-west-1".into(),
            endpoint_url: Some("http://localhost:8000".into()),
            key_prefix: "myapp".into(),
        };
        assert_eq!(cfg.table_name, "my_audit_table");
        assert_eq!(cfg.region, "eu-west-1");
        assert_eq!(cfg.endpoint_url.as_deref(), Some("http://localhost:8000"));
        assert_eq!(cfg.key_prefix, "myapp");
    }
}
