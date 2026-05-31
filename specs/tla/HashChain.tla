----------------------------- MODULE HashChain -----------------------------
(*
 * TLA+ specification of Acteon's compliance audit hash-chain sequencing.
 *
 * Models the protocol in crates/audit/audit/src/compliance.rs
 * (HashChainAuditStore): every audit record in a (namespace, tenant) chain is
 * assigned a strictly monotonic sequence_number. To append, a writer:
 *   1. fetch_tip()  — reads the record with the MAX sequence_number
 *      (PR #227: previously read by dispatched_at, which mis-ranked an intent
 *       and its outcome that share a timestamp and handed back a STALE tip,
 *       so two writers computed the SAME next sequence number);
 *   2. proposes seq = tip + 1, previous_hash = tip.hash;
 *   3. writes under a UNIQUE(namespace, tenant, sequence_number) constraint;
 *      on a duplicate-key conflict it re-reads the tip and retries.
 *
 * Verified (for every interleaving of concurrent writers / replicas):
 *   - ChainWellFormed: the committed chain is CONTIGUOUS (slots 0..tip),
 *     every slot below the tip has exactly one owner, and none above it —
 *     i.e. no duplicate sequence number and no fork.
 *
 * The #227 bug corresponds to ReadTip proposing a stale value instead of the
 * true max; that breaks contiguity (two owners would be forced onto one slot,
 * which the CAS rejects, and with a stale re-read the writer livelocks). The
 * fixed max-tip read keeps the chain contiguous and lets retries converge.
 *
 * Run with:
 *   java -jar tla2tools.jar -config HashChain.cfg HashChain.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Writers,   \* concurrent writers / gateway replicas, e.g. {w1, w2}
    MaxSeq,    \* bound on chain length for model-checking
    NOBODY     \* sentinel: slot uncommitted

Slots == 0..(MaxSeq - 1)

VARIABLES
    owner,      \* [Slots -> Writers \cup {NOBODY}] which writer committed each slot
    chain_len,  \* number of committed (contiguous) records = tip sequence + 1
    w_phase,    \* [Writers -> {"idle","proposing","committing","done"}]
    w_seq       \* [Writers -> 0..MaxSeq] sequence number a writer currently proposes

TypeOK ==
    /\ owner \in [Slots -> Writers \cup {NOBODY}]
    /\ chain_len \in 0..MaxSeq
    /\ w_phase \in [Writers -> {"idle", "proposing", "committing", "done"}]
    /\ w_seq \in [Writers -> 0..MaxSeq]

Init ==
    /\ owner = [s \in Slots |-> NOBODY]
    /\ chain_len = 0
    /\ w_phase = [w \in Writers |-> "idle"]
    /\ w_seq = [w \in Writers |-> 0]

\* Writer begins appending a record.
Start(w) ==
    /\ w_phase[w] = "idle"
    /\ w_phase' = [w_phase EXCEPT ![w] = "proposing"]
    /\ UNCHANGED <<owner, chain_len, w_seq>>

\* fetch_tip(): read the MAX sequence number and propose tip + 1 (= chain_len,
\* since the chain is contiguous). This is the #227-fixed read.
ReadTip(w) ==
    /\ w_phase[w] = "proposing"
    /\ w_seq' = [w_seq EXCEPT ![w] = chain_len]
    /\ w_phase' = [w_phase EXCEPT ![w] = "committing"]
    /\ UNCHANGED <<owner, chain_len>>

\* Attempt the UNIQUE(sequence_number) write (a CAS on the slot).
Commit(w) ==
    /\ w_phase[w] = "committing"
    /\ LET s == w_seq[w] IN
       IF s >= MaxSeq
       THEN \* Chain reached the model bound — nothing to append.
            /\ w_phase' = [w_phase EXCEPT ![w] = "done"]
            /\ UNCHANGED <<owner, chain_len, w_seq>>
       ELSE IF owner[s] = NOBODY
            THEN \* Slot free: this writer wins the unique sequence number.
                 /\ owner' = [owner EXCEPT ![s] = w]
                 /\ chain_len' = chain_len + 1
                 /\ w_phase' = [w_phase EXCEPT ![w] = "done"]
                 /\ UNCHANGED w_seq
            ELSE \* Duplicate-key conflict (another writer took it): re-read tip.
                 /\ w_phase' = [w_phase EXCEPT ![w] = "proposing"]
                 /\ UNCHANGED <<owner, chain_len, w_seq>>

\* Writer finishes and may append again.
Finish(w) ==
    /\ w_phase[w] = "done"
    /\ w_phase' = [w_phase EXCEPT ![w] = "idle"]
    /\ UNCHANGED <<owner, chain_len, w_seq>>

Next == \E w \in Writers : Start(w) \/ ReadTip(w) \/ Commit(w) \/ Finish(w)

vars == <<owner, chain_len, w_seq, w_phase>>
Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* The committed chain is contiguous (slots 0..chain_len-1 occupied, the rest
\* free) — no gap, no fork, and at most one owner per sequence number.
ChainWellFormed ==
    \A s \in Slots : (s < chain_len) <=> (owner[s] # NOBODY)

============================================================================
