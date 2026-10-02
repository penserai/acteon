# Neural observability simulation results

Laya `typed-decisions` ran on `cpu` at revision `55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851`. Governance lock `sha256:2b074d7100d42d8443af8a1dc0c08ac6e42cb440725359d7b7e3380795fd87ba` approved six runtime packages, five checkpoint artifacts, and four question sets before inference. Kafka supplied 12 accepted source records across 4 event-time windows and the correlator rejected 6 duplicates or recovery redeliveries. The runner restored checkpoint generation 1 after an injected pre-commit crash. All 16 neural calls were real HTTP inference requests; Acteon used in-memory state and recording providers for controlled side effects.

| Trial | Signal decisions | Raw fusion | Acteon outcome | Laya inference | Result |
|---|---|---|---|---:|---|
| healthy-baseline | metrics: healthy (0.52)<br>traces: healthy (0.40)<br>logs: healthy (0.41) | downstream_timeout (0.27) | suppressed by suppress-observability-noise | 10621 ms | PASS |
| log-only-noise | metrics: healthy (0.54)<br>traces: healthy (0.45)<br>logs: healthy (0.35) | downstream_timeout (0.28) | suppressed by suppress-observability-noise | 11133 ms | PASS |
| pool-exhaustion | metrics: db_pool_pressure (0.52)<br>traces: database_wait (0.48)<br>logs: pool_timeout (0.58) | db_pool_exhaustion (0.27) | completed observability-incident chain | 11525 ms | PASS |
| ambiguous-regression | metrics: application_errors (0.39)<br>traces: application_work (0.31)<br>logs: downstream_error (0.31) | downstream_timeout (0.25) | rerouted to investigator | 11225 ms | PASS |

## Aggregate

- Governed runtime packages / model artifacts / question sets: **6 / 5 / 4**
- Kafka source records accepted: **12**
- Kafka duplicates and redeliveries rejected: **6**
- Recovery redeliveries deduplicated after restart: **5**
- Atomic checkpoint generations written: **2**
- Final Kafka consumer lag: **0**
- Event-time windows completed: **4**
- Model calls: **16**
- Total model inference: **44504 ms**
- Per-call p50 / p95: **1403 ms / 7634 ms**
- Incident chains: **1** diagnostics capture and **1** on-call notification
- Bounded investigations: **1**
- Duplicate incident dispatches prevented by the runner ledger: **1**

## Interpretation

Laya separated the first-stage signals, including the log-only noise case. Its low-confidence raw fusion choice still selected a non-healthy incident for healthy inputs. The deterministic corroboration gate prevented those raw false positives from reaching a provider. This is the intended safety property: neural decisions contribute bounded evidence, while Acteon policy controls side effects. These fixture results are integration evidence, not a detector-quality benchmark or a calibration claim.
