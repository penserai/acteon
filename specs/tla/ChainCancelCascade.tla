-------------------------- MODULE ChainCancelCascade ------------------------
(*
 * TLA+ specification of Acteon's recursive chain-cancellation cascade.
 *
 * Models crates/gateway/src/gateway.rs:
 *   - fn cancel_chain   (~5461): acquires the per-chain lock "chain:{chain_id}",
 *     guards on status \in {Running, WaitingSubChain, WaitingParallel}. A chain
 *     already terminal (Completed/Failed/Cancelled/TimedOut) returns Err at the
 *     guard (gateway.rs:5488..5499) and is LEFT untouched — crucially, this
 *     return happens BEFORE the child-recursion loop (gateway.rs:5536), so the
 *     cascade does NOT descend into an already-terminal chain's children. A
 *     non-terminal chain is flipped -> Cancelled, persisted; the lock is
 *     RELEASED, and only THEN does cancel_chain recurse over
 *     chain_state.child_chain_ids (best-effort: a child already terminal returns
 *     the same Err, caught and ignored — "may already be terminal").
 *   - fn advance_chain  (~2544): under the SAME per-chain lock; its guard (~2574)
 *     only advances status \in {Running, WaitingSubChain, WaitingParallel}, so a
 *     Cancelled chain can never advance / complete (no resurrection). A chain
 *     with a sub-chain step enters ChainStatus::WaitingSubChain (gateway.rs:2711)
 *     and advances past that step ONLY once the child is observed terminal
 *     (Completed 2735->2763; Failed/TimedOut/Cancelled 2784). So a parent reaches
 *     a terminal status ONLY AFTER all of its children are already terminal.
 *   - ChainState.child_chain_ids / parent_chain_id (crates/core/src/chain.rs).
 *     The chain graph is an acyclic tree (validate_chain_graph enforces this at
 *     build time), so the recursive cascade terminates.
 *
 * Protocol. A small FIXED acyclic tree of chains: root r with children c1, c2,
 * and a grandchild g under c1. Each chain has a status, abstracted to
 *   "running" | "completed" | "cancelled"
 * where "completed" stands for any self-terminal outcome. Concurrent actions are
 * serialized per-chain by the lock (cancel-vs-advance on ONE chain is mutually
 * exclusive); the cascade itself runs WITHOUT the parent lock held.
 *
 *   Advance(c): a Running chain finishes on its own -> Completed, but ONLY once
 *     all of its children are already terminal — the WaitingSubChain coupling
 *     (gateway.rs:2711/2735/2763): a parent never completes ahead of its children.
 *   CancelRoot: open a cancel of the root -> seed the cascade work-set with {r}.
 *   CascadeStep(c): pop c from the work-set. If c is Running, flip it to Cancelled
 *     and RECURSE — enqueue its children (cancel_chain proceeds past the guard
 *     into the child loop). If c is already terminal, SKIP it and do NOT enqueue
 *     its children (cancel_chain returns Err at the guard, before the child loop).
 *   The work-set captures the recursion; it drains to {} when the cascade is done.
 *
 * Verified (over every interleaving of concurrent self-completion and the
 * cancel cascade):
 *   - NoResurrection: a Cancelled chain never becomes Running or Completed again
 *     (advance refuses non-Running) — Cancelled is terminal.
 *   - NoOrphanedRunningDescendant: once a cascade seeded from the cancelled root
 *     has DRAINED (work-set empty after a cancel started), NO descendant of the
 *     root is left Running. This holds for the RIGHT reason: a root cancelled
 *     while Running cascades into its running descendants; a root that had already
 *     Completed never recurses, but the WaitingSubChain coupling guarantees all of
 *     its descendants were already terminal before it completed.
 *
 * Negative check: revert the RECURSION — in CascadeStep's running branch, do NOT
 * enqueue the popped chain's children (cancel only the root). A child/grandchild
 * left Running under the cancelled root then survives the drain, and TLC reports
 * NoOrphanedRunningDescendant violated. (The completion coupling on Advance is
 * likewise load-bearing: drop it and a parent can self-Complete with running
 * children, which — since the faithful cascade does not recurse through a
 * terminal chain — also orphans those children.)
 *
 * SCOPE. Abstracts the per-chain lock to per-chain action mutual exclusion (each
 * chain's status transition is one atomic step, which the lock guarantees) and
 * collapses the self-terminal ChainStatus values (Completed/Failed/TimedOut, plus
 * the Waiting* live states) into "running"/"completed". The cancel guard's
 * acceptance set, the cascade's Err-before-child-loop short-circuit, and the
 * parent/child completion coupling are modeled faithfully. The on_cancel
 * notification dispatch, audit/stream emission, and TTL are out of scope. The
 * chain tree is fixed and acyclic (build-time invariant), so cascade termination
 * is assumed, not proved here.
 *
 * Run with:
 *   java -jar tla2tools.jar -config ChainCancelCascade.cfg ChainCancelCascade.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    R,   \* root chain id          (model value)
    C1,  \* child of R             (model value)
    C2,  \* child of R             (model value)
    G    \* grandchild, child of C1 (model value)

\* The four chain ids form the fixed tree. Chains and Root are DERIVED from the
\* id constants so Children/Root compare against the SAME model values TLC binds
\* (comparing a model value to a string literal would silently never match —
\* which is why the tree edges are keyed on constants, not "r"/"c1" strings).
Chains == {R, C1, C2, G}
Root   == R

\* ---------------------------------------------------------------------------
\* The fixed acyclic chain tree (ChainState.child_chain_ids). Encoded as an
\* OPERATOR (a .cfg function literal would be rejected by TLC):
\*     R -> {C1, C2},  C1 -> {G},  C2 -> {},  G -> {}
\* ---------------------------------------------------------------------------
Children(c) ==
    CASE c = R   -> {C1, C2}
      [] c = C1  -> {G}
      [] OTHER   -> {}

\* Transitive descendants of c (the subtree under c, excluding c itself). The
\* tree has depth 2, so two unrollings of Children cover every descendant.
Descendants(c) ==
    LET d1 == Children(c)
        d2 == UNION { Children(x) : x \in d1 }
    IN d1 \cup d2

VARIABLES
    status,        \* [Chains -> {"running","completed","cancelled"}]
    work,          \* SUBSET Chains : the cascade work-set (chains awaiting cancel)
    started,       \* BOOLEAN : a cancel cascade from Root has been initiated
    was_cancelled  \* SUBSET Chains : history of chains ever set Cancelled this
                   \*   window (reset on Recycle). Lets NoResurrection catch a
                   \*   chain that left "cancelled" — the resurrection bug.

vars == <<status, work, started, was_cancelled>>

TypeOK ==
    /\ status \in [Chains -> {"running", "completed", "cancelled"}]
    /\ work \subseteq Chains
    /\ started \in BOOLEAN
    /\ was_cancelled \subseteq Chains

Init ==
    /\ status = [c \in Chains |-> "running"]
    /\ work = {}
    /\ started = FALSE
    /\ was_cancelled = {}

\* A Running chain finishes on its own -> Completed, but ONLY once every child is
\* already terminal. This is the WaitingSubChain coupling (gateway.rs:2711/2735/
\* 2763): a chain with sub-chain steps blocks in WaitingSubChain and advances past
\* the step only after the child is observed terminal, so a parent never reaches a
\* terminal status ahead of its children. advance_chain's status guard (~2574)
\* also refuses any non-Running chain, so this never resurrects a Cancelled chain.
\* Per-chain mutual exclusion (the lock) makes this one atomic step.
Advance(c) ==
    /\ status[c] = "running"
    /\ \A ch \in Children(c) : status[ch] # "running"
    /\ status' = [status EXCEPT ![c] = "completed"]
    /\ UNCHANGED <<work, started, was_cancelled>>

\* Initiate the cascade: seed the work-set with the Root. Modeled once per window
\* (started gate) so the cascade has a well-defined drain point.
CancelRoot ==
    /\ ~started
    /\ started' = TRUE
    /\ work' = {Root}
    /\ UNCHANGED <<status, was_cancelled>>

\* One cascade step on chain c (cancel_chain's body). Pop c from the work-set:
\*   - if c is Running: cancel_chain proceeds past the status guard, flips
\*     c -> Cancelled (the write under the lock), then — after releasing the lock —
\*     RECURSES into child_chain_ids, so enqueue c's children.
\*   - if c is already terminal (completed/cancelled): cancel_chain returns Err at
\*     the guard (gateway.rs:5496) BEFORE the child loop, so it does NOT recurse —
\*     dequeue c and enqueue NOTHING. (Reverting the running-branch enqueue is the
\*     negative check.)
CascadeStep(c) ==
    /\ c \in work
    /\ IF status[c] = "running"
       THEN /\ status' = [status EXCEPT ![c] = "cancelled"]
            /\ was_cancelled' = was_cancelled \cup {c}
            /\ work' = (work \ {c}) \cup Children(c)
       ELSE /\ work' = work \ {c}
            /\ UNCHANGED <<status, was_cancelled>>
    /\ UNCHANGED started

\* Recycle: once the cascade has been started and fully drained, reset to a fresh
\* tree of Running chains so the system CYCLES (no benign terminal deadlock under
\* -deadlock).
Recycle ==
    /\ started
    /\ work = {}
    /\ status' = [c \in Chains |-> "running"]
    /\ work' = {}
    /\ started' = FALSE
    /\ was_cancelled' = {}

Next ==
    \/ \E c \in Chains : Advance(c)
    \/ CancelRoot
    \/ \E c \in work : CascadeStep(c)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* A Cancelled chain is terminal: advance_chain refuses non-Running chains, so
\* once a chain has been set Cancelled (recorded in was_cancelled) it can never
\* transition back to Running or Completed within the same window. was_cancelled
\* is reset only by the explicit Recycle (a deliberately fresh tree), so any chain
\* that left "cancelled" without a Recycle — i.e. is recorded cancelled yet now
\* Running or Completed — is a resurrection and trips this invariant.
NoResurrection ==
    \A c \in was_cancelled : status[c] = "cancelled"

\* Once a cascade seeded from Root has DRAINED (started /\ work = {}), no
\* descendant of Root is left Running — the central guarantee of the recursive
\* cascade combined with the parent/child completion coupling. A Running
\* descendant here means either the recursion failed to reach it, or a parent
\* completed ahead of a still-running child.
NoOrphanedRunningDescendant ==
    (started /\ work = {}) =>
        \A d \in Descendants(Root) : status[d] # "running"

============================================================================
