--------------------------- MODULE MultiQuotaRollback ---------------------------
(*
 * TLA+ specification of Acteon's MULTI-policy quota enforcement — the
 * CROSS-policy all-or-nothing rollback when one policy blocks. This is distinct
 * from QuotaCounter.tla, which verifies a SINGLE counter's atomic
 * check-and-increment. Here the focus is the interaction ACROSS several Block
 * policies that share one dispatch.
 *
 * Models crates/gateway/src/quota_enforcement.rs:
 *   - enforce_quota_policies      (~line 182)
 *   - increment_all_quota_counters(~line 248)
 *   - pick_winning_quota          (~line 359)
 * and the OverageBehavior::Block path in crates/core/src/quota.rs
 * (QuotaPolicy.max_actions / OverageBehavior::Block, ~line 62/145).
 *
 * Per the doc comment at quota_enforcement.rs:177 — "Each policy increments its
 * own counter first (atomic to avoid races). If any block, every counter touched
 * in this call is rolled back. Otherwise the strictest non-block outcome wins,
 * with counters left advanced." So for ONE dispatch subject to M Block policies:
 *
 *     // increment_all_quota_counters: atomic fetch-add on EVERY counter,
 *     // observing each post-increment value `used` (state.increment returns it):
 *     for p in policies { used[p] = state.increment(key[p], +1); }
 *
 *     // pick_winning_quota: a policy is exceeded iff used[p] > limit[p]
 *     // (the code's `inc.used <= inc.policy.max_actions` is "within quota").
 *     if EXISTS p : used[p] > limit[p] {        // a Block policy is exceeded
 *         // enforce_quota_policies (is_block branch, ~205): roll back EVERY
 *         // counter incremented in THIS call, in parallel (-1 each), so the
 *         // blocked dispatch consumes NO budget on ANY policy.
 *         for p in policies { state.increment(key[p], -1); }   // BLOCK
 *     } else {
 *         // within every limit: ADMIT, all increments stand.
 *     }
 *
 * The admit/block decision uses the per-dispatcher INCREMENT-TIME observed
 * post-values (what state.increment returned), not a fresh re-read — this spec
 * captures each post-value into d_obs[d] at increment time and decides on it.
 *
 * Protocol modeled. N concurrent dispatchers, each subject to the SAME set of
 * M = 2 Block-mode policies (limits La, Lb). A dispatcher's enforcement is one
 * indivisible Step: atomically fetch-add BOTH counters (the per-counter
 * state.increment is itself atomic, and join_all gathers the post-values),
 * observe both post-values, then decide on those observed values. If EITHER
 * observed value exceeds its limit -> BLOCK and roll back BOTH increments (net
 * zero on both counters). Otherwise ADMIT and both increments stand. A blocked
 * dispatcher therefore contributes 0 to BOTH counters.
 *
 * Verified (over every interleaving of concurrent dispatchers across both
 * shared counters):
 *   - AllOrNothingOnBlock / NoPartialLeak: for each policy p the settled
 *     counter equals the number of ADMITTED dispatchers (counter[p] =
 *     Cardinality(admitted)). A blocked dispatcher leaves no stale +1 on ANY
 *     policy — including a policy that did NOT itself exceed its limit.
 *   - NoOverAdmitAnyPolicy: for each policy p, Cardinality(admitted) <= limit[p]
 *     — a dispatch is admitted only when within EVERY policy's limit.
 *
 * SCOPE. This spec deliberately abstracts away: the fail-open paths (a real
 * state.increment can fail and the helper then rolls back and fail-opens; the
 * Block-path rollback is itself best-effort and can leave "ghost consumption"
 * on a store blip — quota_enforcement.rs:208-225); the non-Block overage
 * behaviors (Warn/Degrade/Notify) and the strictest-wins precedence; the
 * provider/principal scope filtering and the policy-key construction. It models
 * the happy-path counters as exact and both policies as Block, isolating the
 * one property under test: cross-policy all-or-nothing rollback.
 *
 * The fix anchored here is rolling back EVERY counter incremented in this call
 * (not just the blocking one). Negative check: change the BLOCK branch to roll
 * back ONLY the policy that exceeded (leaving the non-blocking policy's +1 in
 * place). A dispatcher blocked because policy A exceeded then leaves a stale +1
 * on policy B, so counter[B] > Cardinality(admitted) and AllOrNothingOnBlock is
 * violated (TLC reports counter[B] reaching admitted+1).
 *
 * A WindowReset action (fresh quota window: both counters reset to 0) reopens
 * the work so the system CYCLES — no benign terminal deadlock under -deadlock.
 *
 * Run with:
 *   java -jar tla2tools.jar -config MultiQuotaRollback.cfg MultiQuotaRollback.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Dispatchers,  \* concurrent dispatchers / gateway replicas, e.g. {d1, d2}
    La,           \* limit of policy "a" (Block over this; used <= La is within)
    Lb,           \* limit of policy "b"
    MaxCount,     \* state-space bound on each counter
    NOBODY        \* sentinel: a dispatcher has not yet observed a value

\* The two Block policies sharing every dispatch.
Policies == {"a", "b"}

\* Per-policy limit. (.cfg cannot hold a function literal, so this is an
\* OPERATOR rather than a CONSTANT mapping.)
Limit(p) == IF p = "a" THEN La ELSE Lb

VARIABLES
    counter,    \* [Policies -> 0..MaxCount]: the shared QuotaUsage.count per policy
    admitted,   \* set of Dispatchers whose dispatch was admitted this window
    blocked,    \* set of Dispatchers blocked (rolled back) this window
    d_phase,    \* [Dispatchers -> {"idle","done"}]: enforcement progress
    d_obs       \* [Dispatchers -> [Policies -> NOBODY \cup (1..MaxCount)]]:
                \* per-policy post-increment value observed at increment time

TypeOK ==
    /\ counter \in [Policies -> 0..MaxCount]
    /\ admitted \subseteq Dispatchers
    /\ blocked \subseteq Dispatchers
    /\ d_phase \in [Dispatchers -> {"idle", "done"}]
    /\ d_obs \in [Dispatchers -> [Policies -> (1..MaxCount) \cup {NOBODY}]]

Init ==
    /\ counter = [p \in Policies |-> 0]
    /\ admitted = {}
    /\ blocked = {}
    /\ d_phase = [d \in Dispatchers |-> "idle"]
    /\ d_obs = [d \in Dispatchers |-> [p \in Policies |-> NOBODY]]

\* One dispatcher's enforcement, as ONE indivisible Step. This models
\* increment_all_quota_counters followed by pick_winning_quota and the is_block
\* rollback in enforce_quota_policies. Each counter's fetch-add is atomic
\* (state.increment); the post-values are observed here (d_obs) and the
\* admit/block decision is made on THOSE observed values — not a fresh re-read.
\*
\*   post[p] = counter[p] + 1     for every policy p   (atomic fetch-add)
\*   if EXISTS p : post[p] > Limit(p)   -> BLOCK: roll back EVERY counter
\*                                         (net zero on both) -> blocked
\*   else                               -> ADMIT: both increments stand -> admitted
Step(d) ==
    /\ d_phase[d] = "idle"
    /\ \A p \in Policies : counter[p] < MaxCount
    /\ LET post == [p \in Policies |-> counter[p] + 1] IN
       IF \E p \in Policies : post[p] > Limit(p)
       THEN \* At least one Block policy exceeded: BLOCK and roll back BOTH
            \* counters (net zero change on every policy). The fix under test:
            \* EVERY incremented counter is rolled back, not just the exceeder.
            /\ counter' = counter
            /\ admitted' = admitted
            /\ blocked' = blocked \cup {d}
            /\ d_obs' = [d_obs EXCEPT ![d] = post]
       ELSE \* Within every limit: ADMIT, all increments stand.
            /\ counter' = [p \in Policies |-> counter[p] + 1]
            /\ admitted' = admitted \cup {d}
            /\ blocked' = blocked
            /\ d_obs' = [d_obs EXCEPT ![d] = post]
    /\ d_phase' = [d_phase EXCEPT ![d] = "done"]

\* A fresh quota window opens (epoch-aligned window index rolls over, TTL'd
\* counters reset). Clean boundary: every dispatcher has finished enforcing.
\* Reopens the work so the system cycles — no terminal deadlock.
WindowReset ==
    /\ \A d \in Dispatchers : d_phase[d] = "done"
    /\ counter' = [p \in Policies |-> 0]
    /\ admitted' = {}
    /\ blocked' = {}
    /\ d_phase' = [d \in Dispatchers |-> "idle"]
    /\ d_obs' = [d \in Dispatchers |-> [p \in Policies |-> NOBODY]]

Next ==
    \/ \E d \in Dispatchers : Step(d)
    \/ WindowReset

vars == <<counter, admitted, blocked, d_phase, d_obs>>
Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* All-or-nothing on block / no partial leak: for EVERY policy the settled
\* counter equals the number of admitted dispatchers. A blocked dispatcher
\* contributes 0 to every counter — including a policy that did not itself
\* exceed. With a buggy partial rollback (only the exceeding policy refunded),
\* the non-blocking policy's counter exceeds the admitted count, violating this.
AllOrNothingOnBlock ==
    \A p \in Policies : counter[p] = Cardinality(admitted)

\* No over-admission on any policy: the number of admitted dispatches never
\* exceeds any policy's limit (a dispatch is admitted only when within EVERY
\* policy's limit, blocked if ANY exceeds).
NoOverAdmitAnyPolicy ==
    \A p \in Policies : Cardinality(admitted) <= Limit(p)

===============================================================================
