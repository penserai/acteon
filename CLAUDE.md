# Claude Code Instructions for Acteon

## Pre-commit Checks

Always run these checks before committing:

```bash
# Format code
cargo fmt --all

# Lint with clippy (warnings as errors)
cargo clippy --workspace --no-deps -- -D warnings

# Run all tests
cargo test --workspace

# Build to catch any remaining issues
cargo build --workspace
```

## Quick Validation

```bash
# One-liner to run all checks
cargo fmt --all && cargo clippy --workspace --no-deps -- -D warnings && cargo test --workspace
```

## Running Examples

```bash
# Demo simulation (no external deps)
cargo run -p acteon-simulation --example demo_simulation

# Rules from files example
cargo run -p acteon-simulation --example rules_from_files

# HTTP client simulation (requires running server)
cargo run -p acteon-server &
cargo run -p acteon-simulation --example http_client_simulation

# Backend-specific simulations (require Docker)
cargo run -p acteon-simulation --example redis_simulation --features redis
cargo run -p acteon-simulation --example postgres_simulation --features postgres
```

## Project Structure

- `acteon-core` - Shared types (Action, ActionOutcome)
- `acteon-client` - Native Rust HTTP client
- `acteon-gateway` - Core action routing logic
- `acteon-server` - HTTP server (Axum)
- `acteon-simulation` - Testing framework
- `acteon-state-*` - State backend implementations
- `acteon-audit-*` - Audit backend implementations
- `acteon-rules-*` - Rule parsing frontends

## Common Clippy Fixes

- Use `#[must_use]` on builder methods returning `Self`
- Merge identical match arms with `|`
- Use inlined format args: `"{x}"` instead of `"{}", x`
- Collapse nested `if let` with `&&`
- Use `.clone().unwrap_or_else()` instead of `.as_ref().map(Arc::clone).unwrap_or_else()`
- Add `#[allow(clippy::unused_async)]` for async functions without await (if intentional)
- Wrap technical terms in backticks in doc comments (e.g., `PostgreSQL`)
