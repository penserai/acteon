# Task Queues & Workers

Task queues let **your own code** execute work orchestrated by Acteon. The
server never runs customer code: workers poll a named queue over HTTP,
lease tasks, execute them, and report results. Chain `worker` steps and
[workflow](workflows.md) continuation tasks flow through the same queues.

## The worker model

1. **Enqueue** — a chain `worker` step (or `POST /v1/queues/{queue}/tasks`)
   puts a task on a queue.
2. **Poll / lease** — a worker calls `POST /v1/queues/{queue}/poll` and
   receives tasks with a `lease_token` and a lease deadline.
3. **Heartbeat** — long-running handlers extend the lease via
   `POST /v1/queues/tasks/{id}/heartbeat`. The SDK workers do this
   automatically at half the lease interval.
4. **Settle** — the worker calls `…/complete` with a result or `…/fail`
   with an error. Retryable failures within the task's `max_attempts`
   budget are re-queued with exponential backoff; terminal failures go to
   the dead-letter queue.
5. **Reclaim** — if a worker crashes, its lease expires and the task is
   re-delivered to another worker (or failed once attempts are exhausted).

All transitions are compare-and-swap guarded: concurrent workers polling
the same queue never double-lease a task, and a worker whose lease expired
cannot clobber a re-delivered task's result (the lease token won't match).

## Worker chain steps

Route any chain step to external workers instead of an in-process provider:

```toml
[[chains.steps]]
name = "build"
worker = { queue = "builds", action_type = "compile", timeout_seconds = 3600, max_attempts = 3 }
payload_template = { repo = "{{origin.payload.repo}}", sha = "{{origin.payload.sha}}" }

[[chains.steps]]
name = "notify"
provider = "slack"
action_type = "send_message"
payload_template = { text = "Build done: {{prev.body.artifact}}" }
```

The chain pauses in `waiting_worker` until a worker completes the task; the
worker's result becomes the step's response body. Retries, branching,
failure policies, quotas, and audit all apply to worker steps exactly as
they do to provider steps.

## Writing a worker

Python:

```python
from acteon_client import ActeonClient, Worker

client = ActeonClient("http://localhost:8080", api_key="…")
worker = Worker(client, namespace="ci", tenant="t1", queue="builds")

@worker.handler("compile")
def compile(payload):
    return {"artifact": do_build(payload["repo"], payload["sha"])}

worker.run()   # poll loop with auto-heartbeat
```

TypeScript:

```typescript
import { ActeonClient, Worker } from "@acteon/client";

const client = new ActeonClient({ baseUrl: "http://localhost:8080", apiKey: "…" });
const worker = new Worker(client, { namespace: "ci", tenant: "t1", queue: "builds" });

worker.register("compile", async (payload) => {
  return { artifact: await doBuild(payload.repo, payload.sha) };
});

await worker.run();
```

Go:

```go
worker := acteon.NewWorker(client, acteon.WorkerConfig{
    Namespace: "ci", Tenant: "t1", Queue: "builds",
})
worker.Register("compile", func(ctx context.Context, payload json.RawMessage) (any, error) {
    return doBuild(payload)
})
worker.Run(ctx)
```

Scaling is horizontal: add more workers polling the same queue.

## API reference

| Endpoint | Purpose |
|---|---|
| `POST /v1/queues/{queue}/tasks` | Enqueue a standalone task |
| `POST /v1/queues/{queue}/poll` | Lease up to `max_tasks` tasks |
| `POST /v1/queues/tasks/{id}/heartbeat` | Extend a lease |
| `POST /v1/queues/tasks/{id}/complete` | Report a result |
| `POST /v1/queues/tasks/{id}/fail` | Report a failure (`retryable` flag) |
| `GET /v1/queues/tasks/{id}` | Inspect a task |
| `GET /v1/queues/{queue}/tasks` | List tasks on a queue |

## Reliability notes

- Lease durations are clamped to 1–3600 seconds (default 60).
- Retry backoff is `2^attempt` seconds, capped at 60.
- Expired-lease reclamation runs lazily on poll, so reclamation latency is
  bounded by your workers' poll interval.
- Terminal failures are pushed to the DLQ with provider
  `queue:{queue_name}` and can be inspected and resubmitted from the UI.
