# WeChat Work Provider

Acteon ships with a first-class **WeChat Work** (企业微信 / Enterprise WeChat) provider that sends messages via the [Message Send API][api] — the same endpoint Alertmanager targets via its `wechat_configs`. It's the last receiver in the Phase 4 Alertmanager-parity set and the most architecturally involved, because of three WeChat-specific quirks the provider handles transparently.

[api]: https://developer.work.weixin.qq.com/document/path/90236

## Three things that make WeChat different

1. **Access tokens expire every 7200 seconds.** Every API call passes an `access_token` query parameter that must be refreshed by calling a separate `gettoken` endpoint with the org's `corp_id` + `corp_secret`. The provider caches tokens and refreshes lazily with a configurable buffer window (default 300s / 5 minutes) so a token right at the edge of its TTL does not race an in-flight dispatch.
2. **Token revocation is in-band.** If the server returns `errcode: 42001` ("access_token expired") or `40014` ("invalid access_token") mid-send, the provider invalidates its cached token and retries the request **exactly once** with a fresh token. Operators don't need to restart anything when a token is revoked out of band.
3. **Errors travel in a `{"errcode": 0, "errmsg": "ok", ...}` envelope.** HTTP 200 with `errcode != 0` is the normal failure shape; the provider classifies non-zero errcodes into retryable / non-retryable buckets so the gateway's retry logic handles transient server-busy errors correctly.

## Secret hygiene

Both `corp_id` and `corp_secret` live in `SecretString`, zeroized on drop and redacted in `Debug` output. Neither is logged in normal operation — the `gettoken` URL (which embeds both as query parameters) is deliberately excluded from debug/error log strings. The cached access token is also a `SecretString`.

## Health check

Because `get_access_token` is both a connectivity check and a credential check (a bad `corp_secret` surfaces as `errcode: 40001` from `gettoken`), the provider's `health_check` calls it directly. Bad credentials show up on the provider health dashboard as non-retryable `Configuration` errors, same as the Telegram provider. Network outages surface as retryable `Connection`.

## TOML configuration

```toml
[[providers]]
name = "wechat-ops"
type = "wechat"
wechat.corp_id = "ENC[AES256_GCM,data:abc123...]"
wechat.corp_secret = "ENC[AES256_GCM,data:def456...]"
wechat.agent_id = 1000002
wechat.default_touser = "@all"
wechat.default_msgtype = "text"              # default; also "markdown" or "textcard"
# wechat.default_toparty = "12|15"           # department IDs
# wechat.default_totag = "oncall"            # tag IDs
# wechat.safe = false                        # confidential (no forwarding / screenshots)
# wechat.enable_duplicate_check = true
# wechat.duplicate_check_interval = 1800     # seconds
# wechat.token_refresh_buffer_seconds = 300  # default 5 minutes
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used when dispatching actions |
| `type` | Yes | Must be `"wechat"` |
| `wechat.corp_id` | Yes | Corporation ID from the WeChat Work admin console. Supports `ENC[...]`. |
| `wechat.corp_secret` | Yes | Per-app secret from the admin console. Supports `ENC[...]`. |
| `wechat.agent_id` | Yes | Numeric agent ID — identifies which WeChat Work app is sending. |
| `wechat.default_touser` | No* | Default `\|`-separated user IDs, or `"@all"` for everyone. |
| `wechat.default_toparty` | No* | Default `\|`-separated department IDs. |
| `wechat.default_totag` | No* | Default `\|`-separated tag IDs. |
| `wechat.default_msgtype` | No | `"text"` (default), `"markdown"`, or `"textcard"`. |
| `wechat.safe` | No | Mark outgoing messages as confidential (no forwarding / screenshots). Defaults to `false`. |
| `wechat.enable_duplicate_check` | No | Enable server-side dedup. When `true`, `duplicate_check_interval` is required. |
| `wechat.duplicate_check_interval` | No | Dedup window in seconds (max 1800). |
| `wechat.token_refresh_buffer_seconds` | No | Refresh window before token expiry (default 300). |
| `wechat.api_base_url` | No | Override base URL (testing only). |

\* At least one of `default_touser`, `default_toparty`, or `default_totag` must be set **either** in the provider config **or** in every dispatch payload — WeChat rejects messages with no recipients.

## Payload shape

WeChat has no lifecycle concept — the provider accepts one `event_action` (`"send"`, also the default) and supports three message types:

| `msgtype` | Required fields | Use case |
|---|---|---|
| `"text"` | `content` | Plain text |
| `"markdown"` | `content` | WeChat-flavored markdown (limited syntax — see [API docs](https://developer.work.weixin.qq.com/document/path/90236)) |
| `"textcard"` | `title`, `description`, `url` | Clickable card with title, body, and link (optional `btntxt` for the button label) |

Image, voice, video, file, news, taskcard, template_card, mpnews, and miniprogram_notice message types are **not** supported in v1 — they're for content delivery rather than alerting and have complex nested payload shapes. Add them as a follow-up if demand emerges.

### Text message

```json
{
  "touser": "@all",
  "content": "Deploy #4823 shipped to production."
}
```

### Markdown

```json
{
  "toparty": "12|15",
  "msgtype": "markdown",
  "content": "### Latency spike on **checkout-api**\n> p95 above 2s for 5 minutes\n> [Open runbook](https://wiki.example.com/runbook/checkout-latency)"
}
```

### Textcard

```json
{
  "totag": "oncall",
  "msgtype": "textcard",
  "title": "CRITICAL: checkout-api down",
  "description": "5xx rate above 50% for 2 minutes. Oncall paged.",
  "url": "https://wiki.example.com/runbook/checkout-5xx",
  "btntxt": "Open runbook"
}
```

### Recipient routing

Each of `touser`, `toparty`, and `totag` accepts a `|`-separated string of IDs. The provider resolves them in this order:

1. **Payload-supplied fields** take precedence per field.
2. **Config defaults** fill in any fields the payload omits.
3. At least one of the three must end up populated, or the provider rejects the request with a non-retryable `Serialization` error before it ever hits the WeChat API.

## Rule integration

Because WeChat is just another named provider, every routing primitive Acteon already has works with it:

- **Reroute critical alerts** to a WeChat department by matching on `action.payload.severity == "critical"` with a `reroute` rule.
- **Silence maintenance windows** with [silences](silences.md) — silences apply before the provider dispatch, so a WeChat message never leaves the gateway during an active silence.
- **Quota-bound a WeChat agent** via a [per-provider tenant quota](tenant-quotas.md) scoped to `provider: "wechat-ops"`.
- **Dedup noisy events** with Acteon's [deduplication](deduplication.md) using `Action.dedup_key`. You can also enable WeChat's native server-side `duplicate_check` for defense in depth.

## Outcome body

On success the provider returns an `Executed` outcome whose `body` carries the WeChat response:

```json
{
  "errcode": 0,
  "errmsg": "ok",
  "msgid": "xxxx",
  "invaliduser": "u3",
  "invalidparty": null,
  "invalidtag": null
}
```

The `msgid` is the server-assigned message ID. The `invaliduser` / `invalidparty` / `invalidtag` fields list any recipients the server couldn't reach — a **partial** delivery is still classified as success because the send itself succeeded, but operators can surface these in audit or alerting pipelines if they care about reachability.

## Error mapping

| WeChat response | `ProviderError` | Retryable? |
|---|---|---|
| HTTP 2xx, `errcode: 0` | `Executed` (success) | — |
| `errcode: 42001` / `40014` | Internal retry (refresh token, send once more) | Invisible |
| 42001 / 40014 *twice* (after refresh) | `Configuration` | No |
| `errcode: 40001` / `40013` | `Configuration` | No — bad `corp_id` / `corp_secret` |
| `errcode: 45009` | `RateLimited` | Yes |
| `errcode: -1` | `Connection` (via `Transient`) | Yes — system busy |
| Other non-zero `errcode` | `ExecutionFailed` | No |
| HTTP 401 / 403 | `Configuration` | No |
| HTTP 5xx / 408 | `Connection` (via `Transient`) | Yes |
| Transport failure | `Connection` | Yes |

## Simulation example

A full demo — text broadcast, markdown alert, textcard, plus a rule-based severity reroute — is in `crates/simulation/examples/wechat_simulation.rs`:

```bash
cargo run -p acteon-simulation --example wechat_simulation
```

The simulation uses a recording provider, so it runs offline with no real WeChat Work credentials.
