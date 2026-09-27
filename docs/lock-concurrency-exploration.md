# Memory-lock concurrency exploration

`crates/state/memory/tests/lock_schedules.rs` exercises the production memory
lock with a shared manual clock and an independent single-key reference model.
It runs in the existing workspace test jobs on stable Rust and Rust 1.88.

The bounded explorer enumerates all 120 permutations of five API-level events:
renew the original guard, advance to its original expiry boundary, release the
original guard, attempt a replacement acquisition, and sweep expired entries.
After every event it compares both guards' ownership observations with the model.
It checks renewal failure, acquisition availability, sweep counts, and final
reacquisition. Renewal after consuming the original guard is a no-op, not a call
through an invalid guard. Expired model entries remain present until eviction.

A second test runs 64 barrier-coordinated races on two Tokio worker threads:
an expired original owner releases while its replacement renews. The replacement
must survive the stale release and sweep, exclude a third acquisition, then expire
at its renewed deadline. The stale owner cannot renew the replacement's lease.
No wall-clock sleeps determine expiry.

Run locally:

```sh
cargo test --locked -p acteon-state-memory --test lock_schedules
```

## Negative evidence and limits

Both tests failed when the owner condition on `MemoryLockGuard::release` was
temporarily replaced by unconditional removal. The deterministic failure printed
`[Expire, Renew, Acquire, Release, Sweep]`, step 3, owner 2: the stale release
incorrectly removed the replacement. The mutation was reverted; no production
lock behavior is changed by this slice.

The 120 schedules are exhaustive only over these five external events. They do
not enumerate DashMap internals, all thread interleavings, multiple lock names,
remote backends, partitions, or process crashes. The 64 concurrent trials sample
OS schedules and are not deterministic replay or a fairness/liveness proof.
Expired holders can still execute external effects; leases alone do not provide
downstream fencing or exactly-once execution. Loom-style internal exploration,
fuzzing, and performance budgets remain follow-up work.
