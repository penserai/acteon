--------------------------- MODULE MessageDedup ---------------------------
(*
 * TLA+ specification of Acteon's A2A message idempotency (probe +
 * check_and_set + TTL).
 *
 * Models crates/gateway/src/task_engine.rs — the append_history message-apply
 * flow over N concurrent submitters of the SAME messageId:
 *
 *   append_history (~459), ordering:
 *     2. message_already_applied (~850): a READ-ONLY probe. It does a bare
 *        `state.get(dedup_key(message_id)).is_some()` and does NOT write the
 *        marker — a fast path that spares an already-applied retry the cost of
 *        the reference-graph walk. The docstring (~456) is explicit: the probe
 *        "does not burn its marker"; it is ADVISORY.
 *     4. dedup_message (~865): the REAL idempotency gate. It runs
 *            check_and_set(dedup_key(message_id), now, MESSAGE_DEDUP_TTL)
 *        and returns Ok(!inserted) — i.e. returns TRUE ("already deduped /
 *        duplicate") iff the marker ALREADY existed (check_and_set did NOT
 *        insert). The first writer inserts the marker (inserted=TRUE ->
 *        dedup_message returns FALSE -> the caller APPLIES the message);
 *        concurrent re-submissions find the marker present (inserted=FALSE ->
 *        dedup_message returns TRUE -> the caller SKIPS applying).
 *     check_and_set is atomic — the in-memory store uses dashmap's vacant/
 *     occupied Entry API (crates/state/memory/src/store.rs:138) so exactly one
 *     concurrent caller observes "vacant" and inserts. That atomicity, NOT the
 *     advisory probe, is what bounds application to once per marker window.
 *
 *   MESSAGE_DEDUP_TTL (~97) is 24h: after it expires the marker is gone and a
 *   (very late) resubmission can apply again — a fresh window, by design.
 *
 * Protocol. N submitters of one messageId race to apply it exactly once. Each
 * submitter:
 *   Probe(s):  optionally runs the read-only probe, FREEZING what it observed
 *              (probe_saw_applied[s] := marker present?). The probe is a stale
 *              advisory read: it can observe "not applied" an instant before a
 *              concurrent submitter sets the marker.
 *   Cas(s):    runs check_and_set on the marker. If the marker is absent the
 *              submitter INSERTS it (it is the first writer) -> APPLIES the
 *              message (apply_in_window += 1) and (re)starts the TTL window. If
 *              the marker is already present the submitter SKIPS — no apply.
 * A clock expires the marker (TTL): once expired the marker is absent and a
 * fresh submission opens a new window and may legitimately apply again.
 *
 * Probe and Cas are SEPARATE steps (they are separate instructions in the Rust
 * — a `get` then a `check_and_set`, with the reference-graph walk in between),
 * so a stale probe can survive a concurrent apply. The check_and_set's
 * atomicity is precisely what closes that gap.
 *
 * Verified (over every interleaving of concurrent submitters and TTL expiry):
 *   - AppliedAtMostOncePerWindow: the message is applied at most once while its
 *     dedup marker is live (apply_in_window <= 1). The atomic check_and_set —
 *     not the advisory probe — is the gate that guarantees this.
 *   - ApplyImpliesMarker: an application within the window implies the marker is
 *     set (no apply without first inserting the marker).
 *
 * Negative check: make the advisory PROBE the gate. Apply based on the frozen
 * read-only probe (Apply when probe_saw_applied[s] = FALSE) instead of on the
 * atomic check_and_set insert. Two submitters then both Probe "not applied"
 * before either applies, and BOTH apply -> AppliedAtMostOncePerWindow violated
 * (apply_in_window reaches 2). This isolates the check_and_set as independently
 * load-bearing: reverting it alone (substituting the probe) breaks safety.
 *
 * SCOPE. The probe is ADVISORY and the at-most-once bound is PER-TTL-WINDOW:
 * after the marker's TTL expires a late resubmission applies again, which is
 * correct (a new window), not a bug — exactly as DispatchDedup bounds "at most
 * once within a dedup-TTL window". This spec abstracts the reference-graph walk
 * (step 3), the parent-task-id validation (step 1), and the task CAS-mutate
 * (the actual history append) to a single Apply effect; it models the dedup
 * marker, its TTL, the read-only probe, and the atomic check_and_set faithfully.
 *
 * Run with:
 *   java -jar tla2tools.jar -config MessageDedup.cfg MessageDedup.tla
 *)
EXTENDS Integers, FiniteSets, TLC

\* -----------------------------------------------------------------------
\* Constants
\* -----------------------------------------------------------------------
CONSTANTS
    Submitters,  \* concurrent submitters of the same messageId, e.g. {s1, s2}
    DedupTTL,    \* dedup-marker TTL in ticks (MESSAGE_DEDUP_TTL, abstracted)
    MaxTime,     \* state-space bound on the clock
    EMPTY        \* sentinel: dedup marker unset

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    s_phase,           \* [Submitters -> phase] per-submitter progress
    probe_saw_applied, \* [Submitters -> BOOLEAN] what the read-only probe froze
    marker,            \* {EMPTY, "set"}: the dedup marker for the messageId
    marker_ttl,        \* 0..DedupTTL: remaining life of the marker
    apply_in_window,   \* 0..2: applications since the marker was last set
    clock

Phases == {"idle", "probed", "applied", "skipped"}

vars == <<s_phase, probe_saw_applied, marker, marker_ttl, apply_in_window, clock>>

\* -----------------------------------------------------------------------
\* Type invariant
\* -----------------------------------------------------------------------
TypeOK ==
    /\ s_phase \in [Submitters -> Phases]
    /\ probe_saw_applied \in [Submitters -> BOOLEAN]
    /\ marker \in {EMPTY, "set"}
    /\ marker_ttl \in 0..DedupTTL
    /\ apply_in_window \in 0..2
    /\ clock \in 0..MaxTime

\* -----------------------------------------------------------------------
\* Initial state
\* -----------------------------------------------------------------------
Init ==
    /\ s_phase = [s \in Submitters |-> "idle"]
    /\ probe_saw_applied = [s \in Submitters |-> FALSE]
    /\ marker = EMPTY
    /\ marker_ttl = 0
    /\ apply_in_window = 0
    /\ clock = 0

\* -----------------------------------------------------------------------
\* Actions
\* -----------------------------------------------------------------------

\* Step 2: the READ-ONLY probe. message_already_applied does a bare `get` on the
\* dedup key and does NOT write the marker. We FREEZE what it observed — this is
\* a stale advisory read: a submitter can probe "not applied" (marker = EMPTY) an
\* instant before a concurrent submitter's check_and_set sets the marker, and
\* that frozen FALSE survives the concurrent apply.
Probe(s) ==
    /\ s_phase[s] = "idle"
    /\ probe_saw_applied' = [probe_saw_applied EXCEPT ![s] = (marker = "set")]
    /\ s_phase' = [s_phase EXCEPT ![s] = "probed"]
    /\ UNCHANGED <<marker, marker_ttl, apply_in_window, clock>>

\* Step 4: dedup_message's check_and_set — the REAL idempotency gate, modeled
\* atomically (the dashmap vacant/occupied Entry API). The GATE is the marker's
\* state at THIS instant, not the (possibly stale) frozen probe:
\*   - marker EMPTY -> this submitter is the first writer: INSERT the marker,
\*     (re)start its TTL window, and APPLY the message (inserted=TRUE ->
\*     dedup_message returns FALSE -> caller applies).
\*   - marker "set" -> the marker already existed: SKIP (inserted=FALSE ->
\*     dedup_message returns TRUE -> caller returns the current task, no apply).
Cas(s) ==
    /\ s_phase[s] = "probed"
    /\ IF marker = EMPTY
       THEN /\ marker' = "set"
            /\ marker_ttl' = DedupTTL
            /\ apply_in_window' = apply_in_window + 1
            /\ s_phase' = [s_phase EXCEPT ![s] = "applied"]
       ELSE /\ s_phase' = [s_phase EXCEPT ![s] = "skipped"]
            /\ UNCHANGED <<marker, marker_ttl, apply_in_window>>
    /\ UNCHANGED <<probe_saw_applied, clock>>

\* A submitter that finished (applied or skipped) returns to idle to resubmit —
\* models retries / repeated identical submissions so the system CYCLES.
ReturnToIdle(s) ==
    /\ s_phase[s] \in {"applied", "skipped"}
    /\ s_phase' = [s_phase EXCEPT ![s] = "idle"]
    /\ probe_saw_applied' = [probe_saw_applied EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<marker, marker_ttl, apply_in_window, clock>>

\* Time advances and the marker's TTL decays. When the marker expires it becomes
\* EMPTY and the per-window apply counter resets — a subsequent submission opens
\* a FRESH window and may legitimately apply again (the 24h-TTL late-retry path).
ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ marker_ttl' = IF marker_ttl > 0 THEN marker_ttl - 1 ELSE 0
    /\ marker' = IF marker_ttl = 1 THEN EMPTY ELSE marker
    /\ apply_in_window' = IF marker_ttl = 1 THEN 0 ELSE apply_in_window
    /\ UNCHANGED <<s_phase, probe_saw_applied>>

\* -----------------------------------------------------------------------
\* Next-state relation
\* -----------------------------------------------------------------------
Next ==
    \/ \E s \in Submitters :
        \/ Probe(s)
        \/ Cas(s)
        \/ ReturnToIdle(s)
    \/ ClockTick

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY PROPERTIES
\* =======================================================================

\* THE KEY INVARIANT: within any live-marker (TTL) window, the message is applied
\* at most once — even across concurrent submitters and stale advisory probes.
\* Guaranteed by the atomic check_and_set in dedup_message, not by the probe.
\* (After the marker's TTL expires the counter resets and a late resubmission may
\* apply again — a new window, which is correct, not a violation.)
AppliedAtMostOncePerWindow == apply_in_window <= 1

\* No application happens without the marker having been inserted first: an apply
\* this window implies the marker is set. (The apply and the marker-insert are
\* the same first-writer branch of the atomic check_and_set.)
ApplyImpliesMarker == (apply_in_window >= 1) => (marker = "set")

==========================================================================
