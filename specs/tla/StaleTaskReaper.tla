------------------------- MODULE StaleTaskReaper -------------------------
(*
 * TLA+ specification of Acteon's A2A stale-task reaper's FRESH-staleness
 * re-check — the reaper-vs-heartbeat race.
 *
 * Models:
 *   - crates/gateway/src/background/workers/stale_task.rs : the reaper worker
 *     SCANS for tasks past their working_ttl, then for each candidate calls
 *     fail_if_stale. The scan and fail_if_stale are SEPARATE reads — a producer
 *     can heartbeat in between.
 *   - crates/gateway/src/task_engine.rs :: fail_if_stale (~383), a CAS loop:
 *         let (raw, version) = state.get_versioned(key)?;   // (re-)LOAD row+version
 *         let mut task = deserialize(raw);
 *         if !task.is_stale_at(now) { return Ok(None); }     // FRESH re-check on the
 *                                                            // loaded row
 *         task.transition_to(Failed, reason);
 *         compare_and_swap(key, version, payload)?;          // commit iff version eq
 *     On Conflict the loop re-reads and re-evaluates. Doc (verbatim): "The
 *     staleness re-check runs against the *fresh* CAS-loaded row, so a task a
 *     producer just heartbeated is never failed out from under it."
 *   - crates/core/src/bus_task.rs :: Task::is_stale_at (~925): a NON-TERMINAL
 *     task past its working_ttl without progress. Terminal tasks are NEVER stale.
 *
 * Why the FRESH re-check is the load-bearing mechanism (not just the version-CAS).
 * The reaper decides to attempt a reap from its OUTER SCAN (which saw the row
 * stale). fail_if_stale then does its OWN get_versioned + is_stale_at re-check.
 * A heartbeat that lands BETWEEN the scan and that load makes the loaded row
 * already-fresh: its version is the post-heartbeat version, so the version-CAS
 * would PASS — only the fresh is_stale_at re-check (re-read on the loaded row)
 * refuses the reap. The version-CAS is the complementary gate for a heartbeat
 * that lands between the load and the commit. This spec models all three steps
 * (Scan, Load, Commit) so the fresh re-check is genuinely load-bearing.
 *
 * Protocol. A single task row carries state ("working" | "done"), a `stale`
 * boolean (is_stale_at on a non-terminal row), and a CAS `version`. A PRODUCER
 * may Heartbeat (clears stale, bumps version) or Complete (-> terminal, bumps
 * version). The REAPER runs in THREE steps:
 *   ReaperScan  — the worker's scan flags this row as a stale candidate (it
 *                 currently looks stale). No version is captured here.
 *   ReaperLoad  — fail_if_stale's get_versioned: capture the row's version NOW.
 *   ReaperCommit— commit Failed IFF, at the commit instant, the FRESH row is
 *                 still stale-non-terminal (is_stale_at re-check) AND the version
 *                 is unchanged since the load (version-CAS).
 *
 * Verified (over every interleaving of producer heartbeat/complete, the clock
 * setting staleness, and the reaper scan/load/commit):
 *   - NoReapHeartbeated: the reaper never commits Failed against a row that, at
 *     the commit instant, is NOT stale (heartbeated, or terminal). Checked by an
 *     INDEPENDENT ground-truth flag `reaped_live`, computed from the FRESH
 *     task_state/stale at commit — NOT from the reaper's own guard — so a buggy
 *     guard cannot self-mask the violation. Must stay FALSE.
 *   - ReapOnlyStale: a committed Failed implies the prior row was non-terminal
 *     and stale (no terminal row is ever re-failed) — `reap_from_terminal` stays
 *     FALSE.
 *   - ReapAtMostOnce: at most one reaper Failed-commit per occurrence.
 *
 * Negative check: drop the FRESH is_stale_at re-check at ReaperCommit — commit
 * Failed on the scan's candidacy as long as the version is unchanged since the
 * load (version-CAS only). A producer that Heartbeats (clears stale, bumps
 * version) BETWEEN the scan and the load then presents a fresh row whose version
 * the load captures, so the version-CAS passes; without the fresh re-check the
 * reaper fails this live, just-heartbeated row -> reaped_live flips TRUE ->
 * NoReapHeartbeated violated. (The same revert lets a Completed terminal row be
 * re-failed -> ReapOnlyStale / ReapAtMostOnce violated.)
 *
 * SCOPE. Isolates the reaper-vs-heartbeat race for a SINGLE task row. Staleness
 * is a boolean a Heartbeat clears and the clock can set (the working_ttl_ms /
 * last_progress_at arithmetic is abstracted). Abstracts the audit/stream
 * emission, the per-key dispatch lock, the MAX_CAS_RETRY_ATTEMPTS bound (the
 * loser re-reads), and the legal-transition graph (Failed is always legal from
 * non-terminal, covered by A2aTaskTransition). The version-CAS is faithfully
 * present (the complementary gate for the load->commit window) but in this
 * single-row abstraction the FRESH re-check is the independently load-bearing
 * mechanism. `Recycle` mints a fresh working task once terminal, so the system
 * CYCLES (no benign terminal deadlock under -deadlock).
 *
 * Run with:
 *   java -jar tla2tools.jar -config StaleTaskReaper.cfg StaleTaskReaper.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Producers,   \* concurrent producers heartbeating/completing, e.g. {p1, p2}
    MaxVer       \* state-space bound on the version counter

Working == "working"
Done    == "done"
States  == { Working, Done }

\* is_stale_at(now) ground truth on the FRESH row (bus_task.rs:925): a NON-TERMINAL
\* row past its TTL without progress. Terminal rows are never stale. The SINGLE
\* staleness oracle; both the (correct) reaper guard and the invariant's
\* independent flag read it off the FRESH task_state/stale.
IsStaleAt(st, isStale) == (st = Working) /\ isStale

VARIABLES
    task_state,         \* persisted row state: Working | Done
    stale,              \* BOOLEAN: row past TTL without progress (non-terminal)
    version,            \* optimistic-CAS version, bumped on every committed write
    reaped_live,        \* BOOLEAN ground-truth oracle: a Failed-commit landed on a NOT-stale row
    reap_count,         \* 0..3: reaper Failed-commits for the CURRENT occurrence
    reap_from_terminal, \* BOOLEAN: a reaper Failed-commit fired on a terminal row
    r_phase,            \* reaper: "idle" | "scanned" | "committing"
    r_load_ver          \* 0..MaxVer: version captured at ReaperLoad (get_versioned)

vars == <<task_state, stale, version, reaped_live, reap_count,
          reap_from_terminal, r_phase, r_load_ver>>

TypeOK ==
    /\ task_state \in States
    /\ stale \in BOOLEAN
    /\ version \in 0..MaxVer
    /\ reaped_live \in BOOLEAN
    /\ reap_count \in 0..3
    /\ reap_from_terminal \in BOOLEAN
    /\ r_phase \in {"idle", "scanned", "committing"}
    /\ r_load_ver \in 0..MaxVer

Init ==
    /\ task_state = Working
    /\ stale = FALSE
    /\ version = 0
    /\ reaped_live = FALSE
    /\ reap_count = 0
    /\ reap_from_terminal = FALSE
    /\ r_phase = "idle"
    /\ r_load_ver = 0

\* -----------------------------------------------------------------------
\* PRODUCER actions (the heartbeat / completion side of the race)
\* -----------------------------------------------------------------------

\* The working_ttl elapses with no progress: is_stale_at flips TRUE. Only a
\* non-terminal row can become stale. Does NOT bump the version (staleness is
\* derived on read, not a persisted write).
ClockStale ==
    /\ task_state = Working
    /\ ~stale
    /\ stale' = TRUE
    /\ UNCHANGED <<task_state, version, reaped_live, reap_count,
                   reap_from_terminal, r_phase, r_load_ver>>

\* Producer records progress (the heartbeat): clears staleness, bumps the version.
\* This is what must NEVER be reaped out from under the producer.
Heartbeat(p) ==
    /\ task_state = Working
    /\ version < MaxVer
    /\ stale' = FALSE
    /\ version' = version + 1
    /\ UNCHANGED <<task_state, reaped_live, reap_count, reap_from_terminal,
                   r_phase, r_load_ver>>

\* Producer drives the task to a terminal state: bumps the version; a terminal
\* row is never stale.
Complete(p) ==
    /\ task_state = Working
    /\ version < MaxVer
    /\ task_state' = Done
    /\ stale' = FALSE
    /\ version' = version + 1
    /\ UNCHANGED <<reaped_live, reap_count, reap_from_terminal, r_phase, r_load_ver>>

\* -----------------------------------------------------------------------
\* REAPER actions (the worker scan, then fail_if_stale modeled as Load + Commit)
\* -----------------------------------------------------------------------

\* ReaperScan: the worker's scan flags this row as a stale candidate (it looks
\* stale right now). The candidacy decision is made HERE; no version is captured.
\* A heartbeat may land between this scan and the get_versioned load below.
ReaperScan ==
    /\ r_phase = "idle"
    /\ IsStaleAt(task_state, stale)
    /\ r_phase' = "scanned"
    /\ UNCHANGED <<task_state, stale, version, reaped_live, reap_count,
                   reap_from_terminal, r_load_ver>>

\* ReaperLoad: fail_if_stale's get_versioned — capture the row's CURRENT version.
\* (If a heartbeat landed since the scan, this captures the post-heartbeat
\* version, so the version-CAS below would pass — only the fresh re-check saves it.)
ReaperLoad ==
    /\ r_phase = "scanned"
    /\ r_load_ver' = version
    /\ r_phase' = "committing"
    /\ UNCHANGED <<task_state, stale, version, reaped_live, reap_count,
                   reap_from_terminal>>

\* ReaperCommit: the CAS commit of fail_if_stale.
\*   - FRESH re-check: is_stale_at re-evaluated against the CURRENT row
\*     (`if !task.is_stale_at(now) return Ok(None)`). THE load-bearing guard.
\*   - version-CAS: commit only if the version is unchanged since ReaperLoad.
\* reaped_live / reap_from_terminal are recorded from the FRESH task_state/stale
\* at the commit instant, INDEPENDENT of the reaper's own guard.
ReaperCommit ==
    /\ r_phase = "committing"
    \* `version < MaxVer` bounds the CI counter (saturated -> no-write branch,
    \* modeling MAX_CAS_RETRY_ATTEMPTS exhaustion). NOT a faithfulness guard.
    /\ IF (version < MaxVer) /\ (r_load_ver = version) /\ IsStaleAt(task_state, stale)
       THEN /\ task_state' = Done
            /\ stale' = FALSE
            /\ version' = version + 1
            /\ reap_count' = reap_count + 1
            \* Independent ground truth: did this Failed land on a NOT-stale row?
            \* (terminal or progressed). With the correct guard, impossible.
            /\ reaped_live' = (reaped_live \/ ~IsStaleAt(task_state, stale))
            /\ reap_from_terminal' = (reap_from_terminal \/ (task_state = Done))
       ELSE UNCHANGED <<task_state, stale, version, reap_count,
                        reaped_live, reap_from_terminal>>
    /\ r_phase' = "idle"
    /\ r_load_ver' = 0

\* -----------------------------------------------------------------------
\* Recycle: mint a fresh working task at the same key once the reaper is idle and
\* EITHER the row is terminal OR the version counter saturated the CI bound while
\* still Working. The system CYCLES so there is no benign terminal deadlock.
\* -----------------------------------------------------------------------
Recycle ==
    /\ r_phase = "idle"
    /\ (task_state = Done) \/ (version = MaxVer)
    /\ task_state' = Working
    /\ stale' = FALSE
    /\ version' = 0
    /\ reap_count' = 0
    /\ reap_from_terminal' = FALSE
    /\ r_load_ver' = 0
    /\ UNCHANGED <<reaped_live, r_phase>>

Next ==
    \/ ClockStale
    \/ \E p \in Producers : Heartbeat(p) \/ Complete(p)
    \/ ReaperScan
    \/ ReaperLoad
    \/ ReaperCommit
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* The reaper NEVER fails a row that was not stale at the commit instant — a row
\* that heartbeated (cleared stale) or reached terminal between the scan and the
\* commit is refused by the fresh is_stale_at re-check. `reaped_live` is the
\* INDEPENDENT oracle (read off the fresh task_state/stale at commit, not the
\* reaper guard), so a buggy guard cannot self-mask this. Must stay FALSE.
NoReapHeartbeated == reaped_live = FALSE

\* A committed Failed reap only ever fires on a genuinely-stale, NON-TERMINAL row:
\* a terminal row is never (re-)failed by the reaper.
ReapOnlyStale == reap_from_terminal = FALSE

\* At most one reaper Failed-commit per occurrence: once terminal the reaper makes
\* no further Failed write.
ReapAtMostOnce == reap_count <= 1

============================================================================
