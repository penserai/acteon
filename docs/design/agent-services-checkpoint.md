# Authenticated agent services implementation checkpoint

October 6, 2026. This branch preserves work in progress on configured agent
services following the durable agent task runtime released in PR #432.
It is not ready to merge or advertise as a complete A2A service.

Implemented so far:

- Configuration for pinned agent cards, fixed provider operations, private
  recipient credential environment references, and bounded delegation grants.
- Provider qualification and credential policy projection for declared
  `agent.invoke` ingress effects.
- Host admission using authenticated source and recipient proofs and durable
  governed task acceptance through the configured state backend.
- Deployment publication preflights every configured recipient key against its
  actual authenticated principal, current scoped policy, and fixed operation.
  Failure publishes no deployment permits or grants. Invocation repeats the
  check before source root allocation.
- Individual authenticated REST message admission with explicit route permission
  registration, a bounded request, and a fixed operator-selected runtime.
- Legacy tenant A2A read, append, cancel, events, and push configuration cannot
  access governed service tasks. The boundary reads durable acceptance rather
  than trusting display metadata.
- Requester-isolated observation authenticates the original source credential.
  Agent callers must also present the exact source context returned by admission.
  Observation repairs known completion receipts without starting provider work.
- Governed peer invocation can carry a host-owned parent execution-context
  reference with explicit permit revisions. Admission recovers sealed authority
  and revalidates the caller, current permits, registry, grant, and budgets. All
  five SDKs implement the same paired-header contract.
- A configurable server driver discovers durable acceptances from the configured
  state backend and resumes them through governed execution. It retains in-flight
  and uncertain receipts rather than resending them. Scheduling uses fair cursors
  and bounded concurrency; backend scans still collect whole scopes.

- Typed service errors preserve invalid input (400), denied authority (403),
  unavailable or foreign task identity (404), accepted-work conflicts (409),
  budget/capacity limits (429), and unavailable storage/runtime (503). Public
  errors carry stable codes and no private credential or backend details.
- Real-server Redis contracts cover queued recovery, known completion recovery,
  an HTTP request received with its response lost, and unreadable stored task
  projection. Uncertain delivery survives restart with one provider request and
  retained source/recipient capacity; observation and replay do not resend it.
  The contracts use isolated UUID prefixes and run in CI.

| HTTP status | Service error code | Meaning |
| --- | --- | --- |
| 400 | `invalid_agent_service_request` | Invalid message or unsupported continuation |
| 403 | `agent_service_authority_required` | Current source authority is required |
| 404 | `service_task_unavailable` | Service/task missing or inaccessible to this requester |
| 409 | `agent_service_conflict` | Changed accepted input or conflicting authority observation |
| 429 | `agent_service_limits_exhausted` | Capacity, concurrency, or budget exhausted |
| 503 | `agent_services_unavailable` | Storage, verification, or installed runtime unavailable |

Version negotiation and malformed source-context headers also return 400 with
specific codes. Admission acceptance remains distinct from provider execution.
Errors and task responses use `Cache-Control: no-store`. A 503 is not evidence
that earlier acceptance or an external effect did not occur; retain the same
message identity and inspect durable evidence rather than constructing new work.

- Native receipts and observation helpers exist in Rust, Python sync/async,
  TypeScript, Go, and Java. Each retains response provenance separately from task
  data and uses the original job identity; concurrent jobs do not share headers.
  Missing headers, HTTP failures, and redirects do not trigger a new request.
- The browser agent detail view can accept and observe governed tasks with its
  current identity. Context remains in local host state, outside the rendered
  task data. The configured CORS origins can read receipt/version headers.

- Original requesters can durably stop future starts for an accepted service
  task and its descendant budget subtree. `POST
  /a2a/{namespace}/{tenant}/agents/{agent}/v1/tasks/{id}/stop` uses the same
  private authentication and source-context requirements as observation. Its
  response contains the original task and `future_starts_blocked: true`.
  An optional typed `provider_abort` reports `restricted_only`, `uncertain`, or
  reconciliation-verified finality. The durable abort journal is written before
  one adapter call; crashes and lost responses are never resent automatically.
  This does not refund spent budget, release unresolved capacity, or replace a
  known result with a `cancelled` task. Lost restriction acknowledgements are
  recovered by stopping the same original task again. Recovery scheduling skips
  durably stopped work.

- Receipt-aware future-start stop helpers exist in all five SDKs, including
  Python sync/async. They preserve original route/task identity and source context,
  require a true restriction acknowledgement, and send one request without retry
  or redirect. The browser has an explicit per-job stop control; acknowledgement
  is separate from provider task state and survives local task refreshes.

- Configured service deployment now publishes a mandatory, independently
  approved registry qualification before service grants. `registry_revision`
  defaults to 1 and must increase for requalification. The revision contributes
  to the service binding digest; changing or retiring an epoch cannot be undone
  by restarting the same declaration. Retired or replaced qualifications deny
  new admission before source root allocation, while original requester history
  remains observable under the unchanged service binding. Coordinator checks
  continue to enforce qualified digests at delegated effect starts.

- A registry mutation primitive now stages an exact agent/card projection,
  its observed metadata version, and its input digest in the authority CAS.
  Staging retires the current qualification and blocks requalification and
  delegated starts until a known control-effect result completes the fence.
  Delivery uses create-if-absent, compare-and-swap, or compare-and-delete through
  the configured StateStore. In-flight and uncertain attempts never resend.
  Matching bytes or an absent row do not certify a lost write acknowledgement.
  A known version conflict records no effect and leaves the old epoch retired.
  Ordinary control-event acknowledgements cannot complete this mutation.
  The HTTP registry mutation routes now call the primitive with independently
  declared manager agent bounds. Governed scopes reject legacy metadata writes.
  Production-router contracts now cover exact agent bounds, foreign projection
  identities and oversized values before staging, known replay/version
  preservation, original actor attribution,
  write acknowledgement loss before and after persistence, qualification
  retirement on success and known version conflicts, and legacy card writes
  with absent runtime or unreadable authority. A bus-feature contract covers
  registration, update, deletion, and admin-state legacy write refusal. Approved
  peer discovery reads the actual card and verifies its approved digest; the
  separate presence hint cannot hide a valid card or authorize a missing or
  changed card, and current qualification remains mandatory when enrolled.
  HTTP mutations share the 64 KiB UTF-8 record bound with approved discovery
  and reject larger normalized projections before staging retirement. The
  lower-level mutation kernel retains its independent 256 KiB capacity bound.
  Boundary contracts cover both projection kinds with ASCII/multibyte content
  and exact-size acceptance; older larger records remain inspectable/deletable.
  All five SDKs now expose typed projection inspection and mutation helpers
  (Python sync/async), validate receipt identity/completion, and refuse redirects
  without automatic retries. Shared wire fixtures and the public operator
  workflow cover explicit recovery. The Governance UI now inspects, edits, and
  removes bounded projections through the same protocol. It locks an attempted
  request for exact manual replay, journals it across reload, prevents duplicate
  in-flight submission, rejects false completion, refuses redirects, and
  requires reinspection before a new intent. Desktop and mobile browser
  contracts cover response loss, reload recovery, malformed completion, exact
  replay, update, and removal. One shared backend lifecycle contract now covers
  creation, paused concurrent replacement, restart, exact replay without another
  write, removal/recreation, and requalification on memory, Redis, PostgreSQL, and DynamoDB.
  The three production backend variants use independent clients and isolated
  storage and run in CI. Broader peer/provider lifecycle tests remain pending.
  Unresolved delivery replays return HTTP 503 rather than an authority-denied 403. Clients
  must retain the original change identifier and request; the coordinator does
  not resend a registered write whose outcome is unknown.
- Authority protocol 12 requires explicit reviewed cutover from protocols 10
  and 11; startup does not upgrade automatically. Contracts preserve funded
  descendants, opaque contexts, known/uncertain execution receipts and qualified
  registry records. Hybrid legacy records containing new mutation intents fail
  closed rather than becoming authority during upgrade.

Remaining before release:

- Install concrete qualified provider abort adapters and qualify their restart
  recovery against each supported production backend.
- Expand focused adversarial and integration tests from admission/isolation to
  full provider execution and peer lifecycle.
- Complete qualified outbound A2A lifecycle work.
- Extend lifecycle SDK/UI integration as outbound peer contracts are added.
- Review adversarially, run CI, and verify publication before merging.

Passing existing workspace checks alone does not demonstrate these new service
flows work or satisfy the remaining lifecycle requirements.

Retained service replacement is now a host-qualified platform contract. A scope
can install bounded prior service declarations pinned to their exact binding
digests. They are recovery-only and cannot publish authority or accept new work.
Signed admission lookup recovers exact response-lost replay across an auth and
service revision without allocating work; changed payloads conflict. The driver,
task reads and stop controls route accepted tasks by their immutable binding.
A real Redis/server replacement contract proves revision-1 task observation and
same-message replay, revision-2 admission/execution, restart recovery, and no
provider call from the superseded queued task when current authority refuses it.
After the active declaration is removed, both retained revisions still support
exact replay and task controls while fresh sends are denied.
