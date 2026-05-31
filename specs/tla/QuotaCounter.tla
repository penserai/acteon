----------------------------- MODULE QuotaCounter -----------------------------
(*
 * TLA+ specification of Acteon's per-tenant usage quota counter under
 * concurrent dispatch.
 *
 * Models crates/gateway/src/quota_enforcement.rs
 * (check_quota / check_quota_inner ~line 70, increment_all_quota_counters
 * ~line 248) together with the OverageBehavior::Block path in
 * enforce_quota_policies. The state-store counter is the QuotaUsage.count in
 * crates/core/src/quota.rs.
 *
 * Protocol. N concurrent dispatchers each admit one action under a single
 * Block-mode quota policy with a fixed limit L. The enforcement step is an
 * ATOMIC check-and-increment against the shared counter (the code's comment
 * "Each policy increments its own counter first (atomic to avoid races)",
 * line ~177, backed by state.increment(&key, 1, ttl) returning the
 * post-increment value `new_count` / `used`):
 *
 *     let v = state.increment(key, +1);   // atomic fetch-add, observe v
 *     if v <= L { admit }                 // within quota
 *     else      { block; state.increment(key, -1); }  // refund the slot
 *
 * Per enforce_quota_policies (line ~205), when the winning policy is Block the
 * counter touched in this call is ROLLED BACK so the blocked request does not
 * consume a slot. This spec models that refund faithfully: a blocked
 * dispatcher decrements the counter back, so the counter settles at exactly
 * the number of ADMITTED actions.
 *
 * Verified (over every interleaving of concurrent dispatchers / replicas):
 *   - NoDrift: the counter equals the number of admitted dispatchers — no two
 *     concurrent atomic increments collapse into one. (Blocked dispatchers
 *     refund, so they do not contribute to the settled counter.)
 *   - NoOverAdmit: the number of ADMITTED actions never exceeds the limit L
 *     (Block behavior).
 *
 * SCOPE. NoDrift assumes the Block-path refund ALWAYS succeeds. The real
 * enforcement is fail-open (quota_enforcement.rs ~208-225): if the rollback
 * increment fails on a state-store blip the counter can settle ABOVE the
 * admitted count — that failure mode is deliberately out of scope here. This
 * spec also models a single Block policy; the multi-policy "block wins -> roll
 * back every counter incremented in this call" path is left to a future sibling.
 *
 * The fix anchored here is the ATOMICITY of check-and-increment. The Step
 * action performs the fetch-add and the observation of the post-value as ONE
 * indivisible action — exactly the guarantee state.increment provides.
 *
 * Negative check: split Step into a non-atomic read-then-write (read v from
 * the shared counter, later write v+1) — modeled in the QuotaCounter_BUGGY
 * scratch copy. Two dispatchers then read the same v and both write v+1, so
 * one increment is lost (NoDrift violated: counter < admitted) AND both
 * observe v+1 <= L and admit (NoOverAdmit violated: admitted > L when the slot
 * they both claim is the last one).
 *
 * A WindowReset action (a fresh quota window resets the counter to 0) reopens
 * the work so the system CYCLES — no benign terminal deadlock under -deadlock.
 *
 * Run with:
 *   java -jar tla2tools.jar -config QuotaCounter.cfg QuotaCounter.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Dispatchers,  \* concurrent dispatchers / gateway replicas, e.g. {d1, d2}
    Limit,        \* L: max actions admitted per window (Block over this)
    MaxCount,     \* state-space bound on the counter
    NOBODY        \* sentinel: a dispatcher has not yet observed a value

VARIABLES
    counter,    \* the shared QuotaUsage.count for the current window
    admitted,   \* set of Dispatchers whose action was admitted this window
    blocked,    \* set of Dispatchers that hit the Block limit this window
    d_phase,    \* [Dispatchers -> {"idle","done"}]: enforcement progress
    d_obs       \* [Dispatchers -> NOBODY \cup (1..MaxCount)]: observed post-value

TypeOK ==
    /\ counter \in 0..MaxCount
    /\ admitted \subseteq Dispatchers
    /\ blocked \subseteq Dispatchers
    /\ d_phase \in [Dispatchers -> {"idle", "done"}]
    /\ d_obs \in [Dispatchers -> (1..MaxCount) \cup {NOBODY}]

Init ==
    /\ counter = 0
    /\ admitted = {}
    /\ blocked = {}
    /\ d_phase = [d \in Dispatchers |-> "idle"]
    /\ d_obs = [d \in Dispatchers |-> NOBODY]

\* ATOMIC check-and-increment (state.increment(key, +1) returning the
\* post-increment value). In ONE indivisible action the dispatcher fetch-adds
\* the shared counter and observes the post-value v. If v <= Limit the action
\* is admitted (the increment stands). Otherwise Block fires and the increment
\* is refunded (counter decremented back) so the blocked request consumes no
\* slot — this is the rollback in enforce_quota_policies. The atomicity here is
\* the property under test: no other dispatcher can interleave between the
\* fetch-add and the observation.
Step(d) ==
    /\ d_phase[d] = "idle"
    /\ counter < MaxCount
    /\ LET v == counter + 1 IN
       IF v <= Limit
       THEN \* Within quota: admit, increment stands.
            /\ counter' = v
            /\ admitted' = admitted \cup {d}
            /\ blocked' = blocked
            /\ d_obs' = [d_obs EXCEPT ![d] = v]
       ELSE \* Over limit: Block, then refund the slot (net zero change).
            /\ counter' = counter
            /\ admitted' = admitted
            /\ blocked' = blocked \cup {d}
            /\ d_obs' = [d_obs EXCEPT ![d] = v]
    /\ d_phase' = [d_phase EXCEPT ![d] = "done"]

\* A fresh quota window opens (epoch-aligned window index rolls over, TTL'd
\* counter resets). Clean boundary: every dispatcher has finished enforcing.
\* Reopens the work so the system cycles — no terminal deadlock.
WindowReset ==
    /\ \A d \in Dispatchers : d_phase[d] = "done"
    /\ counter' = 0
    /\ admitted' = {}
    /\ blocked' = {}
    /\ d_phase' = [d \in Dispatchers |-> "idle"]
    /\ d_obs' = [d \in Dispatchers |-> NOBODY]

Next ==
    \/ \E d \in Dispatchers : Step(d)
    \/ WindowReset

vars == <<counter, admitted, blocked, d_phase, d_obs>>
Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* No lost increment / no drift: the settled counter equals the number of
\* dispatchers whose increment stands (the admitted set) — no two concurrent
\* atomic increments collapse into one. Blocked dispatchers refund, so they do
\* not contribute. With a non-atomic read-then-write this is violated because a
\* lost update leaves counter < Cardinality(admitted).
NoDrift == counter = Cardinality(admitted)

\* No over-admission (Block): the number of admitted actions never exceeds the
\* limit. A non-atomic split lets two dispatchers both observe the same
\* sub-limit value and both admit past L, violating this.
NoOverAdmit == Cardinality(admitted) <= Limit

===============================================================================
