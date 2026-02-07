pub mod bridge;
pub mod cache;
pub mod config;
pub mod cosine;
pub mod error;
pub mod http;
pub mod metrics;
pub mod mock;
pub mod provider;

pub use bridge::EmbeddingBridge;
pub use config::{EmbeddingBridgeConfig, EmbeddingConfig};
pub use error::EmbeddingError;
pub use http::HttpEmbeddingProvider;
pub use metrics::{EmbeddingMetrics, EmbeddingMetricsSnapshot};
pub use mock::{FailingEmbeddingProvider, MappingEmbeddingProvider, MockEmbeddingProvider};
pub use provider::EmbeddingProvider;
