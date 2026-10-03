# Neural observability simulation results

Laya `typed-decisions` ran on `cpu` at revision `55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851`. Governance lock `sha256:38eb82188ec4f7c9e635c5f00e1243d537a557983348ba9c28e866c81393bb69` approved six runtime packages, five checkpoint artifacts, four question sets, and a response schema. Kafka supplied 12 accepted source records across 4 event-time windows and the pipeline rejected 6 duplicates or recovery redeliveries. The runner restored checkpoint generation 1 after an injected pre-commit crash. All 16 neural calls went through governed providers and real HTTP inference. Redis persisted checkpoints, delivery state, dispatch receipts, and chain state. One gateway applied admission and routing; recording providers supplied controlled side effects.

| Trial | Signal decisions | Raw fusion | Acteon outcome | Model HTTP elapsed | Result |
|---|---|---|---|---:|---|
| healthy-baseline | metrics: healthy (0.52)<br>traces: healthy (0.40)<br>logs: healthy (0.41) | downstream_timeout (0.27) | suppressed by suppress-observability-noise | 13633 ms | PASS |
| log-only-noise | metrics: healthy (0.54)<br>traces: healthy (0.45)<br>logs: healthy (0.35) | downstream_timeout (0.28) | suppressed by suppress-observability-noise | 14623 ms | PASS |
| pool-exhaustion | metrics: db_pool_pressure (0.52)<br>traces: database_wait (0.48)<br>logs: pool_timeout (0.58) | db_pool_exhaustion (0.27) | completed observability-incident chain | 15642 ms | PASS |
| ambiguous-regression | metrics: application_errors (0.39)<br>traces: application_work (0.31)<br>logs: downstream_error (0.31) | downstream_timeout (0.25) | rerouted to investigator | 14698 ms | PASS |

## Aggregate

- Governed runtime packages / model artifacts / locked contracts: **6 / 5 / 5**
- Kafka source records accepted: **12**
- Kafka duplicates and redeliveries rejected: **6**
- Recovery redeliveries deduplicated after restart: **5**
- Window checkpoints persisted before offset commits: **2**
- Final Kafka consumer lag: **0**
- Event-time windows completed: **4**
- Model calls: **16**
- Total model HTTP elapsed: **58596 ms**
- Per-call p50 / p95: **3496 ms / 7585 ms**
- Incident chains: **1** diagnostics capture and **1** on-call notification
- Bounded investigations: **1**
- Duplicate incident dispatches prevented by durable receipts: **1**

## Live Kafka acknowledgements

| Observation | Result |
|---|---:|
| Active source sessions | 3 |
| Source receipts acknowledged after checkpoint | 13 |
| Stale acknowledgements rejected after rebalance | 1 |
| Replacement delivery offset in fencing probe | 0 |

Source acknowledgements use the consumers that delivered the records. An independent group probe joins a second member, observes revocation, rejects the old receipt, and receives the uncommitted prefix again. It adds no model calls or operational effects.

## Managed delivery recovery

| Observation | Result |
|---|---:|
| State backend | redis |
| Cached decisions recovered | 4 |
| Delivery attempts | 6 |
| Accepted verdicts | 4 |
| Persisted retries | 1 |
| Replaced delivery workers / receiver gateways | 1 / 1 |
| Redeliveries deduplicated by Acteon | 1 |
| Invalid verdicts retained for inspection | 1 |
| Pending window / verdict outputs | 0 / 0 |
| Model calls repeated during retry | 0 |

Dead-letter diagnostic: `invalid verdict envelope or idempotency key`. The probe is inspected and explicitly discarded after measurement.

Timings measure model HTTP request and response validation through the governed provider. They are not the server-only inference timings used in the previous report. Full wall times also include gateway dispatch and runtime identity checks.

Receiver replacement preserved incident Action `b4695d22-268c-443f-8386-93a5f72c6ae0` and chain execution `41f685fc-b21b-4bae-bbb5-11cedfa99d78`; original and recovered receipt outcomes matched.

## Interpretation

The retry fault occurs after the receiver completes dispatch. Both the delivery worker and receiver gateway are replaced; the retry recovers the original durable dispatch receipt and chain identity. The notification chain step re-enters gateway rules. Generic gateway tests additionally exercise interruption before chain creation and after chain completion but before receipt completion. Interrupted external provider calls remain explicitly subject to reconciliation; receipt fencing cannot undo their effects.

Laya separated the first-stage signals, including the log-only noise case. Its low-confidence raw fusion choice still selected a non-healthy incident for healthy inputs. The deterministic corroboration gate prevented those raw false positives from reaching a provider. This is the intended safety property: neural decisions contribute bounded evidence, while Acteon policy controls side effects. These fixture results are integration evidence, not a detector-quality benchmark or a calibration claim.
