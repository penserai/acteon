------------------------- MODULE DistributedLock -------------------------
(*
 * Models Acteon's DistributedLock trait.
 *
 * Key characteristics modelled:
 *   - Mutual exclusion: at most one holder per lock name
 *   - TTL-based expiry: if a holder crashes, the lock expires
 *   - Try-acquire semantics: non-blocking; returns immediately
 *
 * The module exposes operator macros (not actions) so that specs can
 * compose lock acquisition with other state changes atomically where
 * the real code uses a single distributed transaction.
 *)
EXTENDS Integers, FiniteSets

CONSTANTS
    PROCESSES,      \* Set of processes (gateway instances / workers)
    LOCK_NAMES,     \* Set of lock name strings
    MAX_TTL,        \* Maximum TTL ticks before a lock auto-expires
    NOBODY          \* Sentinel: no one holds the lock

(* ------------------------------------------------------------------ *)
(* State variables — the caller spec must declare these.              *)
(*                                                                     *)
(*   lock_holder : [LOCK_NAMES -> PROCESSES \cup {NOBODY}]            *)
(*   lock_ttl    : [LOCK_NAMES -> 0..MAX_TTL]                        *)
(* ------------------------------------------------------------------ *)

\* Type invariant for lock state.
LockTypeOK(lock_holder, lock_ttl) ==
    /\ lock_holder \in [LOCK_NAMES -> PROCESSES \cup {NOBODY}]
    /\ lock_ttl    \in [LOCK_NAMES -> 0..MAX_TTL]

\* Initial state: all locks free.
LockInit(lock_holder, lock_ttl) ==
    /\ lock_holder = [n \in LOCK_NAMES |-> NOBODY]
    /\ lock_ttl    = [n \in LOCK_NAMES |-> 0]

(* ------------------------------------------------------------------ *)
(* Operator macros                                                     *)
(* ------------------------------------------------------------------ *)

\* Try to acquire.  Returns <<new_holder, new_ttl, acquired>>.
TryAcquire(lock_holder, lock_ttl, process, name, ttl) ==
    IF lock_holder[name] = NOBODY
    THEN << [lock_holder EXCEPT ![name] = process],
            [lock_ttl EXCEPT ![name] = ttl],
            TRUE >>
    ELSE << lock_holder, lock_ttl, FALSE >>

\* Release a lock (only if held by this process).
\* Returns <<new_holder, new_ttl>>.
Release(lock_holder, lock_ttl, process, name) ==
    IF lock_holder[name] = process
    THEN << [lock_holder EXCEPT ![name] = NOBODY],
            [lock_ttl EXCEPT ![name] = 0] >>
    ELSE << lock_holder, lock_ttl >>

\* Tick: decrement all TTLs; expire locks that reach 0.
\* Returns <<new_holder, new_ttl>>.
Tick(lock_holder, lock_ttl) ==
    LET new_ttl == [n \in LOCK_NAMES |->
                        IF lock_ttl[n] > 0 THEN lock_ttl[n] - 1
                        ELSE 0]
        new_holder == [n \in LOCK_NAMES |->
                        IF new_ttl[n] = 0 THEN NOBODY
                        ELSE lock_holder[n]]
    IN << new_holder, new_ttl >>

(* ------------------------------------------------------------------ *)
(* Safety property                                                     *)
(* ------------------------------------------------------------------ *)

\* At most one holder per lock at any time.
MutualExclusion(lock_holder) ==
    \A n \in LOCK_NAMES :
        Cardinality({p \in PROCESSES : lock_holder[n] = p}) <= 1

==========================================================================
