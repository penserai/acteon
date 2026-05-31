--------------------------- MODULE DispatchDedup --------------------------
(*
 * TLA+ specification of Acteon's dispatch-pipeline deduplication.
 *
 * Models the core concurrency protocol from gateway.rs:
 *   - dispatch_inner() acquires a per-action distributed lock
 *     (dispatch:{ns}:{tenant}:{action_id}, 30s TTL)
 *   - handle_dedup() does check_and_set on a dedup key WITH A TTL and, only
 *     if it was the first writer, executes the provider:
 *         let is_new = check_and_set(dedup_key, "1", ttl);
 *         if is_new { execute_action().await } else { Deduplicated }
 *
 * Question answered:
 *   If the dispatch LOCK TTL expires while the pipeline runs (slow provider)
 *   and a second gateway re-acquires the lock, can the same dedup key be
 *   executed twice WITHIN ITS DEDUP-TTL WINDOW?
 *
 * Answer (verified by DedupSafety below): no. The atomicity of check_and_set
 * — not the lock — is what bounds execution to once per dedup-TTL window. The
 * spec deliberately models lock-TTL expiry, crashes, and concurrent dispatch
 * to show the CAS holds even when the lock does not.
 *
 * Scope: the claim is "at most once WITHIN a dedup-TTL window". After the
 * dedup key's TTL lapses, a re-dispatch correctly executes again (a new
 * window). The check-and-execute is modeled atomically, i.e. under the
 * assumption — stated in the design doc — that a single dispatch completes
 * within the dedup TTL; if a provider call outlives the dedup TTL the key can
 * expire mid-flight and a concurrent re-dispatch may double-execute, which is
 * the documented limitation, not a safety guarantee.
 *
 * Run with:
 *   java -jar tla2tools.jar -config DispatchDedup.cfg DispatchDedup.tla
 *)
EXTENDS Integers, FiniteSets, TLC

\* -----------------------------------------------------------------------
\* Constants
\* -----------------------------------------------------------------------
CONSTANTS
    Gateways,   \* Set of gateway instances, e.g. {g1, g2}
    Actions,    \* Set of action IDs dispatched concurrently
    LockTTL,    \* Dispatch-lock TTL in ticks
    DedupTTL,   \* Dedup-key TTL in ticks
    MaxTime,    \* State-space bound on the clock
    NOBODY,     \* Sentinel: no lock holder
    EMPTY       \* Sentinel: dedup key unset

\* All actions in this model share ONE dedup key — the adversarial N:1 case
\* (the real system keys on action.dedup_key, frequently shared across
\* actions). Modeling it as a single shared key is the hardest case for the
\* at-most-once property and avoids an unsupported function literal in the cfg.
SharedKey  == "k"
DedupKeys  == {SharedKey}
KeyOf(a)   == SharedKey

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    gw_phase,         \* [Gateways \X Actions -> phase]
    lock_holder,      \* [Actions -> Gateways \cup {NOBODY}]
    lock_ttl,         \* [Actions -> 0..LockTTL]
    dedup_set,        \* [DedupKeys -> {EMPTY, "set"}]
    dedup_ttl,        \* [DedupKeys -> 0..DedupTTL]
    exec_in_window,   \* [DedupKeys -> 0..2] executions since the key was set
    clock

Phases == {"idle", "locking", "locked", "auditing", "releasing", "done"}

\* -----------------------------------------------------------------------
\* Type invariant
\* -----------------------------------------------------------------------
TypeOK ==
    /\ gw_phase \in [Gateways \X Actions -> Phases]
    /\ lock_holder \in [Actions -> Gateways \cup {NOBODY}]
    /\ lock_ttl \in [Actions -> 0..LockTTL]
    /\ dedup_set \in [DedupKeys -> {EMPTY, "set"}]
    /\ dedup_ttl \in [DedupKeys -> 0..DedupTTL]
    /\ exec_in_window \in [DedupKeys -> 0..2]
    /\ clock \in 0..MaxTime

\* -----------------------------------------------------------------------
\* Initial state
\* -----------------------------------------------------------------------
Init ==
    /\ gw_phase = [p \in Gateways \X Actions |-> "idle"]
    /\ lock_holder = [a \in Actions |-> NOBODY]
    /\ lock_ttl = [a \in Actions |-> 0]
    /\ dedup_set = [k \in DedupKeys |-> EMPTY]
    /\ dedup_ttl = [k \in DedupKeys |-> 0]
    /\ exec_in_window = [k \in DedupKeys |-> 0]
    /\ clock = 0

\* -----------------------------------------------------------------------
\* Actions
\* -----------------------------------------------------------------------

\* Gateway begins dispatching an action.
StartDispatch(g, a) ==
    /\ gw_phase[<<g, a>>] = "idle"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "locking"]
    /\ UNCHANGED <<lock_holder, lock_ttl, dedup_set, dedup_ttl, exec_in_window, clock>>

\* Gateway tries to acquire the dispatch lock (first writer wins).
TryLock(g, a) ==
    /\ gw_phase[<<g, a>>] = "locking"
    /\ IF lock_holder[a] = NOBODY
       THEN /\ lock_holder' = [lock_holder EXCEPT ![a] = g]
            /\ lock_ttl' = [lock_ttl EXCEPT ![a] = LockTTL]
            /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "locked"]
       ELSE \* Lock held by another gateway — stay in "locking" and retry.
            UNCHANGED <<lock_holder, lock_ttl, gw_phase>>
    /\ UNCHANGED <<dedup_set, dedup_ttl, exec_in_window, clock>>

\* The dedup CAS and the provider execution, atomically (see scope note):
\*   is_new = check_and_set(key); if is_new { execute() }.
\* First writer claims the key (starting/refreshing its TTL window) and
\* executes; any later gateway whose key is already set is deduplicated.
ClaimAndExecute(g, a) ==
    /\ gw_phase[<<g, a>>] = "locked"
    /\ LET k == KeyOf(a) IN
       IF dedup_set[k] = EMPTY
       THEN /\ dedup_set' = [dedup_set EXCEPT ![k] = "set"]
            /\ dedup_ttl' = [dedup_ttl EXCEPT ![k] = DedupTTL]
            /\ exec_in_window' = [exec_in_window EXCEPT ![k] = exec_in_window[k] + 1]
            /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "auditing"]
       ELSE /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "auditing"]
            /\ UNCHANGED <<dedup_set, dedup_ttl, exec_in_window>>
    /\ UNCHANGED <<lock_holder, lock_ttl, clock>>

\* Gateway writes the audit record.
Audit(g, a) ==
    /\ gw_phase[<<g, a>>] = "auditing"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "releasing"]
    /\ UNCHANGED <<lock_holder, lock_ttl, dedup_set, dedup_ttl, exec_in_window, clock>>

\* Gateway releases the dispatch lock (only if it still holds it; the lock may
\* already have been cleared by TTL expiry).
ReleaseLock(g, a) ==
    /\ gw_phase[<<g, a>>] = "releasing"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "done"]
    /\ IF lock_holder[a] = g
       THEN /\ lock_holder' = [lock_holder EXCEPT ![a] = NOBODY]
            /\ lock_ttl' = [lock_ttl EXCEPT ![a] = 0]
       ELSE UNCHANGED <<lock_holder, lock_ttl>>
    /\ UNCHANGED <<dedup_set, dedup_ttl, exec_in_window, clock>>

\* Gateway returns to idle, ready to dispatch again.
ReturnToIdle(g, a) ==
    /\ gw_phase[<<g, a>>] = "done"
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "idle"]
    /\ UNCHANGED <<lock_holder, lock_ttl, dedup_set, dedup_ttl, exec_in_window, clock>>

\* Gateway crashes mid-pipeline. The lock is NOT released; it expires via TTL.
Crash(g, a) ==
    /\ gw_phase[<<g, a>>] \in {"locked", "auditing", "releasing"}
    /\ gw_phase' = [gw_phase EXCEPT ![<<g, a>>] = "done"]
    /\ UNCHANGED <<lock_holder, lock_ttl, dedup_set, dedup_ttl, exec_in_window, clock>>

\* Time advances: expire the dispatch lock and the dedup key. When a dedup key
\* expires, its execution-window counter resets — a subsequent dispatch opens a
\* fresh window and may legitimately execute again.
ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ lock_ttl' = [a \in Actions |->
            IF lock_ttl[a] > 0 THEN lock_ttl[a] - 1 ELSE 0]
    /\ lock_holder' = [a \in Actions |->
            IF lock_ttl[a] = 1 THEN NOBODY ELSE lock_holder[a]]
    /\ dedup_ttl' = [k \in DedupKeys |->
            IF dedup_ttl[k] > 0 THEN dedup_ttl[k] - 1 ELSE 0]
    /\ dedup_set' = [k \in DedupKeys |->
            IF dedup_ttl[k] = 1 THEN EMPTY ELSE dedup_set[k]]
    /\ exec_in_window' = [k \in DedupKeys |->
            IF dedup_ttl[k] = 1 THEN 0 ELSE exec_in_window[k]]
    /\ UNCHANGED gw_phase

\* -----------------------------------------------------------------------
\* Next-state relation
\* -----------------------------------------------------------------------
Next ==
    \/ \E g \in Gateways, a \in Actions :
        \/ StartDispatch(g, a)
        \/ TryLock(g, a)
        \/ ClaimAndExecute(g, a)
        \/ Audit(g, a)
        \/ ReleaseLock(g, a)
        \/ ReturnToIdle(g, a)
        \/ Crash(g, a)
    \/ ClockTick

vars == <<gw_phase, lock_holder, lock_ttl, dedup_set, dedup_ttl, exec_in_window, clock>>

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY PROPERTIES
\* =======================================================================

(*
 * THE KEY INVARIANT: within any dedup-TTL window, at most one execution per
 * dedup key — even across concurrent gateways and lock-TTL expiry. Guaranteed
 * by the atomic check_and_set, not by the dispatch lock.
 *)
DedupSafety ==
    \A k \in DedupKeys : exec_in_window[k] <= 1

==========================================================================
