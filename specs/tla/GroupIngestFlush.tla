--------------------------- MODULE GroupIngestFlush ---------------------------
(*
 * TLA+ specification of Acteon's event-group INGEST path racing the FLUSH —
 * specifically the Notified -> Pending RE-ARM on a late ingest, which keeps a
 * newly-ingested event from being stranded in an already-flushed group.
 *
 * Models:
 *   - crates/gateway/src/group_manager.rs :: GroupManager::add_to_group (~132).
 *     The in-memory branch (under the groups write-lock) has three cases:
 *       * NEW group        -> create Pending, notify_at = now + group_wait, add event.
 *       * EXISTING PENDING  -> add the event; notify_at is UNCHANGED (the debounce
 *                              window is NOT extended by a later event).
 *       * EXISTING NOTIFIED -> the group transitions BACK to Pending (line 184:
 *                              `group.state = GroupState::Pending`), notify_at is
 *                              RE-ARMED to now + group_interval (line 187), THEN the
 *                              event is added (line 192). A late event RE-OPENS the
 *                              already-flushed group for a fresh notification.
 *   - crates/gateway/src/group_manager.rs :: get_ready_groups (~332): a group is
 *     flushable when notify_at <= now AND state = Pending (Notified is only re-
 *     flushable for persistent groups via the repeat interval; the ingest re-arm
 *     models the path that returns a group to Pending so get_ready_groups picks it
 *     up again). flush_group (modeled in MessageBus.tla) flips a ready Pending
 *     group -> Notified; the events it then carried become notified.
 *   - crates/core/src/group.rs :: GroupState { Pending | Notified | Resolved } and
 *     EventGroup { state, events, notify_at }.
 *
 * Protocol (single group, events as counters). Events are INGESTED over time and
 * the group is Pending or Notified:
 *   - Ingest: add an event (count it as ingested-and-not-yet-notified). If the
 *     group is Notified, RE-ARM it back to Pending (the late-event reopen). If
 *     Pending, just add (window unchanged).
 *   - BecomeReady: the notify_at deadline passes while Pending (group flushable).
 *   - Flush (Pending AND ready): group -> Notified; every event currently in the
 *     group (the not-yet-notified ones) becomes notified.
 * A pending-but-not-yet-notified event must always keep the group on a path to be
 * flushed — i.e. the group must be (or return to) Pending so get_ready_groups can
 * flush it.
 *
 * The RE-ARM (Notified -> Pending on ingest into a Notified group) is the load-
 * bearing mechanism the negative test reverts.
 *
 * Verified (over every interleaving of ingest, deadline-expiry and flush):
 *   - NoStrandedEvent: an event ingested but not yet notified implies the group
 *     is Pending (and thus on a path get_ready_groups will flush). The re-arm is
 *     exactly what prevents a late ingest from sitting in a Notified group that
 *     get_ready_groups never flushes again.
 *   - QuiesceNotifiesAll: when no event is in flight unnotified, the group has
 *     left Pending iff there is nothing left to notify — i.e. a Notified group has
 *     zero unnotified events (everything ingested got notified).
 *
 * Negative check: revert the re-arm — make Ingest into a NOTIFIED group leave the
 * group Notified (drop `state' = "pending"` / `ready' = FALSE`) while still
 * counting the new event as unnotified. The new event is then stranded:
 * unnotified > 0 while state = "notified", which get_ready_groups never flushes ->
 * NoStrandedEvent is violated. Confirmed: the BUGGY variant reports the violation;
 * the real spec is green.
 *
 * The invariant is anchored on GROUND-TRUTH counters (unnotified, the integer
 * count of ingested-not-yet-notified events) and the raw `state`, NOT on any
 * operator the Ingest/Flush actions also use to make their decision — so a buggy
 * re-arm cannot mutate the check into agreeing with itself (no self-masking).
 *
 * SCOPE. Single group. notify_at is ABSTRACTED to a `ready` boolean (the
 * get_ready_groups flush precondition: notify_at <= now while Pending) rather than
 * a concrete clock; the group_wait vs group_interval distinction (both just set a
 * future notify_at) collapses to "ready becomes false again on re-arm". Events are
 * COUNTERS (ingested / notified / unnotified), not payloads — max_group_size FIFO
 * drop, fingerprint dedup, persistence/recovery, encryption, and the persistent
 * repeat-interval re-fire of a Notified group are out of scope; this spec isolates
 * the ingest/flush state-machine race and the late-ingest re-arm. The Resolved
 * terminal state is omitted (the modeled lifecycle cycles Pending<->Notified).
 *
 * Run with:
 *   java -jar tla2tools.jar -config GroupIngestFlush.cfg GroupIngestFlush.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    MaxEvents  \* state-space bound on total ingested events per cycle (small, e.g. 3)

ASSUME MaxEvents \in 1..5

VARIABLES
    state,       \* "pending" | "notified": the EventGroup.state
    ready,       \* BOOLEAN: notify_at <= now while Pending (flush precondition)
    ingested,    \* 0..MaxEvents: total events ingested this cycle
    notified,    \* 0..MaxEvents: events that have been carried through a flush
    unnotified   \* 0..MaxEvents: ingested events not yet notified (ground-truth)

vars == <<state, ready, ingested, notified, unnotified>>

TypeOK ==
    /\ state \in {"pending", "notified"}
    /\ ready \in BOOLEAN
    /\ ingested \in 0..MaxEvents
    /\ notified \in 0..MaxEvents
    /\ unnotified \in 0..MaxEvents
    /\ ingested = notified + unnotified   \* the counters partition every ingested event

Init ==
    /\ state = "pending"
    /\ ready = FALSE
    /\ ingested = 0
    /\ notified = 0
    /\ unnotified = 0

\* Ingest an event (add_to_group). Always counts the event as ingested-and-not-yet-
\* notified. The state handling mirrors add_to_group's three cases collapsed onto a
\* single group:
\*   - NOTIFIED group: RE-ARM back to Pending (group.state = Pending), and reset the
\*     flush deadline (notify_at = now + interval => not yet ready). THIS is the fix.
\*   - PENDING group: just add; notify_at (and hence `ready`) is unchanged.
Ingest ==
    /\ ingested < MaxEvents
    /\ ingested' = ingested + 1
    /\ unnotified' = unnotified + 1
    /\ IF state = "notified"
       THEN /\ state' = "pending"   \* Notified -> Pending re-arm (group_manager.rs:184)
            /\ ready' = FALSE        \* notify_at = now + interval (group_manager.rs:187)
       ELSE /\ UNCHANGED state       \* Pending: add only, window unchanged
            /\ UNCHANGED ready
    /\ UNCHANGED notified

\* The notify_at deadline passes while the group is Pending: get_ready_groups now
\* returns this group (notify_at <= now AND state = Pending).
BecomeReady ==
    /\ state = "pending"
    /\ ready = FALSE
    /\ ready' = TRUE
    /\ UNCHANGED <<state, ingested, notified, unnotified>>

\* Flush a ready Pending group (flush_group): state -> Notified, and every event
\* currently held (the unnotified ones) is delivered by the notification.
Flush ==
    /\ state = "pending"
    /\ ready = TRUE
    /\ state' = "notified"
    /\ ready' = FALSE
    /\ notified' = notified + unnotified
    /\ unnotified' = 0
    /\ UNCHANGED ingested

\* Recycle: a quiesced, fully-notified group resets so the system CYCLES (the group
\* key is reused for a fresh batching window). Only fires when nothing is pending
\* unnotified and the group has been flushed at least once — a clean boundary.
Recycle ==
    /\ state = "notified"
    /\ unnotified = 0
    /\ ingested > 0
    /\ state' = "pending"
    /\ ready' = FALSE
    /\ ingested' = 0
    /\ notified' = 0
    /\ unnotified' = 0

Next ==
    \/ Ingest
    \/ BecomeReady
    \/ Flush
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* GROUND-TRUTH oracle, independent of the Ingest/Flush decision logic: any event
\* ingested but not yet notified implies the group is Pending — and therefore on a
\* path that get_ready_groups (Pending /\ notify_at <= now) will eventually flush.
\* The Notified -> Pending re-arm on a late ingest is exactly what keeps this true;
\* without it, a late event sits unnotified in a Notified group that
\* get_ready_groups never re-flushes (the strand the negative test exhibits).
NoStrandedEvent == (unnotified > 0) => (state = "pending")

\* When the group has quiesced (no unnotified event in flight) and has been
\* notified, every ingested event was delivered — notified accounts for all of
\* them. Ties the abstract state to the concrete delivery count, independent of
\* the re-arm: a Notified group with nothing pending has notified == ingested.
QuiesceNotifiesAll == (state = "notified" /\ unnotified = 0) => (notified = ingested)

============================================================================
