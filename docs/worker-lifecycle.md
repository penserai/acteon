# Worker ticks and task lifecycle time

Implemented from merged base `a514c5b` on 2026-09-05. This continues
[the rehabilitation plan](scenario-evaluation.md#remaining-plan) and builds on
[the shared clock contract](virtual-time.md).

## Runtime contract

`BackgroundProcessorBuilder::clock(Arc<dyn Clock>)`,
`BackgroundProcessor::with_clock`, and `TaskEngine::with_clock` default to
`SystemClock`. Supply the same clock to the gateway, memory state/locks, group
manager, task engine, and processor when using manual time. Attaching a gateway
reference alone does not select its clock. The server explicitly shares the
gateway clock with its background processor and request task engines, and chain
projection shares it with the task engine it constructs.

`BackgroundProcessor::tick(BackgroundJob)` runs one enabled worker cycle at the
current clock time. It neither advances time nor waits for the job's polling
period. Disabled jobs are no-ops. The result exposes errors normally logged by
the polling loop; existing best-effort per-record failures remain logged by each
worker. Drain event channels concurrently if a cycle could fill their capacity.
Callers control ordering between clock advances, task mutations, and ticks.

```rust,ignore
let clock = Arc::new(ManualClock::new(epoch));
let state = Arc::new(MemoryStateStore::with_clock(clock.clone()));
let groups = Arc::new(GroupManager::with_clock(clock.clone()));
let engine = TaskEngine::new(state.clone()).with_clock(clock.clone());
let (mut worker, shutdown) = BackgroundProcessorBuilder::new()
    .clock(clock.clone())
    .state(state)
    .group_manager(groups)
    .metrics(Arc::new(GatewayMetrics::default()))
    .config(BackgroundConfig {
        enable_stale_task_reaper: true,
        ..Default::default()
    })
    .build()?;
let task = Task::new_at("task", "ns", "tenant", clock.now());
engine.create_task(task).await?;
clock.advance_to(Duration::from_millis(60_001))?;
worker.tick(BackgroundJob::StaleTasks).await?;
```

`run()` uses the same tick dispatch. Enabled workers run immediately once, then
wait one full period after their cycle completes. Missed polls are skipped,
preventing catch-up bursts after a pause or slow cycle. Simultaneous deadlines
follow the published `BackgroundJob::ALL` order. A pending shutdown wins over a
due timer. In-flight cycles finish before shutdown is observed, as before;
channel backpressure can delay that completion. Dropping the polling future
unregisters its clock timer. The builder rejects zero intervals for enabled jobs;
`run()` logs and returns if a processor constructed directly has invalid periods.

The shared clock now drives group flush timestamps, state-machine timeout
queries/events, chain readiness, scheduled/recurring due queries, approval retry
expiry, state retention cutoffs, stale-task reaping, and all polling waits.
Workflow timers and pinned-definition GC run on their existing chain/retention
cadences. Gateway attachment is independent of template sync, so disabling
that one feature no longer removes the dependency used by workflow timers or
other synchronization jobs.

## Task semantics

Core task construction, transitions, history appends, artifact updates, and
heartbeats have explicit `_at` variants. Existing convenience methods still use
UTC. The task engine samples its clock after each fresh CAS read and uses it for
that mutation. All successful CAS mutations update `updated_at`; administrative
approval/chain links do not renew `last_progress_at`. Importing an existing task
through `create_task` preserves its supplied timestamps. The A2A server constructs
new tasks and their first history entry with the request engine's clock.

Task human-pause approval lifetimes, task audit/stream emission timestamps, and
bridge rollback backoffs use the engine clock. The stale reaper passes its clock
and stream sender into the task engine; subscribers now receive the transition
when a stale task is failed. Repeated ticks do not re-fail terminal tasks.

Boundary behavior is preserved:

| Decision | At the boundary |
| --- | --- |
| Scheduled action, event timeout, group notification | Due at the deadline |
| Approval retry | Ineligible at `expires_at` |
| Recurring action | Due at scheduled time; ineligible at `ends_at` |
| Task staleness | Stale when elapsed whole milliseconds exceed `working_ttl_ms`; still live at equality |
| State retention | Delete eligible completed/resolved rows strictly older than the cutoff; active rows and compliance holds remain |

## Evaluation and limits

`scenarios/workers.json` selects schema 2 `worker_lifecycle`, three seeded trials,
and memory state. The fixed epoch is `2023-11-14T22:13:20Z`; a named seed chooses
the task working TTL. The suite drives production engines and workers directly,
including the polling future, without spawning work or sleeping on OS time.

Grader `portfolio-v3` adds four mandatory safety dimensions:

| Dimension | Weight | Evidence |
| --- | --- | --- |
| Task timestamps | 20 | Pause creation/expiry, transition time, administrative timestamp without liveness renewal |
| Task liveness | 35 | Before/equal/after TTL, heartbeat survival, terminal preservation, exactly one reap audit and stream event |
| Due worker ticks | 30 | Group, timeout, and scheduled events at 999/1000/1001 ms with correct event timestamps |
| Polling cadence | 15 | Immediate tick, no early tick, exact deadline, skipped missed polls, shutdown priority, timer cleanup |

Freezing only the task clock or worker clock must fail the corresponding safety
gates. CLI contracts cover remote-backend rejection for both manifest schemas,
combined deadline/worker replay byte for byte, and forged clock provenance.
The CI script runs and replays this suite alongside the existing suites, retaining
the exact executable identified by each report.

This evidence observes worker handoff, not completed external delivery. Scheduled
handoff still removes discovery keys before consumer acknowledgement; retaining
the payload alone cannot guarantee recovery if the consumer crashes. Durable
handoff/reconciliation belongs to the next deployment/scheduling phase. Likewise,
clock injection does not make state retention transactional or task audit writes
durable. Remote TTLs, audit-store/DLQ retention, network/process scheduling,
generated UUID timestamps, worker-queue leases/retries (`task_queue.rs`), workflow
convenience constructors, and model capability are outside these assertions.
Those queue clocks must be injected before deterministic deployment and tenant
scheduling scenarios can cover end-to-end recovery.

## Verification

Seven dedicated gateway contracts cover task mutation time, polling deadlines,
missed polls, shutdown/cancellation, disabled jobs/invalid periods, approval
expiry with retained rows, recurring scheduling, and state retention holds.
The simulation contract checks replay and both clock mutations. Local validation
passed 3,088 workspace tests, 90 AWS full-feature tests, and 119 feature-enabled
simulation tests (including CLI replay). Workspace/all-target compilation,
frontend lint/build, and the source secret scan passed; UI lint retains its
existing TanStack compiler compatibility warning.

Memory, disposable Redis, and disposable PostgreSQL each passed 23 kernel
invariants and nine portfolio trials with matching replays. Memory additionally
passed three deadline and three worker trials. All eight suite/backend artifact
pairs were independently checked for matching JSON, valid XML/JSONL, passing
checks, and preserved runner fingerprints. See the pull request for final Clippy
and CI results.
