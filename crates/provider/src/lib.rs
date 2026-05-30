pub mod context;
pub mod error;
pub mod health;
pub mod log;
pub mod provider;
pub mod registry;
pub mod resource_lookup;

#[cfg(feature = "webhook")]
pub mod webhook;

// Shared HTTP response helpers (bounded body reads) — require reqwest.
#[cfg(feature = "reqwest")]
pub mod http;

pub use context::DispatchContext;
pub use error::{ProviderError, truncate_error_body};
#[cfg(feature = "reqwest")]
pub use http::{MAX_ERROR_BODY_READ_BYTES, MAX_RESPONSE_BODY_READ_BYTES, read_bounded_body};
pub use log::LogProvider;
pub use provider::{DynProvider, Provider};
pub use registry::ProviderRegistry;
pub use resource_lookup::ResourceLookup;

// Outbound W3C Trace Context injection — requires reqwest.
#[cfg(feature = "trace-context")]
pub mod trace_context;
#[cfg(feature = "trace-context")]
pub use trace_context::inject_trace_context;
