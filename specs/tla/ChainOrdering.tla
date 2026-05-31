--------------------------- MODULE ChainOrdering ---------------------------
(*
 * TLA+ specification of Acteon's multi-step chain executor advancement.
 *
 * Models the protocol in crates/gateway/src/gateway.rs::advance_chain
 * (around line 2544) and the FRESH re-read CAS idempotency guard at line 2986.
 * The chain's runtime state (crates/core/src/chain.rs::ChainState) carries
 *   current_step       : index of the step being executed,
 *   step_results[i]    : Some(..) once step i has committed a result,
 *   execution_path     : the committed steps in order.
 * Steps run strictly sequentially 0,1,..,K-1; on the last step the chain
 * transitions ChainStatus::Running -> Completed.
 *
 * N concurrent workers / gateway replicas may each call advance_chain on the
 * SAME chain. A worker:
 *   1. reads step_idx = chain_state.current_step (a snapshot, possibly stale);
 *   2. executes step step_idx (dispatches the synthetic action);
 *   3. before persisting, RE-READS fresh state and commits ONLY IF the step is
 *      still unexecuted AND current_step is still step_idx — the CAS guard
 *      `fresh.step_results[step_idx].is_some() || fresh.current_step != step_idx`
 *      (line 2986). If either holds, another worker already advanced, so it
 *      aborts and commits nothing.
 *   4. on commit: records step_results[step_idx], bumps current_step to step_idx+1,
 *      appends to execution_path. Running -> Completed on the final step.
 *
 * Execute and commit are modelled as SEPARATE steps so the re-read CAS guard is
 * load-bearing (mirroring the read/write split in MessageBus.tla), not an
 * artifact of step-atomicity: two workers can both read step_idx = i and both
 * execute step i, but the CAS lets at most one of them commit it.
 *
 * SCOPE. advance_chain serializes commits with THREE layers: the chain:{id}
 * distributed lock (gateway.rs:2554), a per-attempt dedup check_and_set
 * (`chain-step:{chain_id}:{step}:a{n}`), and this fresh re-read CAS (line 2986).
 * This spec isolates the LAST layer — the idempotency guard that must hold on
 * the `!is_new` / crash-restart / lock-expiry path where the lock does not
 * provide exclusion. It abstracts the lock and dedup CAS away (a conservative,
 * stronger setting: it assumes the lock can fail to exclude). So a green run
 * verifies the CAS's safety contribution, not the full three-layer stack.
 * InOrder is asserted for LINEAR (sequential) chains; branching chains (where
 * current_step can move non-sequentially) are intentionally out of scope.
 *
 * Verified (over every interleaving of concurrent workers / replicas):
 *   - StepAtMostOnce: each step index is recorded at most once — exec_count[i] <= 1.
 *   - InOrder (Monotonic + contiguous prefix): recorded steps are exactly the
 *     contiguous prefix 0..current_step-1; current_step never skips or moves
 *     backward; step i is never recorded before step i-1. No gap, no reorder.
 *
 * Negative check: drop the FRESH re-read CAS — let a worker commit step i based
 * on its STALE local snapshot without re-checking current_step / step_results
 * (replace the `step_results[i] = none /\ current_step = i` commit guard with
 * the worker's own w_idx). Two workers then both commit step i and
 * StepAtMostOnce is violated — exec_count[i] reaches 2.
 *
 * Run with:
 *   java -jar tla2tools.jar -config ChainOrdering.cfg ChainOrdering.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Workers,  \* concurrent chain advancers / gateway replicas, e.g. {a1, a2}
    Steps,    \* number of steps in the chain (indices 0..Steps-1)
    NOBODY    \* sentinel: step slot not yet recorded

StepIdx == 0..(Steps - 1)

VARIABLES
    current_step,  \* 0..Steps : chain_state.current_step (= Steps when Completed)
    recorded,      \* [StepIdx -> Workers \cup {NOBODY}] : committer of each step
    exec_count,    \* [StepIdx -> 0..3] : times each step's result was recorded
    completed,     \* BOOLEAN : ChainStatus has reached Completed
    w_phase,       \* [Workers -> {"idle","executing","committing","done"}]
    w_idx          \* [Workers -> 0..Steps] : the step_idx snapshot a worker read

TypeOK ==
    /\ current_step \in 0..Steps
    /\ recorded \in [StepIdx -> Workers \cup {NOBODY}]
    /\ exec_count \in [StepIdx -> 0..3]
    /\ completed \in BOOLEAN
    /\ w_phase \in [Workers -> {"idle", "executing", "committing", "done"}]
    /\ w_idx \in [Workers -> 0..Steps]

Init ==
    /\ current_step = 0
    /\ recorded = [s \in StepIdx |-> NOBODY]
    /\ exec_count = [s \in StepIdx |-> 0]
    /\ completed = FALSE
    /\ w_phase = [w \in Workers |-> "idle"]
    /\ w_idx = [w \in Workers |-> 0]

\* advance_chain start: read step_idx = chain_state.current_step (line 2639).
\* The snapshot is taken now; a concurrent worker may advance current_step
\* before this worker reaches its commit, making the snapshot stale.
Start(w) ==
    /\ w_phase[w] = "idle"
    /\ ~completed
    /\ current_step < Steps
    /\ w_idx' = [w_idx EXCEPT ![w] = current_step]
    /\ w_phase' = [w_phase EXCEPT ![w] = "executing"]
    /\ UNCHANGED <<current_step, recorded, exec_count, completed>>

\* Execute step w_idx[w] (dispatch the synthetic action). This is the side-
\* effecting work BEFORE the persist; two workers that both snapshotted the
\* same index can both reach here. No chain state is mutated yet.
Execute(w) ==
    /\ w_phase[w] = "executing"
    /\ w_phase' = [w_phase EXCEPT ![w] = "committing"]
    /\ UNCHANGED <<current_step, recorded, exec_count, completed, w_idx>>

\* Persist under the FRESH re-read CAS guard (line 2986). Re-read fresh state:
\* commit ONLY IF step i is still unrecorded AND current_step is still i.
\* On commit: record the result, bump current_step to i+1, (execution_path push),
\* Running -> Completed on the final step. Otherwise abort — another worker
\* already advanced this step — and commit nothing.
Commit(w) ==
    /\ w_phase[w] = "committing"
    /\ LET i == w_idx[w] IN
       IF recorded[i] = NOBODY /\ current_step = i
       THEN \* CAS won: this worker commits step i exactly once.
            /\ recorded' = [recorded EXCEPT ![i] = w]
            /\ exec_count' = [exec_count EXCEPT ![i] = exec_count[i] + 1]
            /\ current_step' = i + 1
            /\ completed' = (i + 1 = Steps)
            /\ w_phase' = [w_phase EXCEPT ![w] = "done"]
            /\ UNCHANGED w_idx
       ELSE \* CAS lost: stale snapshot, abort with no state change.
            /\ w_phase' = [w_phase EXCEPT ![w] = "done"]
            /\ UNCHANGED <<current_step, recorded, exec_count, completed, w_idx>>

\* Worker returns; it may advance the chain again (next poll).
Finish(w) ==
    /\ w_phase[w] = "done"
    /\ w_phase' = [w_phase EXCEPT ![w] = "idle"]
    /\ UNCHANGED <<current_step, recorded, exec_count, completed, w_idx>>

\* Recycle: once the chain Completed and no worker is in flight, start a fresh
\* chain (or the next execution window) so the system CYCLES and there is no
\* benign terminal deadlock under -deadlock.
Recycle ==
    /\ completed
    /\ \A w \in Workers : w_phase[w] = "idle"
    /\ current_step' = 0
    /\ recorded' = [s \in StepIdx |-> NOBODY]
    /\ exec_count' = [s \in StepIdx |-> 0]
    /\ completed' = FALSE
    /\ w_idx' = [w \in Workers |-> 0]
    /\ UNCHANGED w_phase

Next ==
    \/ \E w \in Workers : Start(w) \/ Execute(w) \/ Commit(w) \/ Finish(w)
    \/ Recycle

vars == <<current_step, recorded, exec_count, completed, w_phase, w_idx>>
Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* Each step index is recorded (executed-and-committed) at most once, despite
\* concurrent workers that snapshotted the same index and both executed it.
StepAtMostOnce == \A i \in StepIdx : exec_count[i] <= 1

\* The recorded steps are EXACTLY the contiguous prefix 0..current_step-1:
\* step i is recorded iff i < current_step. This rules out gaps (step i+1
\* recorded before step i), out-of-order commits, and a current_step that
\* skips or moves backward relative to what has been recorded.
InOrder == \A i \in StepIdx : (recorded[i] # NOBODY) <=> (i < current_step)

============================================================================
