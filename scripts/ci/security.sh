#!/usr/bin/env bash
set -euo pipefail
# Fail if a feature change makes the lockfile-only RSA exception reachable.
rsa_dependencies="$(cargo tree --locked --workspace --all-features --target all -i rsa --prefix none 2>/dev/null)"
if [[ -n "$rsa_dependencies" ]]; then
  printf '%s\n' 'RSA is now in the compiled dependency graph; reassess RUSTSEC-2023-0071.' >&2
  exit 1
fi
cargo audit
