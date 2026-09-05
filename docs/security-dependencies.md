# Dependency security decisions

Reviewed 2026-09-04 during rehabilitation of commit
`f0e08a14a6d2b684c12df5f06b40c3d95451d2d2`.

The dependency refresh moves Wasmtime to the patched 36 LTS line and compiles
only runtime, Cranelift, text-format, and standard-library support. WebAssembly
threads, component support, profiling, and cache support are not enabled.
The MCP SDK uses its patched 1.x API; Azure SDKs use the 1.x core/identity/storage
family and the compatible Event Hubs release. AWS SDKs use the current HTTPS
connector, with the legacy Hyper/Rustls features disabled. All selected releases
retain the repository's Rust 1.88 minimum.

## RUSTSEC-2023-0071: RSA timing side channel

`rsa` appears in Cargo.lock through SQLx's optional MySQL graph. Acteon uses
PostgreSQL and does not compile that RSA dependency, including under workspace
`--all-features`. The audit configuration excludes this single advisory.
`scripts/ci/security.sh` fails if RSA appears in the resolved compile graph for
any target, so enabling the vulnerable functionality cannot silently inherit
this exception. Review this decision whenever SQLx features change. This is not
an exception for using vulnerable RSA private-key operations.

## Remaining warnings

The scan still reports two unsoundness notices for `lru` 0.12.5:
RUSTSEC-2026-0253 (`pop` panic safety) and RUSTSEC-2026-0002 (`IterMut`).
The dependency enters through optional AWS S3 support in `aws-sdk-s3` 1.119.0,
the selected Rust 1.88-compatible SDK. Its S3 Express cache uses
`get_or_insert_mut`, not either reported method, and Acteon does not call `lru`
directly. This source inspection is a reachability assessment, not a proof that
all transitive unsafe code is sound. Both warnings remain visible; neither has
been added to the ignore list. Reassess when S3 support, its SDK, or the minimum
Rust version changes, and migrate to a fixed compatible release when available.

`instant`, `paste`, and `rustls-pemfile` still have unmaintained-package notices.
The audit gate currently blocks vulnerability advisories and reports these
maintenance/unsoundness warnings. It does not mean the dependency graph is free
of all security or maintenance concerns.

Unmaintained-package notices remain review signals, not proof of exploitable
runtime behavior. Do not disable whole advisory categories or add broad ignores
to make CI pass. Store scan output with release evidence and reassess reachability
when dependencies or feature flags change.

## License, source, and secret policy

`deny.toml` checks all features against an explicit permissive license list and
allows only the crates.io registry; unapproved Git sources or registries fail.
Run `cargo deny --locked check licenses sources` with cargo-deny 0.20.2.
RustSec advisory decisions remain in the separate audit gate described above.

`scripts/ci/secrets.sh` uses Gitleaks 8.30.1 against tracked and new source files,
excluding ignored local files, build output, symlink targets, and Git history.
Findings are redacted in `dependency-evidence/secrets.json`. `.gitleaks.toml`
extends the upstream rules with narrowly scoped exceptions for inspected dummy
tokens and non-credential grouping/deduplication identifiers in named examples.
The clean source scan and rejection of a temporary synthetic credential were
both verified. This source-tree gate does not audit historical commits or
replace incident response and credential rotation after a real disclosure.
