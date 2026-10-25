use crate::{
    DEFAULT_ARCHIVE_TTL_SECONDS, DEFAULT_LEDGER_TTL_SECONDS, DEFAULT_TIMELINE_TTL_SECONDS,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TimelineConfig {
    pub enabled: bool,
    pub backend: String,
    pub url: Option<String>,
    pub collection: String,
    pub allow_memory: bool,
    pub ttl_seconds: u64,
    pub ledger: LedgerConfig,
    pub archive: Option<ArchiveConfig>,
    pub sources: Vec<SourceConfig>,
}
impl Default for TimelineConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            backend: "elasticsearch".into(),
            url: None,
            collection: "acteon_timeline".into(),
            allow_memory: false,
            ttl_seconds: DEFAULT_TIMELINE_TTL_SECONDS,
            ledger: LedgerConfig::default(),
            archive: None,
            sources: vec![],
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LedgerConfig {
    pub directory: String,
    pub ttl_seconds: u64,
    pub max_bytes: u64,
}
impl Default for LedgerConfig {
    fn default() -> Self {
        Self {
            directory: "data/timeline-ledger".into(),
            ttl_seconds: DEFAULT_LEDGER_TTL_SECONDS,
            max_bytes: 10 * 1024 * 1024 * 1024,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArchiveConfig {
    pub bucket: String,
    pub prefix: String,
    pub endpoint: Option<String>,
    pub region: Option<String>,
    #[serde(default = "archive_ttl")]
    pub ttl_seconds: u64,
}
fn archive_ttl() -> u64 {
    DEFAULT_ARCHIVE_TTL_SECONDS
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    #[serde(default)]
    pub poison_policy: PoisonPolicy,
    pub namespace: String,
    pub tenant: String,
    pub source: String,
    pub topic: String,
    pub consumer_group: String,
    #[serde(default)]
    pub debezium: Option<crate::debezium::DebeziumMapping>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PoisonPolicy {
    #[default]
    Halt,
    Quarantine,
}
