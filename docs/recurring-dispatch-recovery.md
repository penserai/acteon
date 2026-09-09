# Recurring dispatch recovery

This phase makes the failure boundary in `specs/tla/RecurringDispatch.tla`
executable at the production background-worker boundary. The formal model uses
two workers, a shared occurrence claim, a polling interval, and a recovery
margin. Its constant assumption requires the claim lease to cover two polling
windows plus that margin.

`recurring-dispatch.json` is a memory-only, schema-2 scenario. It uses the
shared `ManualClock` and `MemoryStateStore`, then drives
`BackgroundProcessor::tick(BackgroundJob::RecurringActions)` directly:

1. Worker A claims a due recurring occurrence and hands it to a deliberately
   unavailable consumer.
2. Before that handoff, the production worker re-arms the pending index at the
   next cron occurrence.
3. The manual clock advances exactly to the derived claim-lease boundary.
4. Worker B polls the same state. It must emit no second handoff because the
   original occurrence is no longer due.

The scenario records the next pending deadline, derived lease, and post-expiry
handoffs. Its negative mutation restores the stale due index after worker A's
handoff. That recreates the old failure mode: once the lease expires, worker B
hands off the same recurrence again, so the `lease_expiry_no_redelivery`
mandatory gate fails.

The fixed one-second polling interval gives a 32-second claim lease:
`2 × 1 second + 30 seconds`. This matches the model configuration's relation,
while the five-minute cron next occurrence keeps the re-armed entry outside the
lease-expiry window. No OS sleeps, process scheduling, or test-only production
API is involved.

Run and replay it with:

```sh
scripts/ci/scenarios.sh memory
```

The phase covers one occurrence between worker handoff and consumer completion.
It does not establish exactly-once external effects after a consumer has begun a
provider call, nor does it model remote-database clocks or network partitions.
