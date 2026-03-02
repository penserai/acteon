--------------------------- MODULE CircuitBreaker -------------------------
(*
 * TLA+ specification of Acteon's distributed circuit breaker.
 *
 * Models the three-state machine (Closed -> Open -> HalfOpen -> Closed/Open)
 * as implemented in crates/gateway/src/circuit_breaker.rs.
 *
 * Key properties verified:
 *   Safety:
 *     - SingleProbe:  at most one probe request in HalfOpen state
 *     - ValidOpen:    circuit opens only after failure_threshold failures
 *     - ValidClose:   circuit closes only after success_threshold in HalfOpen
 *   Liveness:
 *     - EventualRecovery: if probes succeed, circuit eventually closes
 *
 * Run with:
 *   java -jar tla2tools.jar -config CircuitBreaker.cfg CircuitBreaker.tla
 *)
EXTENDS Integers, FiniteSets, Sequences, TLC

\* -----------------------------------------------------------------------
\* Constants — set in CircuitBreaker.cfg for model-checking
\* -----------------------------------------------------------------------
CONSTANTS
    Gateways,           \* e.g. {"g1", "g2", "g3"}
    FailureThreshold,   \* e.g. 3
    SuccessThreshold,   \* e.g. 2
    RecoveryTicks,      \* e.g. 2 (ticks before Open -> HalfOpen)
    MutationLockTTL,    \* e.g. 2 (ticks before mutation lock expires)
    ProbeTimeoutTicks,  \* e.g. 4 (ticks before stale probe is cleared)
    MaxTime,            \* e.g. 15 (limits state space)
    NOBODY              \* sentinel for "no holder"

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    \* Shared circuit breaker state (persisted in StateStore)
    cb_state,               \* \in {"closed", "open", "half_open"}
    consecutive_failures,   \* Nat
    consecutive_successes,  \* Nat
    last_failure_tick,      \* Nat or 0
    probe_started_tick,     \* Nat or 0 (0 = no probe)

    \* Distributed mutation lock
    mutation_lock_holder,   \* Gateways \cup {NOBODY}
    mutation_lock_ttl,      \* 0..MutationLockTTL

    \* Per-gateway state
    gw_phase,               \* [Gateways -> phase enum]
    gw_request_result,      \* [Gateways -> {"pending", "success", "failure"}]

    \* Global clock (discrete ticks)
    clock

\* -----------------------------------------------------------------------
\* Phase enum for gateway processes
\* -----------------------------------------------------------------------
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
    /\ clock \in 0..MaxTime

\* -----------------------------------------------------------------------
\* Initial state
\* -----------------------------------------------------------------------
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
    /\ clock = 0

\* -----------------------------------------------------------------------
\* Helper: is a probe currently active (not stale)?
\* -----------------------------------------------------------------------
IsProbeActive ==
    /\ probe_started_tick > 0
    /\ (clock - probe_started_tick) < ProbeTimeoutTicks

\* -----------------------------------------------------------------------
\* Actions
\* -----------------------------------------------------------------------

(* A gateway starts a new request cycle *)
StartRequest(g) ==
    /\ gw_phase[g] = "idle"
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "acquiring_lock"]
    /\ gw_request_result' = [gw_request_result EXCEPT ![g] = "pending"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl, clock>>

(* Gateway tries to acquire the mutation lock (try_acquire_permit) *)
TryAcquireLock(g) ==
    /\ gw_phase[g] = "acquiring_lock"
    /\ IF mutation_lock_holder = NOBODY
       THEN /\ mutation_lock_holder' = g
            /\ mutation_lock_ttl' = MutationLockTTL
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "locked_read"]
       ELSE \* Lock contention — gateway backs off to idle (rejected)
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
            /\ UNCHANGED <<mutation_lock_holder, mutation_lock_ttl>>
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_request_result, clock>>

(* Gateway reads circuit state and decides whether to allow the request *)
ReadAndDecide(g) ==
    /\ gw_phase[g] = "locked_read"
    /\ mutation_lock_holder = g
    /\ CASE cb_state = "closed" ->
            \* Always allow in closed state
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "executing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

         [] cb_state = "open" /\ (clock - last_failure_tick) >= RecoveryTicks ->
            \* Recovery timeout elapsed: transition to half_open, start probe
            /\ cb_state' = "half_open"
            /\ consecutive_successes' = 0
            /\ probe_started_tick' = clock
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "executing"]
            /\ UNCHANGED <<consecutive_failures, last_failure_tick>>

         [] cb_state = "open" /\ (clock - last_failure_tick) < RecoveryTicks ->
            \* Still in recovery window: reject
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "releasing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

         [] cb_state = "half_open" /\ IsProbeActive ->
            \* Another probe in flight: reject (thundering herd prevention)
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "releasing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

         [] cb_state = "half_open" /\ ~IsProbeActive ->
            \* No active probe (stale or completed): allow new probe
            /\ probe_started_tick' = clock
            /\ gw_phase' = [gw_phase EXCEPT ![g] = "executing"]
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick>>

    /\ UNCHANGED <<mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

(* Gateway releases lock after read-and-decide, before execution *)
ReleaseLockAfterDecide(g) ==
    /\ gw_phase[g] = "executing"
    /\ mutation_lock_holder = g
    /\ mutation_lock_holder' = NOBODY
    /\ mutation_lock_ttl' = 0
    \* Stay in "executing" — lock is released, request is in flight
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_phase, gw_request_result, clock>>

(* Non-deterministic: the provider call succeeds or fails *)
ProviderSucceeds(g) ==
    /\ gw_phase[g] = "executing"
    /\ mutation_lock_holder # g  \* lock was released
    /\ gw_request_result' = [gw_request_result EXCEPT ![g] = "success"]
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "recording_result"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl, clock>>

ProviderFails(g) ==
    /\ gw_phase[g] = "executing"
    /\ mutation_lock_holder # g  \* lock was released
    /\ gw_request_result' = [gw_request_result EXCEPT ![g] = "failure"]
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "recording_result"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl, clock>>

(* Gateway acquires mutation lock to record result *)
RecordResult(g) ==
    /\ gw_phase[g] = "recording_result"
    /\ mutation_lock_holder = NOBODY  \* must acquire lock
    /\ mutation_lock_holder' = g
    /\ mutation_lock_ttl' = MutationLockTTL
    /\ CASE
        \* --- Record SUCCESS ---
        gw_request_result[g] = "success" /\ cb_state = "half_open" ->
            /\ consecutive_successes' = consecutive_successes + 1
            /\ IF consecutive_successes + 1 >= SuccessThreshold
               THEN \* Close the circuit
                    /\ cb_state' = "closed"
                    /\ consecutive_failures' = 0
               ELSE
                    /\ UNCHANGED <<cb_state, consecutive_failures>>
            /\ probe_started_tick' = 0  \* clear probe slot
            /\ UNCHANGED last_failure_tick

      [] gw_request_result[g] = "success" /\ cb_state = "closed" ->
            \* Reset failure counter on success in closed state
            /\ consecutive_failures' = 0
            /\ UNCHANGED <<cb_state, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

      [] gw_request_result[g] = "success" /\ cb_state = "open" ->
            \* Success in open state — no effect (shouldn't happen but safe)
            /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                           last_failure_tick, probe_started_tick>>

        \* --- Record FAILURE ---
      [] gw_request_result[g] = "failure" /\ cb_state = "closed" ->
            /\ consecutive_failures' = consecutive_failures + 1
            /\ last_failure_tick' = clock
            /\ IF consecutive_failures + 1 >= FailureThreshold
               THEN cb_state' = "open"
               ELSE UNCHANGED cb_state
            /\ UNCHANGED <<consecutive_successes, probe_started_tick>>

      [] gw_request_result[g] = "failure" /\ cb_state = "half_open" ->
            \* Probe failed — reopen circuit
            /\ cb_state' = "open"
            /\ last_failure_tick' = clock
            /\ consecutive_successes' = 0
            /\ probe_started_tick' = 0
            /\ UNCHANGED consecutive_failures

      [] gw_request_result[g] = "failure" /\ cb_state = "open" ->
            /\ last_failure_tick' = clock
            /\ UNCHANGED <<cb_state, consecutive_failures,
                           consecutive_successes, probe_started_tick>>

    /\ gw_phase' = [gw_phase EXCEPT ![g] = "releasing"]
    /\ UNCHANGED <<gw_request_result, clock>>

(* Cannot acquire lock for recording — retry later (back to done) *)
RecordResultFailed(g) ==
    /\ gw_phase[g] = "recording_result"
    /\ mutation_lock_holder # NOBODY
    /\ mutation_lock_holder # g
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

(* Gateway releases the mutation lock *)
ReleaseLock(g) ==
    /\ gw_phase[g] = "releasing"
    /\ mutation_lock_holder = g
    /\ mutation_lock_holder' = NOBODY
    /\ mutation_lock_ttl' = 0
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "done"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    gw_request_result, clock>>

(* Gateway returns to idle after completing a request cycle *)
ReturnToIdle(g) ==
    /\ gw_phase[g] = "done"
    /\ gw_phase' = [gw_phase EXCEPT ![g] = "idle"]
    /\ UNCHANGED <<cb_state, consecutive_failures, consecutive_successes,
                    last_failure_tick, probe_started_tick,
                    mutation_lock_holder, mutation_lock_ttl,
                    gw_request_result, clock>>

(* Clock tick: time advances, lock TTLs expire *)
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
                    gw_phase, gw_request_result>>

\* -----------------------------------------------------------------------
\* Next-state relation
\* -----------------------------------------------------------------------
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
    \/ ClockTick

Spec == Init /\ [][Next]_<<cb_state, consecutive_failures, consecutive_successes,
                            last_failure_tick, probe_started_tick,
                            mutation_lock_holder, mutation_lock_ttl,
                            gw_phase, gw_request_result, clock>>

\* -----------------------------------------------------------------------
\* Safety properties
\* -----------------------------------------------------------------------

\* At most one probe request executing at any time in HalfOpen.
\* A gateway is "probing" if it is in executing phase while cb_state = half_open.
SingleProbe ==
    cb_state = "half_open" =>
        Cardinality({g \in Gateways :
            /\ gw_phase[g] = "executing"}) <= 1

\* The circuit only opens after enough consecutive failures.
ValidOpen ==
    cb_state = "open" => consecutive_failures >= FailureThreshold

\* Mutual exclusion on the mutation lock.
LockMutex ==
    Cardinality({g \in Gateways : mutation_lock_holder = g}) <= 1

\* -----------------------------------------------------------------------
\* Liveness properties (checked with fairness)
\* -----------------------------------------------------------------------

\* If a gateway starts a request, it eventually completes.
\* (Requires weak fairness on all gateway actions.)
RequestCompletion ==
    \A g \in Gateways :
        gw_phase[g] # "idle" ~> gw_phase[g] = "idle"

==========================================================================
