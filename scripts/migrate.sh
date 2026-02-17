#!/usr/bin/env bash
# Run database migrations for the configured Acteon state and audit backends.
#
# This script wraps `acteon-server migrate` with convenience flags for
# backend selection and config file discovery.
#
# Usage:
#   scripts/migrate.sh [OPTIONS]
#
# Examples:
#   scripts/migrate.sh --backend postgres -c acteon.toml
#   scripts/migrate.sh --backend clickhouse -c config/acteon.toml
#   scripts/migrate.sh -c examples/incident-response-pipeline/acteon.toml
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Defaults
BACKEND=""
CONFIG_FILE=""
CARGO_ARGS=""
DRY_RUN=false

usage() {
  cat <<EOF
Usage: $(basename "$0") [OPTIONS]

Run database migrations for configured Acteon state and audit backends.

Options:
  -c, --config FILE       Path to acteon.toml config file (default: acteon.toml)
  -b, --backend BACKEND   State backend: postgres, clickhouse, dynamodb, redis, memory
                           (auto-detected from config if omitted; sets cargo feature flag)
  -n, --dry-run           Build the binary but don't run migrations
  -h, --help              Show this help message

Environment:
  DATABASE_URL            PostgreSQL connection string (used by postgres backend)
  CLICKHOUSE_URL          ClickHouse connection string (used by clickhouse backend)
  AWS_REGION              AWS region (used by dynamodb backend)

Examples:
  # Migrate using the example incident pipeline config
  $(basename "$0") -c examples/incident-response-pipeline/acteon.toml

  # Migrate with explicit backend (skips auto-detection)
  $(basename "$0") --backend postgres -c acteon.toml

  # Dry-run: just build, don't run
  $(basename "$0") --dry-run -c acteon.toml
EOF
  exit 0
}

# ── Parse arguments ──────────────────────────────────────────────────────────
while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help)
      usage
      ;;
    -c|--config)
      CONFIG_FILE="$2"
      shift 2
      ;;
    -b|--backend)
      BACKEND="$2"
      shift 2
      ;;
    -n|--dry-run)
      DRY_RUN=true
      shift
      ;;
    *)
      echo "Unknown option: $1"
      echo "Run '$(basename "$0") --help' for usage."
      exit 1
      ;;
  esac
done

# ── Resolve config file ─────────────────────────────────────────────────────
if [[ -z "$CONFIG_FILE" ]]; then
  CONFIG_FILE="acteon.toml"
fi

if [[ ! -f "$CONFIG_FILE" ]]; then
  # Try relative to project root
  if [[ -f "$PROJECT_ROOT/$CONFIG_FILE" ]]; then
    CONFIG_FILE="$PROJECT_ROOT/$CONFIG_FILE"
  else
    echo "Error: config file not found: $CONFIG_FILE"
    exit 1
  fi
fi

# ── Auto-detect backend from config if not specified ─────────────────────────
if [[ -z "$BACKEND" ]]; then
  # Extract state.backend from TOML (simple grep — works for flat config)
  BACKEND=$(grep -E '^\s*backend\s*=' "$CONFIG_FILE" | head -1 | sed 's/.*=\s*"\([^"]*\)".*/\1/' || true)
  if [[ -z "$BACKEND" ]]; then
    BACKEND="memory"
  fi
  echo "Auto-detected backend: $BACKEND"
fi

# ── Map backend to cargo feature flag ────────────────────────────────────────
case "$BACKEND" in
  postgres)
    CARGO_ARGS="--features postgres"
    ;;
  clickhouse)
    CARGO_ARGS="--features clickhouse"
    ;;
  dynamodb)
    CARGO_ARGS="--features dynamodb"
    ;;
  redis|memory)
    # No feature flag needed
    CARGO_ARGS=""
    ;;
  *)
    echo "Error: unknown backend '$BACKEND'"
    echo "Supported backends: postgres, clickhouse, dynamodb, redis, memory"
    exit 1
    ;;
esac

# ── Run migrations ───────────────────────────────────────────────────────────
echo "=== Acteon Database Migration ==="
echo "  Config:  $CONFIG_FILE"
echo "  Backend: $BACKEND"
echo ""

if [[ "$DRY_RUN" == true ]]; then
  echo "Dry-run: building acteon-server..."
  # shellcheck disable=SC2086
  cargo build -p acteon-server $CARGO_ARGS
  echo "Build successful. Run without --dry-run to execute migrations."
  exit 0
fi

# shellcheck disable=SC2086
cargo run -p acteon-server $CARGO_ARGS -- -c "$CONFIG_FILE" migrate
