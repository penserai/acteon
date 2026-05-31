----------------------------- MODULE BusApproval -----------------------------
(*
 * TLA+ specification of Acteon's Kafka-bus PRE-PUBLISH human-in-the-loop (HITL)
 * approval — the Phase-10 two-phase 5-state machine. This is a DIFFERENT
 * subsystem from the gateway HITL path in ApprovalLifecycle.tla: here a parked
 * bus envelope is produced to Kafka only after an operator approves.
 *
 * Models:
 *   - crates/core/src/bus_approval.rs : enum BusApprovalStatus
 *     { Pending, Approving, Approved, Rejected, Expired } and is_terminal()
 *     (Approved | Rejected | Expired terminal; Pending & Approving non-terminal).
 *   - crates/server/src/api/bus.rs : approve_bus_approval (~8023..8262) and
 *     reject_bus_approval (~8308..8374).
 *   - crates/server/src/bus_reconciler.rs : retry_approving_row (~166..264),
 *     the background reconciler for stuck Approving rows.
 *
 * Protocol (the Phase-10 two-phase fix). An operator approval is a TWO-PHASE
 * commit around the Kafka produce:
 *
 *   approve, from Pending:
 *     CAS Pending -> Approving      (committed FIRST, bus.rs:8079; stamps
 *                                    decided_by — row is now ineligible for
 *                                    reject)
 *     backend.produce(envelope)     (fires WHILE Approving, bus.rs:8187)
 *     CAS Approving -> Approved      (only after the produce succeeded,
 *                                    bus.rs:8231; records produced offset)
 *
 *   The produce is IDEMPOTENT, so the reconciler (bus_reconciler.rs) retrying a
 *   stuck Approving row re-produces with no duplicate Kafka record, then runs
 *   the same Approving -> Approved CAS (a no-op if the row already left
 *   Approving, bus.rs:239..243).
 *
 *   reject is reachable ONLY from Pending (bus.rs:8356 — "only Pending is
 *   rejectable"; an Approving row's envelope is in flight). expire is likewise
 *   only from Pending (TTL elapsed before any decision). Approved / Rejected /
 *   Expired are terminal.
 *
 * The V1 (Phase 6c) bug this fixed (bus_approval.rs:54..60): Pending -> Approved
 * in ONE step *after* the produce, so a successful produce + a failed CAS left
 * the row looking Pending. Then (a) a reject could fire on an ALREADY-PUBLISHED
 * envelope, and (b) a non-idempotent retry could DOUBLE-PUBLISH.
 *
 * Verified (over every interleaving of two concurrent operators, a reconciler,
 * and TTL expiry):
 *   - PublishAtMostOnce: the envelope is produced to Kafka at most once across
 *     reconciler retries and crashes (idempotent producer) — publish_count <= 1.
 *   - PublishOnlyIfApproved: a published row is on the approve path only
 *     (published => status \in {approving, approved}); a rejected or expired row
 *     is never published.
 *   - DecisionTerminal: reject/expire are reachable only from Pending; once
 *     Approving the row never becomes Rejected/Expired; Approved/Rejected/
 *     Expired are terminal except for an explicit Recycle to a fresh window.
 *
 * SCOPE. Abstracts away: the per-row CAS version contention (cas_update's retry
 * loop — modeled as an atomic guarded transition), the multi-tenant key shape,
 * the conversation/topic resolution and schema re-validation at produce time,
 * and the audit/index side-writes. It models a SINGLE approval row and treats
 * "produce" as one idempotent boolean side-effect. Models the produce as
 * always-eventually-succeeding (the failure path simply leaves the row
 * Approving for the next retry, which this spec already covers).
 *
 * Negative check: revert to the V1 single-phase machine — produce while still
 * Pending, flip Pending -> Approved AFTER the produce, and keep reject reachable
 * from Pending. TLC then finds a published row that gets Rejected
 * (PublishOnlyIfApproved violated) and, with a non-idempotent re-produce, a
 * publish_count reaching 2 (PublishAtMostOnce violated).
 *
 * Run with:
 *   java -jar tla2tools.jar -config BusApproval.cfg BusApproval.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Operators,    \* concurrent operator approve actors / replicas, e.g. {o1, o2}
    Reconcilers,  \* background reconciler workers retrying Approving, e.g. {x1}
    TTL,          \* approval time-to-live in ticks (expiry gate)
    MaxTime,      \* state-space / clock bound
    NOBODY        \* sentinel: no actor is driving the in-flight approve

\* Actors that can drive the approve produce + flip: operators and reconcilers.
Approvers == Operators \cup Reconcilers

VARIABLES
    status,         \* "pending" | "approving" | "approved" | "rejected" | "expired"
    published,      \* BOOLEAN: the parked envelope landed on Kafka
    publish_count,  \* 0..3: number of distinct Kafka produces (idempotent => <= 1)
    claim,          \* Approvers \cup {NOBODY}: who claimed the row into Approving
    clock           \* 0..MaxTime: monotone tick clock; expiry gate at clock >= TTL

vars == <<status, published, publish_count, claim, clock>>

TypeOK ==
    /\ status \in {"pending", "approving", "approved", "rejected", "expired"}
    /\ published \in BOOLEAN
    /\ publish_count \in 0..3
    /\ claim \in Approvers \cup {NOBODY}
    /\ clock \in 0..MaxTime

Init ==
    /\ status = "pending"
    /\ published = FALSE
    /\ publish_count = 0
    /\ claim = NOBODY
    /\ clock = 0

\* PHASE 1 (bus.rs:8079). An operator wins the CAS Pending -> Approving, FIRST,
\* before any produce. This stamps decided_by and makes the row ineligible for
\* reject. The soft-expire read gate (bus.rs:8039 `expires_at < now`) refuses a
\* claim once the TTL has elapsed — modeled as clock < TTL. cas_update's compare-
\* and-swap makes this an atomic guarded transition: only one claimant wins.
ClaimApproving(o) ==
    /\ o \in Operators
    /\ status = "pending"
    /\ claim = NOBODY
    /\ clock < TTL
    /\ status' = "approving"
    /\ claim' = o
    /\ UNCHANGED <<published, publish_count, clock>>

\* The Kafka produce (bus.rs:8187, bus_reconciler.rs:215). Fires WHILE the row is
\* Approving, BEFORE the flip to Approved. The producer is IDEMPOTENT: the first
\* produce sets published and increments the publish counter; a retry (reconciler
\* re-produce, or a crashed-and-resumed approve) re-runs the produce as a NO-OP
\* for the count. Any claimed Approver (operator or reconciler) may produce.
Produce(p) ==
    /\ p \in Approvers
    /\ status = "approving"
    /\ IF published
       THEN \* Idempotent retry: re-produce is a no-op at the consumer dedup.
            UNCHANGED <<published, publish_count>>
       ELSE \* First produce: the envelope lands on Kafka exactly once.
            /\ published' = TRUE
            /\ publish_count' = publish_count + 1
    /\ UNCHANGED <<status, claim, clock>>

\* PHASE 2 (bus.rs:8231, bus_reconciler.rs:244). CAS Approving -> Approved, ONLY
\* after the produce has landed (published = TRUE). Records the produced offset.
\* A reconciler that finds the row already left Approving treats it as a no-op
\* (bus.rs:239); here the guard status = "approving" gives the same effect.
FlipApproved(p) ==
    /\ p \in Approvers
    /\ status = "approving"
    /\ published = TRUE
    /\ status' = "approved"
    /\ claim' = NOBODY
    /\ UNCHANGED <<published, publish_count, clock>>

\* Reject (bus.rs:8356). Reachable ONLY from Pending — "only Pending is
\* rejectable". Once Approving the envelope is in flight and reject is refused
\* (409). No produce, no Kafka footprint.
Reject(o) ==
    /\ o \in Operators
    /\ status = "pending"
    /\ clock < TTL
    /\ status' = "rejected"
    /\ UNCHANGED <<published, publish_count, claim, clock>>

\* Expire. TTL elapses before any decision: Pending -> Expired (same outcome as
\* Rejected, distinguished for audit). Reachable only from Pending; an Approving
\* row is past the decision point and is finalized by the reconciler instead.
Expire ==
    /\ status = "pending"
    /\ clock >= TTL
    /\ status' = "expired"
    /\ UNCHANGED <<published, publish_count, claim, clock>>

\* Time advances (monotone). Once clock >= TTL the expiry gate refuses new claims
\* and reject, and Expire becomes enabled for a still-pending row.
Tick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ UNCHANGED <<status, published, publish_count, claim>>

\* Recycle: open a FRESH approval window once the current one has settled into a
\* terminal status. Keeps the system cycling — no benign terminal deadlock under
\* -deadlock.
Settled == status \in {"approved", "rejected", "expired"}

Recycle ==
    /\ Settled
    /\ status' = "pending"
    /\ published' = FALSE
    /\ publish_count' = 0
    /\ claim' = NOBODY
    /\ clock' = 0

Next ==
    \/ \E o \in Operators : ClaimApproving(o) \/ Reject(o)
    \/ \E p \in Approvers : Produce(p) \/ FlipApproved(p)
    \/ Expire
    \/ Tick
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* The envelope is produced to Kafka AT MOST ONCE across reconciler retries and
\* crashes — the idempotent producer. A non-idempotent re-produce (V1) reaches 2.
PublishAtMostOnce == publish_count <= 1

\* A published row is on the approve path ONLY: published implies the row is
\* mid-approve (approving) or approved. A Rejected or Expired row is NEVER
\* published. V1 (produce while still Pending, reject still allowed) violates
\* this — a published row gets Rejected.
PublishOnlyIfApproved == published => status \in {"approving", "approved"}

\* Reject/expire are reachable only from Pending; once Approving the row never
\* becomes Rejected/Expired; Approved is published-and-flipped. Encodes the
\* one-way decision boundary the two-phase machine enforces.
DecisionTerminal ==
    /\ (status = "approved" => published)
    /\ (status = "rejected" => ~published)
    /\ (status = "expired" => ~published)

============================================================================
