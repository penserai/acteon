//! Pushover provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider)
//! trait against the [Pushover Messages API][api], a lightweight
//! push-notification service for mobile devices and desktops.
//! `Pushover` is one of Acteon's lowest-ceremony providers — it has
//! no lifecycle (just fire-and-forget sends) and no client-side
//! deduplication, so the provider is thin on purpose.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_pushover::{PushoverConfig, PushoverProvider};
//!
//! // Single user key (the common case).
//! let config = PushoverConfig::single_recipient(
//!     "your-app-token",
//!     "ops-oncall",
//!     "the-user-or-group-key",
//! );
//! let provider = PushoverProvider::new(config);
//! ```
//!
//! Multiple recipients are supported for deployments that fan
//! notifications out to different Pushover user or group keys
//! based on the payload's `user_key` field.
//!
//! # Payload shape
//!
//! The provider accepts one `event_action`, `"send"` (also the
//! default when omitted). `message` is the only required field;
//! everything else passes through to the Pushover API optionally.
//!
//! ```text
//! event_action  "send" (default, optional)
//! message       required — body of the notification
//! user_key      logical recipient name (matches a key in `user_keys`)
//! title         notification title
//! priority      -2..=2 (emergency = 2, requires retry + expire)
//! retry         seconds between re-notifications (emergency only, ≥30)
//! expire        seconds until giving up re-notifying (emergency only, ≤10800)
//! sound         notification sound name
//! url           supplementary URL to display
//! url_title     label for `url`
//! device        target device name (default: all devices on the user's account)
//! html          render message as HTML (bool)
//! monospace     render message as monospace (bool, exclusive with html)
//! ttl           auto-delete after N seconds
//! timestamp     unix timestamp of the originating event
//! ```
//!
//! [api]: https://pushover.net/api

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::PushoverConfig;
pub use error::PushoverError;
pub use provider::PushoverProvider;
pub use types::{PushoverApiResponse, PushoverPriority, PushoverRequest};
