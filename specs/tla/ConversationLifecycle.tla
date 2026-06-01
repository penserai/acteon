-------------------------- MODULE ConversationLifecycle --------------------------
(*
 * TLA+ specification of Acteon's bus-conversation lifecycle under concurrent
 * transitions and message posts.
 *
 * Models:
 *   - crates/core/src/bus_conversation.rs ::
 *       enum ConversationState { Active, Resolved, Archived }
 *       enum ConversationTransition { Resolve, Reopen, Archive }
 *       fn apply_transition (~150) — the EXACT legal edges, everything else
 *         Err(IllegalTransition):
 *             (Active,   Resolve) -> Resolved
 *             (Resolved, Reopen)  -> Active
 *             (Resolved, Archive) -> Archived
 *         (so NO Active->Archive shortcut, NO Resolve/Reopen from Archived).
 *       fn accepts_messages (~181) = !matches!(state, Archived) — Active and
 *         Resolved accept posts; Archived is READ-ONLY.
 *   - crates/server/src/api/bus.rs ::
 *       cas_update (~185): optimistic version-CAS read-modify-write —
 *           let (raw, version) = store.get_versioned(key)?;   // READ state+version
 *           mutate(&mut current)?;          // apply_transition RE-VALIDATES
 *                                           // against the FRESH loaded state
 *           compare_and_swap(key, version, payload)?;  // COMMIT iff version eq
 *         on Conflict the loop RE-READS and RE-APPLIES (MAX_CAS_RETRY_ATTEMPTS).
 *       transition_conversation (~5159): drives a transition through cas_update,
 *         so the legality check runs against the FRESH row at commit.
 *       append_conversation_message (~5225): the post path.
 *
 * Protocol. A single conversation row carries a `state` and a `version`
 * counter. Concurrent actors do two things:
 *   - Transitioners READ (state, version), pick a transition legal from the
 *     READ state, then COMMIT via version-CAS. apply_transition re-validates
 *     the legal-edge graph against the FRESH row; the commit lands ONLY IF the
 *     version is unchanged (faithful to cas_update). The loser of a race re-reads
 *     the winner's new state and is refused if its transition is now illegal.
 *   - Posters READ the state (load_conversation), then attempt to APPEND a
 *     message, accepted ONLY IF accepts_messages() holds against the
 *     FRESH/committed state at the post-commit instant — NOT a stale snapshot
 *     frozen at READ. READ and COMMIT are SEPARATE steps so an Archive can commit
 *     in between; the fresh re-check at commit is what is load-bearing.
 *
 * Verified (over every interleaving of concurrent transitioners and posters):
 *   - OnlyLegalTransitions: every committed state change is one of the three
 *     legal edges (the `illegal_transition` flag stays FALSE).
 *   - NoSkipArchive: the conversation never reaches Archived without having
 *     passed through Resolved first — there is no Active->Archived shortcut
 *     (tracked by `ever_resolved`, set when the row is Resolved; an Archived row
 *     with ever_resolved=FALSE would be a skip).
 *   - ArchivedFinal: once Archived the state never changes again until Recycle
 *     mints a brand-new conversation (terminal by design).
 *   - NoPostAfterArchive: a message is never ACCEPTED once the row is Archived,
 *     even when the post races the Archive transition — the accepts_messages
 *     guard is checked against the committed/fresh state, not a stale read.
 *
 * Negative check (each fix independently load-bearing):
 *   (1) apply_transition legality — add the Active->Archive edge to CanTransition.
 *       A transitioner then archives a never-resolved row -> NoSkipArchive
 *       violated (Archived reached with ever_resolved=FALSE).
 *   (2) accepts_messages-against-fresh-state — gate PostCommit on a stale read of
 *       the state frozen at PostRead (introduce a per-poster snapshot variable and
 *       decide on it) instead of the fresh state. A poster that read Resolved
 *       (open) then commits AFTER a concurrent Archive committed accepts a post
 *       onto an Archived row -> NoPostAfterArchive violated (bad_post set TRUE).
 *
 * SCOPE. Isolates the conversation lifecycle state machine: the legal-edge graph,
 * its linearity (no Active->Archived skip), Archived finality, and the
 * post-acceptance coupling to the committed state. The transition path's
 * version-CAS + fresh re-validation is faithful to transition_conversation +
 * cas_update. The post path's FRESH accepts-check is the MODELED FIX being
 * verified: in today's append_conversation_message the accepts_messages() check
 * is evaluated against load_conversation's plain (non-CAS) READ and the produce
 * is not gated by a version-CAS on the row, so a real TOCTOU window exists
 * between that read and the Kafka produce — this spec demonstrates that closing
 * that window (re-check accepts against the fresh/committed state) is exactly
 * what makes NoPostAfterArchive hold; reverting it (negative (2)) breaks it.
 * This abstracts the participant ACL, sender override, header validation, the
 * Kafka produce side-effect, the schema/topic-registration checks, and the
 * best-effort updated_at bump — separate concerns. Recycle models a fresh Active
 * conversation minted at the same key once the row is Archived, so the system
 * CYCLES (no benign terminal deadlock under -deadlock).
 *
 * Run with:
 *   java -jar tla2tools.jar -config ConversationLifecycle.cfg ConversationLifecycle.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Transitioners,  \* concurrent transition attempts / replicas, e.g. {t1, t2}
    Posters,        \* concurrent message posters, e.g. {p1, p2}
    MaxVer,         \* state-space bound on the version counter
    NOBODY          \* sentinel: actor holds no in-flight read / no target picked

\* -----------------------------------------------------------------------
\* The three conversation states (bus_conversation.rs::ConversationState).
\* -----------------------------------------------------------------------
Active   == "active"
Resolved == "resolved"
Archived == "archived"

States == { Active, Resolved, Archived }

\* The three transitions (bus_conversation.rs::ConversationTransition).
Resolve == "resolve"
Reopen  == "reopen"
Archive == "archive"

Transitions == { Resolve, Reopen, Archive }

\* apply_transition's EXACT legal-edge graph. An operator (not a .cfg function
\* literal) so the .cfg stays clean. Everything not listed is IllegalTransition.
CanTransition(s, tr) ==
    \/ /\ s = Active   /\ tr = Resolve   \* Active   -> Resolved
    \/ /\ s = Resolved /\ tr = Reopen    \* Resolved -> Active
    \/ /\ s = Resolved /\ tr = Archive   \* Resolved -> Archived
    \* No Active->Archive shortcut; Archived is terminal (no outgoing edge).

\* Resulting state of a legal transition (only consulted when CanTransition holds).
ApplyTransition(s, tr) ==
    CASE (s = Active   /\ tr = Resolve) -> Resolved
      [] (s = Resolved /\ tr = Reopen)  -> Active
      [] (s = Resolved /\ tr = Archive) -> Archived
      [] OTHER                          -> s

\* accepts_messages() — Archived is read-only; Active and Resolved accept posts.
AcceptsMessages(s) == s # Archived

\* -----------------------------------------------------------------------
\* Variables
\* -----------------------------------------------------------------------
VARIABLES
    conv_state,          \* current persisted state of the single conversation row
    version,             \* optimistic-CAS version, bumped on every committed transition
    ever_resolved,       \* BOOLEAN: row has been Resolved since the last Recycle
    illegal_transition,  \* BOOLEAN: TRUE iff any commit broke the legal-edge graph
    bad_post,            \* BOOLEAN: TRUE iff any post was accepted onto an Archived row
    t_phase,             \* [Transitioners -> {"idle","committing"}]
    t_read_ver,          \* [Transitioners -> 0..MaxVer]: version observed at READ
    t_target,            \* [Transitioners -> Transitions \cup {NOBODY}]: intended transition
    p_phase              \* [Posters -> {"idle","posting"}]

vars == <<conv_state, version, ever_resolved, illegal_transition, bad_post,
          t_phase, t_read_ver, t_target, p_phase>>

TypeOK ==
    /\ conv_state \in States
    /\ version \in 0..MaxVer
    /\ ever_resolved \in BOOLEAN
    /\ illegal_transition \in BOOLEAN
    /\ bad_post \in BOOLEAN
    /\ t_phase \in [Transitioners -> {"idle", "committing"}]
    /\ t_read_ver \in [Transitioners -> 0..MaxVer]
    /\ t_target \in [Transitioners -> Transitions \cup {NOBODY}]
    /\ p_phase \in [Posters -> {"idle", "posting"}]

Init ==
    /\ conv_state = Active
    /\ version = 0
    /\ ever_resolved = FALSE
    /\ illegal_transition = FALSE
    /\ bad_post = FALSE
    /\ t_phase = [t \in Transitioners |-> "idle"]
    /\ t_read_ver = [t \in Transitioners |-> 0]
    /\ t_target = [t \in Transitioners |-> NOBODY]
    /\ p_phase = [p \in Posters |-> "idle"]

\* -----------------------------------------------------------------------
\* Transitioner actions: cas_update + apply_transition.
\* -----------------------------------------------------------------------

\* READ phase: get_versioned(key) -> (state, version), then pick a transition
\* LEGAL from the just-read state (what the transition_conversation caller hands
\* apply_transition). The snapshot may be stale by the time it commits.
TransRead(t, tr) ==
    /\ t_phase[t] = "idle"
    /\ CanTransition(conv_state, tr)
    /\ t_read_ver' = [t_read_ver EXCEPT ![t] = version]
    /\ t_target'   = [t_target   EXCEPT ![t] = tr]
    /\ t_phase'    = [t_phase    EXCEPT ![t] = "committing"]
    /\ UNCHANGED <<conv_state, version, ever_resolved, illegal_transition,
                   bad_post, p_phase>>

\* COMMIT phase: version-CAS. compare_and_swap(key, read_version, payload) lands
\* ONLY IF the version is unchanged since the read. apply_transition additionally
\* RE-VALIDATES the legal-edge graph against the FRESH loaded row; with the
\* version-CAS the fresh row IS the read row on the winning path, so re-validation
\* holds — modeled explicitly to keep the guard faithful and load-bearing.
TransCommitWin(t) ==
    /\ t_phase[t] = "committing"
    /\ version < MaxVer                          \* bound the version counter (CI)
    /\ t_read_ver[t] = version                   \* CAS: version unchanged
    /\ CanTransition(conv_state, t_target[t])    \* apply_transition re-validation
    /\ conv_state' = ApplyTransition(conv_state, t_target[t])
    /\ version' = version + 1
    /\ ever_resolved' = (ever_resolved \/ ApplyTransition(conv_state, t_target[t]) = Resolved)
    \* Track any illegal committed move; with the guard above it stays FALSE.
    /\ illegal_transition' =
         (illegal_transition \/ ~CanTransition(conv_state, t_target[t]))
    /\ t_phase'  = [t_phase  EXCEPT ![t] = "idle"]
    /\ t_target' = [t_target EXCEPT ![t] = NOBODY]
    /\ UNCHANGED <<bad_post, t_read_ver, p_phase>>

\* COMMIT conflict: a concurrent commit bumped the version since this read
\* (CasResult::Conflict). The actor abandons its stale attempt and re-reads
\* (the cas_update retry loop).
TransCommitConflict(t) ==
    /\ t_phase[t] = "committing"
    /\ t_read_ver[t] # version                   \* CAS failed: version moved
    /\ t_phase'  = [t_phase  EXCEPT ![t] = "idle"]
    /\ t_target' = [t_target EXCEPT ![t] = NOBODY]
    /\ UNCHANGED <<conv_state, version, ever_resolved, illegal_transition,
                   bad_post, t_read_ver, p_phase>>

\* Once the version counter saturates the bound, a mid-flight transitioner gives
\* up (CAS attempts exhausted). Keeps the state space finite without falsely
\* deadlocking before Recycle can fire.
TransGiveUp(t) ==
    /\ t_phase[t] = "committing"
    /\ version = MaxVer
    /\ t_phase'  = [t_phase  EXCEPT ![t] = "idle"]
    /\ t_target' = [t_target EXCEPT ![t] = NOBODY]
    /\ UNCHANGED <<conv_state, version, ever_resolved, illegal_transition,
                   bad_post, t_read_ver, p_phase>>

\* -----------------------------------------------------------------------
\* Poster actions: append_conversation_message.
\* -----------------------------------------------------------------------

\* READ phase: load_conversation -> state. The poster moves to "posting"; the
\* read state is deliberately NOT frozen for the commit decision (the fix is to
\* re-check the FRESH state at commit). A concurrent Archive can commit before the
\* poster's commit step, which is exactly the TOCTOU window this models.
PostRead(p) ==
    /\ p_phase[p] = "idle"
    /\ p_phase' = [p_phase EXCEPT ![p] = "posting"]
    /\ UNCHANGED <<conv_state, version, ever_resolved, illegal_transition,
                   bad_post, t_phase, t_read_ver, t_target>>

\* COMMIT phase: the message is accepted ONLY IF accepts_messages() holds against
\* the FRESH/committed state at THIS instant (the modeled fix). A post that raced
\* an Archive that has since committed is refused (see PostRefused). bad_post is
\* set TRUE only if a post is ever accepted while the fresh row is Archived — under
\* the fresh guard that is unreachable, so it stays FALSE.
PostCommit(p) ==
    /\ p_phase[p] = "posting"
    /\ AcceptsMessages(conv_state)               \* fresh re-check at commit
    /\ bad_post' = (bad_post \/ ~AcceptsMessages(conv_state))
    /\ p_phase' = [p_phase EXCEPT ![p] = "idle"]
    /\ UNCHANGED <<conv_state, version, ever_resolved, illegal_transition,
                   t_phase, t_read_ver, t_target>>

\* COMMIT refused: the fresh state no longer accepts messages (a concurrent
\* Archive committed since the read). The post is rejected (the 400 "archived"
\* path); the poster returns to idle. No message accepted, bad_post untouched.
PostRefused(p) ==
    /\ p_phase[p] = "posting"
    /\ ~AcceptsMessages(conv_state)              \* fresh state is Archived
    /\ p_phase' = [p_phase EXCEPT ![p] = "idle"]
    /\ UNCHANGED <<conv_state, version, ever_resolved, illegal_transition,
                   bad_post, t_phase, t_read_ver, t_target>>

\* -----------------------------------------------------------------------
\* Recycle: once the conversation is Archived (terminal) and no actor is
\* mid-flight, a fresh Active conversation is minted at the same key (state and
\* version reset, ever_resolved cleared). The system CYCLES so there is no benign
\* terminal deadlock under -deadlock.
\* -----------------------------------------------------------------------
Recycle ==
    /\ conv_state = Archived
    /\ \A t \in Transitioners : t_phase[t] = "idle"
    /\ \A p \in Posters : p_phase[p] = "idle"
    /\ conv_state' = Active
    /\ version' = 0
    /\ ever_resolved' = FALSE
    /\ t_read_ver' = [t \in Transitioners |-> 0]
    /\ UNCHANGED <<illegal_transition, bad_post, t_phase, t_target, p_phase>>

Next ==
    \/ \E t \in Transitioners :
        \/ \E tr \in Transitions : TransRead(t, tr)
        \/ TransCommitWin(t)
        \/ TransCommitConflict(t)
        \/ TransGiveUp(t)
    \/ \E p \in Posters :
        \/ PostRead(p)
        \/ PostCommit(p)
        \/ PostRefused(p)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* Every committed transition obeys the legal-edge graph. Set TRUE by any commit
\* moving via an edge outside CanTransition; stays FALSE under apply_transition's
\* re-validation against the fresh row.
OnlyLegalTransitions == illegal_transition = FALSE

\* The conversation never reaches Archived without having passed through Resolved
\* first: an Archived row always has ever_resolved = TRUE. There is no
\* Active->Archived shortcut (linearity by design). Reverting fix (1) — adding the
\* Active->Archive edge — lets a never-resolved row archive and breaks this.
NoSkipArchive == (conv_state = Archived) => ever_resolved

\* Once Archived the row makes no further same-key state change until Recycle
\* mints a brand-new conversation. Encoded as a two-state action property: from
\* Archived, the only allowed change is the Recycle reset to (Active, version 0).
ArchivedFinal ==
    [][ (conv_state = Archived /\ conv_state' # conv_state)
        => (conv_state' = Active /\ version' = 0) ]_vars

\* A message is never accepted once the row is Archived, even when the post races
\* the Archive transition. The fresh accepts_messages re-check at PostCommit is
\* the gate; reverting fix (2) (deciding on a stale read snapshot) lets a post land
\* on an Archived row and sets bad_post.
NoPostAfterArchive == bad_post = FALSE

============================================================================
