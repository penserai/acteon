# Payload Templates Architecture

## Overview

Payload templates provide a two-layer system for rendering dynamic content into
action payloads at dispatch time. **Templates** hold raw `MiniJinja` text;
**profiles** wire template content to target payload fields. The gateway
renders profiles after enrichment and before rule evaluation, using a
sandboxed, fuel-limited `MiniJinja` engine with no filesystem or network
access.

This document describes the design decisions, component interactions, data
flow, storage model, and security characteristics.

---

## 1. Data Model

### `Template` (stored template content)

Defined in `crates/core/src/template.rs`:

```rust
pub struct Template {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Template name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this template belongs to.
    pub namespace: String,
    /// Tenant this template belongs to.
    pub tenant: String,
    /// Raw MiniJinja template content.
    pub content: String,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// When this template was created.
    pub created_at: DateTime<Utc>,
    /// When this template was last updated.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary key-value labels for filtering and organization.
    pub labels: HashMap<String, String>,
}
```

### `TemplateProfile` (field-to-template wiring)

```rust
pub struct TemplateProfile {
    /// Unique identifier (UUID-v4, assigned on creation).
    pub id: String,
    /// Profile name (unique within namespace + tenant scope).
    pub name: String,
    /// Namespace this profile belongs to.
    pub namespace: String,
    /// Tenant this profile belongs to.
    pub tenant: String,
    /// Field-to-template mappings. Keys are target payload field names.
    pub fields: HashMap<String, TemplateProfileField>,
    /// Optional human-readable description.
    pub description: Option<String>,
    /// When this profile was created.
    pub created_at: DateTime<Utc>,
    /// When this profile was last updated.
    pub updated_at: DateTime<Utc>,
    /// Arbitrary key-value labels.
    pub labels: HashMap<String, String>,
}
```

### `TemplateProfileField` (inline vs reference)

```rust
#[serde(untagged)]
pub enum TemplateProfileField {
    /// Inline Jinja template literal (a plain string).
    Inline(String),
    /// Reference to a stored template by name.
    Ref {
        #[serde(rename = "$ref")]
        template_ref: String,
    },
}
```

The `untagged` serde representation allows the JSON API to accept either a
bare string (inline) or an object with a `$ref` key (reference) in the same
`fields` map.

### `RenderResult` (engine output)

```rust
pub struct RenderResult {
    /// Rendered field values keyed by field name.
    pub fields: HashMap<String, String>,
}
```

### `GatewayError::TemplateRender`

```rust
pub enum GatewayError {
    // ...
    #[error("template render error: {0}")]
    TemplateRender(String),
}
```

All template-related failures (missing profile, missing `$ref` target, syntax
errors, fuel exhaustion, output size exceeded) are wrapped in this variant.

---

## 2. Component Architecture

```
┌────────────────────────────────────────────────────────────────┐
│                         Gateway                                 │
│                                                                 │
│  ┌──────────────┐   ┌──────────────────┐   ┌───────────────┐  │
│  │  Dispatch     │──>│  Template Engine  │──>│  MiniJinja    │  │
│  │  Pipeline     │   │  (render_profile) │   │  Environment  │  │
│  │              │<──│                    │<──│  (sandboxed)  │  │
│  └──────────────┘   └──────────────────┘   └───────────────┘  │
│        │                     │                                  │
│        │              ┌──────────────┐    ┌─────────────────┐  │
│        │              │  In-Memory   │    │  State Store    │  │
│        │              │  Cache       │    │  (persistent)   │  │
│        │              │  (RwLock)    │    │                 │  │
│        │              └──────────────┘    └─────────────────┘  │
│        │                                                        │
│  ┌────────────┐   ┌────────────────┐   ┌────────────────────┐ │
│  │  API Layer  │   │  Validation    │   │  Enrichment        │ │
│  │  (CRUD)     │   │  (syntax,size) │   │  (runs before)     │ │
│  └────────────┘   └────────────────┘   └────────────────────┘ │
└────────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

| Layer | Location | Responsibility |
|-------|----------|---------------|
| **Core types** | `acteon-core/src/template.rs` | `Template`, `TemplateProfile`, `TemplateProfileField`, validation functions |
| **Template engine** | `acteon-gateway/src/template_engine.rs` | `render_profile()`, `merge_rendered_into_payload()`, fuel/size limits |
| **Gateway integration** | `acteon-gateway/src/gateway.rs` | In-memory caches, scope resolution, pipeline orchestration |
| **API layer** | `acteon-server/src/api/templates.rs` | CRUD endpoints, syntax validation at creation, render preview |
| **State storage** | `acteon-state` | Persistent storage via `KeyKind::Template` and `KeyKind::TemplateProfile` |
| **Builder** | `acteon-gateway/src/builder.rs` | `template()` and `template_profile()` builder methods |

---

## 3. Storage Design

### State Store Keys

Templates and profiles are persisted in the state store using two `KeyKind`
variants:

```rust
pub enum KeyKind {
    // ...
    Template,
    TemplateProfile,
}
```

Each object is stored twice:

1. **Primary key**: `_system:_templates:{kind}:{uuid}` -- stores the full
   serialized JSON object
2. **Index key**: `_system:_templates:{kind}:idx:{namespace}:{tenant}:{name}`
   -- stores just the UUID, enabling name-based lookups and duplicate detection

```rust
fn template_state_key(id: &str) -> StateKey {
    StateKey::new("_system", "_templates", KeyKind::Template, id)
}

fn template_index_key(namespace: &str, tenant: &str, name: &str) -> StateKey {
    let suffix = format!("idx:{namespace}:{tenant}:{name}");
    StateKey::new("_system", "_templates", KeyKind::Template, &suffix)
}
```

The `_system:_templates` namespace/tenant pair is a reserved internal scope
that does not conflict with user namespaces.

### In-Memory Cache

The gateway maintains two `parking_lot::RwLock<HashMap>` caches:

```rust
pub(crate) templates:
    parking_lot::RwLock<HashMap<String, acteon_core::Template>>,
pub(crate) template_profiles:
    parking_lot::RwLock<HashMap<String, acteon_core::TemplateProfile>>,
```

Both maps are keyed by `"namespace:tenant:name"`. The API layer updates the
cache on every create/update/delete operation via `set_template()`,
`set_template_profile()`, `remove_template()`, and
`remove_template_profile()`.

### Scope Resolution

When rendering a profile, the engine needs access to all templates in the
same namespace + tenant scope (because `$ref` fields reference templates by
name, not by ID). The gateway provides a helper:

```rust
fn templates_for_scope(
    &self,
    namespace: &str,
    tenant: &str,
) -> HashMap<String, acteon_core::Template> {
    let prefix = format!("{namespace}:{tenant}:");
    self.templates
        .read()
        .iter()
        .filter(|(k, _)| k.starts_with(&prefix))
        .map(|(_, v)| (v.name.clone(), v.clone()))
        .collect()
}
```

This returns a name-keyed map (not a `namespace:tenant:name`-keyed map) so the
template engine can resolve `$ref` values directly by name.

---

## 4. Template Engine Design

### Rendering Flow

The core rendering function in `crates/gateway/src/template_engine.rs`:

```rust
pub fn render_profile(
    profile: &TemplateProfile,
    templates_map: &HashMap<String, Template>,
    payload: &serde_json::Value,
) -> Result<RenderResult, GatewayError>
```

For each field in the profile:

1. **`Inline(literal)`**: Renders the string as an ad-hoc `MiniJinja` template
   using `env.render_str(literal, &ctx)`
2. **`Ref { template_ref }`**: Looks up the named template in `templates_map`,
   registers it in the `MiniJinja` environment, and renders it using
   `tmpl.render(&ctx)`

The payload is converted to a `MiniJinja` context via
`minijinja::Value::from_serialize(payload)`, making all top-level and nested
JSON fields available as template variables.

### MiniJinja Environment

A fresh `minijinja::Environment` is created per `render_profile()` call. This
ensures:

- No state leaks between render operations
- No cross-profile template registration contamination
- Fuel counter resets to the limit for each render

The environment is configured with:

```rust
let mut env = minijinja::Environment::new();
env.set_fuel(Some(FUEL_LIMIT)); // 100,000 steps
```

### Fuel-Based DoS Protection

`MiniJinja`'s fuel system counts execution steps (template tag evaluations,
filter applications, loop iterations). When fuel is exhausted, the engine
returns an error immediately. This prevents:

- Infinite or near-infinite loops (`{% for i in range(999999999) %}`)
- Deeply nested template logic that would consume excessive CPU
- Algorithmic complexity attacks via crafted template content

The fuel limit of 100,000 is generous for legitimate templates (a template
with 100 loop iterations and 10 filters per iteration uses roughly 1,000
fuel) while still protecting against abuse.

### Output Size Validation

After rendering each field, the engine checks the output size:

```rust
const MAX_RENDERED_BYTES: usize = 1_024 * 1_024; // 1 MB

if rendered.len() > MAX_RENDERED_BYTES {
    return Err(GatewayError::TemplateRender(format!(
        "rendered output for field '{}' exceeds maximum size of {} bytes",
        field_name, MAX_RENDERED_BYTES
    )));
}
```

This prevents a template with a large loop from producing a multi-gigabyte
string that would exhaust memory.

### Payload Merge Behavior

Rendered fields are merged into the action payload by
`merge_rendered_into_payload()`:

```rust
pub fn merge_rendered_into_payload(
    payload: &mut serde_json::Value,
    rendered: &RenderResult,
) -> Result<(), GatewayError>
```

Key behaviors:

- **Overwrites existing fields**: If the payload already has a field with the
  same name as a rendered field, the rendered value replaces it
- **Adds new fields**: Rendered fields that don't exist in the payload are
  added
- **String values only**: All rendered output is inserted as JSON string values
- **Object requirement**: The payload must be a JSON object; non-object
  payloads cause a render error

The overwrite behavior is intentional -- it allows templates to transform raw
data fields into formatted versions. For example, a `message` field containing
raw text can be overwritten with an HTML-formatted `message` field.

---

## 5. Gateway Integration

### Pipeline Position

Template rendering occupies step 2d in the dispatch pipeline:

```
dispatch_inner()
├── 1.  Lock acquisition
├── 2a. Quota check
├── 2b. Deduplication check
├── 2c. Enrichment (apply_enrichments)
├── 2d. Template rendering           <── HERE
├── 3.  Rule evaluation
├── 4.  Provider dispatch
├── 5.  Audit trail
└── 6.  Lock release
```

This position was chosen because:

- **After enrichment**: Enrichment may add fields (e.g., GeoIP data, user
  metadata) that templates need to reference
- **Before rules**: Rules should evaluate against the final payload content,
  including rendered template output
- **Before provider dispatch**: Providers receive the fully rendered payload

### Dispatch Code Path

```rust
// 2d. Template rendering.
if let Some(ref profile_name) = action.template {
    let profile_key = format!(
        "{}:{}:{profile_name}",
        action.namespace, action.tenant
    );
    if let Some(profile) = self.template_profiles.read().get(&profile_key).cloned() {
        let scoped_templates = self.templates_for_scope(
            action.namespace.as_ref(),
            action.tenant.as_ref(),
        );
        let rendered = crate::template_engine::render_profile(
            &profile,
            &scoped_templates,
            &action.payload,
        )?;
        crate::template_engine::merge_rendered_into_payload(
            &mut action.payload,
            &rendered,
        )?;
    } else {
        return Err(GatewayError::TemplateRender(format!(
            "template profile not found: {profile_name}"
        )));
    }
}
```

Key design decisions in this path:

1. **Missing profile is a hard error**: If an action references a profile that
   does not exist, the dispatch fails immediately. This is intentional --
   a misconfigured `template` field indicates a bug, and silently ignoring it
   would produce unexpected payloads.

2. **Profile is cloned under read lock**: The profile is cloned from the
   `RwLock` before rendering to minimize lock hold time. Rendering can take
   non-trivial time for complex templates.

3. **Scoped template lookup**: Only templates in the same namespace + tenant
   scope are available during rendering. This prevents cross-tenant data
   leakage.

### Action Integration

The `Action` struct in `crates/core/src/action.rs` carries the template
reference:

```rust
pub struct Action {
    // ... existing fields ...
    /// Optional template profile name.
    #[serde(default)]
    pub template: Option<String>,
}
```

The builder pattern provides `with_template()`:

```rust
pub fn with_template(mut self, template: impl Into<String>) -> Self {
    self.template = Some(template.into());
    self
}
```

### Builder Integration

The gateway builder accepts templates and profiles at construction time:

```rust
pub fn template(mut self, template: acteon_core::Template) -> Self {
    let key = format!("{}:{}:{}", template.namespace, template.tenant, template.name);
    self.templates.insert(key, template);
    self
}

pub fn template_profile(mut self, profile: acteon_core::TemplateProfile) -> Self {
    let key = format!("{}:{}:{}", profile.namespace, profile.tenant, profile.name);
    self.template_profiles.insert(key, profile);
    self
}
```

---

## 6. API Design

### Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/templates` | Create a template |
| `GET` | `/v1/templates` | List templates |
| `GET` | `/v1/templates/{id}` | Get template details |
| `PUT` | `/v1/templates/{id}` | Update a template |
| `DELETE` | `/v1/templates/{id}` | Delete a template |
| `POST` | `/v1/templates/profiles` | Create a profile |
| `GET` | `/v1/templates/profiles` | List profiles |
| `GET` | `/v1/templates/profiles/{id}` | Get profile details |
| `PUT` | `/v1/templates/profiles/{id}` | Update a profile |
| `DELETE` | `/v1/templates/profiles/{id}` | Delete a profile |
| `POST` | `/v1/templates/render` | Preview rendering |

### Validation at Creation Time

Template syntax is validated when templates are created or updated:

```rust
fn validate_template_syntax(content: &str) -> Result<(), String> {
    let env = minijinja::Environment::new();
    env.render_str(content, ())
        .map(|_| ())
        .or_else(|e| {
            let msg = e.to_string();
            // Missing variables are fine -- only syntax errors matter.
            if msg.contains("undefined") || msg.contains("unknown") {
                Ok(())
            } else {
                Err(format!("template syntax error: {e}"))
            }
        })
}
```

This catches `MiniJinja` syntax errors (unclosed tags, invalid filter names)
before the template is stored. Missing variable errors are explicitly ignored
because templates will be rendered with real data at dispatch time.

### Referential Integrity

The API enforces referential integrity between profiles and templates:

- **Profile creation/update**: Every `$ref` target must exist as a stored
  template in the same namespace + tenant scope. The check runs against the
  in-memory cache for speed.
- **Template deletion**: Returns 409 Conflict if any profile in the same scope
  references the template. The check scans all profiles in the scope.

### Render Preview

The `POST /v1/templates/render` endpoint provides a side-effect-free way to
test template rendering. It:

1. Looks up the profile by name in the in-memory cache
2. Gathers all scoped templates
3. Calls `render_profile()` with the provided test payload
4. Returns the rendered fields without dispatching or auditing

This is useful for CI/CD pipelines that validate template changes before
deployment.

---

## 7. Error Handling

### Error Categories

| Error | Stage | Behavior |
|-------|-------|----------|
| Invalid name (empty, too long, bad chars) | API validation | 400 Bad Request |
| Content exceeds 512 KB | API validation | 400 Bad Request |
| `MiniJinja` syntax error | API validation | 400 Bad Request |
| Duplicate name in scope | API creation | 409 Conflict |
| `$ref` target not found | API creation/update | 400 Bad Request |
| Template in use by profiles | API deletion | 409 Conflict |
| Profile not found at dispatch | Dispatch pipeline | `GatewayError::TemplateRender` (hard error) |
| `$ref` target not found at render | Dispatch pipeline | `GatewayError::TemplateRender` (hard error) |
| Fuel exhausted | Render engine | `GatewayError::TemplateRender` |
| Output size exceeded | Render engine | `GatewayError::TemplateRender` |
| Non-object payload | Merge step | `GatewayError::TemplateRender` |

### Design Choice: Hard Errors vs Soft Errors

Template rendering failures are **hard errors** that abort the dispatch. The
alternative -- silently skipping template rendering -- was rejected because:

1. The provider would receive an incomplete payload (missing expected fields)
2. The failure would be invisible without careful monitoring
3. The `template` field on the action is an explicit request by the caller

---

## 8. Security Model

### Sandboxing

The `MiniJinja` engine runs in a restricted environment:

- **No filesystem access**: `MiniJinja` does not provide filesystem functions
  by default, and no custom functions are registered
- **No network access**: No HTTP or socket functions are available
- **No system calls**: The engine runs entirely in user space
- **No cross-scope access**: Templates are resolved only within the action's
  namespace + tenant scope

### Resource Limits

| Limit | Value | Purpose |
|-------|-------|---------|
| Template content | 512 KB | Prevents storing excessively large templates |
| Rendered output per field | 1 MB | Prevents memory exhaustion from loop expansion |
| Fuel per render | 100,000 steps | Prevents CPU exhaustion from complex/malicious templates |
| Name length | 128 chars | Prevents excessively long keys in storage and caches |

### Tenant Isolation

Templates and profiles are scoped to namespace + tenant pairs:

- API operations require namespace and tenant parameters
- The gateway resolves templates only within the action's own scope
- A profile in scope A cannot reference a template in scope B
- The `_system:_templates` storage namespace is internal and not addressable
  by user actions

---

## 9. Performance Characteristics

### Render Cost

| Operation | Typical Time |
|-----------|-------------|
| Environment creation | ~1 us |
| Template compilation (inline) | ~5-20 us |
| Template compilation (stored) | ~5-20 us |
| Variable substitution | ~1 us per variable |
| Loop iteration | ~1-2 us per iteration |
| Filter application | ~1 us per filter |
| Output merge | ~1-5 us |
| **Total (simple template)** | **~20-50 us** |
| **Total (complex template with loops)** | **~100-500 us** |

Template rendering is synchronous and runs on the dispatch task. The overhead
is negligible compared to provider network I/O.

### Memory Usage

| Component | Memory |
|-----------|--------|
| Per-template cache entry | ~500 bytes + content size |
| Per-profile cache entry | ~500 bytes + field count * ~100 bytes |
| Per-render Environment | ~4 KB (temporary, freed after render) |
| Rendered output buffer | Up to 1 MB per field (temporary) |

### Concurrency

- The `RwLock` on both caches allows concurrent reads during dispatch
- Write locks are held only during API mutations (create/update/delete)
- Each render creates an independent `MiniJinja` environment with no shared
  mutable state

---

## 10. Module / File Layout

### New Files

```
crates/core/src/template.rs                -- Template, TemplateProfile, TemplateProfileField, validation
crates/gateway/src/template_engine.rs       -- render_profile(), merge_rendered_into_payload()
crates/server/src/api/templates.rs          -- CRUD handlers + render preview
```

### Modified Files

```
crates/core/src/action.rs                   -- template: Option<String> field, with_template() builder
crates/core/src/lib.rs                      -- Re-export template types
crates/gateway/src/gateway.rs               -- In-memory caches, pipeline integration, scope helper
crates/gateway/src/builder.rs               -- template(), template_profile() builder methods
crates/gateway/src/error.rs                 -- GatewayError::TemplateRender variant
crates/gateway/src/lib.rs                   -- pub mod template_engine
crates/state/state/src/key.rs               -- KeyKind::Template, KeyKind::TemplateProfile
crates/server/src/api/mod.rs                -- Register template routes
crates/server/src/api/openapi.rs            -- Register template schemas and endpoints
crates/client/src/lib.rs                    -- Template/profile types and request structs
```

---

## 11. Design Decisions Summary

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Two-layer model | Templates + profiles | Separates content from wiring; enables reuse and independent updates |
| Template engine | `MiniJinja` | Mature Jinja2-compatible Rust library, fuel-based resource limiting, no unsafe |
| Scoping | Namespace + tenant | Matches the existing multi-tenant model, prevents cross-tenant leakage |
| Storage keys | `KeyKind::Template` / `KeyKind::TemplateProfile` | Integrates with existing state store infrastructure |
| In-memory caching | `parking_lot::RwLock<HashMap>` | Fast read path during dispatch; consistent with other gateway caches |
| Missing profile at dispatch | Hard error | Prevents silent payload corruption from misconfigured actions |
| Field collision | Overwrite | Allows templates to transform existing payload fields intentionally |
| Syntax validation | At creation time | Catches errors early; avoids dispatch-time surprises |
| Referential integrity | API-enforced | Prevents dangling `$ref` references; delete blocked if in use |
| Fuel limiting | 100,000 steps | Protects against DoS without limiting legitimate templates |
| Output size cap | 1 MB per field | Prevents memory exhaustion from loop expansion |
| Rendered type | String only | Simplifies merge semantics; JSON-typed output deferred to future |
| `serde(untagged)` for fields | Inline string vs `$ref` object | Clean API ergonomics; no type discriminator needed |
| Fresh environment per render | No shared state | Eliminates cross-render contamination and simplifies concurrency |

---

## 12. Future Directions

- **Typed output**: Allow templates to produce JSON values (numbers, booleans,
  objects) instead of only strings, enabling richer payload transformations
- **Template inheritance**: Support `{% extends %}` and `{% block %}` across
  stored templates for DRY multi-section layouts
- **Cross-scope templates**: Introduce a global template scope that any
  namespace + tenant can reference
- **Template versioning**: Track revision history with rollback capability
- **Custom filters**: Allow registering custom `MiniJinja` filter functions
  (e.g., date formatting, URL encoding)
- **Async data functions**: Let templates call host-provided functions for
  lightweight data lookups without full enrichment configuration
- **Partial rendering**: Render only a subset of profile fields (useful for
  incremental testing)
- **Admin UI**: Template editor with syntax highlighting, live preview, and
  variable autocompletion
