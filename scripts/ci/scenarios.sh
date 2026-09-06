#!/usr/bin/env bash
set -euo pipefail
# Build once, then exercise and replay each explicitly requested backend.
cargo build --locked -p acteon-simulation --features swarm,redis,postgres --bin acteon-scenario
mkdir -p scenario-results/runner
runner_candidate="$(mktemp scenario-results/runner/.candidate.XXXXXX)"
trap 'rm -f "$runner_candidate"' EXIT
cp target/debug/acteon-scenario "$runner_candidate"
# Preserve the exact executable identified by reports. Strip only the CI copy
# before fingerprinting to keep the uploaded Linux artifact small.
if [[ "${CI:-false}" == "true" && "$(uname -s)" == "Linux" ]]; then
  strip "$runner_candidate"
fi
runner_digest="$(python3 - "$runner_candidate" <<'PY'
import hashlib, sys
digest = hashlib.sha256()
with open(sys.argv[1], 'rb') as source:
    for chunk in iter(lambda: source.read(65536), b''):
        digest.update(chunk)
print(digest.hexdigest())
PY
)"
runner="scenario-results/runner/acteon-scenario-$runner_digest"
mv "$runner_candidate" "$runner"
chmod +x "$runner"
for backend in "${@:-memory}"; do
  suites=(rehabilitation portfolio queues handoffs)
  if [[ "$backend" == "memory" ]]; then suites+=(deadlines workers scheduling); fi
  for suite in "${suites[@]}"; do
    directory="scenario-results/$backend/$suite"
    mkdir -p "$directory"
    python3 - "$backend" "$directory/manifest.json" "$suite" <<'PY'
import json, sys
with open('scenarios/' + sys.argv[3] + '.json') as source:
    manifest = json.load(source)
manifest['backend'] = sys.argv[1]
with open(sys.argv[2], 'w') as output:
    json.dump(manifest, output, indent=2)
PY
    "$runner" --manifest "$directory/manifest.json" --output "$directory/first"
    "$runner" --replay "$directory/first/report.json" --output "$directory/replay"
  done
done
