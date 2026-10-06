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

Remaining before release:

- Complete requester cancellation and ambiguity handling; qualify restart
  recovery against each supported production backend.
- Expand focused adversarial and integration tests from admission/isolation to
  full provider execution and peer lifecycle.
- Complete registry revision fencing and qualified outbound A2A lifecycle work.
- Update SDKs, UI, and public documentation when the wire contract is established.
- Review adversarially, run CI, and verify publication before merging.

Passing existing workspace checks alone does not demonstrate these new service
flows work or satisfy the remaining lifecycle requirements.
