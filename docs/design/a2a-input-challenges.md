# Governed A2A structured-input challenges

**Status:** local Task resolution, the source-side durable continuation journal,
the authenticated HTTP handoff, hosted target runtime, public routes, five SDK
surfaces, and independent Redis/PostgreSQL recovery contracts are implemented.

An agent may pause a Task because it needs typed user data. That pause must not
turn a late message into authority to resume some newer challenge, duplicate the
response after a lost acknowledgement, or copy sensitive input into several
control-plane rows.

## Contract

`TaskEngine::pause_for_human(..., PauseKind::UserInput, ...)` creates one
`BusApproval` and points the Task's `pendingApprovalId` at it. The Task enters
`InputRequired`; a supplied reason becomes its agent-authored status prompt.
The two-row creation recovers a lost Task-CAS acknowledgement by reading back
the exact state and approval binding. It deletes an orphan only after a
definitive non-match; an unavailable read retains the gate for recovery.

`TaskEngine::resolve_input` accepts the exact scope, Task, approval ID, user
message and authenticated actor. Before mutation it requires:

- the response `taskId` and `contextId` to match the Task;
- role `user` and a structurally valid bounded message;
- the Task to be in `InputRequired` with that exact pending approval, except
  when repairing an already committed identical response;
- an unexpired `UserInput` approval that points back to the same Task; and
- a response reference graph within the existing cycle, depth and fan-out
  limits.

The approval first moves `Pending → Approving` and records
`TaskPauseResolution { messageId, contentDigest }`. The digest is computed from
canonical JSON. Within this local resolution primitive, the full input remains
only in Task history. A Task-row CAS then appends the response and performs
`InputRequired → Working` together. Finally,
the approval moves `Approving → Approved` and its pending index entry is removed.

This is an intentionally recoverable two-row protocol over the configured
`StateStore`, not a claim of a cross-key transaction. A crash before the Task
CAS leaves a fixed response intent; retrying the same response completes it. A
crash after the Task CAS is recognized by the unique message ID and digest, so
the retry finalizes the approval without appending twice. A different response
cannot take over an `Approving` or `Approved` challenge.

If the Task is canceled or otherwise leaves the interrupt while a response is
claimed, Acteon closes the claimed approval as `Rejected`. If the challenge
expires first, Acteon closes it as `Expired` and moves a still-matching paused
Task to `Failed`. Both paths remove the pending index entry, so reconciliation
does not retain a permanently in-flight challenge.

The REST and JSON-RPC `message/send` methods use the same primitive. A response
to `InputRequired` must echo `pendingApprovalId` in
`metadata["acteon.challengeId"]`. This Acteon safety binding prevents a delayed
response observed for challenge A from silently satisfying a later challenge B.

## Authentication boundary

`AuthRequired` is not resolved by `resolve_input`. Text, data parts, metadata and
model output are not proof of authorization, and raw credentials must not enter
Task history or the peer journal. The
[verifier-backed Task Engine operation](a2a-authorization-challenges.md) now
persists a host-owned requirement bound to the Task, challenge, recipient,
credential authority, audience, scopes, and exact verifier revision. It
rechecks current external authorization before resumption and stores only a
stable decision plus the opaque request digest. Authenticated server/SDK and
remote-peer handoff remain the next exposure layer.

## Durable peer continuation

The source-side mesh operation binds a continuation to the original durable
peer submission and accepted remote Task. It:

1. requires the latest accepted snapshot to be `InputRequired` and pins its exact
   `pendingApprovalId`;
2. rechecks the source credential, context, permits, service binding, onward
   grant, registry card and target binding;
3. persists a digest-pinned continuation intent before network delivery;
4. derives the remote endpoint, Task identity, context and challenge from the
   accepted journal, never request content;
5. classifies response loss as uncertain and does not automatically resend;
   and
6. accepts only a valid same-Task forward snapshot and new opaque progress
   cursor.

The durable identity is one continuation per `(submission, challenge)`, rather
than per response message. After any delivery claim, a different response to
that challenge conflicts and cannot cause another network call. The journal
stores the normalized response and its canonical digest through the configured
payload encryptor. The remote snapshot is accepted only when it contains that
exact normalized response, advances the same Task, and replaces the prior
cursor. Repeating an accepted response recovers the same receipt from Task
history and the continuation journal.

## Hosted target execution

The hosted individual-agent runtime persists the exact normalized response,
paused Task snapshot, provider action, child identity, permits and budget limits
before resolving the Task. Its recovery driver can therefore finish the same
continuation after a lost acknowledgement or process restart, including when
the visible Task is still `InputRequired`.

Each challenge becomes a deterministic same-principal child execution under the
accepted recipient. The child inherits the original permit ceiling and shared
sponsorship, rechecks current closures and revocations, and gets its own governed
provider operation journal. The provider payload contains both `a2a_message`
for compatibility and the complete bounded `a2a_history`, plus an explicit
task/context/challenge binding. Observation and replay follow the latest child
execution; concurrent identical calls share one provider attempt, while a
different response or requester conflicts.

The HTTP adapter authenticates the original private source, carries the retained
source context, conditionally binds the opaque cursor, and accepts only the
same remote Task with an exact new cursor. The source route accepts an unbound
user message so callers cannot choose the task, context, challenge, endpoint,
credential, permits, or cursor. The target route performs the final current
authority and challenge checks before calling this runtime.
