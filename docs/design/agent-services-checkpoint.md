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

Remaining before release:

- Complete requester-isolated task observation, cancellation, and a durable
  worker driver with restart recovery.
- Expand focused adversarial and integration tests from admission/isolation to
  full provider execution and peer lifecycle.
- Complete registry revision fencing and qualified outbound A2A lifecycle work.
- Update SDKs, UI, and public documentation when the wire contract is established.
- Review adversarially, run CI, and verify publication before merging.

Passing existing workspace checks alone does not demonstrate these new service
flows work or satisfy the remaining lifecycle requirements.
