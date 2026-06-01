----------------------------- MODULE SilenceWindow -----------------------------
(*
 * TLA+ specification of Acteon's silence (time-bounded alert suppression)
 * window correctness — the HALF-OPEN interval check at dispatch time.
 *
 * Models:
 *   - crates/core/src/silence.rs :: Silence::is_active_at (~193):
 *         now >= self.starts_at && now < self.ends_at
 *     a HALF-OPEN interval [starts_at, ends_at) — ends_at is EXCLUSIVE.
 *     validate() (~223) rejects ends_at <= starts_at, so the window is
 *     always non-empty (starts_at < ends_at).
 *   - crates/core/src/silence.rs :: Silence::applies_to (~213) and
 *     CachedSilence::applies_to in crates/gateway/src/silence_enforcement.rs
 *     (~86): applies IFF is_active_at(now) AND the matchers match the labels.
 *   - crates/gateway/src/silence_enforcement.rs :: Gateway::check_silence
 *     (~162): the dispatch-time enforcement. It computes `let now = Utc::now()`
 *     FRESH at the call, then for each cached silence checks
 *     `cached.applies_to(labels, now)` and, on the first hit, suppresses the
 *     action (ActionOutcome::Silenced). active_at is RE-EVALUATED against the
 *     dispatch instant's clock — it is NOT a stale "was active earlier" flag.
 *
 * Protocol. A clock advances through ticks. A single silence has a fixed
 * window [StartsAt, EndsAt) (StartsAt < EndsAt, mirroring validate()). The
 * silence has a lifecycle: it can be created and deleted concurrently, and the
 * clock can advance past EndsAt (expiry). An action is dispatched at the
 * current clock tick; tenant/label matching is abstracted to a single per-
 * dispatch BOOLEAN `label_match` (the action's labels either match the
 * matchers or they do not). The dispatch suppresses the action IFF, AT THE
 * DISPATCH TICK:
 *     a silence EXISTS  AND  label_match  AND  (clock >= StartsAt /\ clock < EndsAt)
 * exactly the Utc::now() re-check of is_active_at. The suppression outcome of
 * the most recent dispatch is recorded (suppressed / not-suppressed) along with
 * the tick and label_match it was decided at, for the invariants.
 *
 * The active_at check is RE-EVALUATED at the dispatch instant (Dispatch reads
 * the CURRENT clock, not a flag frozen when the silence was created): this is
 * the load-bearing mechanism the negative test reverts.
 *
 * Verified (over every interleaving of clock ticks, silence create/delete, and
 * dispatch at any tick):
 *   - SuppressIffActiveAndMatching: the last dispatch was suppressed IFF
 *     (a silence existed AND label_match AND the dispatch tick was inside the
 *     half-open window). Suppression decisions agree with is_active_at at the
 *     dispatch instant.
 *   - NoSuppressOutsideWindow: an action dispatched before StartsAt, or at/after
 *     EndsAt (the half-open boundary, EndsAt EXCLUSIVE), is NOT suppressed —
 *     including the EXACT EndsAt boundary tick and any post-expiry tick.
 *
 * Negative check: the half-open upper bound is where the boundary bug lives.
 * A BUGGY variant using a CLOSED upper bound (clock <= EndsAt instead of
 * clock < EndsAt) wrongly suppresses an action dispatched at EXACTLY EndsAt ->
 * NoSuppressOutsideWindow violated (an off-by-one at the exclusive boundary).
 * Independently, re-evaluating active_at at dispatch time is load-bearing: a
 * BUGGY variant that suppresses based on a STALE "was active earlier" flag,
 * after the clock has advanced past EndsAt, suppresses post-expiry ->
 * NoSuppressOutsideWindow violated. The boundary off-by-one is negative-tested.
 *
 * SCOPE. Tenant/label matching (silence_tenant_covers + the matcher AND-cascade)
 * is ABSTRACTED to a single per-dispatch boolean `label_match`; this spec
 * verifies the TIME-WINDOW correctness — the half-open interval and expiry —
 * which is the part where the boundary off-by-one bug can live. The clock is a
 * single shared tick counter (one logical Utc::now() source); the regex DFA
 * caps, the cache load/sync path, and the expired-silence reaper are out of
 * scope. The window is modeled as fixed constants [StartsAt, EndsAt); the
 * create/delete lifecycle toggles the silence's existence, not its window.
 *
 * Run with:
 *   java -jar tla2tools.jar -config SilenceWindow.cfg SilenceWindow.tla
 *)
EXTENDS Integers, FiniteSets, TLC

\* -----------------------------------------------------------------------
\* Constants
\* -----------------------------------------------------------------------
CONSTANTS
    StartsAt,  \* silence window start (inclusive lower bound), e.g. 1
    EndsAt,    \* silence window end   (EXCLUSIVE upper bound),  e.g. 3
    MaxTime,   \* state-space bound on the clock (>= EndsAt so expiry is reachable)
    NONE       \* sentinel: no dispatch decided yet this cycle

\* validate() guarantees a non-empty window: StartsAt < EndsAt, and the clock
\* must be able to reach the exclusive boundary and beyond (expiry).
ASSUME StartsAt < EndsAt
ASSUME EndsAt <= MaxTime

\* is_active_at(now): the IMPLEMENTATION's window check that Dispatch evaluates —
\* the HALF-OPEN interval [StartsAt, EndsAt), EndsAt EXCLUSIVE (silence.rs:193).
ActiveAt(t) == (t >= StartsAt) /\ (t < EndsAt)

\* The TRUE half-open window — the SPECIFICATION ground-truth, used ONLY by the
\* safety invariants and deliberately INDEPENDENT of the Dispatch implementation's
\* ActiveAt. In the correct spec ActiveAt and InWindow coincide; stating the
\* invariants against InWindow (not ActiveAt) means a buggy ActiveAt — e.g. a
\* closed upper bound `t <= EndsAt` — is CAUGHT rather than self-masked by the
\* invariant referencing the same broken check (the offsetting-error trap).
InWindow(t) == (t >= StartsAt) /\ (t < EndsAt)

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    clock,         \* 0..MaxTime: the shared Utc::now() tick source
    silence_exists,\* BOOLEAN: a matching silence is currently in the cache
    last_outcome,  \* {"suppressed","passed"} \cup {NONE}: most recent dispatch result
    last_tick,     \* 0..MaxTime: clock tick the last dispatch was decided at
    last_match,    \* BOOLEAN: label_match the last dispatch was decided with
    last_existed   \* BOOLEAN: whether a silence existed at the last dispatch instant

vars == <<clock, silence_exists, last_outcome, last_tick, last_match, last_existed>>

\* -----------------------------------------------------------------------
\* Type invariant
\* -----------------------------------------------------------------------
TypeOK ==
    /\ clock \in 0..MaxTime
    /\ silence_exists \in BOOLEAN
    /\ last_outcome \in {"suppressed", "passed", NONE}
    /\ last_tick \in 0..MaxTime
    /\ last_match \in BOOLEAN
    /\ last_existed \in BOOLEAN

\* -----------------------------------------------------------------------
\* Initial state
\* -----------------------------------------------------------------------
Init ==
    /\ clock = 0
    /\ silence_exists = FALSE
    /\ last_outcome = NONE
    /\ last_tick = 0
    /\ last_match = FALSE
    /\ last_existed = FALSE

\* -----------------------------------------------------------------------
\* Lifecycle: create / delete the silence (concurrent with dispatch).
\* The window [StartsAt, EndsAt) is fixed; create/delete toggles existence.
\* -----------------------------------------------------------------------
CreateSilence ==
    /\ ~silence_exists
    /\ silence_exists' = TRUE
    /\ UNCHANGED <<clock, last_outcome, last_tick, last_match, last_existed>>

DeleteSilence ==
    /\ silence_exists
    /\ silence_exists' = FALSE
    /\ UNCHANGED <<clock, last_outcome, last_tick, last_match, last_existed>>

\* -----------------------------------------------------------------------
\* The clock advances one tick (Utc::now() moves forward). This is what drives
\* the dispatch instant past StartsAt and eventually past EndsAt (expiry).
\* -----------------------------------------------------------------------
ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ UNCHANGED <<silence_exists, last_outcome, last_tick, last_match, last_existed>>

\* -----------------------------------------------------------------------
\* Dispatch an action at the CURRENT clock tick. tenant/label matching is the
\* per-dispatch boolean `m`. check_silence suppresses IFF a silence exists AND
\* the labels match AND is_active_at(now) holds — where `now` is RE-READ as the
\* CURRENT clock (the Utc::now() re-check), NOT a flag frozen at create time.
\* -----------------------------------------------------------------------
Dispatch(m) ==
    /\ last_outcome' =
         IF silence_exists /\ m /\ ActiveAt(clock)
         THEN "suppressed"
         ELSE "passed"
    /\ last_tick' = clock
    /\ last_match' = m
    /\ last_existed' = silence_exists           \* record existence at the dispatch instant
    /\ UNCHANGED <<clock, silence_exists>>

\* -----------------------------------------------------------------------
\* Recycle: rewind the clock to begin a fresh sweep through the window so the
\* system CYCLES (no benign terminal deadlock under -deadlock). Only fires at
\* the end of a sweep (clock saturated) and clears the silence + last decision.
\* -----------------------------------------------------------------------
Recycle ==
    /\ clock = MaxTime
    /\ clock' = 0
    /\ silence_exists' = FALSE
    /\ last_outcome' = NONE
    /\ last_tick' = 0
    /\ last_match' = FALSE
    /\ last_existed' = FALSE

\* -----------------------------------------------------------------------
\* Next-state relation
\* -----------------------------------------------------------------------
Next ==
    \/ ClockTick
    \/ CreateSilence
    \/ DeleteSilence
    \/ \E m \in BOOLEAN : Dispatch(m)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY PROPERTIES
\* =======================================================================

\* The last dispatch was suppressed IFF, at its dispatch tick, a matching
\* silence existed AND it was active at that tick (half-open window). Because
\* CreateSilence/DeleteSilence cannot interleave WITHIN a single Dispatch step
\* (Dispatch decides atomically against the existence + clock it sees), the
\* outcome agrees exactly with silence-existed /\ label-matched /\ ActiveAt.
\* We anchor on what the dispatch DECIDED, recorded at the decision instant:
\* last_existed (a silence was present), last_match (labels matched), and
\* last_tick (the dispatch clock). The IFF is exact in BOTH directions:
\*   suppressed  =>  existed /\ matched /\ active-at-tick   (no suppress without all three)
\*   any of {~existed, ~matched, ~active}  =>  passed       (any missing condition passes)
\* The existence conjunct closes the gap where a suppression with no silence
\* present would otherwise go unchecked.
SuppressIffActiveAndMatching ==
    /\ (last_outcome = "suppressed") =>
           (last_existed /\ last_match /\ InWindow(last_tick))
    /\ ((last_outcome # NONE)
          /\ (~last_existed \/ ~last_match \/ ~InWindow(last_tick)))
         => (last_outcome = "passed")

\* An action dispatched OUTSIDE the half-open window — before StartsAt, or at/
\* after EndsAt (EndsAt EXCLUSIVE), including the EXACT EndsAt boundary and any
\* post-expiry tick — is NEVER suppressed. This is the invariant the closed-
\* upper-bound (clock <= EndsAt) off-by-one and the stale-active-flag bug break.
NoSuppressOutsideWindow ==
    (last_outcome = "suppressed") => InWindow(last_tick)

============================================================================
