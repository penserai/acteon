# Build Optimization

Acteon's workspace pulls in ~688 crate dependencies at full capacity (wasmtime, 8 AWS SDKs, crypto stacks, etc.). An untuned clean build can take over an hour. This guide covers the optimizations in place and how to get the fastest local development experience.

## Feature Selection

The single biggest lever is **not compiling what you don't need**. AWS providers are behind feature flags and excluded by default:

```bash
# Default build — no AWS SDKs (~480 deps)
cargo build -p acteon-server

# Enable only the providers you're working on
cargo build -p acteon-server --features "aws-sns,aws-lambda"

# Full build with all AWS providers (~688 deps)
cargo build -p acteon-server --features aws-all
```

### Complete Feature Flag Reference

#### Storage Backends

| Feature | Description | Default |
|---------|------------|---------|
| `redis` | Redis state backend | Yes |
| `postgres` | PostgreSQL state + audit | No |
| `clickhouse` | ClickHouse state + audit | No |
| `dynamodb` | DynamoDB state + audit | No |
| `elasticsearch` | Elasticsearch audit | No |
| `all-backends` | All of the above | No |

#### AWS Providers

| Feature | AWS SDK | Default |
|---------|---------|---------|
| `aws-sns` | `aws-sdk-sns` | No |
| `aws-lambda` | `aws-sdk-lambda` | No |
| `aws-eventbridge` | `aws-sdk-eventbridge` | No |
| `aws-sqs` | `aws-sdk-sqs` | No |
| `aws-ses` | `aws-sdk-sesv2` | No |
| `aws-s3` | `aws-sdk-s3` | No |
| `aws-ec2` | `aws-sdk-ec2` | No |
| `aws-autoscaling` | `aws-sdk-autoscaling` | No |
| `aws-all` | All eight AWS SDKs | No |

## Profile Overrides

The workspace `Cargo.toml` includes dev-profile overrides that optimize heavy dependencies even in debug builds:

```toml
[profile.dev.package.wasmtime]
opt-level = 2
[profile.dev.package.cranelift-codegen]
opt-level = 2
[profile.dev.package.ring]
opt-level = 2
[profile.dev.package.rustls]
opt-level = 2
```

These crates are extremely slow at `opt-level = 0` (wasmtime's JIT compiler, ring's crypto). Compiling them at `-O2` makes test execution faster and only adds a few seconds to clean builds since they change rarely.

The build-override section speeds up proc-macro and build-script compilation:

```toml
[profile.dev.build-override]
opt-level = 0
codegen-units = 256
```

## Linker Configuration

The `.cargo/config.toml` file configures platform-specific optimizations:

**macOS (Apple Silicon + Intel):** `split-debuginfo=unpacked` skips the expensive `dsymutil` pass, saving 10-30 seconds per incremental link.

**Linux:** Rust 1.90+ uses `rust-lld` by default. For older toolchains, you can optionally use `mold` for faster linking (uncomment the config section).

## cargo-nextest

[cargo-nextest](https://nexte.st) runs tests as individual processes in parallel, which is significantly faster than `cargo test` for large workspaces. The configuration lives in `.config/nextest.toml`:

```bash
# Install nextest
cargo install cargo-nextest

# Run tests (equivalent to cargo test --workspace --lib --bins --tests)
cargo nextest run --workspace --lib --bins --tests
```

Key configuration:

- **`fail-fast = true`** (default profile) — stop on first failure for fast feedback
- **`http-integration` test group** — limits concurrency to 4 threads for integration tests that spin up HTTP servers, preventing port/resource contention
- **CI profile** — `retries = 2`, `fail-fast = false` for more resilient CI runs

## Skipping Doctests

Doctests are compiled as individual binaries, each re-linking the entire dependency tree. For a workspace this size, this adds minutes of wall-clock time:

```bash
# Fast — skip doctests (recommended for local dev)
cargo test --workspace --lib --bins --tests

# With doctests — only in CI or before releases
cargo test --workspace
```

The CI workflow splits these into separate steps: `nextest` for unit/integration tests, then `cargo test --workspace --doc` for doctests.

## Diagnosing Build Bottlenecks

Use `cargo build --timings` to generate an HTML report showing which crates take the longest:

```bash
cargo build -p acteon-server --timings
# Opens target/cargo-timings/cargo-timing.html
```

Common bottlenecks:

| Crate | Typical Time | Why |
|-------|-------------|-----|
| `wasmtime` + cranelift | 30-60s | JIT compiler codegen |
| `aws-sdk-ec2` | 15-25s | Huge API surface (thousands of types) |
| `aws-sdk-*` (each) | 5-15s | SDK code generation |
| `ring` | 5-10s | Crypto with asm |
| `rustls` | 3-5s | TLS implementation |

## CI Optimizations

The GitHub Actions workflow (`.github/workflows/ci.yml`) uses several optimizations:

- **sccache** — shared compilation cache across CI runs (`RUSTC_WRAPPER=sccache`)
- **Swatinem/rust-cache** — caches `target/` directory between runs
- **nextest** — parallel test execution with the CI profile
- **Split test steps** — unit tests, AWS tests, and doctests run as separate steps so failures are isolated
- **AWS tests isolated** — `cargo nextest run -p acteon-aws --features full` runs separately since AWS features are not in the default build

## Recommended Dev Workflow

```bash
# 1. Build and iterate on your crate
cargo test -p acteon-gateway --lib

# 2. Run the full local check before committing
cargo fmt --all && \
  cargo clippy --workspace --no-deps -- -D warnings && \
  cargo test --workspace --lib --bins --tests && \
  (cd ui && npm run lint && npm run build)

# 3. If you touched AWS code, also run:
cargo test -p acteon-aws --features full --lib --tests
```
