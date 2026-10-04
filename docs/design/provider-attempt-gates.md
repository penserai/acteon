# Provider attempt admission and settlement

**Status:** generic executor boundary merged in PR #413. Current root permit
evaluation merged in PR #414; gateway/server provisioning remains open.
The [durable provider adapter](durable-provider-governance.md) connects these
primitives on the working branch.

## Decision

Place `ProviderAttemptGate::start` after the executor semaphore and before each
selected provider invocation. Repeat it after every retry delay. A check only at
gateway entry cannot prevent a revoked action from starting after either wait.

`ProviderAttempt` identifies the actual provider instance by `provider.name()`,
the complete Action, retry ordinal and a fresh executor-clock sample. Fallback
can select a different provider while `Action.provider` still names the original.
The trusted adapter resolves endpoint and other protected resources from current
host configuration; the provider name alone is not a complete effect tuple.
Ordinal is not a durable identity: each registered retry needs a unique stable
attempt ID associated with the retained work record.

The gate independently verifies provenance, evaluates current authority and
registers the complete effect and root reservation atomically. It must refresh
trusted time if its own operations wait. It returns a privately managed guard
only for a new registration. Existing registration is observation and must not
be returned as permission to invoke the provider again. Failed admission invokes
no provider, consumes no executor retry and returns a stable nonretryable reason.

## Settlement and cancellation

The executor supplies provider success/error or timeout evidence to the guard
before deciding whether to retry. `Settled` requires durable, class-specific
evidence. A timeout, connection error or discarded future alone does not prove a
remote effect finished or never happened. `Uncertain` retains the reservation
and stops automatic retries. Settlement failure also stops retries, including
when the provider returned success; the host must recover retained result and
handoff evidence instead of sending again.

The guard contract requires durable registration to survive cancellation or
panic. Dropping the executor future cannot run async settlement and must not
release capacity. Registration remains unresolved until qualified reconciliation.
The host adapter must retain the result/handoff needed for recovery before
reporting durable settlement; the executor callback is not itself a result store.

Each known-safe retry invokes a fresh gate and spends a separate root unit.
Exhausted gated calls return a nonretryable failure to the host. They do not enter
the legacy DLQ, whose Action-only entries would discard authority lineage.
Governed DLQ/result retention and reconciliation remain integration work.

## Library profile and remaining integration

`ActionExecutor::execute_with_gate` is the explicit trusted-adapter entrypoint,
including optional attachment context. `require_attempt_gate()` refuses legacy
`execute`, attachment and batch calls rather than falling back to unmetered work.
Unconfigured legacy executors keep their existing behavior.

This is a library boundary, not a public enforce-mode switch. A trusted host
supplies the gate; models or SDK metadata cannot choose one. The generic boundary alone enables no server scope profile, gateway effect path
or public API/SDK/UI. The subsequent durable adapter provides a directly mediated
root-permit library path. Principal/permit/mandate lifecycle publication, signed lineage,
current permission matching and root allocation remain required. Providers with
internal retries or several external operations need their own qualified gates;
one provider-method invocation does not prove every internal socket is mediated.
Inference, enrichment, bus, worker and A2A paths remain separate inventory rows.

## Executable evidence

Executor contracts use the real scoped coordinator and root ledger with a trusted
test adapter. They execute provider methods rather than only evaluate descriptors:

- closure during a semaphore wait blocks the next invocation;
- revocation during a manually controlled retry delay blocks the retry;
- selected fallback identity and each new retry reach admission;
- timeout/connection uncertainty retains capacity and stops retries;
- successful effect followed by settlement failure is not repeated;
- cancellation leaves an in-flight registration;
- required gates refuse legacy, attachment and batch entrypoints;
- exhausted governed work cannot become an authority-free legacy DLQ entry.

The fixture is not a production permit evaluator. Its rate-limit provider
explicitly guarantees rejection without an effect, which permits known
settlement and a safe retry. Real provider adapters require equivalent evidence.
