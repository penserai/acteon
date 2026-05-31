----------------------------- MODULE MessageBus -----------------------------
(*
 * TLA+ specification of Acteon's grouped-notification delivery on the message
 * bus (the agentic bus / subscription stream).
 *
 * Models crates/gateway/src/group_manager.rs. Events are coalesced into an
 * EventGroup that moves through Pending -> Notified -> Resolved. When a group's
 * notify_at deadline passes it becomes "ready"; one or more flush workers (the
 * background reaper, across replicas) then call flush_group(), whose body runs
 * under the group write-lock:
 *
 *     let mut groups = self.groups.write();          // acquire write-lock
 *     if group.state == Pending {                    // READ   (under the lock)
 *         group.state = Notified;                    // WRITE  (under the lock)
 *         Some(group)        // -> emit the notification onto the bus, ONCE
 *     } else { None }        // already flushed -> emit nothing
 *
 * The notification side-effect (push onto the subscription stream) fires only
 * for the flusher that observed Pending. Correctness rests on the read and the
 * write being a single critical section: the write-lock is held across BOTH, so
 * no other flusher can interleave between a flusher's read and its write. This
 * spec models the read and the write as SEPARATE steps (they are separate
 * instructions) so the lock is load-bearing, not an artifact of step-atomicity.
 *
 * Verified (over every interleaving of concurrent flush workers / replicas and
 * ongoing event ingestion):
 *   - NotifyOnce: a group's notification is emitted at most once per window.
 *   - NotifiedConsistent: an emitted notification implies the group has left
 *     Pending (no emit without the state transition).
 *
 * Negative check: drop the lock (remove the `lock = NOBODY` precondition on
 * Acquire and the `lock = f` guards on Read/Write). Two flushers then read
 * Pending before either writes Notified, and TLC finds the double-notify —
 * notify_count reaches 2.
 *
 * Run with:
 *   java -jar tla2tools.jar -config MessageBus.cfg MessageBus.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Flushers,  \* concurrent flush workers / replicas, e.g. {f1, f2}
    NOBODY     \* sentinel: group write-lock free

VARIABLES
    g_state,       \* "pending" | "notified" | "resolved"
    notify_count,  \* notifications emitted onto the bus for the current window
    ready,         \* BOOLEAN: notify_at deadline reached (group is flushable)
    lock,          \* Flushers \cup {NOBODY}: holder of the group write-lock
    f_phase,       \* [Flushers -> {"idle","reading","writing"}]
    f_decided      \* [Flushers -> BOOLEAN]: read saw Pending -> will emit

TypeOK ==
    /\ g_state \in {"pending", "notified", "resolved"}
    /\ notify_count \in 0..3
    /\ ready \in BOOLEAN
    /\ lock \in Flushers \cup {NOBODY}
    /\ f_phase \in [Flushers -> {"idle", "reading", "writing"}]
    /\ f_decided \in [Flushers -> BOOLEAN]

Init ==
    /\ g_state = "pending"
    /\ notify_count = 0
    /\ ready = FALSE
    /\ lock = NOBODY
    /\ f_phase = [f \in Flushers |-> "idle"]
    /\ f_decided = [f \in Flushers |-> FALSE]

\* The notify_at deadline passes — the group becomes flushable. (A late event
\* that extends the debounce window only delays this transition.)
BecomeReady ==
    /\ g_state = "pending"
    /\ ready = FALSE
    /\ ready' = TRUE
    /\ UNCHANGED <<g_state, notify_count, lock, f_phase, f_decided>>

\* Acquire the group write-lock. `lock = NOBODY` models parking_lot
\* RwLock::write(): a single holder at a time. Begins the critical section.
Acquire(f) ==
    /\ f_phase[f] = "idle"
    /\ ready = TRUE
    /\ lock = NOBODY
    /\ lock' = f
    /\ f_phase' = [f_phase EXCEPT ![f] = "reading"]
    /\ UNCHANGED <<g_state, notify_count, ready, f_decided>>

\* READ under the lock: decide whether to emit, based on the current state.
Read(f) ==
    /\ f_phase[f] = "reading"
    /\ lock = f
    /\ f_decided' = [f_decided EXCEPT ![f] = (g_state = "pending")]
    /\ f_phase' = [f_phase EXCEPT ![f] = "writing"]
    /\ UNCHANGED <<g_state, notify_count, ready, lock>>

\* WRITE under the lock: if the read saw Pending, flip to Notified and emit the
\* single notification; otherwise (already Notified) emit nothing — the Some/None
\* of flush_group. Release the lock, ending the critical section.
Write(f) ==
    /\ f_phase[f] = "writing"
    /\ lock = f
    /\ IF f_decided[f]
       THEN /\ g_state' = "notified"
            /\ notify_count' = notify_count + 1
       ELSE UNCHANGED <<g_state, notify_count>>
    /\ lock' = NOBODY
    /\ f_phase' = [f_phase EXCEPT ![f] = "idle"]
    /\ UNCHANGED <<ready, f_decided>>

\* The grouped incident resolves after notification.
Resolve ==
    /\ g_state = "notified"
    /\ lock = NOBODY
    /\ g_state' = "resolved"
    /\ UNCHANGED <<notify_count, ready, lock, f_phase, f_decided>>

\* Next grouping window opens (group key reused). Clean boundary: no in-flight
\* flusher. Resets the notification counter for the fresh window.
Recycle ==
    /\ g_state = "resolved"
    /\ lock = NOBODY
    /\ \A f \in Flushers : f_phase[f] = "idle"
    /\ g_state' = "pending"
    /\ notify_count' = 0
    /\ ready' = FALSE
    /\ f_decided' = [f \in Flushers |-> FALSE]
    /\ UNCHANGED <<lock, f_phase>>

Next ==
    \/ BecomeReady
    \/ \E f \in Flushers : Acquire(f) \/ Read(f) \/ Write(f)
    \/ Resolve
    \/ Recycle

vars == <<g_state, notify_count, ready, lock, f_phase, f_decided>>
Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* The group's notification is emitted onto the bus at most once per window,
\* despite concurrent flush workers and replicas.
NotifyOnce == notify_count <= 1

\* No notification is emitted without the state having advanced past Pending.
NotifiedConsistent == (notify_count >= 1) => (g_state \in {"notified", "resolved"})

============================================================================
