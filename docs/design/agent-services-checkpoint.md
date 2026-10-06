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
- Grant publication deferred until the existing post-authentication deployment
  publication stage; recipient authentication precedes source root allocation.

Remaining before release:

- Validate all configured recipient credentials before deployment publication.
- Wire authenticated agent-specific HTTP routes, task observation, cancellation,
  and a durable worker driver with restart recovery.
- Add focused adversarial and integration tests for configuration and admission.
- Complete registry revision fencing and qualified outbound A2A lifecycle work.
- Update SDKs, UI, and public documentation when the wire contract is established.
- Review adversarially, run CI, and verify publication before merging.

Passing existing workspace checks alone does not demonstrate these new service
flows work or satisfy the remaining lifecycle requirements.
