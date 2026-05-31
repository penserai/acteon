-------------------------- MODULE RecurringDispatch ------------------------
(*
 * TLA+ specification of Acteon's recurring-action at-most-once dispatch.
 *
 * Models the protocol in crates/gateway/src/background/workers/recurring.rs
 * (and the PR #235 fix). When an occurrence is due (in the timeout index) a
 * worker:
 *   1. claims it via check_and_set(claim_key, ttl = 60s); a loser skips;
 *   2. RE-ARMS the timeout index to the NEXT occurrence — BEFORE handing off
 *      the dispatch (the PR #235 fix). The actual dispatch happens
 *      consumer-side and can outlive the 60s claim TTL (a chain, an approval,
 *      a slow webhook).
 *
 * The bug #235 fixed: without the pre-arm, a dispatch slower than the claim
 * TTL left the occurrence still "due" with the claim lapsed, so the next poll
 * re-claimed and dispatched the SAME occurrence a second time.
 *
 * Verified: AtMostOnce — each occurrence is dispatched at most once, across
 * claim-TTL expiry and concurrent workers. (Drop the `armed' = FALSE` re-arm
 * in ClaimAndRearm and TLC finds the double-dispatch.)
 *
 * Run with:
 *   java -jar tla2tools.jar -config RecurringDispatch.cfg RecurringDispatch.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Workers,   \* concurrent recurring workers / replicas, e.g. {w1, w2}
    ClaimTTL,  \* claim-key TTL in ticks (60s in production)
    MaxTime,   \* state-space bound
    NOBODY     \* sentinel: claim free

VARIABLES
    armed,        \* BOOLEAN: the current occurrence is still due (re-pollable)
    claim_owner,  \* Workers \cup {NOBODY}
    claim_ttl,    \* 0..ClaimTTL
    disp_count,   \* 0..3 dispatches of the CURRENT occurrence
    w_phase,      \* [Workers -> {"idle","dispatching","done"}]
    clock

TypeOK ==
    /\ armed \in BOOLEAN
    /\ claim_owner \in Workers \cup {NOBODY}
    /\ claim_ttl \in 0..ClaimTTL
    /\ disp_count \in 0..3
    /\ w_phase \in [Workers -> {"idle", "dispatching", "done"}]
    /\ clock \in 0..MaxTime

Init ==
    /\ armed = TRUE
    /\ claim_owner = NOBODY
    /\ claim_ttl = 0
    /\ disp_count = 0
    /\ w_phase = [w \in Workers |-> "idle"]
    /\ clock = 0

\* Poll the due occurrence: atomically claim it (first writer wins) and re-arm
\* the index to the next occurrence BEFORE dispatching. `armed' = FALSE` makes
\* this occurrence no longer re-pollable, so a dispatch that outlives the claim
\* TTL cannot cause a second dispatch of the same occurrence.
ClaimAndRearm(w) ==
    /\ w_phase[w] = "idle"
    /\ armed = TRUE
    /\ claim_owner = NOBODY
    /\ claim_owner' = w
    /\ claim_ttl' = ClaimTTL
    /\ armed' = FALSE
    /\ w_phase' = [w_phase EXCEPT ![w] = "dispatching"]
    /\ UNCHANGED <<disp_count, clock>>

\* The (possibly slow) consumer-side dispatch completes.
Dispatch(w) ==
    /\ w_phase[w] = "dispatching"
    /\ disp_count' = disp_count + 1
    /\ w_phase' = [w_phase EXCEPT ![w] = "done"]
    /\ UNCHANGED <<armed, claim_owner, claim_ttl, clock>>

\* Worker finishes; releases its own claim.
Done(w) ==
    /\ w_phase[w] = "done"
    /\ w_phase' = [w_phase EXCEPT ![w] = "idle"]
    /\ claim_owner' = IF claim_owner = w THEN NOBODY ELSE claim_owner
    /\ claim_ttl' = IF claim_owner = w THEN 0 ELSE claim_ttl
    /\ UNCHANGED <<armed, disp_count, clock>>

\* The cron schedule fires the NEXT occurrence once the current one is fully
\* processed (no in-flight worker). Opens a fresh occurrence window.
NextOccurrence ==
    /\ armed = FALSE
    /\ claim_owner = NOBODY
    /\ \A w \in Workers : w_phase[w] = "idle"
    /\ armed' = TRUE
    /\ disp_count' = 0
    /\ UNCHANGED <<claim_owner, claim_ttl, w_phase, clock>>

\* Time advances; the claim TTL expires (the case the fix must survive).
ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ IF claim_ttl > 0
       THEN /\ claim_ttl' = claim_ttl - 1
            /\ IF claim_ttl - 1 = 0 THEN claim_owner' = NOBODY ELSE UNCHANGED claim_owner
       ELSE UNCHANGED <<claim_ttl, claim_owner>>
    /\ UNCHANGED <<armed, disp_count, w_phase>>

Next ==
    \/ \E w \in Workers : ClaimAndRearm(w) \/ Dispatch(w) \/ Done(w)
    \/ NextOccurrence
    \/ ClockTick

vars == <<armed, claim_owner, claim_ttl, disp_count, w_phase, clock>>
Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* The current occurrence is dispatched at most once, even when a dispatch
\* outlives the claim TTL and another worker polls.
AtMostOnce == disp_count <= 1

============================================================================
