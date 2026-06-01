----------------------------- MODULE StreamReplay -----------------------------
(*
 * TLA+ specification of Acteon's SSE reconnect catch-up: Last-Event-ID replay
 * stitched to the live broadcast tail with no gap and no duplicate.
 *
 * Models crates/server/src/api/stream.rs (the `stream` handler, ~148..230, and
 * `replay_from_audit` ~235, `make_event_stream` ~352). A reconnecting client
 * sends Last-Event-ID = k. The handler runs, IN THIS ORDER:
 *   step 4 (~199): rx = stream_tx().subscribe()   -- subscribe to the live
 *          broadcast channel BEFORE the audit query. From this instant the
 *          broadcast buffers every event sent onto the channel.
 *   step 5 (~211/235): replay_from_audit(audit, k, ...) -- query the audit store
 *          for records from k's timestamp (clamped to MAX_REPLAY_WINDOW),
 *          reverse to chronological, SKIP the record with id == k (the client
 *          already has it), and return (replay_events, last_replayed_id).
 *   step 6 (~218/352): make_event_stream(rx, ..., last_replayed_id) -- the live
 *          tail, which SUPPRESSES (`&event.id <= last_id` -> None, ~369) any
 *          event at or below last_replayed_id, then chains replay ++ live (~222).
 *
 * How a produced event reaches the two surfaces (gateway.rs:613..639 fixes the
 * intra-event order): the gateway first WRITES the audit record (emit_audit_record
 * ~625) and only THEN BROADCASTS the stream event (stream_tx.send ~639). So per
 * event: audit-write happens-before broadcast. An event is in the REPLAY page iff
 * its audit-write preceded the client's query; it is in the LIVE buffer iff its
 * broadcast followed the client's subscribe.
 *
 * The subtle correctness, and why subscribe-BEFORE-query is load-bearing:
 *   - subscribe first => any event broadcast after `now` is captured on rx and
 *     delivered after the replay. An event whose audit-write lands DURING the
 *     replay query (so it is NOT in the page) still had its broadcast after the
 *     earlier subscribe, so the live tail catches it -> NoGap.
 *   - the last_replayed_id cursor => an event present in BOTH the replay page and
 *     the live buffer (audit-written before the query AND broadcast after the
 *     subscribe) is emitted once via replay and SUPPRESSED on the live side
 *     (id <= last_replayed_id) -> NoDuplicate.
 * The two mechanisms are JOINTLY load-bearing and negative-tested separately.
 *
 * Verified (over every interleaving of the concurrent producer with the client's
 * subscribe and query steps, for every reconnect cursor k):
 *   - NoGap: once the client has switched to the live tail, every event with
 *     id > k is delivered AT LEAST once (via replay or via live) -- nothing
 *     between the cursor and the live stream is lost.
 *   - NoDuplicate: no event id is delivered to the client more than once.
 *
 * Negative check (TWO independent fixes, each separately reverted):
 *   (1) subscribe-before-replay: set SubscribeBeforeQuery = FALSE in the .cfg so
 *       the subscribe happens AFTER the query (Query then Subscribe). An event
 *       whose audit-write lands after the query (not in the page) and whose
 *       broadcast lands before the late subscribe (not on rx) is in NEITHER
 *       surface -> NoGap violated.
 *   (2) cursor dedup: set CursorDedup = FALSE so the live tail does NOT suppress
 *       events with id <= last_replayed_id. An event that is both in the replay
 *       page and the live buffer is delivered twice -> NoDuplicate violated.
 *
 * SCOPE. Abstracts UUIDv7 ids to the integers 1..MaxId (monotone, lexicographic =
 * numeric), the broadcast channel to a per-event "captured on rx" flag, and the
 * audit store to a per-event "durably written" flag; the MAX_REPLAY_WINDOW clamp,
 * tenant/query filtering, lagged backpressure, and keep-alive pings are out of
 * scope. The intra-event audit-write-before-broadcast order, subscribe-before-
 * query order, the id==k skip, and the last_replayed_id suppression are modeled
 * faithfully.
 *
 * Run with:
 *   java -jar tla2tools.jar -config StreamReplay.cfg StreamReplay.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    MaxId,                  \* highest event id the producer can append (e.g. 3)
    SubscribeBeforeQuery,   \* TRUE = faithful (subscribe at step 4, before query)
    CursorDedup,            \* TRUE = faithful (live tail suppresses id <= cursor)
    NOBODY                  \* sentinel: cursor / Last-Event-ID not yet set

Ids == 1..MaxId

VARIABLES
    appended,    \* SUBSET Ids: ids the producer has created (monotone next id)
    written,     \* SUBSET Ids: ids whose audit record is durably written
    broadcast,   \* SUBSET Ids: ids that have been sent onto the live channel
    rxbuf,       \* SUBSET Ids: events captured on the client's broadcast rx
                 \*   (an event lands here iff broadcast AFTER the subscribe)
    cursor,      \* the reconnect Last-Event-ID k (a value in 0..MaxId; 0 = none)
    subscribed,  \* BOOLEAN: client has taken rx = subscribe()  (step 4)
    queried,     \* BOOLEAN: client has run replay_from_audit    (step 5)
    last_repl,   \* last_replayed_id returned by replay (NOBODY until queried)
    live,        \* BOOLEAN: client has switched to the live tail (step 6 done)
    delivered    \* bag of ids delivered to the client, as [Ids -> Nat] multiset
                 \*   (counts > 1 catch a duplicate delivery)

vars == <<appended, written, broadcast, rxbuf, cursor, subscribed, queried,
          last_repl, live, delivered>>

\* k as an integer: 0 means "no Last-Event-ID set yet" (cursor = NOBODY).
K == IF cursor = NOBODY THEN 0 ELSE cursor

\* Highest id whose audit record was written by the time the query ran (= the
\* set the replay page draws from). The replay returns ids in `written` with
\* id > k, skipping id == k. We snapshot it at query time into `last_repl`.
ReplaySet == { i \in written : i > K }

TypeOK ==
    /\ appended \subseteq Ids
    /\ written \subseteq Ids
    /\ broadcast \subseteq Ids
    /\ rxbuf \subseteq Ids
    /\ cursor \in (Ids \cup {NOBODY})
    /\ subscribed \in BOOLEAN
    /\ queried \in BOOLEAN
    /\ last_repl \in (Ids \cup {NOBODY})
    /\ live \in BOOLEAN
    /\ delivered \in [Ids -> 0..2]

Init ==
    /\ appended = {}
    /\ written = {}
    /\ broadcast = {}
    /\ rxbuf = {}
    /\ cursor = NOBODY
    /\ subscribed = FALSE
    /\ queried = FALSE
    /\ last_repl = NOBODY
    /\ live = FALSE
    /\ delivered = [i \in Ids |-> 0]

\* ---------------------------------------------------------------------------
\* Producer (concurrent with the reconnect). The lowest un-appended id is created
\* next, so ids stay monotone. The three producer steps fire in the gateway's
\* order: append -> write audit (queryable) -> broadcast (delivered live).
\* ---------------------------------------------------------------------------

\* Create the next event id (monotone). Just establishes the id exists.
Append ==
    /\ \E i \in Ids : i \notin appended /\ (\A j \in Ids : j < i => j \in appended)
                   /\ appended' = appended \cup {i}
    /\ UNCHANGED <<written, broadcast, rxbuf, cursor, subscribed, queried,
                   last_repl, live, delivered>>

\* gateway.rs:625 -- emit_audit_record: the event becomes durably queryable.
\* Happens-before the broadcast of the SAME event.
WriteAudit(i) ==
    /\ i \in appended
    /\ i \notin written
    /\ written' = written \cup {i}
    /\ UNCHANGED <<appended, broadcast, rxbuf, cursor, subscribed, queried,
                   last_repl, live, delivered>>

\* gateway.rs:639 -- stream_tx.send: broadcast onto the live channel. Per event,
\* the audit write already happened (written guard). If the client has already
\* subscribed, the event is captured on its rx buffer; otherwise it is missed by
\* this subscriber (broadcast channels do not retroactively deliver).
Broadcast(i) ==
    /\ i \in written
    /\ i \notin broadcast
    /\ broadcast' = broadcast \cup {i}
    /\ rxbuf' = IF subscribed THEN rxbuf \cup {i} ELSE rxbuf
    /\ UNCHANGED <<appended, written, cursor, subscribed, queried,
                   last_repl, live, delivered>>

\* ---------------------------------------------------------------------------
\* Client reconnect. The cursor k is chosen once (a previously-seen id, or 0). The
\* two ordered handler steps -- subscribe (step 4) then query (step 5) -- may
\* interleave with producer steps. SubscribeBeforeQuery gates their order: TRUE is
\* the faithful subscribe-before-query; FALSE is the reverted (buggy) order.
\* ---------------------------------------------------------------------------

\* Choose the reconnect cursor k: any already-appended id (the client held it from
\* a prior session) or 0 (NOBODY = fresh stream, no Last-Event-ID). Done once.
SetCursor(k) ==
    /\ cursor = NOBODY
    /\ ~subscribed
    /\ k \in appended
    /\ cursor' = k
    /\ UNCHANGED <<appended, written, broadcast, rxbuf, subscribed, queried,
                   last_repl, live, delivered>>

\* Step 4 (~199): rx = subscribe(). Faithful order requires the query has NOT run
\* yet; the reverted order (SubscribeBeforeQuery = FALSE) requires it HAS.
Subscribe ==
    /\ ~subscribed
    /\ IF SubscribeBeforeQuery THEN ~queried ELSE queried
    /\ subscribed' = TRUE
    /\ UNCHANGED <<appended, written, broadcast, rxbuf, cursor, queried,
                   last_repl, live, delivered>>

\* Step 5 (~211/235): replay_from_audit. Query the audit page = ids already
\* written with id > k (skipping id == k, which the loop `continue`s). DELIVER the
\* replay events now, and set last_replayed_id to the max id in the page (NOBODY if
\* empty -- replay_from_audit returns the original last_event_id, but with an empty
\* page nothing was suppressed, modeled as no cursor). Faithful order requires the
\* subscribe already happened; reverted order requires it has not.
Query ==
    /\ ~queried
    /\ IF SubscribeBeforeQuery THEN subscribed ELSE ~subscribed
    /\ queried' = TRUE
    /\ LET page == ReplaySet IN
         /\ delivered' = [i \in Ids |-> IF i \in page THEN delivered[i] + 1
                                                       ELSE delivered[i]]
         /\ last_repl' = IF page = {} THEN NOBODY
                                      ELSE CHOOSE m \in page : \A j \in page : j <= m
    /\ UNCHANGED <<appended, written, broadcast, rxbuf, cursor, subscribed, live>>

\* Step 6 (~218/352): switch to the live tail. Drain the rx buffer, applying the
\* cursor dedup: with CursorDedup (faithful), suppress any buffered event with
\* id <= last_replayed_id (it was already delivered by replay); without it (the
\* reverted bug), deliver every buffered event, double-delivering the overlap.
\* After this the stream is live: each subsequently-broadcast event is delivered
\* on arrival (also under the cursor dedup) -- see DeliverLive.
GoLive ==
    /\ subscribed
    /\ queried
    /\ ~live
    /\ live' = TRUE
    /\ LET cut == IF last_repl = NOBODY THEN 0 ELSE last_repl
           pass == IF CursorDedup THEN { i \in rxbuf : i > cut } ELSE rxbuf
       IN delivered' = [i \in Ids |-> IF i \in pass THEN delivered[i] + 1
                                                    ELSE delivered[i]]
    /\ UNCHANGED <<appended, written, broadcast, rxbuf, cursor, subscribed,
                   queried, last_repl>>

\* Once live, an event broadcast AFTER GoLive arrives on the live tail and is
\* delivered immediately, subject to the same cursor suppression. (Events buffered
\* on rx BEFORE GoLive were already drained by GoLive; this is the post-switch
\* steady state. We model a freshly-broadcast id that is on rx but not yet
\* delivered as one that arrives now.)
DeliverLive(i) ==
    /\ live
    /\ i \in rxbuf
    /\ delivered[i] = 0
    /\ LET cut == IF last_repl = NOBODY THEN 0 ELSE last_repl
       IN IF (~CursorDedup) \/ i > cut
          THEN delivered' = [delivered EXCEPT ![i] = delivered[i] + 1]
          ELSE UNCHANGED delivered
    /\ UNCHANGED <<appended, written, broadcast, rxbuf, cursor, subscribed,
                   queried, last_repl, live>>

\* ---------------------------------------------------------------------------
\* Recycle: the client disconnects and reconnects afresh (a new catch-up window),
\* so the system CYCLES (no benign terminal deadlock under -deadlock). A new
\* session re-subscribes and re-queries against the same monotone log; the
\* delivery bag and the per-session handler state reset.
\* ---------------------------------------------------------------------------
Recycle ==
    /\ live
    /\ subscribed' = FALSE
    /\ queried' = FALSE
    /\ live' = FALSE
    /\ cursor' = NOBODY
    /\ last_repl' = NOBODY
    /\ rxbuf' = {}
    /\ delivered' = [i \in Ids |-> 0]
    /\ UNCHANGED <<appended, written, broadcast>>

Next ==
    \/ Append
    \/ \E i \in Ids : WriteAudit(i)
    \/ \E i \in Ids : Broadcast(i)
    \/ \E k \in Ids : SetCursor(k)
    \/ Subscribe
    \/ Query
    \/ GoLive
    \/ \E i \in Ids : DeliverLive(i)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* Set of ids that genuinely existed at reconnect-relevant time and sit strictly
\* above the cursor -- the ids the client must receive. An event "counts" once it
\* has been broadcast (its live side fired); an event only appended/written but
\* never broadcast is still in flight and not yet owed.
OwedAfterLive == { i \in broadcast : i > K }

\* The client is fully caught up: on the live tail, the producer is quiescent
\* (everything appended has been broadcast), and the live tail has drained (no
\* buffered owed event is still waiting for DeliverLive). Only at this fixpoint is
\* it meaningful to demand that every owed event has arrived; before it, an owed
\* event may legitimately still be in flight on the live tail.
CaughtUp ==
    /\ live
    /\ broadcast = appended
    /\ \A i \in OwedAfterLive : delivered[i] >= 1 \/ i \notin rxbuf

\* NoGap: once the client is caught up, every owed event id > k has been delivered
\* at least once -- via replay if it predated the live subscription, or via the
\* live tail otherwise. Nothing between the cursor and the live stream is lost.
NoGap ==
    CaughtUp => \A i \in OwedAfterLive : delivered[i] >= 1

\* NoDuplicate: no event id is ever delivered to the client more than once -- the
\* last_replayed_id cursor dedups the replay/live boundary.
NoDuplicate == \A i \in Ids : delivered[i] <= 1

===============================================================================
