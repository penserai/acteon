# Authentication

Acteon supports API key and JWT-based authentication with role-based access control.

## Enabling Authentication

```toml title="acteon.toml"
[auth]
enabled = true
config_path = "auth.toml"
watch = true                    # Hot-reload on file changes
```

## Authentication Methods

### API Key

Include the API key in the request header:

```bash
curl -H "Authorization: Bearer your-api-key" http://localhost:8080/v1/dispatch
```

### JWT Token

Obtain a JWT via the login endpoint:

```bash
# Login
curl -X POST http://localhost:8080/v1/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "secret"}'
```

Response:

```json
{
  "token": "eyJhbGciOiJIUzI1NiIs...",
  "expires_in": 3600
}
```

Use the token for subsequent requests:

```bash
curl -H "Authorization: Bearer eyJhbGciOiJIUzI1NiIs..." \
  http://localhost:8080/v1/dispatch
```

### Logout

Revoke a JWT token:

```bash
curl -X POST http://localhost:8080/v1/auth/logout \
  -H "Authorization: Bearer eyJhbGciOiJIUzI1NiIs..."
```

## Hot Reload

When `watch = true`, changes to the auth configuration file are automatically detected and applied without server restart:

```toml
[auth]
enabled = true
config_path = "auth.toml"
watch = true
```

## Security Features

- **Password hashing** — Argon2 for secure password storage
- **JWT signing** — HMAC-SHA256 for token integrity
- **Token revocation** — Immediate logout support
- **HMAC-signed approval URLs** — Tamper-proof approval/rejection links
- **Role-based access control** — Grant-level authorization

## Approval URL Signing

Approval URLs are HMAC-signed with configurable keys:

```toml
[server]
# approval_hmac_keys = [
#   { kid = "key-1", secret = "base64-encoded-secret" }
# ]
```

The signature includes namespace, tenant, approval ID, action (approve/reject), and expiration timestamp.
