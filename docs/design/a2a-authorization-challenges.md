# Verifier-backed A2A authorization challenges

**Status:** the backend-neutral Task Engine primitive, guarded HTTP verifier,
operator configuration, authenticated hosted-agent routes, and typed helpers in
all five SDKs are implemented. Remote peer handoff remains a follow-up.

## Decision

An `AuthRequired` task cannot resume because a user or model sends text saying
that authentication succeeded. Acteon resumes it only after a host-installed
`TaskAuthorizationVerifier` checks an opaque authorization request against its
current source of truth.

The host creates a `TaskAuthorizationRequirement` before pausing the task. It
pins:

- verifier ID and reviewed revision;
- opaque authorization-request ID;
- exact recipient principal;
- credential authority and audience; and
- the complete required scope set.

The requirement is persisted on the `UserAuth` approval. The verifier receives
that immutable requirement plus the exact namespace, tenant, task ID, and
challenge ID. Request content cannot choose or replace any of those fields.
Implementations can introspect an OAuth grant, workload identity, vault lease,
or another authorization service, but the opaque request ID is never itself a
credential.

An old `UserAuth` approval without this binding remains readable for storage
compatibility. It is deliberately unfulfillable. Operators must replace or
cancel that pause through a reviewed migration; Acteon never infers a verifier
from current configuration.

## Resolution protocol

The verifier returns a stable decision ID, authenticated subject, verification
time, and validity deadline. Acteon rejects a future-dated or expired decision.
It then records a secret-free `TaskAuthorizationResolution` containing:

- the verifier ID and revision;
- SHA-256 of the opaque authorization-request ID;
- the stable decision ID and subject; and
- the evidence validity window.

The approval moves `Pending -> Approving` before the Task row changes. The task
then moves `AuthRequired -> Working` and clears `pendingApprovalId` in one CAS.
Finally, the approval moves `Approving -> Approved` and leaves the pending
index. Credential material, bearer tokens, OAuth codes, and model messages are
never accepted or copied into Task history or the approval decision.

This is a recoverable two-row protocol over the configured `StateStore`, not a
cross-key transaction. A retry after the task CAS finalizes the already claimed
decision without contacting the verifier again. A retry before that CAS must
recheck the verifier. The verifier may refresh evidence timestamps only when
the request digest, verifier, stable decision ID, and authenticated subject all
match the claimed intent. Any identity change conflicts rather than taking over
the challenge.

Verifier denial or unavailability leaves a pending challenge untouched. If a
claimed task is canceled before its CAS, Acteon rejects and clears the abandoned
decision. If the challenge expires while still pending, Acteon expires the
approval and fails the still-matching task. Once a verifier decision is durably
claimed, the challenge is no longer awaiting a decision: an identical retry
must recheck current external evidence and finish or remain recoverable.
Evidence is checked again directly before the task CAS, closing the window in
which an already expired decision could resume work.

## Trust boundary

`TaskAuthorizationVerifier` is a host trust adapter. Installing an
implementation asserts that it verifies current authorization and revocation
for the supplied credential authority. The interface does not make an
untrusted callback trustworthy. Production server wiring must therefore:

1. load verifier bindings only from operator-controlled configuration;
2. authenticate callbacks and protect transport independently;
3. map one reviewed verifier revision to one implementation/configuration;
4. fail closed when that exact revision is unavailable;
5. keep authorization secrets in the verifier or its secret store; and
6. recheck the original task requester, recipient, permits, closures, and
   service binding before invoking the Task Engine operation.

The hosted-agent routes implement those rules. The recipient opens a challenge
with only an opaque request ID at `authorization:request`; Acteon fills every
trust field from the service's binding-qualified profile. The original
authenticated requester resolves the exact challenge at
`authorization:resolve`. The guarded adapter disables redirects and proxies,
applies outbound address policy and TLS configuration, sends its bearer secret
only from the named environment variable, bounds the response, and requires an
exact task, challenge, request-digest, and recipient echo.

The verifier receives camel-case JSON with `schema`, `namespace`, `tenant`,
`taskId`, `challengeId`, and the complete `requirement`. A successful response
uses schema 1 and returns the same task and challenge, SHA-256 digests of both
the opaque request ID and canonical requirement JSON, a stable `decisionId`,
the exact recipient `subject`, and the `verifiedAt` / `validUntil` window.
Unknown response fields, an unsupported content type, a body over 16 KiB, or
any binding mismatch are invalid evidence. HTTP 400/401/403/404/409 means
denied; transport errors, 429, and server errors are temporary unavailability.

Remote peer continuation needs a separate source journal. The source may retain
the remote challenge and an opaque authorization-flow handle, but cannot forward
credentials or claim that a network timeout means authorization succeeded.
Ambiguous target acknowledgment must be reconciled by observing the exact task
and challenge, without automatically replaying a non-idempotent authorization
exchange.

## Evidence

`crates/gateway/tests/task_authorization_challenges.rs` proves:

- verifier denial makes no state transition;
- legacy unbound pauses cannot become authority;
- a verified decision resumes once and contains no Task-history input;
- a lost response recovers without a second verifier call;
- pre-commit recovery refreshes validity only for the same stable decision;
  and
- independent Redis and PostgreSQL clients converge on the same result.

CI executes both production-backend contracts explicitly. The remaining peer
gate stays open until two authenticated servers demonstrate the
same behavior across restart, denial, revocation, and lost target response.
