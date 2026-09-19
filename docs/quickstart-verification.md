# Documentation and quick-start verification

The release-hardening follow-up now exercises the published quick-start requests
against the real server, with in-memory state and an explicitly registered log
provider. The server performs no outbound notification delivery in this example.

## Contract

`scripts/ci/quickstart.py` reads the marked shell blocks directly from
`docs/book/getting-started/quickstart.md`, starts the configured server on an
available loopback port, and executes the requests in walkthrough order.
It requires the complete ordered set of checks, rejects duplicate/missing
markers and unmarked curl requests, verifies the documented startup config and
JSON response examples, and checks:

- health and readiness;
- successful log-provider execution;
- deduplication on the repeated request;
- suppression of the test recipient;
- dispatch counters before and after the batch;
- OpenAPI dispatch schema and Swagger UI availability;
- two successful batch outcomes.

Startup, requests, and shutdown have timeouts. The runner stops its own server
even on assertion failure and prints server logs for diagnosis. It executes
trusted repository shell snippets, so it belongs in the same unprivileged CI
context as the repository's other tests.

The stable Rust test job builds the server binary and runs the walkthrough.
A separate documentation job builds MkDocs in strict mode on pushes and pull
requests. It shares the pinned top-level docs dependencies with the Pages
workflow. Transitive Python dependencies are not locked. Publishing still occurs
only through the existing Pages workflow.

## Corrected documentation

The earlier walkthrough omitted required `id`/`created_at` fields, used a batch
object instead of an array, showed the wrong serialized outcomes, and claimed
an unregistered email provider would fall back to a no-op. The new example
configuration and requests correct those mismatches and avoid accidentally
loading the repository's separate demo configuration.

Strict building also exposed links to recovery notes outside MkDocs' source
directory. Those now link to their repository locations; stale swarm anchors
and two README crate links were corrected. Installation instructions distinguish
the container's internal listener from its loopback-only host publication.

## Verification and limits

The walkthrough passed against both the existing local server binary and a
fresh locked source build. The initial rebuild in the existing target directory
hit a disk-space error; a clean temporary target with debug info and incremental
compilation disabled built successfully. A negative
check replacing the first action's `id` with `invalid_id` failed on HTTP 422;
restoring the documented request passed. The strict MkDocs build, workflow YAML
parsing, and diff whitespace checks passed.

This verifies the focused source-build/log-provider walkthrough, not all SDK
snippets, real email delivery, every backend, Docker image construction, or
external website availability. The Docker instructions were checked against
the entrypoint, working directory, and listener validation in source. Hosted CI
execution remains to be observed after publication of the changes.
