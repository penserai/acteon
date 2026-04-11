//! `WeChat` Work (Enterprise `WeChat`, 企业微信) provider for the
//! Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider)
//! trait against the [`WeChat` Work Message Send API][api] — the same
//! endpoint Alertmanager targets via its `wechat_configs`. It's the
//! most architecturally complex receiver in the Alertmanager parity
//! set because of three `WeChat`-specific quirks:
//!
//! 1. **Access tokens expire every 7200 seconds.** Every API call
//!    passes an `access_token` query parameter that must be refreshed
//!    by calling a separate `gettoken` endpoint with the org's
//!    `corpid` + `corpsecret`. The provider caches tokens and
//!    refreshes lazily with a configurable buffer window so a token
//!    at the edge of its TTL does not race a dispatch.
//! 2. **Token revocation is in-band.** If the server returns
//!    `errcode: 42001` (`access_token` expired) or `40014`
//!    (invalid `access_token`) mid-send, the cached token is
//!    invalidated and the request is retried exactly once with a
//!    fresh token. Operators don't need to restart anything when a
//!    token is revoked out of band.
//! 3. **Errors travel in a `{"errcode": 0, "errmsg": "ok", ...}`
//!    envelope.** HTTP 200 with `errcode != 0` is the normal failure
//!    shape; the provider classifies non-zero errcodes into
//!    retryable / non-retryable buckets so the gateway's retry logic
//!    handles transient server-busy errors correctly.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_wechat::{WeChatConfig, WeChatProvider};
//!
//! let config = WeChatConfig::new("corp-id", "corp-secret", 1_000_002)
//!     .with_default_touser("@all")
//!     .with_default_msgtype("text");
//! let provider = WeChatProvider::new(config);
//! ```
//!
//! # Supported message types
//!
//! The provider supports the three message types that cover
//! virtually all alerting use cases. Image, voice, video, file,
//! news, taskcard, `template_card`, mpnews, and
//! `miniprogram_notice` are **not** supported in v1 — they're
//! for content delivery, not alerting, and have complex nested
//! payload shapes that deserve a dedicated follow-up if demand
//! emerges.
//!
//! | `msgtype` | Required payload fields | Purpose |
//! |---|---|---|
//! | `"text"` | `content` | Plain text |
//! | `"markdown"` | `content` | `WeChat`-flavored markdown (limited syntax — see API docs) |
//! | `"textcard"` | `title`, `description`, `url` | Clickable card with title, body, and link |
//!
//! # Recipient routing
//!
//! `WeChat` Work messages target **one or more** of `touser`,
//! `toparty`, and `totag` simultaneously. Each is a `|`-separated
//! string of IDs. The special value `@all` on `touser` broadcasts
//! to every member of the configured `agentid`.
//!
//! The provider accepts recipients from the dispatch payload or
//! falls back to the config's configured defaults, so operators
//! can pin a provider instance to a specific agent / department /
//! tag group and not repeat the routing on every rule.
//!
//! [api]: https://developer.work.weixin.qq.com/document/path/90236

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::{DEFAULT_TOKEN_REFRESH_BUFFER_SECONDS, WeChatConfig, WeChatRecipients};
pub use error::WeChatError;
pub use provider::WeChatProvider;
pub use types::{WeChatApiResponse, WeChatMsgType, WeChatSendRequest, WeChatTokenResponse};
