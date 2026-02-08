pub mod error;
pub mod health;
pub mod provider;
pub mod registry;

#[cfg(feature = "webhook")]
pub mod webhook;

pub use error::ProviderError;
pub use provider::{DynProvider, Provider};
pub use registry::ProviderRegistry;

// Outbound W3C Trace Context injection — requires reqwest.
#[cfg(feature = "trace-context")]
pub mod trace_context;
#[cfg(feature = "trace-context")]
pub use trace_context::inject_trace_context;
