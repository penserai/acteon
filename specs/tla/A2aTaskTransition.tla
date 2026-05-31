------------------------- MODULE A2aTaskTransition -------------------------
(*
 * TLA+ specification of Acteon's A2A Task state machine under concurrent
 * optimistic-CAS transitions.
 *
 * Models:
 *   - crates/core/src/bus_task.rs :: TaskState::can_transition_to — the exact
 *     allowed graph, enforced by Task::transition_to (which errors on an
 *     illegal transition, validating against the task's CURRENT state).
 *   - crates/gateway/src/task_engine.rs :: cas_mutate (~line 808) and
 *     transition_task (~line 327). The mutation is OPTIMISTIC version-CAS:
 *         let (raw, version) = state.get_versioned(key)?;   // READ (state+version)
 *         let mut task = deserialize(raw);
 *         mutate(&mut task);            // task.transition_to(next) RE-VALIDATES
 *                                       // can_transition_to against the FRESH
 *                                       // loaded state, errors if illegal
 *         compare_and_swap(key, version, serialize(task))?;  // COMMIT iff version
 *                                                            // unchanged
 *     On CasResult::Conflict the loop RE-READS and RE-APPLIES the closure
 *     against the new fresh state (up to MAX_CAS_RETRY_ATTEMPTS).
 *
 * Protocol. A single Task row carries a `state` and a `version` counter.
 * N concurrent actors each attempt a transition (a worker advancing
 * Working -> Completed/Failed/interrupt; a user resuming an interrupt to
 * Working; a canceller -> Canceled). Each actor:
 *   1. READS (state, version) and picks an intended `next` that is legal
 *      from the READ state.
 *   2. COMMITS via version-CAS — the commit succeeds ONLY IF the version is
 *      unchanged AND can_transition_to(current_state, next) holds against the
 *      FRESH row (faithful to transition_to re-validating on the fresh load).
 * On a version conflict the actor re-reads and retries. Because the version
 * bumps on every committed transition, the loser of a race re-reads the
 * winner's new state; its intended transition may now be illegal -> it is
 * refused, never committed.
 *
 * The READ and the COMMIT are SEPARATE steps so the version-CAS is load-
 * bearing (a stale read can survive a concurrent commit), mirroring the
 * read/commit split in MessageBus.tla. With version-CAS, a stale committer
 * loses the CAS and never overwrites the winner's state.
 *
 * Verified (over every interleaving of concurrent actors):
 *   - AllTransitionsLegal: every COMMITTED transition obeys can_transition_to;
 *     the `illegal_commit` flag (set TRUE if any commit moves from S to T with
 *     ~can_transition_to(S,T)) stays FALSE.
 *   - TerminalStaysTerminal: once the task is in a terminal state it never
 *     changes state again (until an explicit Recycle replaces the row).
 *
 * Negative check: replace the version-CAS commit with a BLIND versionless
 * write (drop the `version = read_version` guard AND re-validate against the
 * STALE read state instead of the fresh row). A slow actor that read Working
 * then commits AFTER a concurrent actor already drove the task to a terminal
 * state overwrites that terminal state with an illegal transition out of it ->
 * AllTransitionsLegal and TerminalStaysTerminal are both violated.
 *
 * SCOPE. Isolates the optimistic version-CAS + fresh re-validation layer of a
 * single Task row. It abstracts away: the dispatch/per-key lock, the message
 * dedup key, history/artifact mutations, the stale-task reaper, and chain
 * linkage — those are separate concerns. `Recycle` models a fresh Task minted
 * at the same key once the prior one is terminal, so the system CYCLES (no
 * benign terminal deadlock under -deadlock).
 *
 * Run with:
 *   java -jar tla2tools.jar -config A2aTaskTransition.cfg A2aTaskTransition.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Actors,    \* concurrent transition attempts / gateway replicas, e.g. {w1, w2}
    MaxVer,    \* state-space bound on the version counter
    NOBODY     \* sentinel: actor holds no in-flight read (or no target picked)

\* -----------------------------------------------------------------------
\* The eight A2A task states (crates/core/src/bus_task.rs::TaskState).
\* -----------------------------------------------------------------------
Submitted     == "submitted"
Working       == "working"
Completed     == "completed"
Failed        == "failed"
Canceled      == "canceled"
InputRequired == "input_required"
AuthRequired  == "auth_required"
Rejected      == "rejected"

States == { Submitted, Working, Completed, Failed, Canceled,
            InputRequired, AuthRequired, Rejected }

Terminal == { Completed, Failed, Canceled, Rejected }

\* TaskState::can_transition_to — the EXACT allowed graph from bus_task.rs.
\* An operator (not a .cfg function literal) so the .cfg stays clean.
CanTransition(s, t) ==
    \/ /\ s = Submitted     /\ t \in { Working, Canceled, Failed, Rejected }
    \/ /\ s = Working       /\ t \in { Completed, Failed, Canceled,
                                       InputRequired, AuthRequired }
    \/ /\ s \in { InputRequired, AuthRequired }
       /\ t \in { Working, Canceled, Failed }
    \* Terminal states have no outgoing transitions.

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    task_state,      \* current persisted state of the single Task row
    version,         \* optimistic-CAS version, bumped on every committed transition
    illegal_commit,  \* BOOLEAN: TRUE iff any commit ever broke can_transition_to
    a_phase,         \* [Actors -> {"idle","committing"}]
    a_read_state,    \* [Actors -> States \cup {NOBODY}]: state observed at READ
    a_read_ver,      \* [Actors -> 0..MaxVer]: version observed at READ
    a_target         \* [Actors -> States \cup {NOBODY}]: intended next state

TypeOK ==
    /\ task_state \in States
    /\ version \in 0..MaxVer
    /\ illegal_commit \in BOOLEAN
    /\ a_phase \in [Actors -> {"idle", "committing"}]
    /\ a_read_state \in [Actors -> States \cup {NOBODY}]
    /\ a_read_ver \in [Actors -> 0..MaxVer]
    /\ a_target \in [Actors -> States \cup {NOBODY}]

Init ==
    /\ task_state = Submitted
    /\ version = 0
    /\ illegal_commit = FALSE
    /\ a_phase = [a \in Actors |-> "idle"]
    /\ a_read_state = [a \in Actors |-> NOBODY]
    /\ a_read_ver = [a \in Actors |-> 0]
    /\ a_target = [a \in Actors |-> NOBODY]

\* -----------------------------------------------------------------------
\* Actions
\* -----------------------------------------------------------------------

\* READ phase: get_versioned(key) -> (state, version), then pick an intended
\* `next` that is LEGAL from the just-read state — what transition_task's
\* caller chooses. The snapshot may be stale by the time the actor commits.
\* A version conflict re-enters this action (re-read + re-apply), so a
\* committing actor may also re-read here.
Read(a, t) ==
    /\ a_phase[a] = "idle"
    /\ CanTransition(task_state, t)
    /\ a_read_state' = [a_read_state EXCEPT ![a] = task_state]
    /\ a_read_ver'   = [a_read_ver   EXCEPT ![a] = version]
    /\ a_target'     = [a_target     EXCEPT ![a] = t]
    /\ a_phase'      = [a_phase      EXCEPT ![a] = "committing"]
    /\ UNCHANGED <<task_state, version, illegal_commit>>

\* COMMIT phase: version-CAS. compare_and_swap(key, read_version, payload)
\* succeeds ONLY IF the version is unchanged since the read. transition_to
\* additionally RE-VALIDATES can_transition_to against the FRESH loaded row;
\* with the version-CAS the fresh row IS the read row, so re-validation always
\* holds on the winning path — modeled explicitly to keep the guard faithful.
CommitWin(a) ==
    /\ a_phase[a] = "committing"
    /\ version < MaxVer                          \* bound the version counter (CI)
    /\ a_read_ver[a] = version                  \* CAS: version unchanged
    /\ CanTransition(task_state, a_target[a])    \* transition_to re-validation
    /\ task_state' = a_target[a]
    /\ version' = version + 1
    \* Track any illegal committed move; with the guards above it stays FALSE.
    /\ illegal_commit' =
         (illegal_commit \/ ~CanTransition(task_state, a_target[a]))
    /\ a_phase'      = [a_phase      EXCEPT ![a] = "idle"]
    /\ a_read_state' = [a_read_state EXCEPT ![a] = NOBODY]
    /\ a_target'     = [a_target     EXCEPT ![a] = NOBODY]
    /\ UNCHANGED a_read_ver

\* COMMIT conflict: a concurrent actor bumped the version since this actor's
\* read (CasResult::Conflict). The actor abandons its stale attempt and
\* returns to idle to re-read (the cas_mutate retry loop).
CommitConflict(a) ==
    /\ a_phase[a] = "committing"
    /\ a_read_ver[a] # version                  \* CAS failed: version moved
    /\ a_phase'      = [a_phase      EXCEPT ![a] = "idle"]
    /\ a_read_state' = [a_read_state EXCEPT ![a] = NOBODY]
    /\ a_target'     = [a_target     EXCEPT ![a] = NOBODY]
    /\ UNCHANGED <<task_state, version, illegal_commit, a_read_ver>>

\* Once the version counter saturates the bound, an actor mid-flight gives up
\* (CAS attempts exhausted / no further progress representable). Keeps the
\* state space finite without falsely deadlocking before Recycle can fire.
GiveUp(a) ==
    /\ a_phase[a] = "committing"
    /\ version = MaxVer
    /\ a_phase'      = [a_phase      EXCEPT ![a] = "idle"]
    /\ a_read_state' = [a_read_state EXCEPT ![a] = NOBODY]
    /\ a_target'     = [a_target     EXCEPT ![a] = NOBODY]
    /\ UNCHANGED <<task_state, version, illegal_commit, a_read_ver>>

\* Recycle: once the Task is terminal and no actor is mid-flight, a fresh Task
\* is minted at the same key (Submitted, version reset). The system CYCLES so
\* there is no benign terminal deadlock under -deadlock.
Recycle ==
    /\ task_state \in Terminal
    /\ \A a \in Actors : a_phase[a] = "idle"
    /\ task_state' = Submitted
    /\ version' = 0
    /\ a_read_state' = [a \in Actors |-> NOBODY]
    /\ a_read_ver'   = [a \in Actors |-> 0]
    /\ a_target'     = [a \in Actors |-> NOBODY]
    /\ UNCHANGED <<illegal_commit, a_phase>>

Next ==
    \/ \E a \in Actors :
        \/ \E t \in States : Read(a, t)
        \/ CommitWin(a)
        \/ CommitConflict(a)
        \/ GiveUp(a)
    \/ Recycle

vars == <<task_state, version, illegal_commit,
          a_phase, a_read_state, a_read_ver, a_target>>

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* Every committed transition obeys can_transition_to. The flag is set TRUE
\* by any commit that moves from S to T with ~CanTransition(S, T); it must
\* stay FALSE under the version-CAS + fresh re-validation.
AllTransitionsLegal == illegal_commit = FALSE

\* Once the task is in a terminal state it makes no further transition: no
\* actor mid-flight that read a non-terminal state can still win the CAS and
\* overwrite the terminal row. (Recycle is the only path out, and it mints a
\* brand-new task rather than transitioning the terminal one.) Encoded as a
\* two-state action property: from any terminal state, the only allowed
\* same-row change is a Recycle (version drops to 0).
TerminalStaysTerminal ==
    [][ (task_state \in Terminal /\ task_state' # task_state)
        => (task_state' = Submitted /\ version' = 0) ]_vars

============================================================================
