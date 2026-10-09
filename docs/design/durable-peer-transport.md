# Durable qualified peer transport

**Status:** Rust host journal, guarded Acteon HTTP adapter, deployment wiring,
trusted host invocation API, safe agent-facing route, five SDK helpers, durable
remote task refresh with state-backed cursors, and native cancellation are
implemented and exercised between real Acteon servers.

Acteon needs a transport boundary between governed peer selection and a real A2A
network call. Discovery is advisory. A live card, endpoint URL, or model-selected
agent does not authorize a send. The outbound boundary must preserve the exact
parent, permits, message identity, reviewed binding, credential adapter, and
remote result mapping through failures and restarts.

## Implemented contract

`acteon_executor::delegation::DurablePeerTransport` is a backend-neutral host
building block over the configured `Arc<dyn StateStore>`. A submission:

1. resolves only an operator-approved agent/skill binding;
2. re-reads the current individual registry card and lifecycle state;
3. rechecks the parent's current delegation source authority and permits;
4. derives a stable submission ID from the parent execution, binding, and A2A
   message ID;
5. persists the complete bounded send intent before an adapter call;
6. claims the journal record with compare-and-swap so concurrent callers have
   one external sender;
7. invokes one exact host-installed adapter under a bounded timeout; and
8. durably records a known rejection, a verified remote task/source mapping, or
   an uncertain result.

The journal pins the binding digest, adapter revision, submission capability,
endpoint, transport, typed parent reference, permit revisions, complete message,
semantic message digest, creation time, last valid remote task snapshot, and its
optional opaque progress cursor. An optional `PayloadEncryptor` protects this
record with the same host-owned encryption mechanism used by governed provider
execution.

The transport and registry must share the same `StateStore` instance. This
prevents a caller from checking registry state in one backend while journaling
the network operation in another. Memory supports local tests; durable operation
uses whichever qualified state backend the host configured.

## Failure semantics

`registered`, `delivering`, adapter errors, timeouts, invalid remote responses,
and lost settlement writes are externally reported as **uncertain**. None means
the remote peer rejected the message. A normal repeat observes the original
journal and does not issue another adapter call.

An adapter may declare `verified_idempotent` only when its exact reviewed peer
contract guarantees that the same parent and message identity returns the
original acceptance. Only then may a host call `replay_idempotent`. The replay
still rechecks current registry and delegation authority and uses a CAS claim.
An at-most-once adapter remains uncertain and cannot be replayed through this
API.

`observe` repeats current binding and source-authority checks, then reads the
original journal without creating an intent or invoking an adapter. Hosts can
therefore distinguish observation from submission during restart recovery.

An accepted response is retained only when the returned task is valid, belongs
to the parent namespace and tenant, and carries the exact parent source context.
Malformed success is uncertainty. Stored accepted mappings are validated on
every read; corrupt task or source data conflicts instead of becoming evidence.
A repeated message ID with changed content conflicts against the original
intent.

Acteon task acceptance and observation responses expose a strong `ETag` derived
from the authoritative task row's `StateStore` version. The guarded adapter
stores that opaque value and supplies it as `If-None-Match` on refresh. A `304`
is accepted only when it repeats the exact stored cursor. A `200` response may
replace the cursor only with a valid same-task snapshot that does not regress
identity, timestamps or lifecycle state; the same cursor with different content
is refused. Schema-1 journal rows without a cursor remain readable and upgrade
to schema 2 on their next successful mutation. A task learned through a
cancellation response clears the older observation cursor because that cursor
names the pre-cancel representation.

## Trust boundary

`PeerTransportAdapter` is installed by the host for one binding digest. Its
implementation must qualify:

- endpoint confinement and redirect behavior;
- the private credential used for the source principal;
- A2A protocol and response parsing;
- which remote responses prove non-acceptance;
- whether identical submission is actually idempotent; and
- protection of secrets in errors and logs.

The adapter receives typed host state. Models and public request bodies cannot
select an endpoint, credential, binding digest, adapter revision, parent context,
or permit set.

`ActeonPeerHttpAdapter` supplies the first concrete implementation. It holds a
redacted host credential for one binding, uses `GuardedClient` with redirects
disabled, enforces the configured outbound network policy at URL and connection
time, and sends the A2A version, opaque parent context, explicit permits, and
message in their native wire format. Success bodies and error bodies are bounded.
Only a typed task plus one valid source-context response is accepted. Selected
4xx responses are retained as known rejection; redirects, 5xx, transport errors,
malformed success, and oversized response remain uncertain.

For observation, the same adapter confines the derived task URL to the reviewed
agent-service endpoint, validates the A2A version and any returned ETag as one
bounded strong validator, and distinguishes a validated `304` from a full
updated task. Cursors, URLs and source contexts remain host-owned values; agent
or model input cannot supply them.

`ExecutionAuthorityRuntime` now derives an outbound source/target matrix from
each service's explicit `onward_agents`. Preparation computes the transitive
service footprint, seals immediate `agent.invoke` operations into the source's
direct ceiling, and seals the exact downstream grants into its accepted context.
Installation resolves the source service credential from its existing private
environment binding, constructs one adapter per exact target binding, and uses
the same configured `StateStore`, coordinator, clock and payload encryptor for
the journal.

The trusted `AgentPeerInvocation` has no deserializer. The host injects the
source agent and opaque context retained with accepted work; agent/model input
can select only an already configured target, skill and message. Submission,
observation and explicitly qualified idempotent recovery recover the current
context, verify it still belongs to the installed source service binding, use
the source's configured permits, and resolve only the installed transport.
The agent-facing route takes an accepted source task ID as an opaque lookup
handle plus current private authentication. Its body contains only the message;
the runtime proves the caller is that accepted recipient before constructing the
trusted invocation.

## Evidence

Focused contracts cover:

- one adapter call across repeated and concurrent submission;
- conflict on changed content under the same A2A message ID;
- live grant retirement and registry suspension immediately before send;
- uncertain handling for ambiguous and malformed acceptance;
- refusal of replay for at-most-once peers;
- explicit successful recovery through a verified-idempotent adapter;
- rejection of corrupt retained remote task mappings;
- current authority revalidation before remote observation;
- monotonic task projection with compare-and-swap persistence of the latest
  valid snapshot;
- cursor recovery through a reconstructed source transport, including a
  conditional unchanged result without a journal rewrite;
- real two-server Redis and PostgreSQL contracts that observe one conditional
  task read and one `304` after source restart;
- one continuation delivery across concurrent and repeated calls, with the
  exact normalized response present in the accepted forward snapshot;
- real two-server Redis and PostgreSQL continuation contracts that resolve the
  exact challenge, append one response, restart the source, and recover the
  same durable receipt;
- durable uncertain and rejected continuation outcomes without implicit
  resend;
- a distinct authorization journal that carries only an exact challenge
  selector, with durable denial and current-authority revocation;
- observation-only recovery after a committed authorization response is lost,
  without a second verifier call; and
- conflict on a competing response for the same accepted challenge plus
  current-authority refusal after registry retirement.

## Remaining integration

The local governed mesh now has durable submission, cursor-aware observation,
native cancellation, restart/lost-response tests, and Redis/PostgreSQL
qualification. The local Task Engine supplies exact, idempotent `InputRequired`
resolution with durable digest intent and an explicit refusal to treat messages
as authorization. The source transport now also journals one normalized
continuation per accepted challenge, rechecks current authority, preserves
ambiguous delivery without resending, and accepts only an exact same-Task
forward snapshot with a new cursor. The hosted runtime now persists target-side
intent before Task resolution and invokes the provider through a deterministic
same-principal child execution carrying the exact response and complete bounded
history. Its recovery driver also selects registered continuations while their
Task is still paused, with independent Redis and PostgreSQL restart contracts.
The authenticated HTTP adapter now connects both durable halves and exposes
receipt-aware input and authorization helpers in all five SDKs. Redis and
PostgreSQL contracts exercise two authenticated servers across restart,
verifier denial/revocation, and lost target response. Federation trust and
cross-domain revocation protocols remain later work.
