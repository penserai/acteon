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

# Frontend checks
cd ui && npm run lint && npm run build && cd ..

# Build to catch any remaining issues
cargo build --workspace
```

## Quick Validation

```bash
# One-liner to run all checks (Rust + Frontend)
cargo fmt --all && cargo clippy --workspace --no-deps -- -D warnings && cargo test --workspace && (cd ui && npm run lint && npm run build)
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
- `ui/` - Admin UI (React + Vite + Tailwind v4)

## Feature Implementation Steps

When implementing a new feature (provider, capability, etc.), follow this layered approach:

1. **Core types** – Add/modify types in `acteon-core` (e.g., new `ActionOutcome` variant, new config struct)
2. **Crate implementation** – Create or modify the relevant crate under `crates/` (provider, gateway logic, etc.)
3. **Register in workspace** – Add new crate to `Cargo.toml` workspace members and `[workspace.dependencies]`
4. **Server integration** – Wire into `acteon-server` (routes, handlers, query params, OpenAPI annotations)
5. **UI updates** – Update the React frontend in `ui/` to reflect new capabilities
6. **Gateway integration** – Update `acteon-gateway` dispatch pipeline if needed
7. **Rust client** – Add helper methods to `acteon-client` (`crates/client/src/`)
8. **Polyglot client SDKs** – Update Python, Node.js, Go, and Java clients in `clients/`
9. **Tests** – Add unit tests in the crate, SDK tests, and simulation framework tests
10. **Documentation** – Update `docs/book/` pages (concepts, API reference, examples)
11. **Simulation example** – Add a simulation example in `crates/simulation/examples/`
12. **Pre-commit checks** – Run `cargo fmt --all && cargo clippy --workspace --no-deps -- -D warnings && cargo test --workspace && (cd ui && npm run lint && npm run build)`

## Common Clippy Fixes

- Use `#[must_use]` on builder methods returning `Self`
- Merge identical match arms with `|`
- Use inlined format args: `"{x}"` instead of `"{}", x`
- Collapse nested `if let` with `&&`
- Use `.clone().unwrap_or_else()` instead of `.as_ref().map(Arc::clone).unwrap_or_else()`
- Add `#[allow(clippy::unused_async)]` for async functions without await (if intentional)
- Wrap technical terms in backticks in doc comments (e.g., `PostgreSQL`)
