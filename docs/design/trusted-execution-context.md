# Trusted durable execution context

**Status:** root-context substrate on the working branch; gateway/deferred integration remains open.

**Design:** [governed city](governed-agent-city.md), especially Sections 5 and 17.

## Decision

Separate untrusted request metadata, persisted provenance and a verified context.
`Caller` remains audit metadata. A `VerifiedExecutionContext` has private
construction and no Deserialize implementation. The internal
`TrustedContextStore` is explicitly configured with privileged storage,
coordination and signing-key capabilities; it does not authenticate credentials
or evaluate permits itself.

An authenticated/evaluated root admission records stable principal identity,
original credential, scoped exact effect tuples, accepted ceiling revision,
semantic digest, deadline and evaluated authority stamp. It is sealed with
HMAC-SHA256 over the exact serialized payload. The sealed envelope and payload
have independent format checks and reject unknown fields. The signing key never
appears in Debug output, wire records or errors.

Recovery resolves an opaque handle through trusted storage and checks its MAC,
administrative domain, namespace/tenant, handle, expected execution ID, actor,
digest, deadline and coordinator incarnation. The expected binding must come
from an independently trusted durable work record, not from a model's assertions.
Changing authority generation is expected during a durable execution; it
requires fresh evaluation at the next effect checkpoint. Verified provenance
alone never grants a send, reservation or permission to ignore revocation.

## Replay and rotation

Allocate and retain the handle before capture. A lost successful write response
is observed through that same handle. Replaying capture preserves original
facts; changed scope, input, accepted ceilings, deadline, credential or evaluated
stamp cannot replace them. Existing observation does not register an attempt.

Replicas share the configured keyring. A new active key signs new records while
retained old keys verify existing records. Removing an old verification key
parks affected recovery through verification failure; it must not recreate
authority under a new key. Key identifiers are unique within a bounded keyring.
Credential rotation and context signing-key rotation are distinct operations.

## Bounds and integration gates

The substrate supports roots only. It limits each sealed record to 64 KiB,
128 exact effect tuples and 16 resources per tuple; all resources match exact
scope and duplicate resources are refused. Tuple matching never combines an
operation from one entry with a destination from another.

The [workflow provenance integration](workflow-context-propagation.md) calls this
store from a library profile at workflow start and continuation enqueue/repair/poll.
Public entrypoints, standalone/chain workers, schedules and effect authorization
remain unintegrated.
Before integration, define accepted grant/permit ceiling compilation and
coordinator-backed publication of current authority revisions. Add a handle and
independently bound ownership/input to every authoritative deferred record.
Specify legacy parking, compatible reader startup gates, safe record retirement,
shared root budgets and child attenuation. Signing does not make an unsafe
authority source authoritative, or create atomic context/work-record writes.

Capture and work-record persistence are separate: a crash may leave an orphan
context, never a synthesized privileged continuation. The adapter must persist
the captured handle before queueing and reconcile partial admission. Records
have no automatic TTL; deleting them or retiring keys while work depends on them
causes fail-closed recovery. Global storage growth requires a retention contract.

## Verification

Memory tests cover independent replacement recovery, ownership/input conflicts,
exact ceiling matching, record tampering/transplant, unknown envelopes, domain
binding, replay broadening, concurrent capture, lost acknowledgment, expiry,
stale evaluation, missing/recreated authority and signing-key rotation.
An explicitly run independent-client Redis test covers durable capture after a
lost acknowledgment, recovery/replay, wrong owner and tampering. CI runs it
against its Redis service. These tests qualify this context contract, not the
unimplemented full authority lifecycle or backend failover behavior.

The [credential authority follow-up](current-credential-authority.md) introduces
signed context format 2 with a bound credential revision. Format-1 records require
explicit migration or parking; older readers/writers cannot drop the new field.
Actor-only compatibility contexts do not establish current credential authority.
