mod audit;
mod auth;
mod background;
mod chains;
mod circuit_breaker;
mod compliance;
mod enrichment;
mod executor;
mod llm;
mod providers;
mod server;
mod snapshot;
mod state;
mod telemetry;

#[cfg(test)]
mod tests;

pub use audit::*;
pub use auth::*;
pub use background::*;
pub use chains::*;
pub use circuit_breaker::*;
pub use compliance::*;
pub use enrichment::*;
pub use executor::*;
pub use llm::*;
pub use providers::*;
pub use server::*;
pub use snapshot::*;
pub use state::*;
pub use telemetry::*;

use serde::Deserialize;

/// Top-level configuration for the Acteon server, loaded from a TOML file.
#[derive(Debug, Deserialize)]
pub struct ActeonConfig {
    /// State backend configuration.
    #[serde(default)]
    pub state: StateConfig,
    /// Rule loading configuration.
    #[serde(default)]
    pub rules: RulesConfig,
    /// Executor configuration.
    #[serde(default)]
    pub executor: ExecutorConfig,
    /// HTTP server bind configuration.
    #[serde(default)]
    pub server: ServerConfig,
    /// Admin UI configuration.
    #[serde(default)]
    pub ui: UiConfig,
    /// Audit trail configuration.
    #[serde(default)]
    pub audit: AuditConfig,
    /// Authentication and authorization configuration.
    #[serde(default)]
    pub auth: AuthRefConfig,
    /// Rate limiting configuration.
    #[serde(default)]
    pub rate_limit: RateLimitRefConfig,
    /// Background processing configuration.
    #[serde(default)]
    pub background: BackgroundProcessingConfig,
    /// LLM guardrail configuration.
    #[serde(default)]
    pub llm_guardrail: LlmGuardrailServerConfig,
    /// Task chain definitions.
    #[serde(default)]
    pub chains: ChainsConfig,
    /// Embedding provider configuration for semantic routing.
    #[serde(default)]
    pub embedding: EmbeddingServerConfig,
    /// Circuit breaker configuration for provider resilience.
    #[serde(default)]
    pub circuit_breaker: CircuitBreakerServerConfig,
    /// OpenTelemetry distributed tracing configuration.
    #[serde(default)]
    pub telemetry: TelemetryConfig,
    /// Payload encryption at rest configuration.
    #[serde(default)]
    pub encryption: EncryptionConfig,
    /// Quota policy configuration.
    #[serde(default)]
    pub quotas: QuotaConfig,
    /// WASM plugin runtime configuration.
    #[serde(default)]
    pub wasm: WasmServerConfig,
    /// Compliance mode configuration (`SOC2` / `HIPAA`).
    #[serde(default)]
    pub compliance: ComplianceServerConfig,
    /// Payload template configuration.
    #[serde(default)]
    pub templates: TemplateServerConfig,
    /// Attachment size and count limits.
    #[serde(default)]
    pub attachments: AttachmentConfig,
    /// Pre-dispatch enrichment configurations.
    ///
    /// Each entry describes a resource lookup to execute before rule evaluation,
    /// merging live external state into the action payload.
    #[serde(default)]
    pub enrichments: Vec<EnrichmentConfigToml>,
    /// Provider definitions.
    ///
    /// Each entry registers a named provider that actions can be routed to.
    /// Supported types: `"webhook"` (HTTP POST) and `"log"` (logs and returns
    /// success).
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
}
