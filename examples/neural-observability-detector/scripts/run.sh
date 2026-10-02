#!/usr/bin/env bash
set -euo pipefail

EXAMPLE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$EXAMPLE_DIR/../.." && pwd)"

cleanup() {
  if [[ "${KEEP_LAYA:-0}" != "1" ]]; then
    docker compose --project-directory "$EXAMPLE_DIR" \
      -f "$EXAMPLE_DIR/docker-compose.yml" down
  fi
}
trap cleanup EXIT

docker compose --project-directory "$EXAMPLE_DIR" \
  -f "$EXAMPLE_DIR/docker-compose.yml" up -d --build --wait

cd "$REPO_DIR"
cargo run -p acteon-simulation \
  --features bus --example neural_observability_simulation -- --write-results
