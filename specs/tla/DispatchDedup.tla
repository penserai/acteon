--------------------------- MODULE DispatchDedup --------------------------
(*
 * TLA+ specification of Acteon's dispatch pipeline deduplication.
 *
 * Models the core concurrency protocol from gateway.rs:dispatch_inner():
 *   1. Acquire distributed lock (dispatch:{ns}:{tenant}:{action_id})
 *   2. Rule evaluation → Deduplicate verdict
 *   3. check_and_set on dedup key (atomic)
 *   4. If new: execute via provider; if exists: return Deduplicated
 *   5. Audit + release lock
 *
 * Key question answered by this spec:
 *   If the dispatch lock TTL expires while the pipeline is running (e.g.,
 *   slow provider), can a second gateway double-execute the same action?
 *
 * The spec models lock TTL expiry, process crashes, and concurrent dispatch
 * of the same action across multiple gateways.
 *
 * Run with:
 *   java -jar tla2tools.jar -config DispatchDedup.cfg DispatchDedup.tla
 *)
EXTENDS Integers, FiniteSets, Sequences, TLC

\* -----------------------------------------------------------------------
\* Constants
\* -----------------------------------------------------------------------
CONSTANTS
    Gateways,       \* Set of gateway instances, e.g. {"g1", "g2"}
    Actions,        \* Set of action IDs to dispatch
    DedupKeys,      \* Mapping: action ID -> dedup key (can map N:1)
    LockTTL,        \* Lock TTL in ticks (e.g. 3)
    MaxTime,        \* State-space bound
    NOBODY,         \* Sentinel for "no lock holder"
    EMPTY           \* Sentinel for "key not set in store"

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    \* Per-gateway, per-action dispatch state
    gw_phase,           \* [Gateways x Actions -> phase enum]

    \* Distributed dispatch lock (one per action)
    dispatch_holder,    \* [Actions -> Gateways \cup {NOBODY}]
    dispatch_ttl,       \* [Actions -> 0..LockTTL]

    \* Dedup store (check_and_set semantics)
    dedup_store,        \* [DedupKey range -> EMPTY | "set"]

    \* Provider execution counter (ghost variable for verification)
    executed,           \* [Actions -> Nat]

    \* Global clock
    clock

\* -----------------------------------------------------------------------
\* Phase enum
\* -----------------------------------------------------------------------
GWPhases == {"idle", "locking", "locked", "checking_dedup",
             "executing", "auditing", "releasing", "done", "crashed"}

\* Derived: the set of all dedup key values
DedupKeyValues == {DedupKeys[a] : a \in Actions}

\* -----------------------------------------------------------------------
\* Type invariant
\* -----------------------------------------------------------------------
TypeOK ==
    /\ gw_phase \in [Gateways \X Actions -> GWPhases]
    /\ dispatch_holder \in [Actions -> Gateways \cup {NOBODY}]
    /\ dispatch_ttl \in [Actions -> 0..LockTTL]
    /\ dedup_store \in [DedupKeyValues -> {EMPTY, "set"}]
    /\ executed \in [Actions -> Nat]
    /\ clock \in 0..MaxTime

\* -----------------------------------------------------------------------
\* Initial state
\* -----------------------------------------------------------------------
Init ==
    /\ gw_phase = [p \in Gateways \X Actions |-> "idle"]
    /\ dispatch_holder = [a \in Actions |-> NOBODY]
    /\ dispatch_ttl = [a \in Actions |-> 0]
    /\ dedup_store = [k \in DedupKeyValues |-> EMPTY]
    /\ executed = [a \in Actions |-> 0]
    /\ clock = 0

\* -----------------------------------------------------------------------
\* Actions
\* -----------------------------------------------------------------------

(* Gateway starts dispatching an action *)
StartDispatch(g, a) ==
    /\ gw_phase[<<g, a>>] = "idle"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "locking"]
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, dedup_store, executed, clock>>

(* Gateway tries to acquire the dispatch lock *)
TryLock(g, a) ==
    /\ gw_phase[<<g, a>>] = "locking"
    /\ IF dispatch_holder[a] = NOBODY
       THEN /\ dispatch_holder' = [dispatch_holder EXCEPT ![a] = g]
            /\ dispatch_ttl' = [dispatch_ttl EXCEPT ![a] = LockTTL]
            /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "locked"]
       ELSE \* Lock held by someone else — wait (modelled as staying in locking)
            /\ UNCHANGED <<dispatch_holder, dispatch_ttl, gw_phase>>
    /\ UNCHANGED <<dedup_store, executed, clock>>

(* Gateway performs dedup check (check_and_set) *)
CheckDedup(g, a) ==
    /\ gw_phase[<<g, a>>] = "locked"
    /\ LET key == DedupKeys[a]
       IN IF dedup_store[key] = EMPTY
          THEN \* Key is new — set it and proceed to execute
               /\ dedup_store' = [dedup_store EXCEPT ![key] = "set"]
               /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "executing"]
          ELSE \* Already deduplicated — skip execution
               /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "auditing"]
               /\ UNCHANGED dedup_store
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, executed, clock>>

(* Gateway executes the action via provider *)
Execute(g, a) ==
    /\ gw_phase[<<g, a>>] = "executing"
    /\ executed' = [executed EXCEPT ![a] = executed[a] + 1]
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "auditing"]
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, dedup_store, clock>>

(* Gateway writes audit record *)
Audit(g, a) ==
    /\ gw_phase[<<g, a>>] = "auditing"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "releasing"]
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, dedup_store, executed, clock>>

(* Gateway releases the dispatch lock *)
ReleaseLock(g, a) ==
    /\ gw_phase[<<g, a>>] = "releasing"
    /\ dispatch_holder[a] = g  \* only release if we still hold it
    /\ dispatch_holder' = [dispatch_holder EXCEPT ![a] = NOBODY]
    /\ dispatch_ttl' = [dispatch_ttl EXCEPT ![a] = 0]
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "done"]
    /\ UNCHANGED <<dedup_store, executed, clock>>

(* Lock expired before gateway released it — gateway releases anyway *)
ReleaseLockExpired(g, a) ==
    /\ gw_phase[<<g, a>>] = "releasing"
    /\ dispatch_holder[a] # g  \* lock was already taken by TTL expiry
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "done"]
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, dedup_store, executed, clock>>

(* Gateway returns to idle *)
ReturnToIdle(g, a) ==
    /\ gw_phase[<<g, a>>] = "done"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "idle"]
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, dedup_store, executed, clock>>

(* A gateway crashes at any point during the pipeline.
   The lock is NOT released — it expires via TTL. *)
Crash(g, a) ==
    /\ gw_phase[<<g, a>>] \in {"locked", "checking_dedup", "executing",
                                 "auditing", "releasing"}
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "done"]
    \* Lock is NOT released — will expire via ClockTick
    /\ UNCHANGED <<dispatch_holder, dispatch_ttl, dedup_store, executed, clock>>

(* Clock tick: advance time, expire locks *)
ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ dispatch_ttl' = [a \in Actions |->
        IF dispatch_ttl[a] > 0 THEN dispatch_ttl[a] - 1 ELSE 0]
    /\ dispatch_holder' = [a \in Actions |->
        IF dispatch_ttl[a] = 1  \* will become 0 after this tick
        THEN NOBODY
        ELSE dispatch_holder[a]]
    /\ UNCHANGED <<gw_phase, dedup_store, executed>>

\* -----------------------------------------------------------------------
\* Next-state relation
\* -----------------------------------------------------------------------
Next ==
    \/ \E g \in Gateways, a \in Actions :
        \/ StartDispatch(g, a)
        \/ TryLock(g, a)
        \/ CheckDedup(g, a)
        \/ Execute(g, a)
        \/ Audit(g, a)
        \/ ReleaseLock(g, a)
        \/ ReleaseLockExpired(g, a)
        \/ ReturnToIdle(g, a)
        \/ Crash(g, a)
    \/ ClockTick

vars == <<gw_phase, dispatch_holder, dispatch_ttl, dedup_store, executed, clock>>

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY PROPERTIES
\* =======================================================================

(*
 * THE KEY INVARIANT: For each dedup key, at most one provider execution.
 *
 * This is the property that justifies the dedup CAS as a safety net when
 * the dispatch lock TTL expires.
 *)
DedupSafety ==
    \A a \in Actions : executed[a] <= 1

(*
 * Across all actions sharing the same dedup key, total executions <= 1.
 *
 * This catches the case where actions a1 and a2 share dedup key "k1"
 * and both execute (only one should).
 *)
GlobalDedupSafety ==
    \A k \in DedupKeyValues :
        LET actions_for_key == {a \in Actions : DedupKeys[a] = k}
        IN SumExec(actions_for_key) <= 1

\* Helper: sum of executed counts for a set of actions
RECURSIVE SumExecHelper(_, _)
SumExecHelper(S, acc) ==
    IF S = {} THEN acc
    ELSE LET a == CHOOSE x \in S : TRUE
         IN SumExecHelper(S \ {a}, acc + executed[a])

SumExec(S) == SumExecHelper(S, 0)

(*
 * Lock mutual exclusion: at most one holder per action.
 *)
LockMutex ==
    \A a \in Actions :
        Cardinality({g \in Gateways : dispatch_holder[a] = g}) <= 1

\* =======================================================================
\* LIVENESS PROPERTIES
\* =======================================================================

(*
 * Every started dispatch eventually completes.
 * Requires fairness assumptions (enabled in .cfg).
 *)
DispatchProgress ==
    \A g \in Gateways, a \in Actions :
        gw_phase[<<g, a>>] # "idle" ~> gw_phase[<<g, a>>] \in {"idle", "done"}

==========================================================================
