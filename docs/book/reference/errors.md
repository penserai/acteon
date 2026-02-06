# Error Handling

## ActeonError

The top-level error type used throughout the system:

```rust
pub enum ActeonError {
    State(String),          // State store errors
    Rule(String),           // Rule evaluation errors
    Provider(String),       // Provider execution errors
    Executor(String),       // Executor errors
    Gateway(String),        // Gateway orchestration errors
    Serialization(String),  // JSON serialization errors
    Configuration(String),  // Configuration errors
    Other(String),          // Generic errors
}
```

## ProviderError

Errors from provider execution:

```rust
pub enum ProviderError {
    ExecutionFailed(String),    // Non-retryable
    Timeout(String),            // Retryable
    Connection(String),         // Retryable
    RateLimited,                // Retryable
    Configuration(String),      // Non-retryable
}
```

### Retryability

| Error | Retryable | Behavior |
|-------|-----------|----------|
| `ExecutionFailed` | No | Immediately fails, goes to DLQ |
| `Timeout` | Yes | Retries with backoff |
| `Connection` | Yes | Retries with backoff |
| `RateLimited` | Yes | Retries with backoff |
| `Configuration` | No | Immediately fails |

## Client Errors

The `acteon-client` crate defines:

```rust
pub enum Error {
    Connection(String),                    // Network failure (retryable)
    Http { status: u16, message: String }, // HTTP error (5xx retryable)
    Api { code: String, message: String, retryable: bool },
    Deserialization(String),               // Parse error
    Configuration(String),                 // Setup error
}
```

### Client Error Methods

```rust
error.is_retryable()     // Whether to retry
error.api_code()         // Optional API error code
```

## Gateway Errors

```rust
pub enum GatewayError {
    ProviderNotFound(String),
    RuleEvaluation(String),
    StateError(String),
    LockTimeout(String),
    ExecutionError(String),
}
```

## Error Handling Patterns

### In Provider Implementations

```rust
#[async_trait]
impl DynProvider for MyProvider {
    async fn execute(&self, action: &Action)
        -> Result<ProviderResponse, ProviderError>
    {
        let response = self.client.post(&self.url)
            .json(&action.payload)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    ProviderError::Timeout(e.to_string())
                } else if e.is_connect() {
                    ProviderError::Connection(e.to_string())
                } else {
                    ProviderError::ExecutionFailed(e.to_string())
                }
            })?;

        if response.status() == 429 {
            return Err(ProviderError::RateLimited);
        }

        if response.status().is_server_error() {
            return Err(ProviderError::Connection(
                format!("HTTP {}", response.status())
            ));
        }

        Ok(ProviderResponse::success(response.json().await?))
    }
}
```

### In Client Code

```rust
match client.dispatch(&action).await {
    Ok(outcome) => handle_outcome(outcome),
    Err(e) if e.is_retryable() => {
        // Implement retry with backoff
        tokio::time::sleep(Duration::from_secs(1)).await;
        client.dispatch(&action).await?
    }
    Err(e) => return Err(e.into()),
}
```

## Dead Letter Queue

Failed actions (after all retries) are sent to the dead letter queue (DLQ) for later inspection and reprocessing:

```rust
pub struct DeadLetterEntry {
    pub action: Action,
    pub error: ActionError,
    pub attempts: u32,
    pub failed_at: DateTime<Utc>,
}
```

The DLQ is accessible via the `DeadLetterSink` trait:

```rust
#[async_trait]
pub trait DeadLetterSink: Send + Sync {
    async fn send(&self, entry: DeadLetterEntry) -> Result<()>;
}
```
