//! A2A destination checks shared with webhook transports.
//! Preflight resolution is diagnostic; `GuardedClient` enforces the same policy
//! on the addresses actually returned to the HTTP connector.
pub use acteon_http::destination::{
    SsrfReason, check_url_literal, check_url_resolved, is_blocked_ip,
};
