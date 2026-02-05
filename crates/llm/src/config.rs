/// Configuration for the HTTP-based LLM evaluator.
#[derive(Debug, Clone)]
pub struct LlmGuardrailConfig {
    /// OpenAI-compatible API endpoint (e.g., `https://api.openai.com/v1/chat/completions`).
    pub endpoint: String,
    /// Model to use (e.g., `gpt-4o-mini`).
    pub model: String,
    /// API key for authentication.
    pub api_key: String,
    /// Request timeout in seconds.
    pub timeout_seconds: u64,
    /// Temperature for LLM sampling (0.0 = deterministic).
    pub temperature: f64,
    /// Maximum tokens in the response.
    pub max_tokens: u32,
}

impl LlmGuardrailConfig {
    /// Create a new config with the given endpoint, model, and API key.
    ///
    /// Uses sensible defaults: 10s timeout, temperature 0.0, max 256 tokens.
    pub fn new(
        endpoint: impl Into<String>,
        model: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            model: model.into(),
            api_key: api_key.into(),
            timeout_seconds: 10,
            temperature: 0.0,
            max_tokens: 256,
        }
    }

    /// Set the request timeout in seconds.
    #[must_use]
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    /// Set the temperature for LLM sampling.
    #[must_use]
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = temperature;
        self
    }

    /// Set the maximum tokens in the response.
    #[must_use]
    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }
}
