--------------------------- MODULE StateStore ----------------------------
(*
 * Models the Acteon StateStore trait used by all backends (Redis, Postgres,
 * memory, etc.).
 *
 * This is an *abstract* model — it captures the atomicity guarantees that all
 * backends must provide, without modelling network partitions or latency.
 *
 * Operators defined here are *macros* (not actions) — they are meant to be
 * called from within a caller's action to build composite steps.
 *)
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS
    KEYS,       \* Set of possible state keys
    NONE        \* Sentinel value for "key not present"

(* ------------------------------------------------------------------ *)
(* Helpers used by callers.  These are pure functions, not actions.    *)
(* ------------------------------------------------------------------ *)

\* Read a value from the store; returns NONE if absent.
Read(store, key) ==
    IF key \in DOMAIN store THEN store[key] ELSE NONE

\* Write a value, returning the updated store.
Write(store, key, value) ==
    [store EXCEPT ![key] = value]

\* Atomic check-and-set: sets key only if currently NONE.
\* Returns <<new_store, TRUE/FALSE>>.
CheckAndSet(store, key, value) ==
    IF Read(store, key) = NONE
    THEN <<Write(store, key, value), TRUE>>
    ELSE <<store, FALSE>>

\* Atomic increment: treats NONE as 0.
\* Returns <<new_store, new_value>>.
Increment(store, key, delta) ==
    LET current == IF Read(store, key) = NONE THEN 0 ELSE Read(store, key)
        new_val == current + delta
    IN <<Write(store, key, new_val), new_val>>

==========================================================================
