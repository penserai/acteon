# Durable qualified peer transport

**Status:** Rust host foundation implemented; server adapter and public host-tool
surface remain follow-up work.

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
semantic message digest, and creation time. An optional `PayloadEncryptor`
protects this record with the same host-owned encryption mechanism used by
governed provider execution.

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

## Evidence

Focused contracts cover:

- one adapter call across repeated and concurrent submission;
- conflict on changed content under the same A2A message ID;
- live grant retirement and registry suspension immediately before send;
- uncertain handling for ambiguous and malformed acceptance;
- refusal of replay for at-most-once peers;
- explicit successful recovery through a verified-idempotent adapter; and
- rejection of corrupt retained remote task mappings.

## Remaining integration

This foundation does not yet complete the autonomous mesh. The next slices are:

1. an Acteon A2A HTTP adapter using the guarded outbound client, exact configured
   source credential reference, disabled redirects, bounded response reads, and
   the parent-context headers;
2. a host tool/API that resolves the caller's opaque context and never accepts
   authority fields from model output;
3. durable remote observation, progress cursor, bounded artifact transfer,
   required-input handoff, terminal result projection, and cancellation state;
4. restart and lost-acceptance tests against two real Acteon servers; and
5. qualification on each supported durable state backend.

Until those slices land, public documentation must describe parent-context
handoff and authenticated agent services separately from a complete autonomous
outbound mesh.
