# Rehabilitation and verification

Baseline: `f0e08a14a6d2b684c12df5f06b40c3d95451d2d2`.
Implementation branch: `codex/rehabilitation`.
Reviewed and implemented on 2026-09-04.

The two supplied September assessments were checked against source and executable
reproductions before implementation. This record describes concrete changes and
measured checks; it does not assign a projected safety score.

The follow-up from merged revision `ea59463` adds repeated product-workflow
evaluations. See [the evaluation record and remaining plan](scenario-evaluation.md)
for its scope, grading contract, and the work that remains. Follow-ups add
[clock injection and deadline safety evaluation](virtual-time.md), then
[worker ticks and task lifecycle evaluation](worker-lifecycle.md).
Later phases add [durable scheduling](durable-scheduling.md) and
[worker queue recovery](queue-recovery.md), followed by
[terminal worker-result handoffs](task-handoff-recovery.md) and
[chain state/retention fencing](chain-state-fencing.md).

## Implemented behavior

| Area | Result | Main implementation |
| --- | --- | --- |
| Evaluation integrity | Missing, ambiguous, nonfinite, out-of-range, truncated, and unsuccessful process results fail acceptance. JSON reports require nonempty unique checks. Hard gates override aggregate scores. | `crates/swarm/src/orchestrator/eval.rs` |
| Generated evaluation | Typed executable/argument plans replace model-authored shell generation. Literal hostile strings cannot execute through quoting. Generic regression checks cannot certify challenge IDs. | `crates/swarm/src/orchestrator/eval_gen.rs` |
| Recovery | Independent challenge-specific checks are required. Recovery uses detached candidates containing dirty/untracked work, preserves the original index and stashes, and refuses promotion after concurrent original edits. Post-evaluation failures are persisted. | `crates/swarm/src/orchestrator/{adversarial,workspace,engine}.rs` |
| Process lifecycle | Managed agent/evaluator output is bounded. Timeout, cancellation, and completion clean up owned Unix process groups. | `crates/swarm/src/orchestrator/process.rs` |
| Policy contracts | Generated tenant-scoped YAML passes the actual parser. Startup installs and verifies rules, checks the notification provider, and probes effective suppression. Hooks cover every tool, require their binary, preserve operator settings, and decode actual wire outcomes. | `crates/swarm/src/acteon/rules.rs`, `hooks/gate.rs`, `orchestrator/agent_spawner.rs` |
| Confidentiality | DLQ encryption errors never write plaintext. Decryption errors retain ciphertext and prevent redelivery. Persistence failures are explicit and counted by `acteon_dlq_failures_total`. | `crates/gateway/src/encrypting_dlq.rs`, `crates/executor/src/dlq.rs` |
| Outbound requests | Webhook and A2A clients validate URLs and the DNS answers consumed by the connector, disable proxy bypass, and refuse uncontrolled redirects. Explicit internal-host exceptions remain available for operator-configured webhooks. | `crates/http`, webhook providers, A2A push worker |
| Deployment | Nonlocal unauthenticated binds require an explicit development acknowledgement. CORS defaults to same-origin. Compose mounts real Redis/rules configuration and publishes on loopback. | server configuration, `docker-compose.yml`, `deploy/acteon.compose.toml` |
| Backends | Simulation selects real Redis/PostgreSQL state and locks, reports backend identity, and fails on missing services. Memory isolation is honored. Cluster approval keys are shared. Redis honors set/CAS counter values; memory/DynamoDB overflow is rejected before mutation. | simulation harness, state implementations and shared conformance |
| SDK contracts | Python, Node, Go, and Java share a Rust wire fixture for flattened action metadata. Python typing and UTC serialization are repaired; Node clean installs and package contents are checked. | `clients/contract-fixtures`, four clients |
| Reproducible scenarios | Versioned manifests, seeded failure choices, independent invariants, semantic causal traces, JSON/JUnit artifacts, and replay run across memory, Redis, and PostgreSQL. | `crates/simulation/src/scenario.rs`, `scenarios`, `scripts/ci/scenarios.sh` |
| Release checks | Locked stable/MSRV all-target builds, client runtime matrices, browser smoke, real backend contracts, scenarios, RustSec/npm/secret scanning, license/source policy, immutable action references, dependency inventory, and Dependabot are wired into CI. | `.github/workflows`, `scripts/ci/security.sh` |

The final review additionally found the Redis counter representation bug, memory
counter overflow, and DynamoDB's incompatibility between set values and counters.
These are covered by the shared conformance contract, including preservation of
the previous value when an increment fails. DynamoDB now uses bounded conditional
retries and consistent point reads, trading extra reads for coherent string/numeric
counter behavior and overflow protection.

## Verification record

| Check | Local result |
| --- | --- |
| `cargo test --locked --workspace --lib --bins --tests` | 3,068 passed across 61 executables |
| `cargo test --locked --workspace --doc` | 73 passed; 3 explicitly ignored examples |
| Stable and Rust 1.88 workspace all-target compilation | Passed |
| Workspace Clippy with warnings denied (Rust 1.98.1 and Rust 1.88) | Passed |
| Swarm/simulation library, binary, and test Clippy with Redis/PostgreSQL/scenario features | Passed |
| Swarm/simulation feature-enabled tests | 182 passed, including 82 swarm tests |
| Memory/Redis/PostgreSQL state and lock suites | 25 passed, including shared conformance against live disposable services |
| DynamoDB state and lock suite | 8 passed against DynamoDB Local 3.3.0, including concurrent counter updates |
| AWS / Azure / GCP `full` provider tests | 90 / 41 / 48 passed |
| Scenario runner and replay | 23 invariants passed on each of memory, Redis, PostgreSQL; all three replays matched |
| RustSec gate | Passed with the single verified lockfile-only RSA exception; five warnings remain visible |
| `cargo deny --locked check licenses sources` | Passed with all features and explicit license/source policy |
| Gitleaks source scan | Passed with narrow verified documentation exceptions; a synthetic credential was rejected |
| Formatting, diff whitespace, workflow YAML, shell syntax, Compose configuration | Passed |

A broader exploratory Clippy run over simulation examples and benchmarks reports
pre-existing pedantic lint findings (such as long demonstration functions and raw
string formatting). Those targets compile under the all-target gate; their lint
debt is not included in the passing library/binary/test Clippy result.

The SDK checks completed locally: Python has 202 passing tests plus 13 subtests,
clean Ruff/strict MyPy, and an independently installed wheel containing `py.typed`.
Node has 174 passing tests, clean installation/lint/type/build, and a packed-package
installation/import check. Go passed race-enabled tests and vet. Java passed test
and build. UI lint/build and browser navigation smoke passed: 21 desktop/mobile
tests, one explicitly skipped mobile collapse-button case. UI lint retains one
existing TanStack/React Compiler compatibility warning. Node and UI npm audits
report zero vulnerabilities at this check date.

Local tools include Rust 1.98.1, 1.95, and 1.88, Python 3.11, Node 25, and Java 21.
The Python 3.11–3.14 and Node 20/22/24 matrices also passed in GitHub Actions.
CI follows the current stable Rust release, so its Clippy version must be checked
when reproducing lint failures locally. Gitleaks is installed from its pinned Go
module; the Rust installer handles only cargo-audit and cargo-deny.

## Compatibility and operation

- `DeadLetterSink::push` now returns `Result`; custom sinks and callers must handle
  persistence errors. Encryption is independent of storage durability: the default
  DLQ remains in memory.
- Webhook custom clients now use `acteon_http::GuardedClient`. Configure exact
  `internal_hosts` entries for intentional private/HTTP destinations. Metadata and
  link-local endpoints remain denied. Guarded builder customization is trusted TLS
  configuration; do not supply address overrides.
- Enable authentication for remote deployments. The
  `--allow-unauthenticated-remote` switch and matching server setting acknowledge a
  development configuration. Cross-origin UIs must list their origins explicitly.
- Swarm execution requires a shared `safety.rules_directory`, a healthy
  `safety.approval_notify_provider`, and an installed hook executable. Unknown tools
  are denied. Rules use a per-run tenant. Host-wide rules can still affect policy;
  startup verifies a suppression probe, not every possible combination of rules.
- Approval through the gateway does not automatically resume a blocked local CLI
  tool. An integration must explicitly coordinate that resume; it cannot infer
  permission from the original pending outcome.
- The old generated shell API is removed. Use `acteon-swarm eval generate` and
  `eval run`, or configure a trusted program plus arguments. Legacy operator shell
  commands remain supported with strict score parsing, but cannot certify a fix
  without a structured challenge-specific result.
- Wasmtime moves to the patched 36 LTS line with a reduced feature set; MCP and
  Azure SDK integrations are migrated to their updated APIs. Rust 1.88 remains the
  minimum. See [dependency decisions](security-dependencies.md) for the RSA
  lockfile-only exception and the visible maintenance/S3-cache warnings.

Hook matcher behavior follows the [Claude Code hook reference](https://code.claude.com/docs/en/hooks)
and [Gemini CLI hook reference](https://geminicli.com/docs/hooks/reference/).
The checked tests verify generated settings and the gate contract; no live model
subscription is used as a release grader.

## Limits of this evidence

Git worktrees and regex command policy are not an operating-system sandbox.
Operator graders and credentials must be outside agent-writable files, and agents
must run inside an appropriate filesystem/network execution boundary. Deliberately
detached subprocesses also require OS containment; Windows process cleanup does
not have the Unix process-group guarantee.

Kernel and product-portfolio replay compare logical evidence using real time.
The separate deadline suite virtualizes selected gateway/executor/memory paths;
remote database clocks, network schedules, and process scheduling remain real. The initial six
scenarios cover policy, approval, tenant deduplication, retries, evaluator integrity,
and invalid-state recovery. The follow-up adds selected incident, refund, and
prompt-injection workflows with diagnostic weighted scorecards. The full proposed
portfolio, model capability trials, broader crash/partition exploration, and
empirically calibrated scorecards remain separate work. Subsequent phases added
shared clocks, a bounded deterministic scheduler, worker lifecycle timing, and
[durable scheduling with deployment recovery](durable-scheduling.md).

The scenarios exercise production gateways directly with recording providers.
Browser smoke covers navigation/responsiveness; it does not validate every live
API-backed UI operation. Cloud SDK tests and local database contracts do not
certify behavior against every hosted service or production topology. Default
workspace tests may include service-dependent cases that return early without their
service environment; explicit backend runs are recorded separately.

Locks and completed-key deduplication do not establish exactly-once external effects
after an ambiguous timeout or a crash between provider execution and persistence.
Downstream idempotency and reconciliation remain required. See the
[scenario documentation](../scenarios/README.md) and [security policy](../SECURITY.md).
