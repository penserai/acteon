----------------------------- MODULE RetentionReaper -----------------------------
(*
 * TLA+ specification of Acteon's data-retention reaper racing live writes.
 *
 * Models crates/gateway/src/background/workers/retention.rs:
 *   - run_retention_reaper (~16): loads the enabled RetentionPolicy rows, then
 *     calls reap_chains_optimized / reap_events_optimized.
 *   - reap_chains_optimized (~103) / reap_events_optimized (~196): each does ONE
 *     scan_keys_by_kind(...) — a snapshot of (key, raw_value) pairs — then loops:
 *         if policy.compliance_hold { skipped += 1; continue; }   // hold-skip
 *         ...
 *         let cutoff = now - ttl;
 *         let ts = <parsed from the SCANNED raw_value>;            // age from snapshot
 *         if ts < cutoff {                                         // expiry-check
 *             self.state.delete(&state_key).await ...              // UNCONDITIONAL by-key
 *         }
 *     The delete decision (held? expired?) is computed from the value the reaper
 *     SCANNED. The delete itself (crates/state/memory/src/store.rs:227,
 *     trait crates/state/state/src/store.rs:53) is `delete(key)` — a plain
 *     remove-by-key. It does NOT re-read the record and does NOT re-check
 *     compliance_hold or the age at delete time. (gateway.rs:~1343 resolves the
 *     audit TTL — compliance_hold => None => never expires — but that is the same
 *     hold-skip decision, computed from the loaded policy, not re-checked on the
 *     persisted record at delete.)
 *
 * Protocol. A small set of records, each with a current compliance hold flag and a
 * current expiry flag (refreshed => not expired). The reaper processes each record
 * as TWO separate steps so a concurrent live write can interleave between them:
 *   Scan(r):   reads the record's CURRENT held/expired and freezes a per-record
 *              verdict — delete IFF (NOT held AND expired), else keep — exactly the
 *              continue/skip cascade above. It also records scan_held/scan_expired:
 *              what was true the instant the reaper looked.
 *   Delete(r): if the frozen verdict is "delete", removes the record by key,
 *              UNCONDITIONALLY (the bare state.delete). No re-read, no re-check.
 * Concurrent live writes (any time a record is present):
 *   Refresh(r): a live write resets the record's age   -> expired := FALSE.
 *   SetHold(r): a live write sets compliance_hold       -> held    := TRUE.
 *   Age(r):     wall-clock passes the cutoff            -> expired := TRUE.
 *
 * Verified (over every interleaving of the two-step reaper and concurrent live
 * writes), anchored on what the code ACTUALLY guarantees — the SCAN-TIME state,
 * because the delete acts on the verdict frozen at the scan:
 *   - NeverDeleteScanHeld: a record that was on compliance hold AT SCAN TIME is
 *     never deleted (the `if policy.compliance_hold { continue }` skip).
 *   - NeverDeleteScanLive: a record that was NOT expired AT SCAN TIME (e.g. just
 *     refreshed before the reaper looked) is never deleted (the `if ts < cutoff`
 *     expiry-check).
 * These two guards are INDEPENDENT: each alone is load-bearing (see Negative).
 *
 * Negative check (each guard reverted SEPARATELY — each trips ONLY its own
 * invariant, proving the two are independently load-bearing, not offsetting):
 *   (1) Drop the hold-skip, KEEPING the expiry-check: in Scan let
 *       verdict = "delete" iff `expired` (ignoring `held`). A record held at scan
 *       is then deleted -> NeverDeleteScanHeld violated, while NeverDeleteScanLive
 *       still holds (a not-expired record is still kept).
 *   (2) Drop the expiry-check, KEEPING the hold-skip: in Scan let
 *       verdict = "delete" iff `~held` (ignoring `expired`). A not-expired
 *       (just-refreshed) unheld record is then deleted -> NeverDeleteScanLive
 *       violated, while NeverDeleteScanHeld still holds (a held record is kept).
 *
 * SCOPE. Abstracts the policy/record hold and the TTL-vs-now comparison to per-
 * record BOOLEAN flags (held, expired), the per-kind scan to a per-record Scan,
 * and the store delete to remove-by-key. The terminal-status / resolved-state
 * gating (only completed chains / resolved events are eligible) and the metrics
 * are out of scope. IMPORTANT — the TOCTOU window is REAL and modeled faithfully:
 * the code re-checks NOTHING at delete time, so a record that becomes held or gets
 * refreshed AFTER the reaper scanned it but BEFORE the by-key delete fires IS
 * still deleted. That is why the verified invariants are stated over scan-time
 * state (scan_held/scan_expired), NOT current state — the current-state versions
 * (NeverDeleteHeldNow / NeverDeleteLiveNow, defined below as documentation) do NOT
 * hold and are deliberately NOT listed as INVARIANTS.
 *
 * Run with:
 *   java -jar tla2tools.jar -config RetentionReaper.cfg RetentionReaper.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Records,  \* the records under one retention policy, e.g. {r1, r2}
    NOBODY    \* sentinel: no frozen verdict (record not yet scanned this pass)

VARIABLES
    held,         \* [Records -> BOOLEAN]  current compliance_hold on the record
    expired,      \* [Records -> BOOLEAN]  current: record older than the TTL cutoff
    present,      \* [Records -> BOOLEAN]  record still exists in the store
    rphase,       \* [Records -> {"unscanned","scanned","done"}] reaper progress
    verdict,      \* [Records -> {"delete","keep"} \cup {NOBODY}] frozen at Scan
    scan_held,    \* [Records -> BOOLEAN]  held value the reaper OBSERVED at Scan
    scan_expired, \* [Records -> BOOLEAN]  expired value the reaper OBSERVED at Scan
    deleted       \* SUBSET Records: records the reaper has removed this pass

vars == <<held, expired, present, rphase, verdict, scan_held, scan_expired, deleted>>

TypeOK ==
    /\ held \in [Records -> BOOLEAN]
    /\ expired \in [Records -> BOOLEAN]
    /\ present \in [Records -> BOOLEAN]
    /\ rphase \in [Records -> {"unscanned", "scanned", "done"}]
    /\ verdict \in [Records -> {"delete", "keep", NOBODY}]
    /\ scan_held \in [Records -> BOOLEAN]
    /\ scan_expired \in [Records -> BOOLEAN]
    /\ deleted \subseteq Records

Init ==
    /\ held = [r \in Records |-> FALSE]
    /\ expired = [r \in Records |-> FALSE]
    /\ present = [r \in Records |-> TRUE]
    /\ rphase = [r \in Records |-> "unscanned"]
    /\ verdict = [r \in Records |-> NOBODY]
    /\ scan_held = [r \in Records |-> FALSE]
    /\ scan_expired = [r \in Records |-> FALSE]
    /\ deleted = {}

\* --------------------------------------------------------------------------
\* Concurrent LIVE WRITES (may interleave between a record's Scan and Delete).
\* --------------------------------------------------------------------------

\* Wall-clock advances past the cutoff: the record becomes eligible by age. Only
\* meaningful while the record exists. (Source of work + part of the recycle.)
Age(r) ==
    /\ present[r]
    /\ ~expired[r]
    /\ expired' = [expired EXCEPT ![r] = TRUE]
    /\ UNCHANGED <<held, present, rphase, verdict, scan_held, scan_expired, deleted>>

\* A live write touches the record and resets its age -> no longer expired.
Refresh(r) ==
    /\ present[r]
    /\ expired[r]
    /\ expired' = [expired EXCEPT ![r] = FALSE]
    /\ UNCHANGED <<held, present, rphase, verdict, scan_held, scan_expired, deleted>>

\* A live write places the record under compliance hold.
SetHold(r) ==
    /\ present[r]
    /\ ~held[r]
    /\ held' = [held EXCEPT ![r] = TRUE]
    /\ UNCHANGED <<expired, present, rphase, verdict, scan_held, scan_expired, deleted>>

\* --------------------------------------------------------------------------
\* The REAPER — two separate steps per record.
\* --------------------------------------------------------------------------

\* Scan(r): read the record's CURRENT held/expired and freeze the verdict, exactly
\* mirroring reap_*_optimized's per-record cascade:
\*     if policy.compliance_hold { skip }       -> held  => keep
\*     if !expired { continue }                 -> !expired => keep
\*     else (expired & not held)                -> delete
\* scan_held/scan_expired capture what was observed, for the scan-time invariants.
Scan(r) ==
    /\ present[r]
    /\ rphase[r] = "unscanned"
    /\ scan_held' = [scan_held EXCEPT ![r] = held[r]]
    /\ scan_expired' = [scan_expired EXCEPT ![r] = expired[r]]
    /\ verdict' = [verdict EXCEPT ![r] =
                     IF (~held[r]) /\ expired[r] THEN "delete" ELSE "keep"]
    /\ rphase' = [rphase EXCEPT ![r] = "scanned"]
    /\ UNCHANGED <<held, expired, present, deleted>>

\* Delete(r): act on the FROZEN verdict. The store delete is by key and
\* UNCONDITIONAL — no re-read, no re-check of the now-current held/expired. A live
\* SetHold/Refresh that landed since Scan is invisible here: the verdict still says
\* "delete" and the record is removed anyway (the real TOCTOU window).
Delete(r) ==
    /\ rphase[r] = "scanned"
    /\ IF verdict[r] = "delete" /\ present[r]
       THEN /\ present' = [present EXCEPT ![r] = FALSE]
            /\ deleted' = deleted \cup {r}
       ELSE UNCHANGED <<present, deleted>>
    /\ rphase' = [rphase EXCEPT ![r] = "done"]
    /\ UNCHANGED <<held, expired, verdict, scan_held, scan_expired>>

\* --------------------------------------------------------------------------
\* Recycle: the reaper pass finished (every record scanned-and-acted). Reset to a
\* fresh batch — surviving records start a new pass; deleted records are replaced
\* by fresh ones — so the system CYCLES (no benign terminal deadlock under
\* -deadlock). New records arrive un-held and un-expired (freshly written).
\* --------------------------------------------------------------------------
Recycle ==
    /\ \A r \in Records : rphase[r] = "done"
    /\ held' = [r \in Records |-> FALSE]
    /\ expired' = [r \in Records |-> FALSE]
    /\ present' = [r \in Records |-> TRUE]
    /\ rphase' = [r \in Records |-> "unscanned"]
    /\ verdict' = [r \in Records |-> NOBODY]
    /\ scan_held' = [r \in Records |-> FALSE]
    /\ scan_expired' = [r \in Records |-> FALSE]
    /\ deleted' = {}

Next ==
    \/ \E r \in Records : Age(r) \/ Refresh(r) \/ SetHold(r)
    \/ \E r \in Records : Scan(r) \/ Delete(r)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY — anchored on SCAN-TIME state (what the code observed), because the
\* by-key delete acts on the verdict frozen at the scan and re-checks nothing.
\* =======================================================================

\* A record that was on compliance hold AT THE MOMENT THE REAPER SCANNED it is
\* never deleted: the `if policy.compliance_hold { continue }` skip froze a "keep"
\* verdict. (Drop that skip -> a scan-time-held record is deleted -> violated.)
NeverDeleteScanHeld ==
    \A r \in deleted : scan_held[r] = FALSE

\* A record that was NOT expired AT SCAN TIME (e.g. a live write refreshed it before
\* the reaper looked) is never deleted: the `if ts < cutoff` expiry-check froze a
\* "keep" verdict. (Drop that check -> a scan-time-live record is deleted -> violated.)
NeverDeleteScanLive ==
    \A r \in deleted : scan_expired[r] = TRUE

\* ---- Documentation only: the CURRENT-state versions. These do NOT hold (the
\* ---- reaper re-checks nothing at delete time, so a record held/refreshed AFTER
\* ---- the scan is still deleted) and are intentionally NOT listed as INVARIANTS.
\* NeverDeleteHeldNow  == \A r \in deleted : held[r]    = FALSE
\* NeverDeleteLiveNow  == \A r \in deleted : expired[r] = TRUE

============================================================================
