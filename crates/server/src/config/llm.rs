use std::collections::HashMap;

use serde::Deserialize;

/// Configuration for the optional LLM guardrail.
///
/// # Secret management
///
/// The `api_key` field supports encrypted values. To avoid storing your
/// LLM API key in plain text:
///
/// 1. Set the `ACTEON_AUTH_KEY` environment variable to a hex-encoded
///    256-bit master key.
/// 2. Run `acteon-server encrypt` and paste your API key on stdin.
/// 3. Copy the `ENC[...]` output into `api_key` in your `acteon.toml`.
#[derive(Debug, Deserialize)]
pub struct LlmGuardrailServerConfig {
    /// Whether the LLM guardrail is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OpenAI-compatible API endpoint.
    #[serde(default = "default_llm_endpoint")]
    pub endpoint: String,
    /// Model to use.
    #[serde(default = "default_llm_model")]
    pub model: String,
    /// API key for authentication.
    ///
    /// Supports `ENC[...]` encrypted values (requires `ACTEON_AUTH_KEY`).
    /// Use `acteon-server encrypt` to generate encrypted values.
    #[serde(default)]
    pub api_key: String,
    /// System prompt / policy sent to the LLM.
    #[serde(default)]
    pub policy: String,
    /// Per-action-type policy overrides.
    ///
    /// Keys are action type strings, values are policy prompts. These take
    /// precedence over the global `policy` but are overridden by per-rule
    /// metadata `llm_policy` entries.
    #[serde(default)]
    pub policies: HashMap<String, String>,
    /// Whether to allow actions when the LLM is unreachable.
    #[serde(default = "default_llm_fail_open")]
    pub fail_open: bool,
    /// Request timeout in seconds.
    pub timeout_seconds: Option<u64>,
    /// Temperature for LLM sampling.
    pub temperature: Option<f64>,
    /// Maximum tokens in the response.
    pub max_tokens: Option<u32>,
}

impl Default for LlmGuardrailServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_llm_endpoint(),
            model: default_llm_model(),
            api_key: String::new(),
            policy: String::new(),
            policies: HashMap::new(),
            fail_open: default_llm_fail_open(),
            timeout_seconds: None,
            temperature: None,
            max_tokens: None,
        }
    }
}

fn default_llm_endpoint() -> String {
    "https://api.openai.com/v1/chat/completions".to_owned()
}

fn default_llm_model() -> String {
    "gpt-4o-mini".to_owned()
}

fn default_llm_fail_open() -> bool {
    true
}

/// Configuration for the embedding provider used by semantic routing.
///
/// # Secret management
///
/// The `api_key` field supports encrypted values. To avoid storing your
/// embedding API key in plain text:
///
/// 1. Set the `ACTEON_AUTH_KEY` environment variable to a hex-encoded
///    256-bit master key.
/// 2. Run `acteon-server encrypt` and paste your API key on stdin.
/// 3. Copy the `ENC[...]` output into `api_key` in your `acteon.toml`.
///
/// ```toml
/// [embedding]
/// enabled = true
/// api_key = "ENC[AES256-GCM,...]"
/// ```
#[derive(Debug, Deserialize)]
pub struct EmbeddingServerConfig {
    /// Whether the embedding provider is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// OpenAI-compatible embeddings API endpoint.
    #[serde(default = "default_embedding_endpoint")]
    pub endpoint: String,
    /// Embedding model name.
    #[serde(default = "default_embedding_model")]
    pub model: String,
    /// API key for authentication.
    ///
    /// Supports `ENC[...]` encrypted values (requires `ACTEON_AUTH_KEY`).
    /// Use `acteon-server encrypt` to generate encrypted values.
    #[serde(default)]
    pub api_key: String,
    /// Request timeout in seconds.
    #[serde(default = "default_embedding_timeout")]
    pub timeout_seconds: u64,
    /// Whether to allow actions when the embedding API is unreachable.
    #[serde(default = "default_embedding_fail_open")]
    pub fail_open: bool,
    /// Maximum number of topic embeddings to cache.
    #[serde(default = "default_topic_cache_capacity")]
    pub topic_cache_capacity: u64,
    /// TTL in seconds for cached topic embeddings.
    #[serde(default = "default_topic_cache_ttl")]
    pub topic_cache_ttl_seconds: u64,
    /// Maximum number of text embeddings to cache.
    #[serde(default = "default_text_cache_capacity")]
    pub text_cache_capacity: u64,
    /// TTL in seconds for cached text embeddings.
    #[serde(default = "default_text_cache_ttl")]
    pub text_cache_ttl_seconds: u64,
}

impl Default for EmbeddingServerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_embedding_endpoint(),
            model: default_embedding_model(),
            api_key: String::new(),
            timeout_seconds: default_embedding_timeout(),
            fail_open: default_embedding_fail_open(),
            topic_cache_capacity: default_topic_cache_capacity(),
            topic_cache_ttl_seconds: default_topic_cache_ttl(),
            text_cache_capacity: default_text_cache_capacity(),
            text_cache_ttl_seconds: default_text_cache_ttl(),
        }
    }
}

fn default_embedding_endpoint() -> String {
    "https://api.openai.com/v1/embeddings".to_owned()
}

fn default_embedding_model() -> String {
    "text-embedding-3-small".to_owned()
}

fn default_embedding_timeout() -> u64 {
    10
}

fn default_embedding_fail_open() -> bool {
    true
}

fn default_topic_cache_capacity() -> u64 {
    10_000
}

fn default_topic_cache_ttl() -> u64 {
    3600
}

fn default_text_cache_capacity() -> u64 {
    1_000
}

fn default_text_cache_ttl() -> u64 {
    60
}
