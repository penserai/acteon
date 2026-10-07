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
  This is an Acteon control extension. It does not certify provider abort,
  refund spent budget, release unresolved capacity, or replace a known result
  with a `cancelled` task. Lost acknowledgements are recovered by stopping the
  same original task again. Recovery scheduling skips durably stopped work.

Remaining before release:

- Complete provider abort/reconciliation and cancellation SDK/UI integration; qualify restart
  recovery against each supported production backend.
- Expand focused adversarial and integration tests from admission/isolation to
  full provider execution and peer lifecycle.
- Complete registry revision fencing and qualified outbound A2A lifecycle work.
- Complete lifecycle SDK/UI integration as cancellation and outbound peer contracts are added.
- Review adversarially, run CI, and verify publication before merging.

Passing existing workspace checks alone does not demonstrate these new service
flows work or satisfy the remaining lifecycle requirements.
