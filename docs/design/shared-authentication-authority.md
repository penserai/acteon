# Shared authentication authority

**Status:** server integration on the working branch; focused memory, independent
Redis-client HTTP, and actual server startup/watcher contracts pass. Required local
checks pass with 3,432 workspace tests; PR review, merge and publication remain
pending.

## Outcome and boundary

Optional `[auth.authority]` in `acteon.toml` gives one logical auth file a shared
source identity. A positive top-level `authority_revision` in `auth.toml` binds
its decrypted security configuration. Server startup and the real file watcher
publish the version before exposing new tables. Login, JWT validation and API-key
lookup reject tables that do not match the shared current source, and reject
currently disabled stable principals. Middleware retains a host-created,
non-deserializable `AuthenticatedConfiguration` alongside `CallerIdentity`.

This is request-authentication integration. It does not project credential grants
into execution permits or establish effect-start authorization. Existing queued
work and already authenticated requests can remain in flight. The private
binding is an observation of a specific configuration/stamp, not a bearer grant
or permission to execute indefinitely. Public payloads cannot supply it.

## Dedicated control scope

Use the existing whole-configuration coordinator with an **empty credential
projection** for the auth-source epoch. Its opaque fingerprint binds the complete
file; the source head advances atomically. No credential execution authorities,
work permits, root ledgers or effect starts are created in this control scope.
Validation refuses mixed execution state and unrelated auth sources. Principal
disablement is a restrictive overlay inspected from the same snapshot as source
freshness.

The trusted server supplies a fixed system publisher and bounded issuance ceiling
for this source. That is a configuration-file trust boundary, not an HTTP
management permission or an authenticated user's borrowed privileges. Backend
write access and access to deployment files/secrets remain privileged.

The binary currently requires the qualified Redis backend. `bootstrap = true`
is an explicit reviewed initialization action; normal startup uses `connect`
and refuses missing state. Initialization does not repair old jobs, settle
uncertain effects or establish backend failover/archival guarantees. Control
history is bounded; publication at capacity fails without acknowledging a reload.

## Security fingerprint and table installation

HMAC-SHA256 with separate `ACTEON_AUTH_AUTHORITY_KEY` binds source/scope identity,
JWT signing settings, usernames/key names, stable principal bindings, normalized
roles, password/key hashes and every grant dimension including bus identity.
Users, keys, grants and dimension sets are canonically ordered. Reordered or
duplicated equivalent grant entries do not spuriously change the fingerprint.
Raw secrets, password/key hashes and roles/grants are not persisted in the control
coordinator. The fingerprint key must be consistent across replicas; rotation
requires a coordinated new version. It is distinct from the auth decryption key.

Startup validates lookup tables before publication. Reload prepares new tables,
then holds the write lock across publication and installation. Concurrent local
reloads cannot install out of order. Publication failure retains previous local
tables; if the newer version committed but its acknowledgment was lost, those
previous tables fail source-freshness checks until the exact version is retried.
A later competing revision can supersede an acknowledged version; authentication
always rechecks freshness rather than treating acknowledgment as a permanent
claim to be the latest replica.

JWT signing secret/expiry cannot change during a governed reload, since the
existing manager would still use its startup settings. Restart with the new
version instead. Existing sessions refresh current roles/grants as before;
password rotation alone is not a promise to invalidate every JWT. Removed users
are denied; principal changes still require a new login. Terminal credential
re-enrollment and explicit credential authority IDs remain execution-profile work.

## Verification and surfaces

`crates/server/tests/auth_authority.rs` verifies:

- actual HTTP middleware with independent providers and memory/Redis clients;
- stale replica request refusal, key rotation, role/grant narrowing and principal
  disablement;
- stale startup and reload refusal, same-version fingerprint/key conflicts,
  canonical ordering and no sensitive inputs in persisted state;
- missing/recreated incarnation refusal and required stable principal/version;
- publication response loss before/after commit and safe identical retry;
- JWT refresh from current tables and stale-replica login refusal;
- actual binary startup, one-time bootstrap, production filesystem watcher,
  outdated process restart failure and recovery using the current file.

The Redis tests use unique prefixes and clean only their fixture coordinator.
CI explicitly runs both ignored contracts; default ignored counts are not evidence.
The public authentication/configuration docs describe the guard. Sanitized server
configuration and the UI expose source/scope and startup mode without secrets.
All five SDKs use their existing auth headers and generic configuration operation;
there is no new caller-supplied authority argument or endpoint.

## Next integration gates

1. Assign stable credential authority IDs with explicit re-enrollment and reject
   ambiguous identity mappings for the execution profile.
2. Resolve each credential's own grants against complete qualified actual effects,
   including selected provider/endpoint versions, fallbacks and auxiliary effects.
3. Publish execution-scope credential snapshots from the same verified security
   inputs. Define partial multi-scope publication/reconciliation explicitly; a
   source observation cannot approximate a transaction spanning coordinators.
4. Retain exact scope configuration and credential references with authentication.
   Management mutations must validate the same scope snapshot/stamp they mutate;
   root capture must never refresh an old identity into newer/broader authority.
5. Capture credentialed permitted roots and wire the real server/gateway effect
   adapters. Every retry/deferred continuation checks current authority. Refuse
   unsupported paths in enabled scopes; cover real replicas and recovery.
6. Deliver provisioning/inspection APIs, all SDKs/UI, migration and public scope
   controls, then implement team memberships and personal/team mandate lineage.

These gates remain part of the original platform objective. This auth guard does
not complete the governed execution or workforce phases.
