------------------------- MODULE EmbeddingSingleFlight -------------------------
(*
 * TLA+ specification of Acteon's embedding-cache single-flight coalescing
 * (thundering-herd protection).
 *
 * Models crates/embedding/src/cache.rs :: EmbeddingCache::get (~59):
 *
 *     // advisory metrics probe — racy with try_get_with, NOT the gate (~62)
 *     if let Some(val) = self.cache.get(text).await { record_hit; return Ok(val) }
 *     record_miss;
 *     // THE single-flight gate (~70): moka entry-level coalescing
 *     self.cache
 *         .try_get_with(key, async move { provider.embed(text).await })
 *         .await
 *
 * moka's try_get_with provides ENTRY-LEVEL single-flight: for the SAME key, only
 * ONE concurrent init future (here `provider.embed(text)`) runs; every other
 * concurrent caller for that key WAITS for and SHARES that single result. The
 * provider call — the expensive embedding computation — is therefore made at most
 * ONCE per key per population window, no matter how many callers stampede in
 * concurrently. The bare `cache.get(text)` above it is an ADVISORY metrics probe
 * (the docstring at ~57 says hit/miss counters are "approximate under high
 * concurrency"); it does NOT gate the provider call and is NOT load-bearing for
 * single-flight.
 *
 * Protocol. N concurrent callers each request get(text) for the SAME initially-
 * uncached text. The cache key is in one of three states:
 *     absent     — not populated; no computation in flight
 *     computing  — single-flight in progress, OWNED by the first caller to miss
 *     cached(v)  — populated with the single committed value v
 * Each caller:
 *   Probe(c):  runs the advisory cache.get probe. If it observes a live cached
 *              entry it short-circuits and returns that value (the record_hit fast
 *              path); otherwise it falls through to the try_get_with gate. The
 *              probe does NOT gate the provider call (a probe that saw "absent" can
 *              still reach the gate after the owner has populated the entry — that
 *              is the Hit action below).
 *   Claim(c):  try_get_with on an ABSENT key — this caller is the FIRST writer:
 *              atomically claims the entry (absent -> computing), becomes the
 *              single-flight OWNER, and begins the ONE provider.embed() call.
 *   Wait(c):   try_get_with on a COMPUTING key — a concurrent caller observes the
 *              in-flight entry and WAITS (it does NOT start its own provider call).
 *   Hit(c):    try_get_with (or the probe) on a CACHED key — returns the shared
 *              committed value with NO provider call.
 *   Commit:    the owner's provider.embed() completes; the key becomes cached(v)
 *              with the single computed value; every waiter (and the owner) then
 *              reads that SAME v.
 * A TTL clock expires the cached entry (moka time_to_live): cached -> absent,
 * re-opening a FRESH single-flight window in which exactly one new provider call
 * is allowed. So the system CYCLES (no benign terminal deadlock under -deadlock).
 *
 * The single load-bearing mechanism is the ATOMIC entry-level claim (Claim:
 * absent -> computing). It is modeled as a first-writer-wins claim (like the
 * check_and_set claim in RecurringDispatch.tla): the precondition key_state =
 * "absent" can hold for only one caller at a time, so only one claim succeeds per
 * window. Wait is the modeled coalescing PATH a concurrent caller takes while the
 * owner computes; it is not independently load-bearing for the SingleFlight bound
 * (a caller that does not Wait simply Hits the cached entry after Commit) — the
 * atomic claim alone bounds the provider to one call per window.
 *
 * Verified (over every interleaving of concurrent callers and TTL expiry):
 *   - SingleFlight: the provider is called at most once per single-flight window
 *     per key — provider_calls <= 1 while the key is absent-or-computing (the
 *     thundering-herd bound). After a TTL expiry a fresh window allows one fresh
 *     call; the counter resets on expiry.
 *   - ConsistentValue: every caller that returned a value returned the SAME
 *     committed value — no caller fabricates or returns a different value than the
 *     one single committed computation (a returned NoVal sentinel means "has not
 *     returned a value yet" and is excluded).
 *   - ComputingHasOwner: while the key is computing there is exactly one owner
 *     (the claim's first-writer-wins shape); a sanity guard on the claim.
 *
 * Negative check: revert the atomic claim — widen Claim's precondition from
 * key_state = "absent" to key_state \in {"absent","computing"} so a concurrent
 * caller that arrives while a computation is in flight starts its OWN provider
 * call instead of coalescing. Two concurrent callers then both observe a miss and
 * BOTH call the provider -> provider_calls reaches 2 -> SingleFlight violated (the
 * thundering herd). This isolates the entry-level claim as THE load-bearing
 * mechanism: reverting it alone breaks the bound.
 *
 * SCOPE. moka's internal eviction/admission machinery and the exact TTL timer are
 * ABSTRACTED to a cached -> absent expiry action; the advisory metric probe is
 * modeled but is explicitly NOT the gate. This spec verifies the single-flight
 * COALESCING (at-most-one provider call per key per population window) and the
 * value CONSISTENCY of the shared result. The cache is single-key (the moka
 * coalescing is per-entry, so one key is sufficient to model the property).
 *
 * Run with:
 *   java -jar tla2tools.jar -config EmbeddingSingleFlight.cfg EmbeddingSingleFlight.tla
 *)
EXTENDS Integers, FiniteSets, TLC

CONSTANTS
    Callers,   \* concurrent get(text) callers for the SAME key, e.g. {c1, c2}
    Values,    \* candidate embedding values the provider might return, e.g. {v1, v2}
    TTL,       \* cached-entry TTL in ticks (moka time_to_live, abstracted)
    MaxTime,   \* state-space bound on the clock
    NOVAL,     \* sentinel: caller has not returned a value yet
    NOBODY     \* sentinel: no single-flight owner (key not computing)

\* Operators (NOT .cfg function literals) keep the .cfg clean.
ValuesOrNoVal == Values \cup {NOVAL}

VARIABLES
    key_state,         \* "absent" | "computing" | "cached"
    cached_value,      \* Values \cup {NOVAL}: the single committed value (NOVAL when absent/computing)
    owner,             \* Callers \cup {NOBODY}: single-flight owner while computing
    provider_calls,    \* 0..3: provider.embed() calls in the CURRENT window
    cell_ttl,          \* 0..TTL: remaining life of the cached entry
    c_phase,           \* [Callers -> phase] per-caller progress
    c_ret,             \* [Callers -> ValuesOrNoVal]: value this caller returned
    clock

Phases == {"idle", "probed", "waiting", "returned"}

vars == <<key_state, cached_value, owner, provider_calls, cell_ttl,
          c_phase, c_ret, clock>>

TypeOK ==
    /\ key_state \in {"absent", "computing", "cached"}
    /\ cached_value \in ValuesOrNoVal
    /\ owner \in Callers \cup {NOBODY}
    /\ provider_calls \in 0..3
    /\ cell_ttl \in 0..TTL
    /\ c_phase \in [Callers -> Phases]
    /\ c_ret \in [Callers -> ValuesOrNoVal]
    /\ clock \in 0..MaxTime

Init ==
    /\ key_state = "absent"
    /\ cached_value = NOVAL
    /\ owner = NOBODY
    /\ provider_calls = 0
    /\ cell_ttl = 0
    /\ c_phase = [c \in Callers |-> "idle"]
    /\ c_ret = [c \in Callers |-> NOVAL]
    /\ clock = 0

\* ---------------------------------------------------------------------------
\* The advisory metrics probe (cache.get at ~62): a bare read that does NOT gate
\* the provider call. If it observes a live cached entry it short-circuits and
\* returns that value (the record_hit fast path); otherwise it falls through to the
\* try_get_with gate. A probe that saw "absent" still reaches the gate, where the
\* entry may already be cached (the Hit action) — so the probe is not the gate.
\* ---------------------------------------------------------------------------
Probe(c) ==
    /\ c_phase[c] = "idle"
    /\ IF key_state = "cached"
       THEN \* probe hit: return the shared cached value, no provider call
            /\ c_ret' = [c_ret EXCEPT ![c] = cached_value]
            /\ c_phase' = [c_phase EXCEPT ![c] = "returned"]
       ELSE \* probe miss: fall through to the try_get_with gate
            /\ c_phase' = [c_phase EXCEPT ![c] = "probed"]
            /\ UNCHANGED c_ret
    /\ UNCHANGED <<key_state, cached_value, owner, provider_calls, cell_ttl, clock>>

\* ---------------------------------------------------------------------------
\* try_get_with on an ABSENT key: this caller is the FIRST writer. It atomically
\* CLAIMS the entry (absent -> computing), becomes the single-flight owner, and
\* begins the ONE provider.embed() call (provider_calls += 1). This is the load-
\* bearing atomic step — modeled first-writer-wins: the precondition key_state =
\* "absent" can hold for exactly one caller at a time, so only one claim succeeds
\* per window.
\* ---------------------------------------------------------------------------
Claim(c) ==
    /\ c_phase[c] = "probed"
    /\ key_state = "absent"
    /\ provider_calls < 3            \* bound the counter for CI; violation surfaces at 2
    /\ key_state' = "computing"
    /\ owner' = c
    /\ provider_calls' = provider_calls + 1
    /\ c_phase' = [c_phase EXCEPT ![c] = "waiting"]
    /\ UNCHANGED <<cached_value, cell_ttl, c_ret, clock>>

\* try_get_with on a COMPUTING key: a concurrent caller observes the in-flight
\* entry and WAITS — it does NOT start its own provider call. moka coalesces it
\* onto the owner's single computation.
Wait(c) ==
    /\ c_phase[c] = "probed"
    /\ key_state = "computing"
    /\ c_phase' = [c_phase EXCEPT ![c] = "waiting"]
    /\ UNCHANGED <<key_state, cached_value, owner, provider_calls, cell_ttl,
                   c_ret, clock>>

\* The owner's provider.embed() completes: the entry becomes cached(v) with the
\* single computed value v. (Any value in Values may be returned; whichever it is,
\* it becomes THE committed value all callers share.)
Commit(v) ==
    /\ key_state = "computing"
    /\ key_state' = "cached"
    /\ cached_value' = v
    /\ cell_ttl' = TTL
    /\ owner' = NOBODY
    /\ UNCHANGED <<provider_calls, c_phase, c_ret, clock>>

\* A waiting caller (owner or coalesced waiter) reads the committed value once the
\* entry is cached — every waiter shares the SAME cached_value.
TakeCached(c) ==
    /\ c_phase[c] = "waiting"
    /\ key_state = "cached"
    /\ c_ret' = [c_ret EXCEPT ![c] = cached_value]
    /\ c_phase' = [c_phase EXCEPT ![c] = "returned"]
    /\ UNCHANGED <<key_state, cached_value, owner, provider_calls, cell_ttl, clock>>

\* try_get_with on an already-CACHED key (the caller's probe missed the entry but
\* it was populated by the time it reached the gate): returns the shared value with
\* no provider call.
Hit(c) ==
    /\ c_phase[c] = "probed"
    /\ key_state = "cached"
    /\ c_ret' = [c_ret EXCEPT ![c] = cached_value]
    /\ c_phase' = [c_phase EXCEPT ![c] = "returned"]
    /\ UNCHANGED <<key_state, cached_value, owner, provider_calls, cell_ttl, clock>>

\* A caller that returned re-enters to request the same key again (retries /
\* repeated gets), so the system CYCLES.
ReturnToIdle(c) ==
    /\ c_phase[c] = "returned"
    /\ c_phase' = [c_phase EXCEPT ![c] = "idle"]
    /\ c_ret' = [c_ret EXCEPT ![c] = NOVAL]
    /\ UNCHANGED <<key_state, cached_value, owner, provider_calls, cell_ttl, clock>>

\* moka time_to_live: the cached entry expires (cached -> absent), re-opening a
\* FRESH single-flight window. The provider-call counter resets so the next
\* window's single call is bounded independently. Fires at a clean window boundary
\* (every caller back to idle, none mid-flight holding a value from this window),
\* so the next window's callers re-request against a freshly-absent key.
Expire ==
    /\ key_state = "cached"
    /\ cell_ttl = 0
    /\ \A c \in Callers : c_phase[c] = "idle"
    /\ key_state' = "absent"
    /\ cached_value' = NOVAL
    /\ provider_calls' = 0
    /\ UNCHANGED <<owner, cell_ttl, c_phase, c_ret, clock>>

\* Time advances and the cached entry's TTL decays toward expiry.
ClockTick ==
    /\ clock < MaxTime
    /\ clock' = clock + 1
    /\ cell_ttl' = IF cell_ttl > 0 THEN cell_ttl - 1 ELSE 0
    /\ UNCHANGED <<key_state, cached_value, owner, provider_calls,
                   c_phase, c_ret>>

Next ==
    \/ \E c \in Callers :
        \/ Probe(c)
        \/ Claim(c)
        \/ Wait(c)
        \/ TakeCached(c)
        \/ Hit(c)
        \/ ReturnToIdle(c)
    \/ \E v \in Values : Commit(v)
    \/ Expire
    \/ ClockTick

Spec == Init /\ [][Next]_vars

\* =======================================================================
\* SAFETY
\* =======================================================================

\* THE KEY INVARIANT: the thundering-herd bound. While the key is being populated
\* (absent or computing) the provider has been called at most once this window.
\* Guaranteed by moka's atomic entry-level claim (absent -> computing), not by the
\* advisory probe. After a TTL expiry the counter resets and a fresh window allows
\* one new call — correct, not a violation.
SingleFlight == provider_calls <= 1

\* Every caller that has returned a value returned the SINGLE committed value: no
\* caller fabricates or returns a value other than the one computed by the single
\* coalesced provider call. (NOVAL means "not yet returned" and is excluded.) While
\* computing, cached_value is NOVAL; the only non-NOVAL value any caller can read
\* is the one Commit wrote.
ConsistentValue ==
    \A c \in Callers : (c_ret[c] # NOVAL) => (c_ret[c] = cached_value)

\* While the key is computing there is exactly one single-flight owner (the first-
\* writer-wins claim shape). A sanity guard: a window with no owner or two owners
\* would mean the atomic claim was not load-bearing.
ComputingHasOwner ==
    (key_state = "computing") => (owner \in Callers)

============================================================================
