-------------------------- MODULE ApprovalLifecycle ------------------------
(*
 * TLA+ specification of Acteon's human-in-the-loop (HITL) approval lifecycle —
 * the gateway approve/reject path, which is what PR #225 hardened.
 *
 * Models crates/gateway/src/gateway.rs:
 *   - execute_approval / execute_approval_inner (~5645..5841)
 *   - reject_approval / reject_approval_inner   (~5847..5935)
 * The persisted ApprovalRecord.status (gateway.rs:99) holds exactly one of
 * "pending" | "approved" | "rejected" — there is NO persisted intermediate
 * "approving" state and NO "expired" status. (The 5-state Approving/Expired
 * machine in crates/core/src/bus_approval.rs is a DIFFERENT subsystem — the
 * Kafka-envelope bus — and would warrant its own spec; this spec is the
 * gateway path only.)
 *
 * Protocol. An approval is created Pending with a TTL. Approve and reject both
 * race for ONE shared claim key ({id}:claim) via check_and_set — first writer
 * wins, so approve and reject are mutually exclusive. Expiry is an authoritative
 * read-side GATE (gateway.rs:5663 `now > expires_at => ApprovalNotFound`): once
 * the TTL has elapsed a NEW claim is refused, but the row stays "pending" — no
 * status is written. The winning claimant then runs its critical section:
 *
 *   APPROVE (the PR #225 ordering — each step is a distinct instruction):
 *     claim ----------------------------------------> (claim held, still pending)
 *     write durable pre-execution INTENT ------------> (intent_written)   [#225]
 *     flip status pending -> approved ---------------> (only once intent durable)
 *     run the side-effect, exactly once -------------> (executed)
 *   The intent is recorded BEFORE the status flip and the side-effect, so a
 *   transient audit/store outage between claim and flip leaves the row pending
 *   and retryable — never executed-but-unaudited, never approved-but-stranded.
 *
 *   REJECT: claim and flip pending -> rejected. No intent, no side-effect.
 *
 * Verified (over every interleaving of concurrent approvers, a rejecter, and
 * TTL expiry):
 *   - DecidedOnce: the terminal status is owned by a claimant of the matching
 *     role (approved => an approver holds the claim; rejected => a rejecter),
 *     a recorded intent only ever exists on the approve path, and an executed
 *     row is approved. No approve-after-reject, no reject-after-approve, no
 *     cross-decision contamination.
 *   - ExecuteOnceIfApproved: the side-effect fires at most once, and only while
 *     the row is approved (never after a reject, never after an unclaimed
 *     expiry, never twice).
 *   - IntentBeforeExecute (PR #225): Execute is reachable only once the durable
 *     pre-execution intent is recorded  (executed => intent_written).
 *
 * Negative check: revert the #225 ordering — drop the `intent_written = TRUE`
 * precondition on FlipApproved and Execute and remove WriteIntent from Next, so
 * the approver can flip and run the side-effect without first recording the
 * intent. TLC then reaches executed = TRUE /\ intent_written = FALSE and reports
 * IntentBeforeExecute violated.
 *
 * Run with:
 *   java -jar tla2tools.jar -config ApprovalLifecycle.cfg ApprovalLifecycle.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Approvers,  \* concurrent approve actors / replicas, e.g. {a1, a2}
    Rejecters,  \* concurrent reject actors / replicas, e.g. {r1}
    TTL,        \* approval time-to-live in ticks (read-side expiry gate)
    MaxTime,    \* state-space / clock bound
    NOBODY      \* sentinel: the decision claim is free

\* Actors contending for the single shared claim key.
Actors == Approvers \cup Rejecters

VARIABLES
    status,         \* "pending" | "approved" | "rejected"
    intent_written, \* BOOLEAN: durable pre-execution intent recorded (approve path)
    executed,       \* BOOLEAN: the side-effect ran
    exec_count,     \* 0..3: number of Execute firings (catches double-exec)
    claim,          \* Actors \cup {NOBODY}: holder of the shared decision claim
    clock           \* 0..MaxTime: monotone tick clock; expiry gate at clock >= TTL

vars == <<status, intent_written, executed, exec_count, claim, clock>>

TypeOK ==
    /\ status \in {"pending", "approved", "rejected"}
    /\ intent_written \in BOOLEAN
    /\ executed \in BOOLEAN
    /\ exec_count \in 0..3
    /\ claim \in Actors \cup {NOBODY}
    /\ clock \in 0..MaxTime

Init ==
    /\ status = "pending"
    /\ intent_written = FALSE
    /\ executed = FALSE
    /\ exec_count = 0
    /\ claim = NOBODY
    /\ clock = 0

\* An approver wins the shared claim (check_and_set, first writer wins). The
\* read-side expiry gate (clock < TTL) is the authoritative server-side check at
\* gateway.rs:5663 — a NEW claim after the TTL is refused. The row stays
\* "pending" while claimed; there is no persisted "approving" status.
ClaimApprove(a) ==
    /\ a \in Approvers
    /\ claim = NOBODY
    /\ status = "pending"
    /\ clock < TTL
    /\ claim' = a
    /\ UNCHANGED <<status, intent_written, executed, exec_count, clock>>

\* PR #225 phase 1: durably record the pre-execution INTENT, BEFORE the status
\* flip and the side-effect. Runs under the claim while the row is still pending.
\* (A failure here keeps the row pending and retryable.)
WriteIntent(a) ==
    /\ a \in Approvers
    /\ claim = a
    /\ status = "pending"
    /\ intent_written = FALSE
    /\ intent_written' = TRUE
    /\ UNCHANGED <<status, executed, exec_count, claim, clock>>

\* PR #225 phase 2: flip pending -> approved, but ONLY once the intent is
\* durable. This precondition is the fix; reverting it is the negative check.
FlipApproved(a) ==
    /\ a \in Approvers
    /\ claim = a
    /\ status = "pending"
    /\ intent_written = TRUE
    /\ status' = "approved"
    /\ UNCHANGED <<intent_written, executed, exec_count, claim, clock>>

\* The side-effecting dispatch. Reachable ONLY from approved (so never after a
\* reject or an unclaimed expiry) and ONLY with the intent durable; fires once.
Execute(a) ==
    /\ a \in Approvers
    /\ claim = a
    /\ status = "approved"
    /\ intent_written = TRUE
    /\ executed = FALSE
    /\ executed' = TRUE
    /\ exec_count' = exec_count + 1
    /\ UNCHANGED <<status, intent_written, claim, clock>>

\* A rejecter wins the same shared claim and flips pending -> rejected. No
\* intent, no side-effect. Mutual exclusion with approve is via the claim.
ClaimReject(r) ==
    /\ r \in Rejecters
    /\ claim = NOBODY
    /\ status = "pending"
    /\ clock < TTL
    /\ claim' = r
    /\ status' = "rejected"
    /\ UNCHANGED <<intent_written, executed, exec_count, clock>>

\* Time advances (monotone). Once clock >= TTL the read-side gate refuses new
\* claims; a row never claimed by then stays pending forever (read-side expired).
Tick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ UNCHANGED <<status, intent_written, executed, exec_count, claim>>

\* Recycle: open a FRESH approval window once the current one has settled —
\* rejected, approved-and-executed, or expired-while-unclaimed (pending, no
\* claim, past TTL). Keeps the system cycling (no benign terminal deadlock).
Settled ==
    \/ status = "rejected"
    \/ (status = "approved" /\ executed = TRUE)
    \/ (status = "pending" /\ claim = NOBODY /\ clock >= TTL)

Recycle ==
    /\ Settled
    /\ status' = "pending"
    /\ intent_written' = FALSE
    /\ executed' = FALSE
    /\ exec_count' = 0
    /\ claim' = NOBODY
    /\ clock' = 0

Next ==
    \/ \E a \in Approvers :
         ClaimApprove(a) \/ WriteIntent(a) \/ FlipApproved(a) \/ Execute(a)
    \/ \E r \in Rejecters : ClaimReject(r)
    \/ Tick
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* The side-effect fires at most once and only while the row is approved —
\* never after a reject, never after an unclaimed expiry, never twice.
ExecuteOnceIfApproved ==
    /\ exec_count <= 1
    /\ (executed => status = "approved")

\* PR #225: the durable pre-execution intent precedes the side-effect.
IntentBeforeExecute == executed => intent_written

\* The terminal status is owned by a claimant of the matching role, an intent
\* exists only on the approve path, and an executed row is approved — so no
\* approve-after-reject, reject-after-approve, or cross-decision contamination.
DecidedOnce ==
    /\ (executed => status = "approved")
    /\ (intent_written => claim \in Approvers)
    /\ (status = "approved" => claim \in Approvers)
    /\ (status = "rejected" => claim \in Rejecters)

============================================================================
