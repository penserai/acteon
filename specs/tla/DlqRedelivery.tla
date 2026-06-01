----------------------------- MODULE DlqRedelivery -----------------------------
(*
 * TLA+ specification of Acteon's dead-letter queue push/drain under concurrency.
 *
 * Models crates/executor/src/dlq.rs (DeadLetterQueue, the DeadLetterSink impl):
 *   - push(action, error, attempts): locks the Mutex<Vec<DeadLetterEntry>> and
 *     APPENDS one entry (dlq.rs:84). Called by send_to_dlq / push_to_dlq when an
 *     action exhausts its retries (executor.rs:77/178/209/226).
 *   - drain() -> Vec<DeadLetterEntry>: locks the SAME Mutex and runs
 *     std::mem::take(&mut *guard) (dlq.rs:91-92) — it RETURNS all entries AND
 *     EMPTIES the queue in a SINGLE critical section (the take reads out the Vec
 *     and leaves Vec::new() behind, atomically, under one lock acquisition).
 *     Confirmed by the tests drain_returns_all_entries_and_empties_queue (the Vec
 *     is non-empty, then dlq.is_empty()) and push_increments_len.
 *   drain() is consumed by an operator-facing path (POST /v1/dlq/drain, the CLI,
 *   the MCP tool), which processes / resubmits each drained entry once; the
 *   re-dispatch of a resubmitted entry then flows through the gateway dispatch
 *   dedup, modeled separately in DispatchDedup.tla.
 *
 * The single load-bearing mechanism is the ATOMICITY of drain's take+clear:
 * because std::mem::take reads out the Vec AND clears it in one Mutex critical
 * section, no two drains can both observe the same entry, and no push can be lost
 * "between" a drain's read and its clear. push and drain are each one critical
 * section under the same Mutex, modeled here as one atomic TLA+ action apiece
 * (TLA+ action atomicity is exactly the mutual exclusion the Mutex provides — so
 * there is no separate lock variable to model; that would be dead state).
 *
 * Protocol. Producers PUSH failed entries (one append per call); one or more
 * drainers DRAIN (take-all) and hand each drained entry to its consumer exactly
 * once. A push either lands before a take (included in that drain) or after it
 * (left queued for the next drain) — never dropped. A take empties the queue, so
 * a second concurrent drain gets only the empty remainder.
 *
 * Verified (over every interleaving of concurrent producers and drainers):
 *   - RedeliveredAtMostOnce: no entry is drained / redelivered more than once.
 *     The atomic take empties the queue as it returns the batch, so two drains
 *     never both return the same entry.
 *   - NoLostEntry: when the system has quiesced (every entry pushed, nothing
 *     queued, no drainer mid-flight), every pushed entry has been drained — a
 *     push concurrent with a drain is never dropped.
 *   - QueuedConsistent: a queued entry has been pushed and not yet drained; a
 *     drained entry is not still queued (the take cleared it).
 *
 * Negative check: revert the ATOMIC take — split drain() into a READ step that
 * snapshots the queued entries and hands them to the consumer WITHOUT clearing,
 * and a separate CLEAR step (two critical sections instead of std::mem::take's
 * one). The one atomicity break manifests two ways, confirming it is genuinely
 * load-bearing:
 *   - two drainers both READ the same queued entry before either CLEARs, so it is
 *     redelivered twice -> RedeliveredAtMostOnce violated (rd_count[e] = 2); and
 *   - a PUSH that lands between a drainer's READ and its CLEAR is wiped by the
 *     unconditional clear -> NoLostEntry violated (the entry is stranded).
 *
 * SCOPE. Models a single DeadLetterQueue. Entries are an abstract finite set;
 * their payload (action, error, attempts, timestamp) and FIFO order within the
 * Vec are not modeled — only the set membership the safety properties depend on.
 * The downstream re-dispatch dedup of each resubmitted entry is out of scope and
 * lives in DispatchDedup.tla.
 *
 * Run with:
 *   java -jar tla2tools.jar -config DlqRedelivery.cfg DlqRedelivery.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Entries,   \* the distinct failed actions that may be pushed, e.g. {e1, e2}
    Drainers   \* concurrent drainers / redelivery workers, e.g. {d1, d2}

VARIABLES
    pushed,       \* SUBSET Entries: entries appended to the queue at least once
    queued,       \* SUBSET Entries: entries currently sitting in the Vec
    redelivered,  \* SUBSET Entries: entries drained (taken) and handed to a consumer
    rd_count,     \* [Entries -> 0..3]: times each entry has been drained
    d_phase       \* [Drainers -> {"idle","redelivering"}]

vars == <<pushed, queued, redelivered, rd_count, d_phase>>

TypeOK ==
    /\ pushed \subseteq Entries
    /\ queued \subseteq Entries
    /\ redelivered \subseteq Entries
    /\ rd_count \in [Entries -> 0..3]
    /\ d_phase \in [Drainers -> {"idle", "redelivering"}]

Init ==
    /\ pushed = {}
    /\ queued = {}
    /\ redelivered = {}
    /\ rd_count = [e \in Entries |-> 0]
    /\ d_phase = [d \in Drainers |-> "idle"]

\* push(action, error, attempts): a producer locks the Mutex and APPENDS one
\* entry (dlq.rs:84). Modeled as one atomic critical section. Each distinct failed
\* action is pushed once per window — an entry is enqueued only if not already
\* pushed (a re-failure after drain is a fresh window, modeled by Recycle).
\* `pushed` records the enqueue; `queued` is the live Vec contents.
Push(e) ==
    /\ e \notin pushed
    /\ queued' = queued \cup {e}
    /\ pushed' = pushed \cup {e}
    /\ UNCHANGED <<redelivered, rd_count, d_phase>>

\* drain() -> Vec: a drainer locks the SAME Mutex and runs std::mem::take — it
\* RETURNS all queued entries AND EMPTIES the queue in ONE critical section
\* (dlq.rs:91-92). Modeled as a single atomic action: read the current `queued`
\* snapshot, hand every entry in it to the consumer (mark redelivered, bump each
\* entry's count), and clear `queued` — all in one indivisible step. Because the
\* read-out and the clear are one atomic step, a drainer that runs next sees
\* queued = {} and takes nothing, so no entry is drained twice.
Drain(d) ==
    /\ d_phase[d] = "idle"
    /\ redelivered' = redelivered \cup queued
    /\ rd_count' = [e \in Entries |->
                       IF e \in queued THEN rd_count[e] + 1 ELSE rd_count[e]]
    /\ queued' = {}
    /\ d_phase' = [d_phase EXCEPT ![d] = "redelivering"]
    /\ UNCHANGED pushed

\* The drainer finishes handing off its taken batch and is ready to drain again.
\* (The re-dispatch of each entry flows through the gateway dedup — DispatchDedup.
\* tla — and is out of scope here.) Releases the worker back to idle.
DrainDone(d) ==
    /\ d_phase[d] = "redelivering"
    /\ d_phase' = [d_phase EXCEPT ![d] = "idle"]
    /\ UNCHANGED <<pushed, queued, redelivered, rd_count>>

\* Recycle: once everything pushed has been drained, the queue is empty, and no
\* drainer is mid-flight, reset the per-window history so the system CYCLES (fresh
\* failures can be pushed again). No benign terminal deadlock under -deadlock.
\* rd_count resets so the at-most-once bound is per-window.
Recycle ==
    /\ pushed # {}
    /\ queued = {}
    /\ pushed \subseteq redelivered
    /\ \A d \in Drainers : d_phase[d] = "idle"
    /\ pushed' = {}
    /\ redelivered' = {}
    /\ rd_count' = [e \in Entries |-> 0]
    /\ UNCHANGED <<queued, d_phase>>

Next ==
    \/ \E e \in Entries : Push(e)
    \/ \E d \in Drainers : Drain(d) \/ DrainDone(d)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* No entry is drained / redelivered more than once. The atomic take-under-the-
\* Mutex empties the queue as it returns the batch, so two concurrent drains
\* never both return the same entry.
RedeliveredAtMostOnce == \A e \in Entries : rd_count[e] <= 1

\* When the window has quiesced — nothing queued and no drainer mid-flight —
\* every pushed entry has been drained. A push that races a drain is never
\* dropped: the take and the (would-be) concurrent append cannot interleave
\* within one critical section, so the entry lands in that drain or the next one.
NoLostEntry ==
    (queued = {} /\ \A d \in Drainers : d_phase[d] = "idle")
        => (pushed \subseteq redelivered)

\* A queued entry has been pushed and not yet drained; a drained entry is not
\* still queued (the take cleared it).
QueuedConsistent ==
    /\ queued \subseteq pushed
    /\ queued \cap redelivered = {}

============================================================================
