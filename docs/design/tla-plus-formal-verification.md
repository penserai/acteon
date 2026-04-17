# TLA+ Formal Verification for Acteon

## 1. Executive Summary

Acteon is a distributed action orchestration gateway with a rich pipeline involving
distributed locking, state machine transitions, deduplication, rate limiting, multi-step
chain execution, circuit breakers, event grouping, and approval workflows. These
subsystems interact concurrently across multiple gateway instances sharing a common state
backend.

**TLA+** (Temporal Logic of Actions) is a formal specification language created by Leslie
Lamport for modeling concurrent and distributed systems. It enables *exhaustive*
verification of safety and liveness properties through its TLC model checker,
catching subtle bugs that are impossible to reproduce through testing alone.

This document identifies **the highest-value areas** in Acteon where TLA+ modeling would
harden correctness, proposes concrete specifications, and provides an implementation
roadmap.

---

## 2. Why TLA+ for Acteon

### 2.1 Industry Precedent

TLA+ has been proven at scale by organizations with similar distributed systems challenges:

- **Amazon Web Services** (2014): Used TLA+ for DynamoDB, S3, EBS, and internal
  lock managers. Found "subtle, serious bugs" in designs that had passed extensive
  code reviews and testing. Key finding: TLA+ caught bugs that would only manifest
  under rare interleavings of concurrent operations.

- **Microsoft Azure** (Cosmos DB): Used TLA+ to verify the five consistency models.
  The specification caught edge cases in the bounded staleness guarantee that testing
  missed.

- **CockroachDB**: Used TLA+ to verify their Parallel Commits protocol, catching a
  subtle stale-read vulnerability before production deployment.

- **Elastic (Elasticsearch)**: Used TLA+ to verify their cluster consensus protocol
  and shard replication logic.

- **MongoDB**: Formally specified the Raft consensus protocol modifications used in
  their replication layer.

### 2.2 What Makes Acteon a Strong Candidate

Acteon exhibits multiple patterns that are known to harbor concurrency bugs that are
difficult to catch through testing but tractable for TLA+ model checking:

| Pattern | Location in Acteon | Why TLA+ Helps |
|---------|-------------------|----------------|
| Distributed locking with TTL | `gateway.rs` dispatch lock | Lock expiry races |
| Check-then-act (CAS) | Dedup `check_and_set` | TOCTOU vulnerabilities |
| State machine + lock | Fingerprint state transitions | Lost updates |
| Atomic counters with fail-open | Throttle `increment` | Counter drift |
| Multi-phase chain execution | Chain advancement worker | Step ordering violations |
| Circuit breaker state machine | Half-open probe serialization | Thundering herd |
| In-memory + persistent cache | Group manager, quota policies | Stale cache reads |
| Background workers + indices | Timeout/chain-ready scanning | Orphaned entries |
| Approval lifecycle | Approve/reject/expire | Decision races |

### 2.3 What TLA+ Verifies

TLA+ can verify two fundamental classes of properties:

**Safety properties** ("nothing bad ever happens"):
- Mutual exclusion: at most one gateway executes a given action
- Deduplication: duplicate actions never result in double execution
- State machine: only valid transitions occur
- Approval: no action is both approved and rejected

**Liveness properties** ("something good eventually happens"):
- Every dispatched action eventually completes
- Locks are eventually released (TTL or explicit)
- Chains eventually terminate (completion or failure)
- Groups are eventually flushed

---

## 3. Acteon Architecture: Concurrency-Critical Subsystems

### 3.1 Dispatch Pipeline

The core `Gateway::dispatch_inner` method (in `crates/gateway/src/gateway.rs`) processes
each action through this pipeline:

```
1. Acquire distributed lock  (dispatch:{ns}:{tenant}:{action_id})
2. Quota check               (atomic increment, fail-open)
3. Enrichment                (read-only provider lookups)
4. Template rendering         (blocking task)
5. Rule evaluation            (async, may query state)
6. LLM guardrail             (optional, async)
7. Verdict execution          (dedup/throttle/state-machine/chain/...)
8. Audit emission             (sync or async per compliance config)
9. Stream event broadcast     (fire-and-forget)
10. Lock release
```

**Concurrency concern**: Steps 1-10 form a critical section guarded by the dispatch lock.
If the lock holder crashes or the lock TTL (30s) expires before step 10, another gateway
instance can acquire the lock and re-enter the pipeline for the same action. The CAS-based
dedup check (step 7) is the last line of defense against double execution.

### 3.2 State Machine Transitions

State machine handling (`handle_state_machine` at gateway.rs:1472) uses a *nested* lock:

```
Outer lock:  dispatch:{ns}:{tenant}:{action_id}   (30s TTL)
Inner lock:  state:{ns}:{tenant}:{fingerprint}     (30s TTL)
```

Within the inner lock:
1. Read current state from state store
2. Validate transition against `StateMachineConfig`
3. Write new state
4. Update active events index
5. Create/clear timeout entry
6. Release inner lock

**Concurrency concern**: Two different actions with the same fingerprint but different
action IDs each hold their own dispatch lock but contend on the fingerprint lock. If the
fingerprint lock holder's TTL expires before release, the second action could read stale
state and overwrite the first action's transition.

### 3.3 Circuit Breaker

The circuit breaker (`crates/gateway/src/circuit_breaker.rs`) implements a distributed
three-state machine: Closed -> Open -> HalfOpen -> Closed/Open.

Key distributed coordination:
- State persisted in `StateStore` under `KeyKind::Custom("cb:...")`
- HalfOpen probe serialized via a `DistributedLock` with 5s mutation TTL
- Stale probe detection via 30s `PROBE_TIMEOUT_MS`
- Failure/success counters use CAS for state transitions

**Concurrency concern**: Multiple gateway instances independently track the circuit state.
A race exists between probe completion and the stale-probe timer: if a probe takes >30s,
another instance may start a second probe, potentially allowing two concurrent requests
to a failing provider.

### 3.4 Chain (Multi-Step Workflow) Execution

Chains are multi-step workflows where each step dispatches an action and feeds its result
to the next step. Chain state is persisted in the state store and advanced by a background
worker.

Chain advancement flow:
1. Background worker scans `chain_ready_index` for chains with `ready_at <= now`
2. Acquires lock on chain state key
3. Reads chain state, determines next step
4. Creates a step-dedup key to prevent re-execution
5. Dispatches the step action
6. Writes updated chain state
7. Releases lock

**Concurrency concern**: If the advance worker crashes between step 5 (dispatch) and
step 6 (state write), the step was executed but the chain state wasn't updated. On restart,
the step-dedup key prevents re-execution, but the chain is stuck. The background worker
must handle this by checking for existing step-dedup keys and advancing past completed steps.

### 3.5 Event Grouping

The GroupManager (`crates/gateway/src/group_manager.rs`) maintains an in-memory cache
(`Arc<RwLock<HashMap<String, EventGroup>>>`) backed by state store persistence.

Flow:
1. Action arrives with Group verdict
2. GroupManager adds event to in-memory group (acquires write lock)
3. Group metadata persisted to state store
4. Background worker periodically checks for groups ready to flush
5. On flush: dispatch grouped notification, mark as Notified, remove from cache

**Concurrency concern**: In a multi-node deployment, each node has its own in-memory
group cache. Events for the same group arriving at different nodes create split groups.
The state store is the source of truth, but the in-memory cache is not synchronized
across nodes. Group recovery on startup (`recover_groups`) only loads from state store
into the local cache.

### 3.6 Approval Workflow

Approvals have a lifecycle: PendingApproval -> Approved/Rejected/Expired.

Flow:
1. Rule verdict is `RequestApproval`
2. Gateway stores `ApprovalRecord` in state store with TTL
3. Notification sent to human (via configured provider)
4. Human clicks approve/reject URL (HMAC-signed)
5. Gateway validates HMAC, checks expiry, executes decision
6. Background worker retries failed notifications

**Concurrency concern**: Race between expiry check and decision persistence. If an
approval expires at time T and a human approves at time T-epsilon, there's a window where
the gateway reads the approval as valid, processes it, but the background expiry worker
also processes it. The approval should have a distributed lock to serialize decisions.

---

## 4. Proposed TLA+ Specifications

### 4.1 Spec 1: Dispatch Pipeline Deduplication (Priority: HIGH)

**Goal**: Prove that for any action with a `dedup_key`, at most one provider execution
occurs within the dedup TTL window, even under concurrent dispatch attempts across
multiple gateway instances.

**Model scope**:
- N gateway instances (parameter: 2-3)
- M actions with K distinct dedup keys (parameter: 2-4 actions, 1-2 keys)
- State store with atomic `check_and_set`
- Distributed lock with TTL and timeout
- Lock holder crash (lock expires before release)

**Variables**:
```
gateway_state[g]    \in {"idle", "locking", "locked", "evaluating", "executing",
                         "releasing", "crashed"}
lock_holder[name]   \in Gateways \cup {NONE}
lock_ttl[name]      \in 0..MAX_TTL
dedup_store[key]    \in {EMPTY, SET}
executed[action_id] \in Nat              \* count of provider executions
clock               \in Nat              \* monotonic time
```

**Safety property** (the key invariant):
```
DedupSafety ==
    \A key \in DEDUP_KEYS:
        LET actions_with_key == {a \in ACTIONS : a.dedup_key = key}
        IN  SumOf(executed, actions_with_key) <= 1
```

**Liveness property**:
```
DispatchProgress ==
    \A a \in ACTIONS: <>(gateway_state[a] = "done" \/ gateway_state[a] = "crashed")
```

**What this catches**:
- Lock TTL expiry causing a second gateway to enter the critical section while the
  first is still executing (the CAS on dedup_key should still prevent double execution)
- State store failure during `check_and_set` leading to execution without dedup check
- Race between two gateways where both see dedup_key as absent (impossible with
  atomic CAS, but the model proves it)

### 4.2 Spec 2: State Machine Transition Safety (Priority: HIGH)

**Goal**: Prove that state machine transitions are serialized per fingerprint and that
only valid transitions (as defined by `StateMachineConfig`) occur, even when multiple
actions for the same fingerprint arrive concurrently.

**Model scope**:
- N gateway instances (2-3)
- One state machine config with states {open, in_progress, closed} and defined transitions
- M actions with overlapping fingerprints (2-4)
- Nested lock (dispatch lock + fingerprint lock)
- Lock TTL expiry

**Variables**:
```
fingerprint_state[fp]    \in STATES \cup {UNINITIALIZED}
fp_lock_holder[fp]       \in Gateways \cup {NONE}
dispatch_lock_holder[id] \in Gateways \cup {NONE}
transition_history[fp]   \in Seq(STATES)
```

**Safety properties**:
```
ValidTransitionsOnly ==
    \A fp \in FINGERPRINTS:
        \A i \in 1..Len(transition_history[fp]) - 1:
            LET from == transition_history[fp][i]
                to   == transition_history[fp][i+1]
            IN  <<from, to>> \in ALLOWED_TRANSITIONS

NoLostUpdates ==
    \A fp \in FINGERPRINTS:
        \* If two transitions happen concurrently, both are reflected in final state
        fingerprint_state[fp] = Last(transition_history[fp])
```

**What this catches**:
- Fingerprint lock expiry allowing two concurrent transitions
- Read-modify-write race where second reader sees pre-first-write state
- Invalid transition when lock holder crashes between state read and write

### 4.3 Spec 3: Circuit Breaker State Machine (Priority: HIGH)

**Goal**: Prove that the circuit breaker correctly transitions between Closed, Open, and
HalfOpen states, that at most one probe request is in flight during HalfOpen, and that
the circuit eventually recovers if probes succeed.

**Model scope**:
- N gateway instances (2-3)
- One circuit breaker for one provider
- Failure threshold, success threshold, recovery timeout
- Probe timeout and stale probe detection
- Distributed lock for state mutations

**Variables**:
```
cb_state           \in {Closed, Open, HalfOpen}
failure_count      \in 0..MAX_FAILURES
success_count      \in 0..MAX_SUCCESSES
probe_in_flight    \in BOOLEAN
probe_started_at   \in 0..MAX_TIME
mutation_lock      \in Gateways \cup {NONE}
clock              \in Nat
```

**Safety properties**:
```
\* At most one probe in flight
SingleProbe ==
    cb_state = HalfOpen => Cardinality({g \in Gateways : probing[g]}) <= 1

\* Circuit opens only after threshold failures
ValidOpen ==
    cb_state = Open => failure_count >= FAILURE_THRESHOLD

\* Circuit closes only after threshold successes in HalfOpen
ValidClose ==
    cb_state' = Closed /\ cb_state = HalfOpen
    => success_count >= SUCCESS_THRESHOLD
```

**Liveness property**:
```
\* If probes always succeed, circuit eventually closes
EventualRecovery ==
    (cb_state = HalfOpen /\ AlwaysSucceeds) ~> (cb_state = Closed)
```

**What this catches**:
- Thundering herd: multiple probes in HalfOpen due to stale probe detection race
- Stuck-open circuit: probe completes but state update is lost
- Counter reset race: failure count reset while another failure is being recorded

### 4.4 Spec 4: Chain Execution Ordering (Priority: HIGH)

**Goal**: Prove that chain steps execute in order, each step executes at most once, and
chains eventually complete (either successfully or via failure policy).

**Model scope**:
- One chain with 3-4 steps (including branches)
- Background advance worker
- Step dedup keys for idempotency
- Worker crash during advancement
- Parallel step groups (optional, increases state space)

**Variables**:
```
chain_state[id]       \in {"pending", "running", "completed", "failed"}
current_step[id]      \in 0..NUM_STEPS
step_executed[id][s]  \in BOOLEAN
step_dedup[id][s]     \in {EMPTY, SET}
chain_lock[id]        \in Workers \cup {NONE}
worker_state[w]       \in {"idle", "advancing", "crashed"}
```

**Safety properties**:
```
\* Steps execute in order
StepOrdering ==
    \A id \in CHAINS, s \in 1..NUM_STEPS - 1:
        step_executed[id][s+1] => step_executed[id][s]

\* Each step executes at most once
StepIdempotency ==
    \A id \in CHAINS, s \in 0..NUM_STEPS - 1:
        \* Can't be executed if dedup key already set by a prior execution
        step_dedup[id][s] = SET => ExecutionCount(id, s) = 1

\* Terminal states are stable
TerminalStability ==
    \A id \in CHAINS:
        chain_state[id] \in {"completed", "failed"} =>
            chain_state[id]' = chain_state[id]
```

**Liveness property**:
```
\* Chains eventually terminate
ChainTermination ==
    \A id \in CHAINS:
        <>(chain_state[id] \in {"completed", "failed"})
```

**What this catches**:
- Worker crash between step dispatch and state update leaving chain stuck
- Duplicate step execution if dedup key is not checked
- Branch condition evaluated against stale step results
- Infinite loop in chains with circular branch conditions

### 4.5 Spec 5: Approval Lifecycle (Priority: MEDIUM)

**Goal**: Prove that each approval resolves to exactly one terminal state (approved,
rejected, or expired), and that approved actions are eventually executed.

**Model scope**:
- One approval per model run
- Human actor (may approve, reject, or do nothing)
- Expiry background worker
- Notification retry worker
- Gateway decision handler

**Variables**:
```
approval_state    \in {"pending", "approved", "rejected", "expired"}
action_executed   \in BOOLEAN
notification_sent \in BOOLEAN
expiry_time       \in Nat
clock             \in Nat
```

**Safety properties**:
```
\* Terminal states don't change
ApprovalFinalityOnce ==
    approval_state \in {"approved", "rejected", "expired"} =>
        approval_state' = approval_state

\* Mutually exclusive outcomes
MutualExclusion ==
    ~(approval_state = "approved" /\ approval_state = "rejected")

\* Only approved actions execute
OnlyApprovedExecute ==
    action_executed => approval_state = "approved"
```

**What this catches**:
- Race between human approve and background expiry
- Double execution if approval is processed twice
- Notification retry re-triggering an already-decided approval

### 4.6 Spec 6: Group Flush Consistency (Priority: MEDIUM)

**Goal**: Prove that grouped events are flushed exactly once and that no events are lost
between grouping and flushing.

**Model scope**:
- M events arriving at N gateway instances (2-3)
- In-memory group cache per node
- State store persistence
- Background flush worker
- Node crash and recovery

**Safety properties**:
```
\* No event is lost
NoEventLoss ==
    \A e \in EVENTS:
        e \in grouped_events =>
            (e \in flushed_events \/ e \in pending_groups)

\* Groups flush at most once
FlushIdempotency ==
    \A g \in GROUPS:
        FlushCount(g) <= 1
```

**What this catches**:
- Split-brain grouping across nodes (events for same group on different nodes)
- Group flushed while new event is being appended
- Node crash loses in-memory events not yet persisted

---

## 5. TLA+ Tooling and Project Structure

### 5.1 Recommended Tools

| Tool | Purpose | Notes |
|------|---------|-------|
| **TLC Model Checker** | Exhaustive state exploration | The primary verification tool; built into the TLA+ Toolbox |
| **TLA+ Toolbox** | IDE for writing and running specs | Java-based IDE with visual state exploration |
| **VS Code Extension** | IDE integration | TLA+ extension for VS Code (tlaplus-community/vscode-tlaplus) |
| **PlusCal** | Algorithm-level notation | Translates to TLA+; easier for imperative-minded developers |
| **Apalache** | Symbolic model checker | Handles larger state spaces via SMT solving; finds bugs TLC can't reach |
| **TLAPS** | Proof system | For inductive proofs when state space is too large for model checking |

### 5.2 Proposed Project Structure

```
specs/
  tla/
    README.md                        # Overview and how to run
    Dispatch.tla                     # Spec 1: Dispatch pipeline dedup
    Dispatch.cfg                     # TLC config (constants, invariants)
    StateMachineTransition.tla       # Spec 2: State machine transitions
    StateMachineTransition.cfg
    CircuitBreaker.tla               # Spec 3: Circuit breaker
    CircuitBreaker.cfg
    ChainExecution.tla               # Spec 4: Chain ordering
    ChainExecution.cfg
    ApprovalLifecycle.tla            # Spec 5: Approval workflow
    ApprovalLifecycle.cfg
    GroupFlush.tla                   # Spec 6: Group flush consistency
    GroupFlush.cfg
    common/
      StateStore.tla                 # Shared state store model
      DistributedLock.tla            # Shared lock model
      Clock.tla                      # Discrete clock model
    Makefile                         # Automation: `make check-all`
    ci/
      run-tlc.sh                     # CI script for running model checker
```

### 5.3 CI Integration

TLC can be run headless from the command line, making it suitable for CI pipelines:

```bash
# Example CI step
java -jar tla2tools.jar -config Dispatch.cfg -workers auto Dispatch.tla
```

Recommended CI approach:
1. Run TLC on all specs in `specs/tla/` on every PR that modifies gateway or state code
2. Use small model parameters (2 gateways, 2 actions) for fast CI feedback (~30s)
3. Run larger parameter sweeps nightly (3 gateways, 4 actions) for deeper coverage
4. Track model checking time as a metric to detect specification bloat

---

## 6. Modeling Patterns for Acteon

### 6.1 Modeling the State Store

The `StateStore` trait is the foundation of Acteon's distributed state. A TLA+ model
should capture its key atomic operations:

```tla+
---- MODULE StateStore ----
EXTENDS Integers, Sequences, FiniteSets, TLC

CONSTANTS KEYS, VALUES, NONE

VARIABLES store, versions

TypeOK ==
    /\ store \in [KEYS -> VALUES \cup {NONE}]
    /\ versions \in [KEYS -> Nat]

Init ==
    /\ store = [k \in KEYS |-> NONE]
    /\ versions = [k \in KEYS |-> 0]

\* Atomic check-and-set: returns TRUE if key was newly set
CheckAndSet(key, value) ==
    IF store[key] = NONE
    THEN /\ store' = [store EXCEPT ![key] = value]
         /\ versions' = [versions EXCEPT ![key] = versions[key] + 1]
         /\ TRUE
    ELSE /\ UNCHANGED <<store, versions>>
         /\ FALSE

\* Atomic increment: returns new value
Increment(key, delta) ==
    LET current == IF store[key] = NONE THEN 0 ELSE store[key]
        new_val == current + delta
    IN /\ store' = [store EXCEPT ![key] = new_val]
       /\ versions' = [versions EXCEPT ![key] = versions[key] + 1]
       /\ new_val

\* Compare-and-swap: succeeds only if version matches
CompareAndSwap(key, expected_version, new_value) ==
    IF versions[key] = expected_version
    THEN /\ store' = [store EXCEPT ![key] = new_value]
         /\ versions' = [versions EXCEPT ![key] = versions[key] + 1]
         /\ "Ok"
    ELSE /\ UNCHANGED <<store, versions>>
         /\ "Conflict"

====
```

### 6.2 Modeling the Distributed Lock

```tla+
---- MODULE DistributedLock ----
EXTENDS Integers, FiniteSets

CONSTANTS PROCESSES, LOCK_NAMES, MAX_TTL, NONE

VARIABLES lock_holder, lock_ttl, clock

TypeOK ==
    /\ lock_holder \in [LOCK_NAMES -> PROCESSES \cup {NONE}]
    /\ lock_ttl \in [LOCK_NAMES -> 0..MAX_TTL]
    /\ clock \in Nat

Init ==
    /\ lock_holder = [n \in LOCK_NAMES |-> NONE]
    /\ lock_ttl = [n \in LOCK_NAMES |-> 0]
    /\ clock = 0

\* Try to acquire a lock
TryAcquire(process, name, ttl) ==
    IF lock_holder[name] = NONE
    THEN /\ lock_holder' = [lock_holder EXCEPT ![name] = process]
         /\ lock_ttl' = [lock_ttl EXCEPT ![name] = ttl]
         /\ TRUE
    ELSE /\ UNCHANGED <<lock_holder, lock_ttl>>
         /\ FALSE

\* Release a lock (only if held by this process)
Release(process, name) ==
    IF lock_holder[name] = process
    THEN /\ lock_holder' = [lock_holder EXCEPT ![name] = NONE]
         /\ lock_ttl' = [lock_ttl EXCEPT ![name] = 0]
    ELSE UNCHANGED <<lock_holder, lock_ttl>>

\* Time tick: expire locks whose TTL has elapsed
Tick ==
    /\ clock' = clock + 1
    /\ lock_holder' = [n \in LOCK_NAMES |->
        IF lock_ttl[n] <= 1 /\ lock_holder[n] # NONE
        THEN NONE
        ELSE lock_holder[n]]
    /\ lock_ttl' = [n \in LOCK_NAMES |->
        IF lock_ttl[n] > 0 THEN lock_ttl[n] - 1 ELSE 0]

\* Safety: mutual exclusion
MutualExclusion ==
    \A n \in LOCK_NAMES:
        Cardinality({p \in PROCESSES : lock_holder[n] = p}) <= 1

====
```

### 6.3 Modeling Crash and Recovery

A critical aspect of modeling Acteon is handling process crashes. TLA+ models this
naturally through non-deterministic transitions:

```tla+
\* A process can crash at any point, releasing its state but NOT its lock
\* (the lock will expire via TTL)
Crash(process) ==
    /\ process_state' = [process_state EXCEPT ![process] = "crashed"]
    \* Lock is NOT released - it will expire via Tick
    /\ UNCHANGED <<lock_holder, lock_ttl, store>>

\* A crashed process can recover
Recover(process) ==
    /\ process_state[process] = "crashed"
    /\ process_state' = [process_state EXCEPT ![process] = "idle"]
    /\ UNCHANGED <<lock_holder, lock_ttl, store>>
```

This is where TLA+ excels: by allowing crashes at *every* interleaving point, the model
checker explores all possible failure scenarios, including:
- Crash after lock acquisition but before state mutation
- Crash after state mutation but before lock release
- Crash during multi-step operations (chain advancement)

---

## 7. Concrete Findings: Potential Issues to Verify

Through codebase analysis, the following areas deserve formal verification:

### 7.1 Dispatch Lock TTL vs. Pipeline Duration

**Risk**: The dispatch lock has a 30-second TTL (`gateway.rs:432`). If the pipeline takes
longer than 30 seconds (due to slow providers, LLM guardrails, or enrichment lookups),
the lock expires and another gateway can enter the critical section.

**Current mitigation**: Dedup CAS is the backup, but only applies when the verdict is
`Deduplicate`. For `Allow` verdicts, there is no secondary guard against double execution
of the same action.

**TLA+ verification**: Model the dispatch pipeline with configurable step durations
and verify that even with lock TTL expiry, no double execution occurs. If the model
finds a violation, the fix would be to extend the lock TTL during execution or add
a CAS guard for all verdict types.

### 7.2 State Machine: Nested Lock Ordering

**Risk**: The state machine handler acquires a fingerprint lock *inside* the dispatch
lock (`gateway.rs:1492-1500`). If two actions A and B have the same fingerprint but
different action IDs, they hold different dispatch locks but contend on the same
fingerprint lock. This creates a potential for:
- Action A holds dispatch lock for A, waiting for fingerprint lock
- Action B holds dispatch lock for B, waiting for fingerprint lock
- No deadlock risk (different dispatch locks), but the fingerprint lock TTL could expire
  if A holds it too long

**TLA+ verification**: Model the nested lock acquisition and verify that state transitions
are never lost even when the inner lock expires.

### 7.3 Circuit Breaker Probe Race

**Risk**: In `circuit_breaker.rs`, the HalfOpen state allows one probe at a time. The
probe is serialized via a distributed lock with 5s TTL (`MUTATION_LOCK_TTL`). If the
probe takes longer than 5s, the mutation lock expires and another gateway could start
a second probe. The `PROBE_TIMEOUT_MS` (30s) is a secondary guard, but there's a window
between lock expiry (5s) and probe timeout (30s) where dual probes could occur.

**TLA+ verification**: Model the probe lifecycle with timing parameters and verify the
`SingleProbe` invariant under all interleavings including lock TTL expiry.

### 7.4 Chain Step Dedup on Worker Crash

**Risk**: During chain advancement, the worker creates a step-dedup key *before*
dispatching the step action. If the dispatch succeeds but the worker crashes before
updating the chain state, the chain is stuck: the step-dedup key prevents re-execution,
but the chain state still points to the same step.

**TLA+ verification**: Model the advance worker's crash points and verify that chains
always make progress (liveness) even with worker crashes.

### 7.5 Group Split-Brain Across Nodes

**Risk**: Each gateway node has its own in-memory `GroupManager` cache. Events for the
same group arriving at different nodes create independent in-memory groups. While
events are persisted to the state store, the flush logic operates on the in-memory
cache, so events persisted by node A may not be included in node B's flush.

**TLA+ verification**: Model multi-node group accumulation and verify the `NoEventLoss`
invariant. This would likely reveal that the current design can lose events in
multi-node deployments.

### 7.6 Approval Expiry Race

**Risk**: The approval decision handler checks `is_expired()` and then writes the
decision. The background expiry worker also checks `is_expired()` and writes
"expired" status. Without a distributed lock on the approval record, these can race:
1. Human clicks approve at T-1
2. Decision handler reads record, sees not expired
3. Background worker runs at T, sees expired, writes "expired"
4. Decision handler writes "approved", overwriting "expired"

**TLA+ verification**: Model the approval lifecycle with concurrent human decision and
background expiry. Verify `ApprovalFinalityOnce`.

### 7.7 Quota Counter Drift

**Risk**: Quota enforcement (`quota_enforcement.rs:17`) uses atomic `increment()` with
fail-open semantics. If the state store fails intermittently, some increments are lost
(the action executes but the counter isn't bumped). Over time, the counter drifts below
actual usage, effectively allowing the tenant to exceed their quota.

**TLA+ verification**: Model quota enforcement with intermittent state store failures
and verify the bound on counter drift. This would quantify the worst-case drift to
inform operational decisions about fail-open vs. fail-closed.

---

## 8. Implementation Roadmap

### Phase 1: Foundation (Weeks 1-2)

1. **Set up TLA+ tooling**
   - Install TLA+ Toolbox or VS Code extension
   - Create `specs/tla/` directory structure
   - Write `common/StateStore.tla` and `common/DistributedLock.tla` modules

2. **Write Spec 1: Dispatch Deduplication**
   - Start with simplest model: 2 gateways, 1 action, 1 dedup key
   - Verify `DedupSafety` invariant
   - Add crash/recovery and lock TTL expiry
   - Gradually increase parameters to find the boundary of correctness

3. **CI integration**
   - Add TLC to CI pipeline (GitHub Actions or equivalent)
   - Run with small parameters for fast feedback

### Phase 2: Core Protocols (Weeks 3-4)

4. **Write Spec 2: State Machine Transitions**
   - Model the alert lifecycle (open -> in_progress -> closed)
   - Verify `ValidTransitionsOnly` and `NoLostUpdates`
   - Model nested lock behavior

5. **Write Spec 3: Circuit Breaker**
   - Model the three-state machine with probe serialization
   - Verify `SingleProbe` and `EventualRecovery`
   - Find probe timeout parameter bounds

### Phase 3: Complex Workflows (Weeks 5-6)

6. **Write Spec 4: Chain Execution**
   - Model sequential and branching chains
   - Verify `StepOrdering`, `StepIdempotency`, and `ChainTermination`
   - Add parallel step groups (increases state space significantly)

7. **Write Spec 5: Approval Lifecycle**
   - Model the human-in-the-loop workflow
   - Verify `ApprovalFinalityOnce` and `MutualExclusion`

### Phase 4: Multi-Node Consistency (Weeks 7-8)

8. **Write Spec 6: Group Flush**
   - Model multi-node group accumulation
   - Verify `NoEventLoss` and `FlushIdempotency`
   - Use findings to improve multi-node group consistency

9. **Document findings and recommendations**
   - Catalog any bugs found
   - Propose code fixes based on TLA+ insights
   - Establish spec maintenance process

### Ongoing

- **Update specs when protocols change**: Treat TLA+ specs as living documentation
- **Run enlarged parameter sweeps**: Nightly CI with larger state spaces
- **Onboard team members**: TLA+ workshop (2-3 hours) for engineers working on
  concurrency-critical code

---

## 9. PlusCal: An Alternative Entry Point

For engineers more comfortable with imperative code than mathematical notation, **PlusCal**
is an algorithm-level language that translates to TLA+. It looks closer to pseudocode:

```pcal
(* --algorithm DispatchDedup
variables
    lock = "free",
    dedup_store = [k \in DEDUP_KEYS |-> FALSE],
    executed = [a \in ACTIONS |-> 0];

process gateway \in GATEWAYS
variables action \in ACTIONS;
begin
  AcquireLock:
    await lock = "free";
    lock := self;

  CheckDedup:
    if dedup_store[action.dedup_key] = FALSE then
      dedup_store[action.dedup_key] := TRUE;
      goto Execute;
    else
      goto Release;
    end if;

  Execute:
    executed[action.id] := executed[action.id] + 1;

  Release:
    lock := "free";
end process;
end algorithm; *)
```

PlusCal is recommended for:
- Initial prototyping of specifications
- Team members new to formal methods
- Algorithms with sequential control flow

For complex concurrent specifications with multiple interleaving processes, pure TLA+
provides more expressive power.

---

## 10. Trace Validation: Bridging TLA+ Specs and Rust Code

The gap between a TLA+ specification and the Rust implementation is a practical challenge.
The most effective approach for Acteon combines three strategies:

### 10.1 Design-Level Verification (Primary)

Use TLA+ to model-check the *design* of Acteon's concurrency protocols before
implementation. This is the approach used by AWS, Microsoft, and CockroachDB:

1. Write a TLA+ spec of the algorithm (e.g., dispatch dedup protocol)
2. Model-check exhaustively with TLC to find design bugs
3. Fix the design in the spec
4. Implement the corrected design in Rust
5. Treat the TLA+ spec as a precise design document

### 10.2 Runtime Trace Validation (Secondary)

The `acteon-simulation` crate is uniquely positioned for **trace validation**. This
approach, pioneered by Ron Pressler and used by MongoDB for conformance checking, works
as follows:

1. Instrument the simulation harness to emit structured trace logs
2. Each trace entry records: action taken, state before, state after
3. Feed traces to TLC's trace-checking mode
4. TLC verifies that every trace is a valid behavior of the TLA+ spec

This is practical because:
- The simulation framework already has `RecordingProvider` that captures all calls
- Multi-node simulation with shared state mirrors the TLA+ model
- No production instrumentation needed; validation happens in CI

Example trace format:
```json
{"step": 1, "action": "AcquireLock", "gateway": "g1", "lock": "dispatch:ns:t:a1", "result": "acquired"}
{"step": 2, "action": "CheckAndSet", "gateway": "g1", "key": "dedup:k1", "result": "new"}
{"step": 3, "action": "Execute", "gateway": "g1", "action_id": "a1", "provider": "email"}
{"step": 4, "action": "ReleaseLock", "gateway": "g1", "lock": "dispatch:ns:t:a1"}
```

### 10.3 Property-Based Testing as Bridge

Use Rust's `proptest` or `quickcheck` crates to generate random action sequences and
verify that the implementation satisfies the same invariants as the TLA+ spec:

```rust
proptest! {
    #[test]
    fn dedup_safety(actions in vec(arb_action(), 1..10)) {
        let harness = SimulationHarness::multi_node_memory(3).await;
        let results = dispatch_all(&harness, actions).await;
        // Same invariant as TLA+ spec: at most one execution per dedup key
        for key in dedup_keys(&actions) {
            assert!(count_executed(&results, key) <= 1);
        }
    }
}
```

### 10.4 Complementary Rust Tools

| Tool | Purpose | Complements TLA+ For |
|------|---------|---------------------|
| **Kani** (AWS) | Rust model checker via CBMC | Memory safety, panic freedom in critical paths |
| **Miri** | Rust interpreter for UB detection | Unsafe code, data race detection |
| **proptest** | Property-based testing | Randomized invariant checking at code level |
| **Loom** | Concurrency testing | Exploring thread interleavings in `std::sync` code |

---

## 11. Expected Outcomes

### 11.1 What TLA+ Will Find

Based on the patterns identified in the codebase, TLA+ modeling is likely to:

1. **Confirm** that dedup CAS provides adequate double-execution protection even when
   dispatch locks expire (validating the current design)

2. **Identify** the exact timing conditions under which circuit breaker dual-probes
   can occur, enabling precise timeout parameter tuning

3. **Reveal** whether chain step-dedup keys are sufficient to guarantee exactly-once
   step execution under worker crashes

4. **Expose** the multi-node group split-brain issue (if present), leading to a design
   improvement for distributed group accumulation

5. **Quantify** the approval expiry race window, informing whether a distributed lock
   is needed for approval decisions

### 11.2 ROI Estimation

- **Cost**: ~40-60 engineer-hours for Phase 1-2 (foundation + core protocols)
- **Benefit**: Proactive discovery of concurrency bugs that would otherwise manifest
  as rare production incidents (data loss, double execution, stuck workflows)
- **Industry benchmark**: Amazon reported that TLA+ specs caught bugs that survived
  months of testing and code review, some of which would have caused data loss in
  production

---

## 12. References

- Lamport, L. *Specifying Systems: The TLA+ Language and Tools for Hardware and Software Engineers*. Addison-Wesley, 2002.
- Newcombe, C. et al. "How Amazon Web Services Uses Formal Methods." *Communications of the ACM*, 58(4), 2015.
- Lamport, L. "The TLA+ Home Page." https://lamport.azurewebsites.net/tla/tla.html
- TLA+ Foundation. "Learn TLA+." https://learntla.com
- Hillel Wayne. *Practical TLA+*. Apress, 2018.
- Davis, J. et al. "Extreme Modelling in Practice." *VLDB 2020* (CockroachDB parallel commits verification).
