----------------------------- MODULE KeyRotation -----------------------------
(*
 * TLA+ specification of Acteon's encrypted-state key-rotation contract.
 *
 * Models crates/crypto/src/lib.rs:
 *   - struct PayloadEncryptor { keys: Vec<PayloadKeyEntry> } where each entry has
 *     a `kid` (~242). `keys[0]` is the ACTIVE encryption key.
 *   - encrypt_json/encrypt_str (~297/317): encrypt with keys[0] and STAMP its kid
 *     into the envelope via encrypt_value_with_kid (~200) =>
 *     ENC[AES256-GCM,kid:<KID>,data:...]. current_kid (~290) returns keys[0].kid.
 *   - decrypt_raw (~334): extract the kid from the envelope (extract_kid ~235),
 *     look up the entry whose `kid` matches and decrypt with it; if the kid is
 *     absent/legacy or not found, FALL BACK to trying every key in order. So a
 *     value decrypts IFF its stamping key is still present in the keyset.
 *   - with_keys(Vec<PayloadKeyEntry>) (~280): build an encryptor from an explicit
 *     ordered keyset. IMPORTANT: the code does NOT auto-re-encrypt stored values
 *     and does NOT auto-remove keys. ROTATION is an OPERATOR action: prepend a new
 *     active key (it becomes keys[0]) and RETAIN the old keys so their ciphertext
 *     still decrypts. Removing an old key is likewise an operator decision.
 *
 * Protocol (the operational rotation contract that decrypt-by-kid imposes). A
 * keyset = a set of kids with one marked ACTIVE (= keys[0]), plus a set of stored
 * values, each STAMPED with the kid it was encrypted under. Operations:
 *   Encrypt(v): stamp value v with the ACTIVE kid and store it (faithful to
 *               encrypt_*: keys[0].kid goes into the envelope).
 *   Rotate(k):  add a fresh kid k to the keyset and make it active (prepend); the
 *               previously-active kid is RETAINED (operator rotation pass).
 *   ReEncrypt(v): re-encrypt a stored value from its old kid to the current active
 *               kid (operator re-encryption pass over already-stored ciphertext).
 *   RetireKey(k): remove kid k from the keyset. SAFE ONLY IF no stored value is
 *               still stamped with k (the modeled fix / precondition).
 *   Decrypt(v): succeeds IFF v's stamping kid is still in the keyset (decrypt_raw:
 *               the lookup-by-kid, and the all-keys fallback, both need the key).
 *
 * Verified (over every interleaving of encrypt / rotate / re-encrypt / retire):
 *   - NoUndecryptable: every stored value's kid is still present in the keyset, so
 *     decrypt_raw can always find a key for it — every value is always decryptable.
 *     This is exactly the contract decrypt-by-kid depends on.
 *   - ActiveInKeyset / NonEmptyKeyset (well-formedness the code assumes: keys[0]
 *     exists, so current_kid and encrypt always have an active key).
 *
 * The fix anchored here is RetireKey's precondition: retire a kid ONLY when it has
 * NO live (still-stamped) ciphertext. ReEncrypt is what DRAINS a kid's live values
 * onto the active kid so that retiring it later becomes safe.
 *
 * Negative check: revert the RetireKey precondition (allow retiring a kid even
 * while a stored value is still stamped with it). That value's kid leaves the
 * keyset while the value persists -> decrypt_raw can no longer find its key ->
 * NoUndecryptable is violated and TLC reports the counterexample (a stored value
 * whose kid is gone from the keyset). The precondition is INDEPENDENTLY load-
 * bearing: it is the only guard standing between ReEncrypt-draining and retirement.
 *
 * SCOPE. encrypt-with-active-kid and decrypt-by-kid (lookup-then-all-keys-fallback)
 * are FAITHFUL to crypto/lib.rs — a value is decryptable exactly when its stamping
 * key is in the keyset, which is what NoUndecryptable checks. Rotate / ReEncrypt /
 * RetireKey model the OPERATOR rotation PROCEDURE, NOT code in lib.rs: the crate
 * retains keys and never auto-retires, so these are the operational steps an
 * operator performs with with_keys(...). The AES-GCM crypto itself, IV/tag
 * handling, legacy no-kid envelopes, and the SecretString/zeroize machinery are
 * abstracted to "stamped with a kid". The spec verifies the rotation CONTRACT that
 * the code's decrypt-by-kid relies on: never retire a key with live ciphertext.
 *
 * Run with:
 *   java -jar tla2tools.jar -config KeyRotation.cfg KeyRotation.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Kids,    \* the pool of key identifiers the operator may introduce, e.g. {ka,kb,kc}
    Values,  \* the stored ciphertext values, e.g. {v1, v2}
    NOBODY   \* sentinel: a value not yet stored / no stamping kid

\* NOBODY is a single model value (NOBODY = NOBODY in the .cfg). KidSlots is the
\* domain of `stamp` — a stored value carries a real kid, an unstored one carries
\* NOBODY. (Operator, not a .cfg literal.)
KidSlots == Kids \cup {NOBODY}

VARIABLES
    keyset,   \* SUBSET Kids : the kids currently present in PayloadEncryptor.keys
    active,   \* Kids : the active kid (= keys[0].kid) used for new encryption
    stored,   \* SUBSET Values : values currently stored (have ciphertext at rest)
    stamp     \* [Values -> KidSlots] : the kid each value was encrypted under
              \*   (NOBODY when the value is not stored)

vars == <<keyset, active, stored, stamp>>

\* --------------------------------------------------------------------------
\* A kid is "live" if some STORED value is still stamped with it: retiring such a
\* kid would strand that value. This is the predicate RetireKey must respect.
\* --------------------------------------------------------------------------
LiveKid(k) == \E v \in stored : stamp[v] = k

TypeOK ==
    /\ keyset \subseteq Kids
    /\ active \in Kids
    /\ stored \subseteq Values
    /\ stamp \in [Values -> KidSlots]

\* Initially the keyset holds a single kid (a fresh single-key encryptor, like
\* PayloadEncryptor::new), it is the active kid, and nothing is stored yet.
Init ==
    /\ \E k0 \in Kids :
         /\ keyset = {k0}
         /\ active = k0
    /\ stored = {}
    /\ stamp = [v \in Values |-> NOBODY]

\* --------------------------------------------------------------------------
\* Encrypt(v): store a not-yet-stored value, stamping it with the ACTIVE kid.
\* Faithful to encrypt_str/encrypt_json: the envelope carries keys[0].kid.
\* --------------------------------------------------------------------------
Encrypt(v) ==
    /\ v \notin stored
    /\ stored' = stored \cup {v}
    /\ stamp' = [stamp EXCEPT ![v] = active]
    /\ UNCHANGED <<keyset, active>>

\* --------------------------------------------------------------------------
\* Rotate(k): operator prepends a fresh active key. k must be a kid NOT already in
\* the keyset (a new key). The previously-active kid is RETAINED in the keyset so
\* values stamped with it still decrypt (the code never drops keys on rotation).
\* --------------------------------------------------------------------------
Rotate(k) ==
    /\ k \notin keyset
    /\ keyset' = keyset \cup {k}
    /\ active' = k
    /\ UNCHANGED <<stored, stamp>>

\* --------------------------------------------------------------------------
\* ReEncrypt(v): operator re-encryption pass. A stored value stamped with an OLD
\* (non-active) kid is decrypted and re-encrypted under the current active kid, so
\* its stamp moves to `active`. This is what DRAINS an old kid's live values,
\* eventually making that kid retireable. The active kid is always in the keyset
\* (ActiveInKeyset), so the re-stamped value stays decryptable.
\* --------------------------------------------------------------------------
ReEncrypt(v) ==
    /\ v \in stored
    /\ stamp[v] # active
    /\ stamp' = [stamp EXCEPT ![v] = active]
    /\ UNCHANGED <<keyset, active, stored>>

\* --------------------------------------------------------------------------
\* RetireKey(k): operator removes kid k from the keyset (drops it from
\* with_keys(...)). THE MODELED FIX / PRECONDITION: k may be retired ONLY IF it is
\* not the active kid AND no stored value is still stamped with it (~LiveKid(k)).
\* Retiring a kid with live ciphertext would leave that value undecryptable — the
\* exact failure NoUndecryptable guards against (and the negative check reverts).
\* --------------------------------------------------------------------------
RetireKey(k) ==
    /\ k \in keyset
    /\ k # active
    /\ ~LiveKid(k)
    /\ keyset' = keyset \ {k}
    /\ UNCHANGED <<active, stored, stamp>>

\* --------------------------------------------------------------------------
\* Recycle: when every value has been stored and re-encrypted onto the active kid
\* (no value stamped with a non-active kid) and the keyset has been pruned to just
\* the active kid, reset to a fresh single-key encryptor so the system CYCLES
\* (no benign terminal deadlock under -deadlock). Picks a fresh active kid.
\* --------------------------------------------------------------------------
Recycle ==
    /\ keyset = {active}
    /\ stored = Values
    /\ \A v \in stored : stamp[v] = active
    /\ \E k0 \in Kids :
         /\ keyset' = {k0}
         /\ active' = k0
    /\ stored' = {}
    /\ stamp' = [v \in Values |-> NOBODY]

Next ==
    \/ \E v \in Values : Encrypt(v) \/ ReEncrypt(v)
    \/ \E k \in Kids : Rotate(k) \/ RetireKey(k)
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* THE KEY INVARIANT: every stored value's stamping kid is still present in the
\* keyset. decrypt_raw finds a key for a value exactly when its kid is in the
\* keyset (direct kid lookup, or the all-keys fallback) — so this is precisely
\* "every stored ciphertext is always decryptable". RetireKey's precondition is
\* what preserves it; reverting that precondition strands a value and trips this.
NoUndecryptable ==
    \A v \in stored : stamp[v] \in keyset

\* The active kid (keys[0].kid) is always part of the keyset, so encrypt always
\* stamps a kid that the keyset can later decrypt, and current_kid is well-defined.
ActiveInKeyset == active \in keyset

\* The keyset is never empty — PayloadEncryptor requires at least one key
\* (with_keys asserts non-empty; keys[0] must exist for current_kid/encrypt).
NonEmptyKeyset == keyset # {}

==============================================================================
