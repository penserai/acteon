//! Telegram Bot provider for the Acteon notification gateway.
//!
//! This crate implements the [`Provider`](acteon_provider::Provider)
//! trait against the [Telegram Bot API's][api] `sendMessage`
//! endpoint. It's the same endpoint Alertmanager targets via its
//! `telegram_configs`, so a migration is just re-authoring the
//! routing.
//!
//! # Quick start
//!
//! ```rust,no_run
//! use acteon_telegram::{TelegramConfig, TelegramProvider};
//!
//! // Single chat (the common case).
//! let config = TelegramConfig::single_chat(
//!     "123456:ABC-DEF-bot-token",
//!     "ops-channel",
//!     "-1001234567890",
//! )
//! .with_default_parse_mode("HTML");
//! let provider = TelegramProvider::new(config);
//! ```
//!
//! Multiple chats are supported for deployments that route
//! different alerts to different Telegram groups, users, or
//! channels based on the payload's `chat` field.
//!
//! # Supported event actions
//!
//! Telegram has no lifecycle concept — the provider accepts one
//! `event_action`, `"send"` (also the default when omitted). The
//! only required payload field is `text`.
//!
//! | Payload field | Type | Purpose |
//! |---|---|---|
//! | `text` | string | **Required.** Message body (max 4096 UTF-16 code units per the API) |
//! | `chat` | string | Logical chat name matching an entry in `chats` |
//! | `parse_mode` | `"HTML"` / `"Markdown"` / `"MarkdownV2"` | Rich-text rendering |
//! | `disable_notification` | bool | Silent delivery |
//! | `disable_web_page_preview` | bool | Suppress URL previews |
//! | `protect_content` | bool | Block forwarding / saving |
//! | `reply_to_message_id` | int | Threaded reply |
//! | `message_thread_id` | int | Target a topic in a forum group |
//!
//! [api]: https://core.telegram.org/bots/api#sendmessage

pub mod config;
pub mod error;
pub mod provider;
pub mod types;

pub use config::{DEFAULT_TEXT_MAX_UTF16_UNITS, TelegramConfig};
pub use error::TelegramError;
pub use provider::TelegramProvider;
pub use types::{TelegramApiResponse, TelegramSendMessageRequest};
