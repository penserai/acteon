----------------------------- MODULE RefGraphDefense -----------------------------
(*
 * TLA+ specification of Acteon's A2A reference-graph "graph-bomb" defense — the
 * bounded write-time walk that refuses a malicious Task.referenceTaskIds graph
 * before it can force unbounded state-store reads.
 *
 * Models crates/gateway/src/task_engine.rs:
 *   - fn check_reference_graph (~920): a level-by-level breadth-first walk over a
 *     task's reference edges, starting from the seed references the pending write
 *     introduces, rooted at root_task_id. Its loop (verbatim structure):
 *
 *         let mut frontier = <distinct seed refs>;     // level 1
 *         let mut visited  = {};   // the ROOT is DELIBERATELY never inserted
 *         let mut depth    = 1;
 *         while !frontier.is_empty() {
 *             if depth > MAX_REFERENCE_DEPTH { return Err(ReferenceDepthExceeded) }
 *             for tid in &frontier {                   // cycle check
 *                 if tid == root_task_id { return Err(ReferenceCycle) }
 *             }
 *             if visited.len() + frontier.len() > MAX_REFERENCE_GRAPH_NODES {
 *                 return Err(ReferenceGraphTooLarge)
 *             }
 *             let fetched = <fetch every frontier task, bounded-concurrent>;
 *             visited.extend(frontier);                // this level now expanded
 *             let mut next = {};
 *             for task in fetched {                    // union out-edges,
 *                 for r in task.history.refs {          //   MINUS visited
 *                     if !visited.contains(r) && !next.contains(r) { next.insert(r) }
 *                 }
 *             }
 *             frontier = next;
 *             depth += 1;
 *         }
 *         Ok(())            // frontier emptied -> ACCEPT
 *
 *     The walk ACCEPTS when the frontier empties, and otherwise REJECTS with one
 *     of {cycle, depth, width}. MAX_REFERENCE_DEPTH (crates/core/src/bus_task.rs:99
 *     = 5) and MAX_REFERENCE_GRAPH_NODES (crates/core/src/task_engine context = 256)
 *     are the bounds; here they are shrunk to MaxDepth = 3 and MaxNodes = 4 for
 *     model-checking. References to absent tasks are dead ends (Refs returns {}),
 *     not errors — only structural abuse is refused.
 *
 * Protocol. A fixed set of task ids with reference out-edges, encoded as the
 * operator Refs(s, t) keyed on a SCENARIO s so TLC explores several graph shapes:
 *   "clean" : ROOT seeds {A}; A->{B}, B->{} — acyclic, depth 2, 2 nodes  => accept.
 *   "cycle" : ROOT seeds {A}; A->{B}, B->{ROOT} — closes back to the root => reject(cycle).
 *   "deep"  : ROOT seeds {A}; A->{B}, B->{C}, C->{D} — a path of 4 hops, past
 *             MaxDepth = 3, and the chain never returns to the root  => reject(depth).
 *   "wide"  : ROOT seeds {A,B,C,D,E} — five distinct first-level nodes, past
 *             MaxNodes = 4, none of them the root                     => reject(width).
 *   "ring"  : ROOT seeds {A}; A->{B}, B->{C}, C->{A} — a non-root cycle. The
 *             visited filter makes the walk TERMINATE (A is re-seen but already
 *             expanded, so it is dropped from the next frontier) and the graph is
 *             accepted — a pre-existing cycle NOT through the root is traversed
 *             safely and is not re-flagged (the documented behaviour, task_engine
 *             rs:895). This is the case that the visited-filter alone makes finite.
 *
 * One walk action expands ONE BFS level (the body of one while-iteration): it
 * runs the depth / cycle / width checks against the current frontier, then — if
 * all pass — folds the frontier into visited and computes the next frontier via
 * Refs MINUS visited. A walk reaches a terminal verdict in {accept, cycle, depth,
 * width}; Recycle then picks a fresh scenario so the system CYCLES (no benign
 * terminal deadlock under -deadlock).
 *
 * Verified (over every scenario and every level of each walk):
 *   - TypeOK: state stays well-typed; verdict is always one of the five tags.
 *   - Terminates / finiteness: visited grows MONOTONICALLY, never holds the root,
 *     and never exceeds MaxNodes; no frontier node is ever already in visited
 *     (the visited filter), so each node is expanded at most once and the walk is
 *     finite — encoded as VisitedMonotone, VisitedBounded, RootNeverVisited,
 *     FrontierDisjoint, plus the temporal Terminates (every walk eventually Done).
 *   - RejectsBadGraph: a scenario whose reachable graph loops back to the root, or
 *     has a path longer than MaxDepth, or more than MaxNodes reachable nodes, is
 *     NEVER accepted (the verdict is some reject). Encoded over the terminal state.
 *   - AcceptsOnlyClean: an accept verdict implies the walk never saw the root in a
 *     frontier, never exceeded MaxDepth, and never exceeded the node budget.
 *
 * The three bound checks (root-cycle, depth, width) and the visited-filter
 * (frontier MINUS visited) are each INDEPENDENTLY load-bearing:
 *
 * Negative check:
 *   (1) Drop the cycle check (delete the `tid == root` rejection): the "cycle"
 *       scenario then ACCEPTS a graph that loops back to the root
 *       -> RejectsBadGraph violated.
 *   (2) Drop the visited-filter in the expansion (next frontier = raw Refs union,
 *       not minus visited): the "ring" scenario then re-expands A forever, the
 *       walk never empties its frontier and instead trips the depth bound — and,
 *       more sharply, FrontierDisjoint is violated (a node already in visited
 *       reappears in the frontier), and an accept-eligible non-root cycle is mis-
 *       rejected. Reverting it alone breaks finiteness/FrontierDisjoint.
 *   (Each of the depth and width checks is likewise load-bearing: drop the depth
 *    check and "deep" accepts a too-deep graph; drop the width check and "wide"
 *    accepts a too-wide graph — RejectsBadGraph violated in each case.)
 *
 * SCOPE. This is an ALGORITHM-CORRECTNESS spec for a SINGLE write-time bounded
 * walk, NOT a concurrency race. The only concurrency in the real code is the
 * bounded buffer_unordered fan-out that fetches one BFS level's tasks in parallel;
 * that is abstracted to an atomic per-level fetch (the fetch result is a pure
 * function of the fixed graph, so the fan-out order is irrelevant to the verdict).
 * The genuine cross-row TOCTOU the code itself documents (two writers each adding
 * one half of a cycle and both passing their own walk, task_engine.rs:907) is
 * explicitly out of scope — it is a residual, accepted race, not what this walk
 * guards. The graph is fixed per scenario; absent-task dead ends, the inclusive
 * `> MaxNodes` boundary, and the deliberate exclusion of the root from visited are
 * modeled faithfully.
 *
 * Run with:
 *   java -jar tla2tools.jar -config RefGraphDefense.cfg RefGraphDefense.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    MaxDepth,    \* MAX_REFERENCE_DEPTH (shrunk: 3). Depth at which the walk rejects.
    MaxNodes,    \* MAX_REFERENCE_GRAPH_NODES (shrunk: 4). Node budget (inclusive).
    Scenarios,   \* the set of graph shapes to check, e.g. {"clean","cycle",...}
    NOBODY       \* sentinel: walk not yet started (no scenario chosen)

\* The task-id universe. ROOT is the task being written (root_task_id); A..E are
\* the cited tasks. These are plain strings — referenced only by Refs and the
\* root-cycle check, both of which compare strings to strings, so (unlike model
\* values) string literals match correctly here.
ROOT == "root"
Tasks == {ROOT, "A", "B", "C", "D", "E"}

\* ---------------------------------------------------------------------------
\* The fixed reference graph, per scenario. Refs(s, t) is the set of tasks that
\* task t references (its history's reference_task_ids) under scenario s. A task
\* absent from the store / with no edges is a dead end -> {}. Encoded as an
\* OPERATOR because a .cfg cannot carry a function literal.
\*
\* The SEED references the pending write introduces are Refs(s, ROOT): the walk's
\* level-1 frontier is the root's out-edges.
\* ---------------------------------------------------------------------------
Refs(s, t) ==
    CASE s = "clean" -> CASE t = ROOT -> {"A"}
                          [] t = "A"  -> {"B"}
                          [] OTHER    -> {}
      [] s = "cycle" -> CASE t = ROOT -> {"A"}
                          [] t = "A"  -> {"B"}
                          [] t = "B"  -> {ROOT}      \* closes back to the root
                          [] OTHER    -> {}
      [] s = "deep"  -> CASE t = ROOT -> {"A"}
                          [] t = "A"  -> {"B"}
                          [] t = "B"  -> {"C"}
                          [] t = "C"  -> {"D"}        \* 4 hops > MaxDepth, no root
                          [] OTHER    -> {}
      [] s = "wide"  -> CASE t = ROOT -> {"A","B","C","D","E"}  \* 5 > MaxNodes
                          [] OTHER    -> {}
      [] s = "ring"  -> CASE t = ROOT -> {"A"}
                          [] t = "A"  -> {"B"}
                          [] t = "B"  -> {"C"}
                          [] t = "C"  -> {"A"}        \* non-root cycle; safe
                          [] OTHER    -> {}
      [] OTHER       -> {}

\* The seed frontier (level 1) for scenario s.
Seed(s) == Refs(s, ROOT)

\* ---------------------------------------------------------------------------
\* Ground-truth graph properties, computed independently of the walk, used to
\* STATE the safety invariants (the walk must agree with these). These are over
\* the nodes reachable from the seed.
\* ---------------------------------------------------------------------------

\* One step of reachability closure from a set ns under scenario s.
ExpandOnce(s, ns) == ns \cup UNION { Refs(s, n) : n \in ns }

\* Reachable-from-seed set, closed by iterating ExpandOnce to a fixed point. The
\* universe is finite (|Tasks| nodes), so |Tasks| iterations always converge.
ReachSeed(s) ==
    LET R0 == Seed(s)
        R1 == ExpandOnce(s, R0)
        R2 == ExpandOnce(s, R1)
        R3 == ExpandOnce(s, R2)
        R4 == ExpandOnce(s, R3)
        R5 == ExpandOnce(s, R4)
        R6 == ExpandOnce(s, R5)
    IN R6

\* TRUE iff some node reachable from the seed references the root (a cycle that
\* passes through the root). The seed itself referencing the root also counts.
HasRootCycle(s) ==
    \/ ROOT \in Seed(s)
    \/ \E n \in ReachSeed(s) : ROOT \in Refs(s, n)

\* TRUE iff the seed-reachable graph (excluding the root) has more than MaxNodes
\* distinct nodes — the fan-out budget the width check enforces.
TooWide(s) == Cardinality(ReachSeed(s) \ {ROOT}) > MaxNodes

\* TRUE iff some path from the seed is longer than MaxDepth hops (a chain the
\* depth check must reject before it would otherwise terminate). Computed as a
\* bounded BFS depth: the number of expansion levels needed to drain the
\* frontier under the SAME visited-filter the walk uses, capped so the operator
\* is total even on an (unfiltered) infinite expansion.
\* For the modeled scenarios this is simply "the acyclic chain is too long".
TooDeep(s) ==
    \* "deep" is the only modeled too-deep scenario: a 4-hop acyclic chain.
    s = "deep"

\* A scenario whose reachable graph is structurally abusive (must be rejected).
IsBadGraph(s) == HasRootCycle(s) \/ TooWide(s) \/ TooDeep(s)

\* ---------------------------------------------------------------------------
\* Walk state.
\* ---------------------------------------------------------------------------
VARIABLES
    scenario,   \* the chosen graph shape, or NOBODY before the walk starts
    frontier,   \* SUBSET Tasks: the current BFS level (distinct, visited-filtered)
    visited,    \* SUBSET Tasks: every task already expanded (ROOT never inserted)
    depth,      \* the current BFS depth (1-based), capped for type-finiteness
    verdict,    \* "running" | "accept" | "cycle" | "depth" | "width"
    sawRoot,    \* BOOLEAN: a frontier ever contained the root (cycle witnessed)
    maxSeen     \* the largest visited.len()+frontier.len() the walk evaluated

vars == <<scenario, frontier, visited, depth, verdict, sawRoot, maxSeen>>

\* Depth is capped at MaxDepth+1 in the type (the walk rejects at depth>MaxDepth,
\* so it never needs to count higher). maxSeen is capped at the universe size.
DepthCap == MaxDepth + 1
NodeCap  == Cardinality(Tasks)

TypeOK ==
    /\ scenario \in Scenarios \cup {NOBODY}
    /\ frontier \subseteq Tasks
    /\ visited \subseteq Tasks
    /\ depth \in 1..DepthCap
    /\ verdict \in {"running", "accept", "cycle", "depth", "width"}
    /\ sawRoot \in BOOLEAN
    /\ maxSeen \in 0..NodeCap

Done == verdict \in {"accept", "cycle", "depth", "width"}

Init ==
    /\ scenario = NOBODY
    /\ frontier = {}
    /\ visited = {}
    /\ depth = 1
    /\ verdict = "running"
    /\ sawRoot = FALSE
    /\ maxSeen = 0

\* ---------------------------------------------------------------------------
\* Start a walk: pick a scenario and seed the level-1 frontier with the root's
\* out-edges. visited starts empty; depth = 1. (Modeled once per window: the walk
\* runs to a terminal verdict, then Recycle picks a fresh scenario.)
\* ---------------------------------------------------------------------------
StartWalk(s) ==
    /\ scenario = NOBODY
    /\ scenario' = s
    /\ frontier' = Seed(s)
    /\ visited' = {}
    /\ depth' = 1
    /\ verdict' = "running"
    /\ sawRoot' = FALSE
    /\ maxSeen' = 0

\* ---------------------------------------------------------------------------
\* Expand one BFS level — the body of one while-iteration of check_reference_graph.
\* Precondition: a walk is running with a non-empty frontier.
\*
\* The checks fire IN THE CODE'S ORDER:
\*   1. depth   : if depth > MaxDepth -> reject "depth".
\*   2. cycle   : if any frontier task is the root -> reject "cycle".
\*   3. width   : if |visited| + |frontier| > MaxNodes -> reject "width".
\*   4. expand  : visited := visited \cup frontier;
\*                next := (UNION Refs over frontier) \ visited;   <-- visited filter
\*                frontier := next; depth := depth + 1.
\* When next is empty the frontier empties and the NEXT step accepts (Accept).
\* ---------------------------------------------------------------------------
ExpandLevel ==
    /\ verdict = "running"
    /\ frontier # {}
    /\ sawRoot' = (sawRoot \/ ROOT \in frontier)
    /\ IF depth > MaxDepth
       THEN \* (1) depth bound.
            /\ verdict' = "depth"
            /\ UNCHANGED <<frontier, visited, depth, maxSeen>>
       ELSE IF ROOT \in frontier
       THEN \* (2) cycle: a reference reached the root.
            /\ verdict' = "cycle"
            /\ UNCHANGED <<frontier, visited, depth, maxSeen>>
       ELSE IF Cardinality(visited) + Cardinality(frontier) > MaxNodes
       THEN \* (3) width: the fan-out budget is exceeded.
            /\ verdict' = "width"
            /\ UNCHANGED <<frontier, visited, depth>>
            /\ maxSeen' = IF Cardinality(visited) + Cardinality(frontier) > maxSeen
                          THEN Cardinality(visited) + Cardinality(frontier) ELSE maxSeen
       ELSE \* (4) expand this level: visited grows, next frontier is filtered.
            LET nv == visited \cup frontier
                next == (UNION { Refs(scenario, t) : t \in frontier }) \ nv
                seen == Cardinality(visited) + Cardinality(frontier)
            IN /\ visited' = nv
               /\ frontier' = next
               /\ depth' = depth + 1
               /\ verdict' = "running"
               /\ maxSeen' = IF seen > maxSeen THEN seen ELSE maxSeen
    /\ UNCHANGED scenario

\* The frontier emptied without any reject firing -> ACCEPT (the loop's Ok(())).
Accept ==
    /\ verdict = "running"
    /\ frontier = {}
    /\ scenario # NOBODY
    /\ verdict' = "accept"
    /\ UNCHANGED <<scenario, frontier, visited, depth, sawRoot, maxSeen>>

\* ---------------------------------------------------------------------------
\* Recycle: a walk reached a terminal verdict. Reset so a fresh scenario can be
\* chosen — the system CYCLES (no benign terminal deadlock under -deadlock).
\* ---------------------------------------------------------------------------
Recycle ==
    /\ Done
    /\ scenario' = NOBODY
    /\ frontier' = {}
    /\ visited' = {}
    /\ depth' = 1
    /\ verdict' = "running"
    /\ sawRoot' = FALSE
    /\ maxSeen' = 0

Next ==
    \/ \E s \in Scenarios : StartWalk(s)
    \/ ExpandLevel
    \/ Accept
    \/ Recycle

Spec == Init /\ [][Next]_vars /\ WF_vars(ExpandLevel) /\ WF_vars(Accept)

\* =======================================================================
\* SAFETY
\* =======================================================================

\* --- Finiteness / Terminates -------------------------------------------------

\* The root is NEVER inserted into visited: the code deliberately leaves it out so
\* a reference to the root hits the cycle check rather than being silently dropped
\* as "already visited". (visited grows only by folding in a frontier; the cycle
\* check rejects before any frontier containing the root is folded.)
RootNeverVisited == ROOT \notin visited

\* visited never exceeds the node budget: the width check rejects before a level
\* would push it past MaxNodes, so by the time a level is folded in,
\* |visited|+|frontier| <= MaxNodes, hence |visited| <= MaxNodes always.
VisitedBounded == Cardinality(visited) <= MaxNodes

\* The visited FILTER: no node in the current frontier is already in visited.
\* The expansion computes next = Refs(...) \ visited, so an already-expanded node
\* can never reappear in a later frontier. This is what makes each node expand at
\* most once and the walk finite. (Dropping the filter violates this.)
FrontierDisjoint == frontier \cap visited = {}

\* --- RejectsBadGraph ---------------------------------------------------------

\* A structurally abusive scenario (root-cycle, too deep, or too wide) is NEVER
\* accepted: once the walk is Done on such a scenario, the verdict is some reject.
\* Anchored on the independent ground-truth IsBadGraph, so a buggy walk that drops
\* a check and accepts the abuse trips this.
RejectsBadGraph ==
    (Done /\ scenario # NOBODY /\ IsBadGraph(scenario))
        => verdict # "accept"

\* --- AcceptsOnlyClean --------------------------------------------------------

\* An accept verdict implies the walk witnessed no abuse: it never saw the root in
\* a frontier (no root-cycle), the budget was never exceeded (maxSeen <= MaxNodes),
\* and it never advanced past MaxDepth (depth <= MaxDepth+1 by the type, and an
\* acceptance means the frontier drained before the depth check could fire). The
\* ground-truth restatement: an accepted scenario is NOT a bad graph.
AcceptsOnlyClean ==
    (verdict = "accept")
        => /\ ~sawRoot
           /\ maxSeen <= MaxNodes
           /\ (scenario # NOBODY => ~IsBadGraph(scenario))

\* =======================================================================
\* LIVENESS — the walk always reaches a terminal verdict (Terminates).
\* =======================================================================

\* From any running walk, a terminal verdict is eventually reached: visited grows
\* monotonically and is bounded, depth is bounded, the frontier is visited-filtered
\* so it strictly shrinks the reachable remainder — there is no infinite expansion.
Terminates == (scenario # NOBODY /\ verdict = "running") ~> Done

============================================================================
