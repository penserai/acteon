# ADR: stable actors and credential bindings

Status: implementation branch; complete execution-context propagation pending.

## Decision

Bind each configured login or API key optionally to a validated `PrincipalIdentity`
containing a domain-local stable ID and descriptive kind. Configurations can use
several credentials for the same actor with different grants. Credential names,
authentication methods, and roles remain separate from actor identity. Kind alone
never grants privileges. No binding is inferred from a username, payload, bus
message, card, or model output.

The authentication adapter stamps the core Caller with the configured identity.
This retains the original actor through existing admission/chain serialization.
Caller remains serializable audit provenance, not a verified execution context.
Unbound legacy Callers preserve their exact old wire spelling so existing durable
receipt digests remain verifiable.

For a bound caller, durable idempotency binds semantic action and principal ID/kind
rather than transport credential name or authentication method. Credential rotation
can replay one original receipt. The original credential and principal remain in
that receipt and in its chain; a retry cannot replace provenance. A different
principal conflicts. Server transport authorization is applied before admission or
replay, including narrower replacement credentials.

Enabling a binding for existing unbound work is an explicit identity migration and
changes its semantic digest. Do not silently adopt the old request. Use a new key
or operator reconciliation. Existing credential-name audit/quota filters retain
their semantics until separately versioned principal-aware controls ship.

### Downgrade boundary

All replicas processing bound work must use a principal-aware reader. Older chain
readers can discard unknown principal metadata when rewriting a record; identity
retention is not certified across that downgrade. Stop admission and drain/park
bound chains before reverting. Bound receipt digests also differ from the legacy
caller digest, so older receipt validation refuses them instead of safely replaying
them. Unbound legacy records retain their previous wire/digest behavior. Complete
versioned execution-context rollout/rollback remains a separate phase gate.

## Session and reload rules

JWT claims record the principal at issuance. After signature, expiry, and token
revocation validation, the provider looks up the current user. Current roles/grants
apply, but a changed principal binding rejects the session rather than transferring
it to another actor. This also applies when adding/removing a binding. Fresh login
is required after migration.

Build and validate replacement auth tables before swapping them. Reject duplicate
usernames, duplicate key hashes, unknown roles, and conflicting kinds for one
principal ID. A failed reload leaves current tables intact. Never expose key hashes
or secrets through identity inspection.

The optional binding preserves existing file-auth deployments. It does not establish
a durable principal lifecycle registry, revocation generation updates, permit
issuance, or a complete deferred authority ceiling. These require coordinator-backed
provisioning before enforce mode. Anonymous development remains explicitly unbound.

## Platform surface and proof

Expose authenticated self-inspection through `/v1/auth/identity` with Session
permission, full route catalog/OpenAPI coverage, and typed methods in all five SDKs.
A shared wire fixture includes bound API-key and JWT identities and legacy null.
The operator UI shows credential, role, and principal separately.

Production-router tests exercise actual key replacement, durable replay, changed
actor conflict, refusal under narrowed grants, and caller metadata spoofing. JWT
and reload tests cover remapping and invalid configuration. A replacement-gateway
chain test verifies retained original provenance through deferred handoff. The
live agent guide verifies the configured binding against a real server.

The next gate is a trusted, versioned execution context with scoped principal
references, root/parent lineage, current ceilings, authority revisions, deadlines,
and reservations propagated through every durable path. This metadata slice cannot
substitute for that gate.
