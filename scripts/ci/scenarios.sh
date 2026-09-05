#!/usr/bin/env bash
set -euo pipefail
# Build once, then exercise and replay each explicitly requested backend.
cargo build --locked -p acteon-simulation --features swarm,redis,postgres --bin acteon-scenario
for backend in "${@:-memory}"; do
  directory="scenario-results/$backend"
  mkdir -p "$directory"
  python3 - "$backend" "$directory/manifest.json" <<'PY'
import json, sys
with open('scenarios/rehabilitation.json') as source:
    manifest = json.load(source)
manifest['backend'] = sys.argv[1]
with open(sys.argv[2], 'w') as output:
    json.dump(manifest, output, indent=2)
PY
  target/debug/acteon-scenario --manifest "$directory/manifest.json" --output "$directory/first"
  target/debug/acteon-scenario --replay "$directory/first/report.json" --output "$directory/replay"
done
