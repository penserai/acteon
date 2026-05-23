# Mutual TLS (mTLS) Support

Acteon supports TLS across the full stack: HTTPS termination for the server, client certificate authentication for outbound backend connections, and TLS-enabled HTTP clients for provider egress. All TLS is backed by `rustls` (pure Rust).

## Configuration

Add a `[tls]` section to your `acteon.toml`:

```toml
[tls]
enabled = true

# Server-side: HTTPS termination
[tls.server]
cert_path = "/etc/acteon/tls/server.crt"
key_path = "/etc/acteon/tls/server.key"
# Optional: require client certificates (inbound mTLS)
# client_ca_path = "/etc/acteon/tls/client-ca.crt"
min_version = "1.2"  # "1.2" (default) or "1.3"

# Client-side: outbound mTLS for backends and providers
[tls.client]
cert_path = "/etc/acteon/tls/client.crt"
key_path = "/etc/acteon/tls/client.key"
ca_bundle_path = "/etc/acteon/tls/ca-bundle.crt"  # omit to use Mozilla roots
danger_accept_invalid_certs = false                # dev/test only
```

## Server HTTPS

When `tls.enabled = true` and `tls.server.cert_path`/`key_path` are set, the server binds an HTTPS listener instead of plain TCP. The server uses `tokio-rustls` for the TLS handshake and `hyper-util` to serve HTTP/1.1 and HTTP/2 over TLS.

### Client Certificate Verification (Inbound mTLS)

Set `tls.server.client_ca_path` to a PEM file containing the CA certificate(s) that sign client certificates. When configured, the server requires valid client certificates for all connections.

## Provider TLS

When TLS is enabled, a shared `reqwest::Client` is built with the client TLS configuration and injected into all HTTP-based providers (webhook, Twilio, Teams, Discord). This ensures outbound calls use the configured client certificates and CA bundle.

## Backend TLS

### PostgreSQL

Add SSL fields to the `[state]` or `[audit]` section:

```toml
[state]
backend = "postgres"
url = "postgres://user:pass@db.example.com/acteon"
ssl_mode = "verify-full"       # disable, prefer, require, verify-ca, verify-full
ssl_root_cert = "/etc/acteon/tls/pg-ca.crt"
ssl_cert = "/etc/acteon/tls/pg-client.crt"
ssl_key = "/etc/acteon/tls/pg-client.key"

[audit]
backend = "postgres"
url = "postgres://user:pass@db.example.com/acteon_audit"
ssl_mode = "require"
ssl_root_cert = "/etc/acteon/tls/pg-ca.crt"
```

These fields map to `sqlx` `PgConnectOptions` SSL settings. The `tls-rustls` feature is used for the TLS backend.

### Redis

Redis TLS works via URL scheme. Set `tls_enabled = true` to automatically upgrade `redis://` to `rediss://`:

```toml
[state]
backend = "redis"
url = "redis://redis.example.com:6380"
tls_enabled = true
# tls_insecure = false  # dev/test only
```

You can also use `rediss://` directly in the URL:

```toml
[state]
backend = "redis"
url = "rediss://redis.example.com:6380"
```

### Elasticsearch

The Elasticsearch audit backend shares the global TLS-configured HTTP client when `tls.enabled = true`. No additional configuration is needed beyond the global `[tls.client]` section.

## Architecture

```
                 TLS Config (acteon.toml)
                        |
        +---------------+---------------+
        |               |               |
   Server TLS     Client TLS      Backend TLS
   (inbound)      (providers)   (Postgres/Redis/ES)
        |               |               |
   rustls         reqwest+rustls    sqlx+rustls
   ServerConfig    Client          PgConnectOptions
   TlsAcceptor                    deadpool-redis
```

- **acteon-crypto**: Certificate loading (`load_certs`, `load_private_key`), `rustls` config builders, `reqwest::Client` builder
- **acteon-server**: TLS listener wrapping, shared HTTP client injection, config threading to backend factories
- **Backend crates**: SSL fields on config structs, `connect_with()` for Postgres, `rediss://` for Redis

## Certificate Formats

All certificate and key files must be in PEM format. The server certificate file may contain the full certificate chain (leaf + intermediates). Private keys may be PKCS#8, RSA (PKCS#1), or EC (SEC1) encoded.
