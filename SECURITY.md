# Security policy

Acteon is pre-1.0 software. Security fixes target the current main branch and
latest release. Older releases do not have a separate maintenance guarantee.

Report vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/penserai/acteon/security/advisories/new).
Include the affected commit, enabled features, deployment configuration with
secrets removed, and a minimal reproduction. Avoid real customer data and credentials.
Do not open a public issue containing an unpatched exploit or secrets.

## Deployment boundaries

Enable authentication for remote listeners. The unauthenticated remote-bind
acknowledgement is for development. CORS is same-origin by default. Operator
credentials, internal-host allowlists, evaluator programs, and shared policy
directories are trusted configuration.

Swarm command filters and Git worktrees do not constitute an operating-system
sandbox. Execute agents and project commands in containers or another suitable
sandbox with restricted network/filesystem access. Keep acceptance graders and
credentials outside files the agent can modify. Never treat model prose or a
scalar regression score as proof that a reported vulnerability was fixed.

The DLQ's default storage is in memory; encryption does not make it durable.
Monitor `acteon_dlq_failures_total`; a failed write means the action was not
retained. Decryption failures remain as ciphertext and are excluded from redelivery.

## Dependency review

CI checks RustSec and npm advisories, licenses, dependency sources, and source-tree
secrets. Secret findings are redacted; documented dummy values have narrow scoped
exceptions. Workflow actions use immutable commits.
Dependabot opens update proposals. Updates must pass the same compatibility and
behavior checks as other changes. See [dependency exceptions](docs/security-dependencies.md)
for the narrow lockfile-only exception, including the executable reachability check.
