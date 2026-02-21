pub mod context;
pub mod error;
pub mod health;
pub mod log;
pub mod provider;
pub mod registry;
pub mod resource_lookup;

#[cfg(feature = "webhook")]
pub mod webhook;

pub use context::DispatchContext;
pub use error::ProviderError;
pub use log::LogProvider;
pub use provider::{DynProvider, Provider};
pub use registry::ProviderRegistry;
pub use resource_lookup::ResourceLookup;

// Outbound W3C Trace Context injection — requires reqwest.
#[cfg(feature = "trace-context")]
pub mod trace_context;
#[cfg(feature = "trace-context")]
pub use trace_context::inject_trace_context;
