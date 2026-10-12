# Durable A2A push outbox

Status: release candidate, October 11, 2026. Phases 1–5, reviewed migration,
portable backend conformance, and the callback failure matrix are implemented.
The release gate is the required CI run over those contracts.

## Problem

Acteon's A2A callback projection and configuration boundary are governed, but
delivery still starts from a process-local broadcast. A receiver can miss an
update when the server restarts, when a broadcast subscriber lags, or when an
inline configuration is persisted after the Task becomes runnable. Detached
delivery tasks can also reorder two updates for the same callback.

Retries and a dead-letter queue do not close that gap. They begin only after the
worker has observed an event. The durable source of truth must contain both the
Task mutation and the delivery intent before either becomes visible.

## Decision

Persist a private, versioned Task record envelope in the configured
`StateStore`. The public `Task` schema remains unchanged.

```text
DurableTaskRecord v1
├── task: Task
└── delivery
    ├── next_sequence
    ├── events[]
    │   ├── sequence
    │   ├── event_id
    │   ├── occurred_at
    │   └── StreamResponse
    └── callbacks[]
        ├── public A2A configuration
        ├── encrypted credential/reference
        ├── acknowledged_sequence
        ├── lease
        ├── next_attempt_at
        └── failure state
```

Every Task CAS serializes the updated Task and its new callback event in the same
value. Inline callback registration enters the envelope before the admission
method makes the Task runnable. Explicit callback CRUD uses the same Task CAS,
so configuration identity, task authority, delivery cursor, and secret custody
cannot drift across keys.

This is a reusable Task mutation-log and delivery-lease building block. A2A
callbacks are its first adapter. SSE replay, durable task watchers, and other
Task-derived integrations can consume the same ordered log later without
changing state backends.

## Invariants

1. A committed Task transition and its event have one CAS outcome.
2. Sequence numbers increase by exactly one inside one Task record.
3. One callback has at most one live lease and delivers only its next
   unacknowledged sequence.
4. HTTP success followed by process loss may repeat a callback; it cannot skip
   it. Receivers use the stable delivery ID and sequence headers for
   deduplication.
5. A callback cursor advances only after a successful response.
6. A retry or lease takeover never advances past an earlier sequence.
7. The log prunes only through the minimum acknowledged sequence of all live
   callbacks. Capacity exhaustion blocks the Task mutation instead of silently
   dropping an event.
8. Removing a callback removes its cursor from the pruning floor. Adding a
   normal callback starts at the current tail; inline admission starts before
   the first runnable event.
9. One Task holds at most 64 callback configurations, bounding the CAS value
   across state backends.
10. Public Task projections never expose the envelope, leases, retry state,
   credentials, or governance metadata.
11. Configuration mutation produces sanitized audit evidence. Tokens and
    credentials never enter audit payloads.

## Recovery and scheduling

The Task envelope is authoritative. A best-effort `A2aPushReady` index may wake
workers efficiently, but correctness cannot depend on it. Each worker also
walks bounded `A2aTask` pages and repairs missing ready-index entries. This keeps
the design correct on Memory, Redis, PostgreSQL, DynamoDB, and custom stores
without requiring a new cross-key transaction primitive.

The ready index stores only scope, Task identity, and the earliest time that one
callback may be claimed. Workers validate the stored identity against its
canonical state key, reload the Task under CAS before claiming, and ignore a
hint that is early, malformed, or stale. Updating the Task and its hint is not a
cross-key transaction: a crash or racing writer can leave a missing or stale
hint. The bounded Task walk rebuilds the hint from the authoritative envelope,
so this changes wake-up latency rather than delivery correctness. The portable
index is key ordered; a backend-native due-time index remains an optional scale
optimization rather than part of the correctness contract.

A worker claims the next callback event by CAS, recording its worker ID, lease
expiry, sequence, and attempt number. A successful POST advances the
acknowledged cursor and clears the lease by CAS. A transient failure stores the
next attempt time. A terminal or exhausted failure moves that sequence into the
envelope's bounded dead-letter state and advances only according to an explicit
operator policy; the default blocks later sequences so ordering remains true.

The existing tenant DLQ endpoint becomes a projection over envelope dead
letters during migration. Purge, replay, and discard remain explicit management
operations.

Mutable standalone callback configurations remain present during the
mixed-version window. Each configuration first imported into an envelope keeps
a private content digest as migration provenance. Native envelope CRUD removes
that provenance for the affected ID and retains a bounded native-override
marker; a delete keeps the marker as a tombstone. After pre-envelope writers
have stopped, the cutover reconciler can therefore apply old-writer creates and
updates, remove old-writer deletions, preserve native edits and deletes, and
clear all provenance in one Task CAS. Deleting standalone rows before that CAS
would make successful cleanup indistinguishable from an old-writer delete.

The backend-wide `a2a-push-cutover` command inventories every Task, standalone
configuration and standalone dead letter through `StateStore`. Its public
preview contains canonical row identities and SHA-256 content evidence, never
callback tokens, credentials, payloads or error bodies. Apply requires that
exact review digest, rejects inventory drift, rechecks row versions before each
Task CAS, and uses compare-and-delete for cleanup. It writes the durable global
`Migration` marker only after a rescan proves the legacy key spaces empty.
Server startup enforces that marker: a standalone row written after cutover is
a visible startup failure.

## Secret custody

The A2A wire accepts callback credentials. Durable records should store either:

- an envelope-encrypted value whose key revision is bound to the configuration;
  or
- a qualified secret-provider reference resolved only by the delivery worker.

Get/list returns plaintext only through the existing management-authorized
boundary when the chosen custody mode permits recovery. A non-exportable secret
reference returns sanitized provider metadata. Debug output, audit evidence,
metrics, DLQ entries, and ordinary Task projections stay redacted.

## Phases

### Phase 1: private Task envelope

- [x] Add `DurableTaskRecord` with strict schema validation and backward-compatible
  reads of legacy raw `Task` rows.
- [x] Centralize Task serialization/deserialization in `TaskEngine`; remove direct
  raw `Task` decoding from mutation paths.
- [x] Prove existing Task behavior remains stable with the full gateway suite;
  configured Redis and PostgreSQL conformance remains part of the phase 5 gate.

### Phase 2: atomic mutation log

- [x] Append schema-checked status and artifact `StreamResponse` entries in the
  same Task CAS and share their A2A 1.0 projection with live delivery.
- [x] Add bounded capacity, acknowledgement-floor pruning, and fail-closed
  mutation backpressure.
- [x] Preserve stable event IDs and sequences across CAS retries.
- [x] Add fault tests for conflict, engine replacement, pruning, and capacity
  exhaustion.

### Phase 3: callback state in the envelope

- [x] Move exact-agent and tenant callback CRUD behind Task CAS.
- [x] Migrate legacy `A2aTaskPushConfig` rows idempotently on first access.
- [x] Pass validated inline configuration into admission before the first runnable
  transition.
- [x] Emit sanitized mutation audit receipts.

### Phase 4: leased ordered dispatcher

- [x] Persist CAS-fenced claims, retry deadlines, acknowledgements, blocked
  failures, and bounded dead letters in the Task envelope.
- [x] Add backend-neutral cross-scope Task-page recovery pagination.
- [x] Replace broadcast fan-out with bounded Task-page recovery.
- [x] Keep the broadcast only as a low-latency wake-up signal.
- [x] Add the repairable ready-work index and retain bounded Task-page scans as
  its correctness repair path.
- [x] Prove two replicas cannot concurrently deliver different sequences for one
  callback.
- [x] Project Task-embedded dead letters through the operator surface and make
  retry/discard atomic with callback unblocking.

### Phase 5: migration and release gate

- [x] Drain append-only standalone DLQ rows into Task envelopes idempotently on
  operator access. Conflicting duplicate evidence fails closed, and standalone
  rows are deleted only after the Task CAS succeeds.
- [x] Drain existing standalone configuration and DLQ rows through an explicit,
  reviewed backend-wide cutover. Bind apply to the exact redacted inventory,
  reconcile each Task under CAS, delete source rows by version, and persist a
  global completion marker only after an empty rescan.
- [x] Require a coordinated binary cutover: stop pre-envelope writers before
  apply. New binaries enforce the durable marker at startup and at compatibility
  reads, so a legacy writer that returns after cutover fails visibly.
- [x] Run one cutover contract against Memory, Redis, PostgreSQL, and DynamoDB.
  CI provisions the three external stores; the same contract verifies native
  edits and deletes, legacy updates, DLQ import, redacted review, cleanup, the
  completion marker, and the post-marker fence.
- [x] Cover the official A2A callback failure matrix. The official SDK exercises
  push configuration and inline replay over the public protocol, while focused
  durability tests exercise the persistence boundaries that an HTTP client
  cannot deterministically interrupt.

| Scenario | Release evidence |
| --- | --- |
| Restart | `callback_retry_and_dead_letter_survive_engine_replacement` and bounded Task/ready-index recovery |
| Lost acknowledgement | `committed_acknowledgement_survives_lost_response_and_worker_replacement` injects loss after the ACK CAS commits and proves the replacement does not redeliver |
| Broadcast lag | `recovery_page_repairs_a_missed_broadcast` and `ready_index_recovers_work_without_a_broadcast` |
| Callback deletion | `callback_delete_releases_its_delivery_retention_floor` |
| Inline replay | official SDK conformance plus `official_governed_inline_push_is_deterministic_on_exact_send_replay` |
| Credential rotation | `credential_rotation_applies_only_to_subsequent_callback_events` captures real HTTP token, authorization, delivery ID, and sequence headers |

- [x] Advertise ordered at-least-once push with stable delivery identity after
  the portable contracts and failure matrix became required CI evidence.

## Rejected alternatives

Writing an outbox row after the Task CAS preserves a crash window. Writing it
before the Task CAS can publish a transition that never committed. Treating the
live broadcast as an outbox still loses events before persistence. Adding a
multi-key transaction to `StateStore` would impose a new distributed transaction
contract on every backend and custom implementation; the single-record envelope
uses the CAS guarantee they already provide.
