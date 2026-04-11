# Telegram Bot Provider

Acteon ships with a first-class **Telegram Bot** provider that sends messages via the [Telegram Bot API's `sendMessage` endpoint][api] — the same endpoint Alertmanager targets via its `telegram_configs`. It was built as part of Phase 4d of the Alertmanager feature-parity initiative so teams migrating off Alertmanager can re-use their existing Telegram receivers.

[api]: https://core.telegram.org/bots/api#sendmessage

Like the other native providers, `acteon-telegram`:

- Holds the bot token as `SecretString`, zeroized on drop. The token is percent-encoded in the URL path so the `{bot_id}:{auth}` colon separator cannot collapse the route.
- Supports multiple chats per provider instance so one config can fan messages out to several groups, users, or channels.
- Maps 5xx / 408 → retryable `Connection` (via a `Transient` variant); 429 → retryable `RateLimited`; 401/403/404 → non-retryable `Configuration` (404 on `/bot{token}/...` means an unrecognized token — operator error, not transient); other 4xx → non-retryable `ExecutionFailed`.
- Handles Telegram's **200 OK + `ok: false`** edge case by classifying it as a permanent `ExecutionFailed`.
- Reuses the server's shared HTTP client, so it participates in circuit breaking, provider health checks, and per-provider metrics automatically.
- **Verifies credentials** in the health check: unlike most provider APIs, Telegram ships a free, idempotent `getMe` endpoint explicitly designed as a credential-validity probe. The Telegram provider parses the response and surfaces bad tokens as non-retryable `Configuration` errors — operators see token problems on the health dashboard instead of only at dispatch time.
- Propagates W3C Trace Context headers.

## TOML configuration

```toml
[[providers]]
name = "telegram-ops"
type = "telegram"
telegram.bot_token = "ENC[AES256_GCM,data:abc123...]"
telegram.default_chat = "ops-channel"
telegram.default_parse_mode = "HTML"         # optional
# telegram.text_max_utf16_units = 4096        # default — matches the Bot API's UTF-16 unit cap

[providers.telegram.chats]
ops-channel  = "-1001234567890"
dev-channel  = "@devchannel"
alice-direct = "123456789"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used when dispatching actions |
| `type` | Yes | Must be `"telegram"` |
| `telegram.bot_token` | Yes | Bot token (`{bot_id}:{auth-string}`). Supports `ENC[...]`. |
| `telegram.chats` | Yes (≥1) | Map of logical chat name → Telegram `chat_id`. Values can be numeric (`-1001234567890`) or string handles (`@channelusername`). Chat IDs are **not** secrets. |
| `telegram.default_chat` | No | Name of the default chat used when the dispatch payload omits `chat`. If there is only one chat, it is used implicitly. |
| `telegram.default_parse_mode` | No | Default `parse_mode` applied when the payload omits it: `"HTML"`, `"Markdown"`, or `"MarkdownV2"`. |
| `telegram.text_max_utf16_units` | No | Client-side `text` truncation cap, in **UTF-16 code units** — the same unit Telegram's API uses for its 4096-unit cap. Default `4096`. |
| `telegram.api_base_url` | No | Override the Bot API base URL. Tests only. |

### Why UTF-16 code units (not bytes)

The Telegram Bot API expresses the 4096-character `text` limit in UTF-16 code units, not UTF-8 bytes. One BMP character (Latin, Cyrillic, Hebrew, Arabic, most CJK) costs 1 code unit; one non-BMP character (most emoji at U+1F000 and above, supplementary CJK ideographs) costs 2 code units (a surrogate pair).

Counting in UTF-16 units matches the API exactly. The earlier revision of this provider counted bytes instead, which meant CJK traffic truncated at ~1365 characters (because CJK is 3 bytes per character in UTF-8) even though the API would have accepted the full 4096-character message. The current implementation gives CJK and emoji-heavy deployments the full runway the API actually permits.

## Payload shape

Telegram has no lifecycle concept — the provider accepts one `event_action`, `"send"` (also the default when omitted). Only `text` is required.

```json
{
  "text": "Deploy #4823 completed successfully.",
  "chat": "ops-channel"
}
```

### HTML-formatted alert to a forum-group topic

```json
{
  "text": "<b>CRITICAL</b>: checkout-api error rate above SLO.\n<i>Investigate immediately.</i>",
  "chat": "ops-channel",
  "parse_mode": "HTML",
  "disable_web_page_preview": true,
  "protect_content": true,
  "message_thread_id": 7
}
```

| Field | Type | Notes |
|-------|------|-------|
| `text` | string | **Required.** Message body. Truncated client-side to `text_max_utf16_units` UTF-16 code units (default 4096, matching the API limit). Multi-byte characters are never split. |
| `chat` | string | Logical chat name (matching a key in `telegram.chats`). Falls back to `telegram.default_chat` or the single-entry implicit default. |
| `parse_mode` | string | `"HTML"`, `"Markdown"`, or `"MarkdownV2"`. Overrides `telegram.default_parse_mode`. |
| `disable_notification` | bool | Silent delivery — recipients get no sound or vibration. |
| `disable_web_page_preview` | bool | Suppress URL previews. |
| `protect_content` | bool | Block forwarding and saving. |
| `reply_to_message_id` | int | Mark this message as a reply to an existing one. |
| `message_thread_id` | int | Target a specific topic inside a forum-enabled group. |

## Rule integration

Because Telegram is just another named provider, every routing primitive Acteon already has works with it:

- **Reroute critical alerts** to a Telegram channel by matching on `action.payload.severity == "critical"` with a `reroute` rule.
- **Silence maintenance windows** with [silences](silences.md) — silences apply before the provider dispatch.
- **Quota-bound a Telegram bot** with a [per-provider tenant quota](tenant-quotas.md) scoped to `provider: "telegram-ops"` — Telegram's own per-bot limits are 30 messages/second globally and 20 messages/minute per group, so a quota is useful defense-in-depth.
- **Dedup noisy events** with Acteon's [deduplication](deduplication.md) using `Action.dedup_key`.

## Outcome body

On success the provider returns an `Executed` outcome whose `body` carries the Telegram response:

```json
{
  "ok": true,
  "message_id": 42
}
```

The `message_id` is the Telegram server's assignment for the delivered message — useful in chains where a later step needs to edit or reply to an earlier Telegram post.

## Error mapping

| HTTP status | Telegram `ok` | `ProviderError` | Retryable? |
|-------------|----------------|-----------------|------------|
| 2xx | `true` | `Executed` (success) | — |
| 2xx | `false` | `ExecutionFailed(...)` | No — permanent API error in the body |
| 401 / 403 / 404 | — | `Configuration(...)` | No — bad bot token |
| 429 | — | `RateLimited` | Yes |
| 408, 5xx | — | `Connection(...)` (via `Transient`) | **Yes** |
| Other 4xx | — | `ExecutionFailed(...)` | No |
| Transport failure | — | `Connection(...)` | Yes |

**Why 404 → `Configuration`:** the Telegram Bot API routes every call through `/bot{token}/...`. A 404 on that path means the server did not recognize the token — which is an operator problem, not a transient one. The same class of response on the other provider crates would be 401, but Telegram sometimes uses 404 here.

## Simulation example

A full demo — plain-text notification, HTML alert to a forum-group topic, and a rule-based reroute — is in `crates/simulation/examples/telegram_simulation.rs`:

```bash
cargo run -p acteon-simulation --example telegram_simulation
```

The simulation uses a recording provider, so it runs offline with no real Telegram credentials.
