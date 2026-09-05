# Contributing

Use an isolated branch and keep changes focused on an observable behavior.
Describe the problem, resulting behavior, compatibility effects, and verification.
Follow the repository conventions in [CLAUDE.md](CLAUDE.md).

Before proposing a change, run:

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --no-deps -- -D warnings
cargo test --locked --workspace --lib --bins --tests
cargo check --locked --workspace --all-targets
(cd clients/python && ruff check . && ruff format --check . && mypy acteon_client && pytest -q)
(cd clients/nodejs && npm ci && npm run lint && npm run typecheck && npm test && npm run build && npm run package:check)
(cd ui && npm ci && npm run lint && npm run build && npm run test:smoke)
```

Backend changes also require the shared state/lock conformance tests against
real services. Use `REDIS_URL` and `DATABASE_URL` pointing to disposable databases.
`cargo test -p acteon-state-redis -p acteon-state-postgres --features integration --lib`
fails when services are unavailable; it does not silently skip.

Run `scripts/ci/scenarios.sh memory redis postgres` to generate and replay scenario
evidence. Keep scenario manifests versioned and add invariants at producer/consumer
boundaries. A passing regex search or model explanation is not sufficient evidence.

Run `scripts/ci/security.sh`, `cargo deny --locked check licenses sources`, and
`scripts/ci/secrets.sh` for dependency/security changes. The pinned CI tools are
cargo-audit 0.22.1, cargo-deny 0.20.2, and Gitleaks 8.30.1. Review the reason for
any exception; do not disable a rule category merely to make a check pass.

Security-sensitive reports should follow [SECURITY.md](SECURITY.md).
