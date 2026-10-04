# Verified workflow context propagation

**Status:** library integration on the working branch; public server provisioning
and current permit enforcement remain open.

The [trusted root context](trusted-execution-context.md) is now referenced by
internal workflow execution and continuation-task records through a validated
`ExecutionContextReference`. The reference records opaque context/execution IDs,
scope, actual principal and root semantic digest. Its deserialization validates
shape but never establishes authority.

`GatewayBuilder::workflow_context_store` enables the workflow provenance profile.
`start_workflow_with_context` requires an independently captured verified root.
It binds the workflow ID, name, queue, scope and canonical input digest, and
matches the complete workflow/queue tuple against its accepted ceiling. Worker
queues have a distinct exact resource kind. Create-only workflow persistence
prevents repeated root use from resetting existing work or duplicating start
history.

Continuation construction preserves the reference. Enqueue, discovery repair and
poll-before-lease resolve the signed context and compare it with the authoritative
workflow, current continuation identity and slim payload. Missing, altered,
expired, unconfigured or mismatched provenance refuses delivery and retains the
record for repair. A replacement gateway uses the same trusted storage/keyring;
it never borrows the worker identity or server administrator's grants.

Observation and operator cancellation remain available after context expiry.
Already leased work may report its result; future continuations require fresh
provenance verification. This profile does not register effect starts or evaluate
current permits, principal revocation or team membership. Those remain Phase 2
checkpoints, including externally executed worker operations.

Child workflow creation is refused for context-bearing/profile-managed parents
until it can derive attenuated child authority. Dropping the context would be an
unsafe implementation shortcut. Legacy workflows retain prior behavior only on
gateways without the profile; enabling it parks actor-less continuations. Public
HTTP DTOs omit the internal reference, and no new request field lets a model or
SDK establish provenance. No public SDK/API contract is introduced in this slice.

## Workforce follow-up

This preserves the actual principal only. The [agent workforce model](agent-workforce.md)
also requires initiator, represented person/team, ownership and verified mandate
lineage. Extend the signed context format explicitly, then propagate those facts
through these same references and checkpoints. Do not infer representation from
agent ownership or store it in unverified workflow input.

## Compatibility and tests

The new optional fields are omitted on legacy serialization. Their presence does
not certify old-reader compatibility: older binaries can discard them and execute
without verification. Profile-managed work therefore requires coordinated
compatible readers and must be parked/drained before rollback. Context retention,
legacy migration and safe reader rollout remain production integration gates.

Tests cover replacement/timer continuation, original actor retention, missing
provenance, wrong scope/input/queue/target, repeated-root refusal, task-reference
corruption/stripping, workflow-input corruption, expiry, observation/cancellation,
unsupported child creation and an unconfigured replacement. Existing task and
workflow contracts continue to pass. No test claims current revocation or team
mandate enforcement from integrity verification alone.
