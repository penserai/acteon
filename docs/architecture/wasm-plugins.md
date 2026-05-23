# WASM Rule Plugins Architecture

## Overview

WASM rule plugins extend the Acteon rule engine with user-supplied WebAssembly
modules executed in a sandboxed `Wasmtime` runtime. Each plugin receives an
action context as JSON and returns a boolean verdict with optional metadata.
The system enforces strict resource limits (memory, CPU time) and provides
full observability through metrics, structured logging, and Rule Playground
trace integration.

This document describes the design decisions, component interactions, data
flow, sandbox model, and performance characteristics.

---

## 1. Data Model

### `WasmPluginConfig` (per-plugin configuration)

Defined in `crates/wasm-runtime/src/config.rs`:

```rust
pub struct WasmPluginConfig {
    /// Unique plugin identifier.
    pub name: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// Maximum memory in bytes (default: 16 MB, max: 256 MB).
    pub memory_limit_bytes: u64,
    /// Maximum execution time in milliseconds (default: 100 ms, max: 30 s).
    pub timeout_ms: u64,
    /// Whether the plugin is enabled.
    pub enabled: bool,
    /// Path to the `.wasm` file (if loaded from disk).
    pub wasm_path: Option<String>,
}
```

### `WasmRuntimeConfig` (global runtime configuration)

```rust
pub struct WasmRuntimeConfig {
    /// Whether the WASM runtime is enabled (default: false).
    pub enabled: bool,
    /// Directory to scan for `.wasm` files on startup.
    pub plugin_dir: Option<String>,
    /// Default memory limit for plugins.
    pub default_memory_limit_bytes: u64,
    /// Default timeout for plugins.
    pub default_timeout_ms: u64,
}
```

### `WasmInvocationResult` (plugin output)

```rust
pub struct WasmInvocationResult {
    /// Whether the plugin condition passed.
    pub verdict: bool,
    /// Optional message (explanation, reason).
    pub message: Option<String>,
    /// Optional structured metadata.
    pub metadata: serde_json::Value,
}
```

### `WasmError` (error types)

```rust
pub enum WasmError {
    PluginNotFound(String),
    Compilation(String),
    Invocation(String),
    Timeout(u64),
    MemoryExceeded(u64),
    InvalidOutput(String),
    Io(std::io::Error),
    RegistryFull(usize),
}
```

---

## 2. Component Architecture

```
┌─────────────────────────────────────────────────────────┐
│                     Gateway                              │
│                                                          │
│  ┌──────────┐   ┌────────────┐   ┌───────────────────┐  │
│  │  Rule     │──>│  WASM      │──>│   Wasmtime        │  │
│  │  Engine   │   │  Runtime   │   │   Sandbox         │  │
│  │          │<──│  (trait)   │<──│   (per-plugin)    │  │
│  └──────────┘   └────────────┘   └───────────────────┘  │
│       │              │                     │             │
│       │         ┌────────────┐      ┌─────────────┐     │
│       │         │  Plugin    │      │  Compiled   │     │
│       │         │  Registry  │      │  Modules    │     │
│       │         └────────────┘      └─────────────┘     │
│       │                                                  │
│  ┌──────────────────────────────────────────────────┐   │
│  │                  Metrics                          │   │
│  │  invocations | errors | duration | memory         │   │
│  └──────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

| Layer | Crate | Responsibility |
|-------|-------|---------------|
| **Rule Engine** | `acteon-rules` | Detects `RuleSource::WasmPlugin`, delegates to runtime |
| **Runtime Trait** | `acteon-wasm-runtime` | Abstract `WasmPluginRuntime` trait with `invoke()`, `has_plugin()`, `list_plugins()` |
| **Registry** | `acteon-wasm-runtime` | Maps plugin names to compiled WASM modules and configs |
| **Wasmtime Sandbox** | `acteon-wasm-runtime` | Instantiates WASM modules with resource limits, manages memory protocol |
| **Server API** | `acteon-server` | CRUD endpoints for plugin management |
| **Gateway** | `acteon-gateway` | Holds `Arc<dyn WasmPluginRuntime>`, passes to rule engine |

---

## 3. Runtime Trait Design

The `WasmPluginRuntime` trait abstracts the WASM execution layer:

```rust
#[async_trait]
pub trait WasmPluginRuntime: Send + Sync + Debug {
    async fn invoke(
        &self,
        plugin: &str,
        function: &str,
        input: &serde_json::Value,
    ) -> Result<WasmInvocationResult, WasmError>;

    fn has_plugin(&self, name: &str) -> bool;

    fn list_plugins(&self) -> Vec<String>;
}
```

### Trait Implementations

| Implementation | Use Case |
|---------------|----------|
| `WasmPluginRegistry` | Production: manages compiled modules, enforces limits |
| `MockWasmRuntime` | Testing: returns configured verdict without actual WASM |
| `FailingWasmRuntime` | Testing: always returns an error |

The trait is `Send + Sync` so it can be shared as `Arc<dyn WasmPluginRuntime>`
across async tasks in the gateway.

---

## 4. Plugin Registry Design

The `WasmPluginRegistry` manages the lifecycle of WASM plugins:

```rust
pub struct WasmPluginRegistry {
    engine: wasmtime::Engine,
    plugins: RwLock<HashMap<String, RegisteredPlugin>>,
    config: WasmRuntimeConfig,
}

struct RegisteredPlugin {
    config: WasmPluginConfig,
    module: wasmtime::Module,
    invocation_count: AtomicU64,
    last_invoked_at: AtomicI64,
    registered_at: DateTime<Utc>,
}
```

### Key Design Decisions

1. **Single `wasmtime::Engine`**: One engine is shared across all plugins.
   Wasmtime's `Engine` is thread-safe and caches compilation results.

2. **Pre-compiled modules**: WASM binaries are compiled to native code at
   registration time (not at invocation time). This amortizes the compilation
   cost across all invocations.

3. **Per-invocation instances**: Each `invoke()` call creates a fresh
   `wasmtime::Instance` from the pre-compiled module. This ensures complete
   isolation between invocations (no shared mutable state).

4. **`parking_lot::RwLock`**: The plugin map uses a read-write lock. Plugin
   invocation takes a read lock (concurrent), while registration/deletion
   takes a write lock (exclusive). This matches the read-heavy access pattern.

5. **Maximum 256 plugins**: The `MAX_TRACKED_PLUGINS` constant prevents
   unbounded growth of the registry and associated memory.

---

## 5. Sandbox Model

### Memory Isolation

Each WASM plugin invocation runs in an isolated linear memory:

- The `wasmtime::Store` is configured with a `StoreLimiter` that enforces
  the per-plugin `memory_limit_bytes`
- Memory growth beyond the limit causes a trap (caught as `WasmError::MemoryExceeded`)
- The plugin's memory is freed when the `Store` is dropped at the end of
  the invocation

### CPU Isolation

- Each invocation is wrapped in a `tokio::time::timeout` with the per-plugin
  `timeout_ms` duration
- Wasmtime's epoch-based interruption is used as a secondary mechanism:
  the engine's epoch is incremented on a background timer, and the store
  is configured to trap after a deadline epoch
- Both mechanisms together ensure that runaway plugins are terminated reliably

### Capability Restriction

- Only WASI preview-1 imports are provided: `clock_time_get` and `random_get`
- No filesystem, network, or environment variable access
- No custom host function imports (plugins are pure functions)

### Security Boundaries

```
┌──────────────┐     JSON input        ┌──────────────────┐
│  Host (Rust) │ ─────────────────────> │  Guest (WASM)    │
│              │                        │                  │
│  - Validates │     JSON output        │  - No FS access  │
│    output    │ <───────────────────── │  - No net access │
│  - Enforces  │                        │  - No env vars   │
│    timeout   │                        │  - Memory capped │
│  - Enforces  │                        │  - CPU capped    │
│    memory    │                        │                  │
└──────────────┘                        └──────────────────┘
```

---

## 6. Data Flow

### Invocation Sequence

```
1. Rule engine encounters rule with source=WasmPlugin
2. Rule engine calls wasm_runtime.invoke(plugin_name, function, action_json)
3. Registry looks up the pre-compiled Module for plugin_name
4. Registry creates a new Store with:
   - Memory limiter (memory_limit_bytes)
   - Epoch deadline (timeout_ms)
5. Registry instantiates the Module in the Store
6. Registry serializes action_json to bytes
7. Registry allocates memory in WASM instance, writes input bytes
8. Registry calls the exported function(ptr, len)
9. Plugin processes input, writes output to its linear memory
10. Plugin returns packed (ptr, len) result
11. Registry reads output bytes from WASM memory
12. Registry deserializes output as WasmInvocationResult
13. Registry validates: output must have "verdict" boolean
14. Store is dropped (memory freed)
15. Result returned to rule engine
```

### API Registration Flow

```
1. Client uploads .wasm file via POST /v1/wasm/plugins (multipart)
2. Server validates: name non-empty, file not empty, registry not full
3. Server calls registry.register(name, wasm_bytes, config)
4. Registry compiles WASM bytes to native code (wasmtime::Module)
5. Registry stores RegisteredPlugin in the HashMap
6. Server returns 201 with plugin metadata
```

---

## 7. Rule Engine Integration

### New Rule Source

The `RuleSource` enum gains a `WasmPlugin` variant:

```rust
pub enum RuleSource {
    Yaml,
    Cel,
    Api,
    WasmPlugin,
}
```

### New Rule Fields

The `Rule` struct gains optional WASM-specific fields:

```rust
pub struct Rule {
    // ... existing fields ...
    /// WASM plugin name (required when source is WasmPlugin).
    pub wasm_plugin: Option<String>,
    /// WASM function to call (defaults to "evaluate").
    pub wasm_function: Option<String>,
}
```

### Evaluation Path

In `RuleEngine::evaluate()`:

```rust
if rule.source == RuleSource::WasmPlugin {
    let plugin_name = rule.wasm_plugin.as_deref()
        .ok_or("wasm_plugin field required")?;
    let function = rule.wasm_function.as_deref().unwrap_or("evaluate");

    let action_json = build_action_context(&action, &eval_context);
    let result = wasm_runtime.invoke(plugin_name, function, &action_json).await?;

    return Ok(result.verdict);
}
```

When `evaluate_all` mode is used (Rule Playground), WASM details are captured
in the trace entry for debugging.

### YAML Frontend

The YAML parser recognizes:

```yaml
source: wasm_plugin
wasm_plugin: plugin-name
wasm_function: evaluate  # optional
```

### CEL Frontend

The CEL evaluator recognizes the `wasm()` function:

```
wasm("plugin-name", "function-name")
```

---

## 8. Server API Design

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/wasm/plugins` | Register a new plugin (multipart upload) |
| `GET` | `/v1/wasm/plugins` | List all registered plugins |
| `GET` | `/v1/wasm/plugins/{name}` | Get plugin detail |
| `DELETE` | `/v1/wasm/plugins/{name}` | Delete a plugin |
| `POST` | `/v1/wasm/plugins/{name}/test` | Test invocation |

### Permission Model

| Endpoint | Required Permission |
|----------|-------------------|
| `GET /v1/wasm/plugins` | `WasmManage` or `Admin` |
| `POST /v1/wasm/plugins` | `WasmManage` or `Admin` |
| `DELETE /v1/wasm/plugins/{name}` | `WasmManage` or `Admin` |
| `POST /v1/wasm/plugins/{name}/test` | `WasmManage` or `Admin` |

WASM plugin management is admin-only by default to prevent untrusted code
from being loaded.

### Server Configuration

```toml
[wasm]
enabled = false
plugin_dir = "/etc/acteon/plugins"
default_memory_limit_bytes = 16777216
default_timeout_ms = 100
```

The `[wasm]` section maps to `WasmRuntimeConfig`.

---

## 9. Metrics and Observability

### Gateway Metrics

```rust
// New AtomicU64 counters in GatewayMetrics
pub wasm_invocations: AtomicU64,
pub wasm_errors: AtomicU64,
```

### Per-Plugin Metrics (in Registry)

Each `RegisteredPlugin` tracks:
- `invocation_count: AtomicU64`
- `last_invoked_at: AtomicI64`

These are exposed via the `GET /v1/wasm/plugins` list response.

### Prometheus Metrics

```
# HELP acteon_wasm_invocations_total Total WASM plugin invocations
# TYPE acteon_wasm_invocations_total counter
acteon_wasm_invocations_total 1024

# HELP acteon_wasm_invocation_errors Total WASM invocation errors
# TYPE acteon_wasm_invocation_errors counter
acteon_wasm_invocation_errors 3
```

### Structured Logging

All WASM operations emit `tracing` spans and events:

- `wasm.plugin.register` -- plugin registered (info)
- `wasm.plugin.delete` -- plugin removed (info)
- `wasm.plugin.invoke` -- invocation started/completed (debug)
- `wasm.plugin.timeout` -- invocation timed out (warn)
- `wasm.plugin.memory_exceeded` -- memory limit exceeded (warn)
- `wasm.plugin.error` -- invocation failed (error)

---

## 10. Performance Characteristics

### Compilation Cost

WASM compilation happens once at registration time. Wasmtime's Cranelift
backend compiles WASM to optimized native code. Expected compilation time:

| Module Size | Compilation Time |
|-------------|-----------------|
| 10 KB | ~5 ms |
| 100 KB | ~20 ms |
| 1 MB | ~200 ms |
| 10 MB | ~2 s |

### Invocation Cost

Each invocation creates a new `Store` + `Instance`. Wasmtime's instance
creation is fast because the Module is pre-compiled:

| Operation | Typical Time |
|-----------|-------------|
| Store creation | ~1 us |
| Instance creation | ~10 us |
| Input serialization | ~1-5 us |
| Function call overhead | ~5 us |
| Plugin logic | Depends on plugin |
| Output deserialization | ~1-5 us |
| **Total overhead** | **~20-30 us** |

The WASM runtime overhead is negligible compared to network I/O for provider
dispatch.

### Memory Usage

| Component | Memory |
|-----------|--------|
| Wasmtime Engine | ~10 MB (shared) |
| Pre-compiled Module | ~2x module size (per plugin) |
| Per-invocation Store | Up to `memory_limit_bytes` (freed after call) |

### Concurrency

- The registry uses `RwLock` (read-heavy pattern)
- Multiple invocations of the same plugin run concurrently (each with its own Store)
- No global serialization point in the invocation path

---

## 11. Module / File Layout

### New Files

```
crates/wasm-runtime/src/lib.rs          -- Re-exports, mock impls
crates/wasm-runtime/src/config.rs       -- WasmPluginConfig, WasmRuntimeConfig
crates/wasm-runtime/src/error.rs        -- WasmError enum
crates/wasm-runtime/src/runtime.rs      -- WasmPluginRuntime trait, WasmInvocationResult
crates/wasm-runtime/src/registry.rs     -- WasmPluginRegistry (production impl)
crates/wasm-runtime/Cargo.toml          -- Depends on wasmtime, acteon-core
```

### Modified Files

```
crates/rules/rules/src/ir/rule.rs       -- RuleSource::WasmPlugin, wasm_plugin, wasm_function fields
crates/rules/rules/src/engine/executor.rs -- WASM plugin evaluation path
crates/rules/yaml/src/parser.rs         -- Parse wasm_plugin, wasm_function
crates/rules/yaml/src/frontend.rs       -- Build Rule with WasmPlugin source
crates/rules/cel/src/frontend.rs        -- wasm() function support
crates/gateway/src/gateway.rs           -- Hold Arc<dyn WasmPluginRuntime>
crates/gateway/src/builder.rs           -- wasm_runtime() builder method
crates/gateway/src/metrics.rs           -- wasm_invocations, wasm_errors counters
crates/server/src/config.rs             -- [wasm] section parsing
crates/server/src/api/mod.rs            -- Register WASM routes
crates/server/src/api/wasm.rs           -- WASM plugin handlers (new file)
crates/server/src/api/openapi.rs        -- Register WASM schemas
Cargo.toml                              -- Add acteon-wasm-runtime to workspace
```

---

## 12. Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| WASM runtime | Wasmtime | Most mature, battle-tested, best security audit coverage |
| Plugin model | Per-invocation instances | Complete isolation, no shared mutable state |
| Module caching | Pre-compiled at registration | Amortizes compilation cost, sub-millisecond invocation |
| Memory management | StoreLimiter + hard cap | Prevents OOM, configurable per-plugin |
| CPU management | Epoch interruption + tokio timeout | Double-layer protection against runaway code |
| Trait abstraction | `WasmPluginRuntime` trait | Enables mock/test implementations without Wasmtime |
| Plugin storage | In-memory registry | Fast lookup, plugins re-loaded from disk/API on restart |
| WASI version | Preview 1 (minimal) | Only clock + random, no FS/net access |
| Max plugins | 256 | Prevents registry bloat, sufficient for all known use cases |
| API model | Registration-based (not hot deploy) | Explicit, auditable, no filesystem watching needed |

---

## 13. Future Directions

- **WASI preview-2 (Component Model)**: When the component model stabilizes,
  support typed interfaces instead of raw JSON for better performance and
  type safety
- **Plugin versioning**: Track multiple versions of a plugin, with rollback
  capability and canary deployment
- **Hot reloading**: Watch the plugin directory for changes and auto-reload
  updated modules without server restart
- **Host function imports**: Allow plugins to call host-provided functions
  (e.g., state lookup, embedding similarity) for richer logic
- **Instance pooling**: Pool pre-instantiated instances for plugins that are
  invoked very frequently, reducing per-call overhead
- **Plugin marketplace**: A community repository of pre-built plugins for
  common use cases (spam detection, rate limiting, content classification)
