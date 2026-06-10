# Workflows as Code

Workflows are durable functions written in your own language, executed on
your own workers, and orchestrated by Acteon. A workflow can sleep for
months, wait for external signals, spawn child workflows, and survive any
worker crash — picking up exactly where it left off.

```python
@worker.workflow("order-flow")
def order_flow(ctx, input):
    charge = ctx.step("charge", lambda: charge_card(input["order_id"]))

    # Durable sleep — consumes no resources, survives restarts.
    ctx.sleep(72 * 3600)

    approval = ctx.wait_for_signal("approved", timeout_seconds=86400)
    if approval is None:                      # timed out
        ctx.step("refund", lambda: refund(charge["charge_id"]))
        return {"status": "refunded"}

    child = ctx.start_child("fulfillment", {"order_id": input["order_id"]})
    result = ctx.wait_for_child(child)
    return {"status": "fulfilled", "tracking": result["result"]["tracking"]}
```

## The checkpoint execution model

Acteon uses **checkpoint-based durable execution** (the model used by
Restate, Inngest, and Azure Durable Functions) rather than replay-based
determinism:

- The server persists a list of named **checkpoints** — the recorded result
  of every completed operation (`step:charge#1`, `sleep#1`,
  `signal:approved#1`, …).
- On every resume, the worker re-runs the workflow function from the top.
  The context **replays** recorded checkpoints instantly: `ctx.step()`
  returns the stored result without re-executing.
- The function therefore reaches the first un-recorded operation
  deterministically and continues from there.
- At a suspension point (`sleep`, `wait_for_signal`) with no recorded
  checkpoint, the SDK settles the continuation task with a *directive*;
  the server schedules the timer / signal wait and enqueues a new
  continuation task when it resolves.

The only rule: **the order and names of checkpointed operations must be
stable across re-runs** (same code path up to the suspension point). There
is no determinism sandbox — ordinary logging, metrics, and clock reads are
fine, as long as they don't change which steps run.

Side-effecting code belongs inside `ctx.step()`: steps are executed at
most once per recorded checkpoint, but a crash *between* executing the
step and recording its checkpoint re-executes it on resume — so make step
bodies idempotent where it matters.

## Lifecycle

1. `POST /v1/workflows/start` creates the execution and enqueues the first
   continuation task (action type `__workflow__`) on a
   [worker queue](task-queues.md).
2. A worker with `register_workflow("order-flow", fn)` polls the queue,
   builds a context from the task's checkpoint snapshot, and runs the
   function.
3. The function returns → execution `completed`; raises → `failed`;
   suspends → `waiting_timer` / `waiting_signal` until the server resumes
   it with a fresh continuation task.
4. Signals: `POST /v1/workflows/executions/{id}/signal/{name}`. Signals
   that arrive before the matching `wait_for_signal` are buffered.
5. Child workflows complete by signalling their parent
   (`__child:{child_id}`); `parent_close_policy: cancel` tears children
   down when the parent closes.

## Context API

| Operation | Checkpoint key | Behavior |
|---|---|---|
| `ctx.step(name, fn)` | `step:{name}#{k}` | Execute once, record result, replay thereafter |
| `ctx.sleep(seconds)` | `sleep#{k}` | Durable timer; suspends the worker-side run |
| `ctx.wait_for_signal(name, timeout_seconds=None)` | `signal:{name}#{k}` | Suspends until the signal arrives; returns its payload (`None` on timeout) |
| `ctx.start_child(workflow, input, queue=None, parent_close_policy="abandon")` | `child:{workflow}#{k}` | Starts a child execution (idempotent), returns its ID |
| `ctx.wait_for_child(child_id)` | `signal:__child:{id}#{k}` | Awaits the child's terminal result |

`{k}` is the per-name occurrence counter within one run, so calling the
same step name in a loop yields stable, distinct checkpoints.

## Operations & visibility

- `GET /v1/workflows/executions` — list/filter executions.
- `GET /v1/workflows/executions/{id}` — status, checkpoints, awaiting state.
- `GET /v1/executions/{id}/history` — the same event-history endpoint used
  by chains: started, checkpoints, timers, signals, terminal outcome.
- `POST /v1/workflows/executions/{id}/cancel` — cancel (cancels the
  in-flight continuation task and `cancel`-policy children).
- Continuation tasks inherit the queue's retry semantics: a worker crash
  mid-run simply re-delivers the task and the function replays its
  checkpoints (3 attempts by default, then the execution fails).

## Workflows vs. chains

| | Chains | Workflows |
|---|---|---|
| Defined as | Declarative YAML/TOML/API config | Code on your workers |
| Logic | Steps, branches, templates | Arbitrary (loops, conditionals, local state) |
| Executes in | Acteon server (providers) or workers (`worker` steps) | Your workers only |
| Best for | Routing pipelines, notification fan-out, policy-gated dispatch | Business processes, sagas, long-lived orchestration |

Both share the same execution-history format, signal delivery, durable
timers, and worker queues — chains are the low-code surface, workflows the
full-code surface, one engine underneath.
