----------------------------- MODULE DistributedLock -----------------------------
(*
 * TLA+ specification of Acteon's distributed lock primitive — the foundational
 * mutual-exclusion + owner-fencing lease that EVERY other spec assumes.
 *
 * Models:
 *   - crates/state/state/src/lock.rs :: trait DistributedLock { try_acquire,
 *     acquire } and trait LockGuard { release }.
 *   - crates/state/redis/src/lock.rs + crates/state/redis/src/scripts.rs :: the
 *     Redis impl. try_acquire runs `SET key <owner-uuid> NX PX <ttl_ms>` (NX =
 *     set only if absent; PX = millisecond TTL auto-expiry). A FRESH owner UUID
 *     is minted per acquisition and stored as the value (LOCK_ACQUIRE). release()
 *     runs the LOCK_RELEASE Lua script:
 *         local owner = redis.call('GET', KEYS[1])
 *         if owner == ARGV[1] then redis.call('DEL', KEYS[1]); return 1 end
 *         return 0
 *     a COMPARE-AND-DELETE on the owner token — it deletes the key ONLY IF the
 *     stored value still equals THIS guard's token. The TTL means the key
 *     auto-expires (the lease) if the holder never releases.
 *   - crates/state/memory/src/lock.rs :: the in-memory impl mirrors this — an
 *     owner-tagged entry with `expires_at`, and release() does
 *     `remove_if(name, |_, e| e.owner == self.owner)` (owner-checked delete).
 *
 * Protocol. A single lock key. N contenders each move idle -> InCS (in the
 * critical section, lease live) -> PendingRelease (left the critical section,
 * about to call release; the lease MAY have lapsed in the meantime) -> idle.
 *   - TryAcquire: succeeds, becoming the holder with a FRESH owner token + a
 *     TTL, ONLY IF the key is free; enters the critical section.
 *   - LeaveCS: finish the protected work and leave the critical section (still
 *     about to release). The lease may expire only AFTER this — modeling the
 *     documented precondition "the lock TTL is longer than the critical section"
 *     (redis/lock.rs ~line 35): a live holder's lease does not lapse mid-CS.
 *   - Release: deletes the key ONLY IF the stored owner token still equals this
 *     contender's token (the Lua compare-and-delete).
 * A clock expires the lease (TTL) once the holder has LEFT its critical section
 * (it abandoned/finished without releasing yet, or crashed): the expired lock
 * becomes free, and another contender can acquire it (with a NEW owner token).
 * A contender whose lease EXPIRED is "stale" and may still attempt to release —
 * the owner-token compare-and-delete must make that a no-op on the NEW holder's
 * lock. This stale-releaser race is the owner-fencing case under test.
 *
 * The OWNER-TOKEN check on release (compare-and-delete on the UUID) is the
 * load-bearing fix. A contender carries the token it acquired with; the lock
 * carries the token of its CURRENT holder. Release frees the lock ONLY when the
 * releaser's token equals the lock's current token. Tokens are modeled as a
 * monotonic counter so every acquisition gets a distinct, fresh value — faithful
 * to the per-acquisition UUID, and so a stale token can never collide with a
 * later holder's token.
 *
 * Verified (over every interleaving of concurrent contenders, releases, and TTL
 * expiry):
 *   - MutualExclusion: at most one contender is in its critical section at any
 *     time. The invariant counts contenders whose phase is "holding" — an
 *     INDEPENDENT ground-truth oracle, NOT the lock_owner/token bookkeeping the
 *     implementation mutates, so a buggy release that displaces a holder is
 *     caught rather than self-masked.
 *   - HolderMatchesLock: a contender in its critical section holds a token equal
 *     to the lock's current owner token (the holder is not silently displaced).
 *   - OwnerFencedRelease: the holder is never displaced from the lock except by
 *     its OWN release or a TTL expiry — a stale releaser's compare-and-delete
 *     never frees a re-acquired lock owned by someone else. Checked via an
 *     independent ground-truth witness (bad_free) that no release frees a lock
 *     owned by a different token.
 *
 * Negative check: replace the owner-token compare-and-delete in Release with a
 * BLIND unconditional delete (drop the `cont_token[c] = lock_owner` guard, free
 * the key regardless of token). A STALE contender in PendingRelease — whose lease
 * expired and the lock was re-acquired by ANOTHER contender now in its critical
 * section — then frees the NEW holder's lock; a THIRD contender acquires
 * concurrently while the displaced holder is STILL in its critical section, so
 * two contenders sit in the critical section at once -> MutualExclusion (count
 * "InCS" >= 2) is violated. With the token check, the stale release returns 0
 * (no-op) and the race is impossible.
 *
 * SCOPE. The `SET NX PX` acquire and the `GET / compare-and-DEL` Lua release are
 * each modeled as a SINGLE atomic TLA action — faithful to Redis executing them
 * atomically (NX is atomic; the Lua script runs atomically server-side). This
 * spec verifies the single-instance mutual-exclusion + owner-fencing contract.
 * ABSTRACTED: the acquire() timeout/poll-retry wait loop (try_acquire is the
 * primitive under test), extend()/is_held(), the PostgreSQL advisory-lock and
 * DynamoDB conditional-write backends, the connection pool, and the exact Lua
 * text. The documented Redis Cluster/Sentinel failover hole (async replication
 * losing a just-written key) is OUT OF SCOPE — this models the single-instance
 * guarantee the code claims to provide. The TTL is a logical tick counter.
 *
 * Run with:
 *   java -jar tla2tools.jar -config DistributedLock.cfg DistributedLock.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Contenders,  \* concurrent lock contenders / replicas, e.g. {c1, c2, c3}
    TTL,         \* lease TTL in ticks (PX milliseconds in production)
    MaxToken,    \* state-space bound on the monotonic owner-token counter
    NOBODY       \* sentinel: lock key is free (absent in Redis), token 0

VARIABLES
    lock_owner,   \* owner TOKEN currently stored at the key, or NOBODY if free
    lock_ttl,     \* 0..TTL: remaining lease ticks; 0 means free/expired
    next_token,   \* 1..MaxToken: next fresh owner token to mint on acquire
    cont_phase,   \* [Contenders -> {"idle","incs","pending_release"}]
                  \*   "incs"            = inside the critical section, lease live
                  \*   "pending_release" = left the critical section, about to call
                  \*                       release (lease may have lapsed by now)
    cont_token,   \* [Contenders -> 0..MaxToken]: token this contender last acquired
                  \*   with (0 = never acquired / released). Compared against
                  \*   lock_owner on release (the compare-and-delete) and used to
                  \*   detect staleness (token # lock_owner after a re-acquire).
    bad_free      \* BOOLEAN witness: TRUE iff some Release ever freed the lock
                  \*   while the releaser's token did NOT equal the lock's owner
                  \*   token — i.e. a non-owner displaced the current holder. The
                  \*   compare-and-delete keeps it FALSE; the blind-delete bug
                  \*   sets it. An INDEPENDENT ground-truth oracle (cf.
                  \*   illegal_commit in A2aTaskTransition.tla), read only by the
                  \*   OwnerFencedRelease invariant — never by any guard — so a
                  \*   buggy impl cannot self-mask the check.

vars == <<lock_owner, lock_ttl, next_token, cont_phase, cont_token, bad_free>>

TypeOK ==
    /\ lock_owner \in (1..MaxToken) \cup {NOBODY}
    /\ lock_ttl \in 0..TTL
    /\ next_token \in 1..(MaxToken + 1)
    /\ cont_phase \in [Contenders -> {"idle", "incs", "pending_release"}]
    /\ cont_token \in [Contenders -> 0..MaxToken]
    /\ bad_free \in BOOLEAN

Init ==
    /\ lock_owner = NOBODY
    /\ lock_ttl = 0
    /\ next_token = 1
    /\ cont_phase = [c \in Contenders |-> "idle"]
    /\ cont_token = [c \in Contenders |-> 0]
    /\ bad_free = FALSE

\* -----------------------------------------------------------------------
\* TryAcquire: SET key <fresh-owner> NX PX ttl. Succeeds ONLY IF the key is
\* free (NX): lock_owner = NOBODY (absent or just expired). Mints a FRESH owner
\* token (distinct monotonic value), stores it at the key with a full TTL, and
\* the contender enters its critical section as the holder.
\* Bounded by MaxToken so the token counter stays finite for TLC.
\* -----------------------------------------------------------------------
TryAcquire(c) ==
    /\ cont_phase[c] = "idle"
    /\ lock_owner = NOBODY                 \* NX: set only if key absent/free
    /\ next_token <= MaxToken              \* bound the monotonic token (CI)
    /\ lock_owner' = next_token            \* store the fresh owner token
    /\ lock_ttl' = TTL                     \* PX: full lease
    /\ next_token' = next_token + 1        \* next acquisition gets a new token
    /\ cont_token' = [cont_token EXCEPT ![c] = next_token]
    /\ cont_phase' = [cont_phase EXCEPT ![c] = "incs"]
    /\ UNCHANGED bad_free

\* -----------------------------------------------------------------------
\* LeaveCS: finish the protected work and leave the critical section. The
\* contender is now in pending_release (about to call release) and STILL "owns"
\* the lock token, but its lease may lapse before it releases. Separating this
\* from Release means the lease can only expire AFTER the contender has left the
\* critical section — faithful to the documented precondition that the TTL is
\* longer than the critical section (a live in-CS holder's lease never lapses
\* mid-section; the lease exists to reclaim a holder that finished/crashed
\* without releasing).
\* -----------------------------------------------------------------------
LeaveCS(c) ==
    /\ cont_phase[c] = "incs"
    /\ cont_phase' = [cont_phase EXCEPT ![c] = "pending_release"]
    /\ UNCHANGED <<lock_owner, lock_ttl, next_token, cont_token, bad_free>>

\* -----------------------------------------------------------------------
\* Release: the LOCK_RELEASE Lua compare-and-delete. GET the stored owner; DEL
\* the key ONLY IF it still equals THIS contender's token. A contender may call
\* release even while STALE (its lease expired and another contender re-acquired):
\* the token mismatch then makes the DEL a NO-OP on the new holder's lock — the
\* owner-fencing guarantee. Either way the contender leaves its critical section
\* (the guard is dropped / returns).
\*
\* The fix anchored by the negative test is the `cont_token[c] = lock_owner`
\* guard on the actual free: dropping it (unconditional DEL) lets a stale
\* releaser free the new holder's lock.
\* -----------------------------------------------------------------------
Release(c) ==
    /\ cont_phase[c] = "pending_release"
    /\ IF cont_token[c] = lock_owner       \* compare-and-delete on the owner token
       THEN /\ lock_owner' = NOBODY        \* token matches -> DEL the key (free it)
            /\ lock_ttl' = 0
       ELSE /\ UNCHANGED <<lock_owner, lock_ttl>>   \* stale: no-op, new holder kept
    /\ cont_phase' = [cont_phase EXCEPT ![c] = "idle"]
    /\ cont_token' = [cont_token EXCEPT ![c] = 0]   \* guard dropped
    \* WITNESS (independent of the guard above): record if this release FREED a
    \* lock whose CURRENT owner token was someone else's — a non-owner displacing
    \* the holder. Computed from ground truth (the key went free though our token
    \* didn't own it), so the blind-delete bug trips it while the correct
    \* compare-and-delete leaves lock_owner untouched and keeps bad_free FALSE.
    /\ bad_free' =
         (bad_free \/ (lock_owner # NOBODY
                        /\ cont_token[c] # lock_owner
                        /\ lock_owner' = NOBODY))
    /\ UNCHANGED next_token

\* -----------------------------------------------------------------------
\* TtlExpire: the PX lease elapses. A tick decrements the remaining TTL; when it
\* hits 0 the key auto-expires and the lock becomes free (lock_owner = NOBODY) —
\* exactly Redis dropping the key, or the in-memory `is_expired()` eviction. The
\* lease only counts down once NO contender is still inside its critical section
\* (faithful to "the TTL is longer than the critical section"): the lease reclaims
\* a holder that finished/crashed in pending_release without releasing. Such a
\* holder becomes STALE — its cont_token no longer matches the now-free, possibly
\* re-acquired lock — yet may still call release. This is the stale-releaser race
\* the owner-token check must fence.
\* -----------------------------------------------------------------------
TtlExpire ==
    /\ lock_ttl > 0
    /\ \A c \in Contenders : cont_phase[c] # "incs"   \* lease > critical section
    /\ lock_ttl' = lock_ttl - 1
    /\ IF lock_ttl - 1 = 0
       THEN lock_owner' = NOBODY           \* key auto-expired -> free
       ELSE UNCHANGED lock_owner
    /\ UNCHANGED <<next_token, cont_phase, cont_token, bad_free>>

\* -----------------------------------------------------------------------
\* Recycle: once the token counter saturates and no contender is mid-critical-
\* section and the key is free, reset the monotonic token so the system CYCLES
\* (no benign terminal deadlock under -deadlock). Models the lock name being
\* reused indefinitely; the fresh-token invariant within one sweep already
\* exercised every race.
\* -----------------------------------------------------------------------
Recycle ==
    /\ next_token > MaxToken
    /\ lock_owner = NOBODY
    /\ \A c \in Contenders : cont_phase[c] = "idle"
    /\ next_token' = 1
    /\ cont_token' = [c \in Contenders |-> 0]
    /\ UNCHANGED <<lock_owner, lock_ttl, cont_phase, bad_free>>

Next ==
    \/ \E c \in Contenders : TryAcquire(c) \/ LeaveCS(c) \/ Release(c)
    \/ TtlExpire
    \/ Recycle

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* GROUND-TRUTH ORACLE: the set of contenders actually in their critical section,
\* derived purely from cont_phase — INDEPENDENT of lock_owner / cont_token, the
\* bookkeeping the implementation reads and mutates. Stating MutualExclusion
\* against this set (not against "the lock has one owner") means a buggy release
\* that displaces a live holder is CAUGHT, not self-masked by the invariant
\* referencing the same compare-and-delete the implementation uses.
HoldersInCS == { c \in Contenders : cont_phase[c] = "incs" }

\* MUTUAL EXCLUSION: at most one contender is in its critical section at any
\* instant. This is the core lock contract every other spec assumes.
MutualExclusion == Cardinality(HoldersInCS) <= 1

\* A contender in its critical section holds the token currently stored at the
\* key: it has not been silently displaced by another contender's release. While
\* a contender is in its critical section the lease is live and has NOT been
\* re-acquired by anyone else, so its token must equal lock_owner. (After it
\* leaves CS into pending_release the lease may lapse and the lock be re-acquired,
\* making it stale — that case is handled by the owner-token check on release.)
HolderMatchesLock ==
    \A c \in Contenders :
        (cont_phase[c] = "incs") => (cont_token[c] = lock_owner)

\* OWNER-FENCED RELEASE: a Release only frees the lock if the releaser's token
\* equals the lock's CURRENT owner token — so a stale contender (lease expired,
\* lock re-acquired by someone else) NEVER frees the new holder's lock. The
\* bad_free witness is set TRUE the instant ANY release frees a lock owned by a
\* different token; under the owner-token compare-and-delete that is impossible,
\* so it stays FALSE. The witness is INDEPENDENT ground truth (it is computed from
\* "the key went free though our token didn't own it", not from the guard the impl
\* uses), so a buggy blind-delete trips it rather than self-masking it.
OwnerFencedRelease == bad_free = FALSE

============================================================================
