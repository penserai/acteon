# ADR: exact governance resource identity

Status: implemented in the resource-identity branch; execution integration pending.

## Decision

Use `acteon_core::ResourceRef` to identify exactly one resource by kind, namespace,
tenant, and opaque ID. The type validates construction and JSON decoding and
rejects unknown fields, unknown kinds, wildcard-only components, empty values,
controls, boundary whitespace, and excessive UTF-8 byte lengths. Resource equality
is exact: tenant hierarchy in legacy authentication grants does not apply here.

Use the canonical spelling
`acteon-resource:v1:<kind>:<hex(namespace)>:<hex(tenant)>:<hex(id)>` for storage,
audit correlation, and selector keys. Hex is lowercase UTF-8 and each component is
encoded independently. A colon or Unicode character in an ID cannot shift scope
boundaries. Parsing rejects unsupported versions and alternate spellings; no
normalization changes a resource's identity. Hex is encoding, not encryption.

Kinds cover agents, providers, actions, topics, worker queues, subscriptions, chains, workflows,
external services, models, skills, endpoint bindings, and routes. A reference is
neither a URL nor a permission. It must be resolved against trusted registry or
configuration state. In particular, a fallback check uses the actual selected
provider reference; it must not inherit the action's original provider string.

Labels, sets, hierarchical scopes, and wildcard selectors are distinct future
selector types. A skill, endpoint, or route ID identifies an operator-maintained
binding; embedding an arbitrary URL or source/destination tuple in the ID does
not establish that binding. Operations touching multiple resources must resolve
and authorize every required reference.

## Coordinator integration

The current [format-3 accounting contract](atomic-effect-reservations.md) stores
complete resource sets and root reservations in one CAS. Format 2 below describes
the earlier identity slice; format 3 now refuses both earlier formats.

Start records and resource restrictions store typed references. Registering a
start or changing a restriction for another namespace/tenant fails before CAS.
Restart readers validate retained resource scope. Two resources with the same
name but different kinds remain independently restrictable.

Coordinator format 2 replaces format 1's untyped strings. A reader rejects format
1 rather than infer kinds, reopen resources, or recreate the domain. Although the
substrate is not yet integrated into gateway effects, any standalone adopter needs
an explicit reviewed migration with admission stopped and uncertainty preserved.
Rollback readers likewise reject unknown formats.

The coordinator remains a trusted library boundary. It does not authenticate a
principal or resolve an endpoint, and its internal restriction toggles do not yet
implement overlapping public closure records.

## Alternatives

Unescaped delimiter-separated names can collide across scope boundaries. Plain
URLs identify network locations rather than governed logical resources and can
change after admission. Hash-only IDs lose operator-readable reconstruction.
JSON object equality is suitable in memory, but a defined canonical string avoids
serializer ordering/spacing choices becoming storage identities. Lowercase hex
uses existing dependencies and a simple strict parser at the cost of doubled byte
size; sizes remain bounded.

## Evidence and remaining gate

Tests cover JSON/canonical round trips, Unicode and delimiters, tenant and kind
separation, alternate spellings, unknown versions/kinds/fields, byte bounds,
foreign-scope start/closure refusal, maximum-size IDs, and legacy-format refusal.
Existing race, recovery, and real-Redis contracts continue to run with typed
references. OpenAPI compilation checks the schema derivation.

This decision closes the identity-encoding prerequisite. It does not complete
resource resolution, checked effect coverage, permits, or trusted execution
context propagation. Those retain their own acceptance gates in the phased plan.
