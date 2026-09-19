# Container scanning and build evidence

The `Container evidence` workflow builds the Dockerfile on a Linux runner for
`linux/amd64`, without publishing an image to a registry. It runs on pull requests,
main pushes, and manual dispatch. The macOS self-hosted runner continues to run
the dependency audit; this job requires a Linux Docker host.

## Evidence and gates

BuildKit exports an OCI archive with an SPDX SBOM and `mode=max` provenance.
Skopeo converts its runnable platform image to a Docker archive for Trivy and
the smoke test. The verifier checks content-addressed metadata, requires both
attestations to name the image manifest, compares both exports' image config,
and hashes the Docker archive's uncompressed layers against the attested
config's `rootfs.diff_ids`. Missing or mismatched evidence fails the job.

The packaged image must start, serve health and the bundled UI, and run as the
`acteon` user. Trivy scans the runtime archive for OS/library vulnerabilities
and fails on HIGH or CRITICAL findings, including those without fixes. Findings
are retained in JSON; there is no new vulnerability exception or fail-open gate.

The `container-evidence` artifact (seven-day retention) contains:

- `image.oci.tar`: image and native BuildKit attestations;
- `image.docker.tar`: converted image used for scanning and execution;
- `sbom.spdx.json`, `provenance.json`, and `identity.json`;
- `vulnerabilities.json` when the scan ran;
- `SHA256SUMS` for both archives.

Artifact upload runs even after failures, so an incomplete artifact is diagnostic
evidence, not a release approval. The job's successful conclusion is required.
No registry credentials, package write permissions, or signing credentials are
used. These are unsigned BuildKit statements, not signed release provenance or
a claimed SLSA assurance level. Archive hashes detect changes only when compared
against a trusted workflow artifact.

The Dockerfile builds only the server package using `--locked` and installs its
native build requirements. It no longer installs an unpinned cargo-chef release
or prebuilds the entire workspace. BuildKit layer caching remains enabled; source
changes can require a full server rebuild. Build context excludes local dependency
caches, credentials, and generated evidence. Base image tags remain mutable;
provenance records the resolved build inputs rather than promising bit-for-bit
reproducibility. Runtime SBOM coverage does not replace Cargo/Node lockfile audits
for statically linked or bundled dependencies.

## Validation

Run the evidence validator tests without Docker:

```sh
python3 -m unittest discover -s scripts/ci -p test_container_evidence.py
```

They cover matching exports and rejection of missing provenance, wrong subjects,
wrong image configs, altered metadata, and altered filesystem layers. Local
workflow YAML parsing and whitespace checks also passed. Full image build,
attestation generation, scanning, and smoke execution require Linux CI; the
local Docker daemon was unavailable when this workflow was introduced.

See Docker's [attestation storage format](https://docs.docker.com/build/metadata/attestations/attestation-storage/)
and [SBOM/provenance options](https://docs.docker.com/build/ci/github-actions/attestations/).
