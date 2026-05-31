--------------------------- MODULE CircuitBreaker -------------------------
(*
 * TLA+ specification of Acteon's distributed circuit breaker.
 *
 * Models the three-state machine (Closed -> Open -> HalfOpen -> Closed/Open)
 * from crates/gateway/src/circuit_breaker.rs, where shared breaker state is
 * mutated under a distributed lock and a single probe is admitted per
 * half-open window.
 *
 * Verified safety property:
 *   - SingleProbe: at most one LIVE (non-stale) probe executes at a time in
 *     HalfOpen. A probe that outlives ProbeTimeoutTicks is considered stale
 *     and a fresh probe may then be admitted — that overlap is expected and
 *     is NOT a violation, so the property counts only non-stale probes.
 *
 * Not asserted here (deliberately):
 *   - "opens only after FailureThreshold failures": false in general — the
 *     HalfOpen->Open reopen path sets Open after a single failed probe,
 *     regardless of the (reset-on-close) failure counter. This faithfully
 *     mirrors the Rust code, so it is not an invariant.
 *   - liveness (RequestCompletion): defined below but requires fairness that
 *     the cfg does not configure, so it is not model-checked.
 *
 * Run with:
 *   java -jar tla2tools.jar -config CircuitBreaker.cfg CircuitBreaker.tla
 *)
EXTENDS Integers, FiniteSets, TLC

\* -----------------------------------------------------------------------
\* Constants — set in CircuitBreaker.cfg
\* -----------------------------------------------------------------------
CONSTANTS
    Gateways,           \* e.g. {g1, g2}
    FailureThreshold,   \* failures (closed) before opening
    SuccessThreshold,   \* successes (half-open) before closing
    RecoveryTicks,      \* ticks Open -> HalfOpen eligible
    MutationLockTTL,    \* ticks before the mutation lock expires
    ProbeTimeoutTicks,  \* ticks before an in-flight probe is considered stale
    MaxTime,            \* state-space bound
    NOBODY              \* sentinel for "no lock holder"

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    cb_state,               \* {"closed","open","half_open"}
    consecutive_failures,
    consecutive_successes,
    last_failure_tick,
    probe_started_tick,     \* tick the current half-open probe slot opened (0 = none)

    mutation_lock_holder,   \* Gateways \cup {NOBODY}
    mutation_lock_ttl,      \* 0..MutationLockTTL

    gw_phase,               \* [Gateways -> phase]
    gw_request_result,      \* [Gateways -> {"pending","success","failure"}]
    gw_probe_start,         \* [Gateways -> 0..MaxTime] tick this gw was admitted as a probe (0 = not a probe)

    clock

Phases == {"idle", "acquiring_lock", "locked_read", "executing",
           "recording_result", "releasing", "done"}

ResultStates == {"pending", "success", "failure"}

\* -----------------------------------------------------------------------
\* Type invariant
\* -----------------------------------------------------------------------
TypeOK ==
    /\ cb_state \in {"closed", "open", "half_open"}
    /\ consecutive_failures \in 0..100
    /\ consecutive_successes \in 0..100
    /\ last_failure_tick \in 0..MaxTime
    /\ probe_started_tick \in 0..MaxTime
    /\ mutation_lock_holder \in Gateways \cup {NOBODY}
    /\ mutation_lock_ttl \in 0..MutationLockTTL
    /\ gw_phase \in [Gateways -> Phases]
    /\ gw_request_result \in [Gateways -> ResultStates]
    /\ gw_probe_start \in [Gateways -> 0..MaxTime]
    /\ clock \in 0..MaxTime

Init ==
    /\ cb_state = "closed"
    /\ consecutive_failures = 0
    /\ consecutive_successes = 0
    /\ last_failure_tick = 0
    /\ probe_started_tick = 0
    /\ mutation_lock_holder = NOBODY
    /\ mutation_lock_ttl = 0
    /\ gw_phase = [g \in Gateways |-> "idle"]
    /\ gw_request_result = [g \in Gateways |-> "pending"]
    /\ gw_probe_start = [g \in Gateways |-> 0]
    /\ clock = 0

\* A half-open probe slot is active (occupied, not yet stale).
IsProbeActive ==
    /\ probe_started_tick > 0
    /\ (clock - probe_started_tick) < ProbeTimeoutTicks

\* -----------------------------------------------------------------------
\* Actions
\* -----------------------------------------------------------------------

StartRequest(g) ==
    /\ gw_phase[g] = "idle"
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "acquiring_lock"]
    /\ gw_request_result' = [gw_request_result EXCEPT ![g] = "pending"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_probe_start, clock>>

TryAcquireLock(g) ==
    /\ gw_phase[g] = "acquiring_lock"
    /\ IF mutation_lock_holder = NOBODY
       THEN /\ mutation_lock_holder' = g
            /\ mutation_lock_ttl' = MutationLockTTL
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "locked_read"]
       ELSE \* Lock contention — gateway is rejected (conservative vs the real
            \* lock-free fallback read; rejecting is safe for the invariant).
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
            /\ UNCHANGED <<mutation_lock_holder, mutation_lock_ttl>>
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_request_result, gw_probe_start, clock>>

\* Read circuit state and decide whether to admit the request. The three arms
\* that proceed to "executing" set gw_probe_start (clock if admitted AS a probe,
\* 0 if a normal closed-state request); the two reject arms leave it.
ReadAndDecide(g) ==
    /\ gw_phase[g] = "locked_read"
    /\ mutation_lock_holder = g
    /\ CASE cb_state = "closed" ->
            \* Normal request — not a probe.
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "executing"]
            /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = 0]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

         [] cb_state = "open" /\ (clock - last_failure_tick) >= RecoveryTicks ->
            \* Recovery elapsed: open the first probe of a half-open window.
            /\ cb_state' = "half_open"
            /\ consecutive_successes' = 0
            /\ probe_started_tick' = clock
            /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = clock]
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "executing"]
            /\ UNCHANGED <<consecutive_failures, last_failure_tick>>

         [] cb_state = "open" /\ (clock - last_failure_tick) < RecoveryTicks ->
            \* Still recovering: reject.
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "releasing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick, gw_probe_start>>

         [] cb_state = "half_open" /\ IsProbeActive ->
            \* A live probe is already in flight: reject (thundering-herd guard).
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "releasing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick, gw_probe_start>>

         [] cb_state = "half_open" /\ ~IsProbeActive ->
            \* No live probe (slot free / previous probe stale): admit a probe.
            /\ probe_started_tick' = clock
            /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = clock]
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "executing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick>>

    /\ UNCHANGED <<mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

\* Release the lock before the (in-flight) request executes.
ReleaseLockAfterDecide(g) ==
    /\ gw_phase[g] = "executing"
    /\ mutation_lock_holder = g
    /\ mutation_lock_holder' = NOBODY
    /\ mutation_lock_ttl' = 0
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_phase, gw_request_result, gw_probe_start, clock>>

ProviderSucceeds(g) ==
    /\ gw_phase[g] = "executing"
    /\ mutation_lock_holder # g
    /\ gw_request_result' = [gw_request_result EXCEPT ![g] = "success"]
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "recording_result"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_probe_start, clock>>

ProviderFails(g) ==
    /\ gw_phase[g] = "executing"
    /\ mutation_lock_holder # g
    /\ gw_request_result' = [gw_request_result EXCEPT ![g] = "failure"]
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "recording_result"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_probe_start, clock>>

\* Re-acquire the lock and record the result. Clears this gateway's probe mark.
RecordResult(g) ==
    /\ gw_phase[g] = "recording_result"
    /\ mutation_lock_holder = NOBODY
    /\ mutation_lock_holder' = g
    /\ mutation_lock_ttl' = MutationLockTTL
    /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = 0]
    /\ CASE
        \* HalfOpen, g still OWNS the probe slot, success: advance / close.
        cb_state = "half_open" /\ gw_probe_start[g] = probe_started_tick
            /\ gw_request_result[g] = "success" ->
            /\ consecutive_successes' = consecutive_successes + 1
            /\ IF consecutive_successes + 1 >= SuccessThreshold
               THEN /\ cb_state' = "closed"
                    /\ consecutive_failures' = 0
               ELSE UNCHANGED <<cb_state, consecutive_failures>>
            /\ probe_started_tick' = 0   \* release the slot (owner)
            /\ UNCHANGED last_failure_tick

        \* HalfOpen, g still OWNS the probe slot, failure: reopen.
      [] cb_state = "half_open" /\ gw_probe_start[g] = probe_started_tick
            /\ gw_request_result[g] = "failure" ->
            /\ cb_state' = "open"
            /\ last_failure_tick' = clock
            /\ consecutive_successes' = 0
            /\ probe_started_tick' = 0
            /\ UNCHANGED consecutive_failures

        \* HalfOpen, but a NEWER probe has since reserved the slot: g is a
        \* stale/orphaned probe. Its late result must not disturb the current
        \* probe's slot or the breaker state — the slot owner is authoritative.
      [] cb_state = "half_open" /\ gw_probe_start[g] # probe_started_tick ->
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

      [] gw_request_result[g] = "success" /\ cb_state = "closed" ->
            /\ consecutive_failures' = 0
            /\ UNCHANGED <<cb_state, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

      [] gw_request_result[g] = "failure" /\ cb_state = "closed" ->
            /\ consecutive_failures' = consecutive_failures + 1
            /\ last_failure_tick' = clock
            /\ IF consecutive_failures + 1 >= FailureThreshold
               THEN cb_state' = "open"
               ELSE UNCHANGED cb_state
            /\ UNCHANGED <<consecutive_successes, probe_started_tick>>

      [] cb_state = "open" ->
            \* Result recorded after the window changed to Open (e.g. a stale
            \* probe). Refresh the failure clock on a failure; otherwise inert.
            /\ IF gw_request_result[g] = "failure"
               THEN last_failure_tick' = clock
               ELSE UNCHANGED last_failure_tick
            /\ UNCHANGED <<cb_state, consecutive_failures,
                           consecutive_successes, probe_started_tick>>

    /\ gw_phase' = [gw_phase EXCEPT ![g] = "releasing"]
    /\ UNCHANGED <<gw_request_result, clock>>

\* Could not re-acquire the lock for recording — give up this cycle.
RecordResultFailed(g) ==
    /\ gw_phase[g] = "recording_result"
    /\ mutation_lock_holder # NOBODY
    /\ mutation_lock_holder # g
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
    /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = 0]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

ReleaseLock(g) ==
    /\ gw_phase[g] = "releasing"
    /\ mutation_lock_holder = g
    /\ mutation_lock_holder' = NOBODY
    /\ mutation_lock_ttl' = 0
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_request_result, gw_probe_start, clock>>

ReturnToIdle(g) ==
    /\ gw_phase[g] = "done"
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "idle"]
    /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = 0]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

\* A gateway holding the lock loses it to TTL expiry mid-critical-section
\* (in locked_read or releasing). It abandons this cycle gracefully rather
\* than wedging — mirrors the real code surfacing a LockExpired error.
AbandonOnLockLoss(g) ==
    /\ gw_phase[g] \in {"locked_read", "releasing"}
    /\ mutation_lock_holder # g
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
    /\ gw_probe_start' = [gw_probe_start EXCEPT ![g] = 0]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ IF mutation_lock_ttl > 0
       THEN /\ mutation_lock_ttl' = mutation_lock_ttl - 1
            /\ IF mutation_lock_ttl - 1 = 0
               THEN mutation_lock_holder' = NOBODY
               ELSE UNCHANGED mutation_lock_holder
       ELSE UNCHANGED <<mutation_lock_holder, mutation_lock_ttl>>
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_phase, gw_request_result, gw_probe_start>>

Next ==
    \/ \E g \in Gateways :
        \/ StartRequest(g)
        \/ TryAcquireLock(g)
        \/ ReadAndDecide(g)
        \/ ReleaseLockAfterDecide(g)
        \/ ProviderSucceeds(g)
        \/ ProviderFails(g)
        \/ RecordResult(g)
        \/ RecordResultFailed(g)
        \/ ReleaseLock(g)
        \/ ReturnToIdle(g)
        \/ AbandonOnLockLoss(g)
    \/ ClockTick

vars == <<cb_state, consecutive_failures, consecutive_successes,
          last_failure_tick, probe_started_tick,
          mutation_lock_holder, mutation_lock_ttl,
          gw_phase, gw_request_result, gw_probe_start, clock>>

Spec == Init /\ [][Next]_vars

\* -----------------------------------------------------------------------
\* Safety
\* -----------------------------------------------------------------------

\* A gateway is a LIVE probe iff it was admitted as a probe (gw_probe_start>0)
\* and that probe has not yet timed out. At most one such probe may exist —
\* this is the half-open thundering-herd guarantee.
IsLiveProbe(g) ==
    /\ gw_probe_start[g] > 0
    /\ (clock - gw_probe_start[g]) < ProbeTimeoutTicks
    /\ gw_phase[g] \in {"executing", "recording_result"}

SingleProbe ==
    Cardinality({g \in Gateways : IsLiveProbe(g)}) <= 1

\* -----------------------------------------------------------------------
\* Liveness (NOT model-checked here — requires fairness the cfg omits)
\* -----------------------------------------------------------------------
RequestCompletion ==
    \A g \in Gateways :
        gw_phase[g] # "idle" ~> gw_phase[g] = "idle"

==========================================================================
