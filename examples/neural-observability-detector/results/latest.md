# Neural observability simulation results

Measured on 2026-10-01 with Laya 0.3.23, the `typed-decisions` checkpoint at
revision `55cf4c4ebb4ebe31b2550e8bdf3bd21b99753851`, four CPU threads, and the
four committed fixtures. The Docker image was 1.57 GB on arm64; model weights
were held in the external cache volume.

All **16 model calls were real requests** to the local Laya
`POST /v1/systemone` endpoint. Acteon's in-process verification then exercised
the same admitted policy bands with memory state and recording providers.

| Trial | Metrics | Traces | Logs | Raw Laya fusion | Admitted Acteon path | Laya inference | Result |
|---|---|---|---|---|---|---:|---|
| Healthy baseline | `healthy` (0.63) | `healthy` (0.42) | `healthy` (0.55) | `db_pool_exhaustion` (0.32) | suppressed | 16,256 ms | PASS |
| Log-only noise | `healthy` (0.63) | `healthy` (0.45) | `healthy` (0.41) | `db_pool_exhaustion` (0.31) | suppressed | 10,918 ms | PASS |
| Pool exhaustion | `db_pool_pressure` (0.45) | `database_wait` (0.57) | `pool_timeout` (0.60) | `db_pool_exhaustion` (0.39) | completed incident chain | 10,663 ms | PASS |
| Ambiguous regression | `application_errors` (0.33) | `application_work` (0.31) | `downstream_error` (0.29; source marked missing) | `db_pool_exhaustion` (0.29) | bounded investigator | 11,082 ms | PASS |

## Aggregate

| Measure | Result |
|---|---:|
| Expected policy outcomes | 4 / 4 |
| Real Laya calls | 16 |
| Total Laya inference | 48,919 ms |
| Per-call p50 | 1,324 ms |
| Per-call p95 | 12,650 ms |
| Diagnostics captures | 1 |
| On-call notifications | 1 |
| Investigator calls | 1 |
| Duplicate incident dispatches | 0 |

The first-stage decisions distinguished healthy, noisy, exhausted, and
conflicting signal windows. The raw fusion head still selected
`db_pool_exhaustion` for every fixture, with low answer confidence. The
corroboration policy admitted that incident only when metrics, traces, and logs
all returned the required typed conditions. This prevented two raw fusion false
positives from reaching any provider.

Laya logged that some shipped temperature entries for this question shape were
outside its accepted calibration range. The confidence values above are
therefore observations, not calibrated probabilities. The result supports the
architecture—bounded neural evidence followed by deterministic policy—without
claiming production detector quality.

The Acteon verification passed:

```text
running 2 tests
test tests::policy_routes_measured_live_laya_decisions ... ok
test tests::acteon_executes_the_three_policy_paths_once ... ok

test result: ok. 2 passed; 0 failed
```
