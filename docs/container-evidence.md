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
nonroot UID/GID `65532:65532`. It must not include shells, apt/dpkg, or Perl.
Trivy scans the runtime archive for OS/library vulnerabilities
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
caches, credentials, and generated evidence. Build-stage base tags remain mutable;
the runtime base is pinned by digest. Provenance records the resolved build inputs rather than promising bit-for-bit
reproducibility. Runtime SBOM coverage does not replace Cargo/Node lockfile audits
for statically linked or bundled dependencies.

## Runtime image and compatibility

The runtime uses `gcr.io/distroless/cc-debian13:nonroot`, pinned by digest.
It supplies glibc, OpenSSL, the C/C++ runtime libraries, and CA certificates;
the server is still compiled with Rust 1.88 on Debian 12. Linux CI exercises
the resulting binary in the newer runtime to catch missing shared libraries.
The server, UI location, port, and CLI arguments are unchanged. The entrypoint
is an absolute executable path because the runtime has no shell.

The previous Debian 12 slim runtime produced 56 HIGH/CRITICAL package findings
(18 distinct CVEs) in run `35422891743`, none with a fixed version listed by
Trivy. Replacing unused distribution tools reduces the shipped attack surface;
it is not a vulnerability waiver. The same severity gate, including unfixed
findings, applies to the replacement image.

Deployment changes:

- The former `acteon` account is replaced by Distroless's `nonroot` account,
  explicitly UID/GID `65532:65532`. Make writable bind mounts accessible to this
  identity; existing volumes owned by the old UID may need an ownership change.
- Shell-based health checks and `docker exec ... sh` are not supported. Probe
  `/health` from the orchestrator, as the CI smoke test does. Use a separate
  debugging container when inspecting a deployment.
- Do not install packages at container startup. Add deliberate runtime
  dependencies at build time and rerun the scan and smoke tests.
- Refresh the pinned runtime digest regularly; a tag update alone will not
  pull security fixes into an already pinned build.

See the upstream [Distroless image documentation](https://github.com/GoogleContainerTools/distroless).

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
