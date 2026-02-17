#!/bin/bash
# Orchestrates three concurrent headless Claude Code sessions that collide
# through shared Acteon state (throttle counters, dedup keys, quotas).
#
# Each agent gets its own workspace copy of dummy-project/ but dispatches to
# the same Acteon namespace+tenant, maximizing observable collisions.
#
# Environment:
#   ACTEON_URL       - Acteon gateway URL (default: http://localhost:8080)
#   ACTEON_AGENT_KEY - API key for the claude-code-agent tenant
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
EXAMPLE_DIR="$(dirname "$SCRIPT_DIR")"
ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

AGENTS=("api-builder" "test-writer" "security-auditor")
PIDS=()
QUOTA_ID="q-swarm-demo"
QUOTA_CREATED=false

# ── Helpers ──────────────────────────────────────────────────────────────────

cleanup() {
  echo ""
  echo "Cleaning up..."

  # Kill any still-running agent sessions
  for pid in "${PIDS[@]}"; do
    if kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done

  # Remove workspaces
  for agent in "${AGENTS[@]}"; do
    rm -rf "$SCRIPT_DIR/workspaces/$agent"
  done

  # Only delete the quota if this script created it
  if [ "$QUOTA_CREATED" = true ]; then
    curl -s -X DELETE "$ACTEON_URL/v1/quotas/$QUOTA_ID" \
      ${ACTEON_AGENT_KEY:+-H "Authorization: Bearer $ACTEON_AGENT_KEY"} \
      > /dev/null 2>&1 || true
    echo "  Removed demo quota ($QUOTA_ID)"
  fi

  echo "Done."
}

trap cleanup EXIT

# ── Prerequisites ────────────────────────────────────────────────────────────

echo "=== Multi-Agent Swarm Demo ==="
echo ""

# Check Acteon is reachable
if ! curl -sf "$ACTEON_URL/healthz" > /dev/null 2>&1; then
  echo "ERROR: Acteon is not reachable at $ACTEON_URL"
  echo "Start it with: cargo run -p acteon-server --features postgres -- -c $EXAMPLE_DIR/acteon.toml"
  exit 1
fi
echo "[ok] Acteon is running at $ACTEON_URL"

# Check claude CLI is available
if ! command -v claude &> /dev/null; then
  echo "ERROR: 'claude' CLI not found in PATH"
  echo "Install Claude Code: https://claude.com/claude-code"
  exit 1
fi
echo "[ok] Claude Code CLI found"

# Check jq is available
if ! command -v jq &> /dev/null; then
  echo "ERROR: 'jq' not found in PATH"
  echo "Install it: brew install jq (macOS) or apt-get install jq (Linux)"
  exit 1
fi
echo "[ok] jq found"

# ── Create quota policy ─────────────────────────────────────────────────────

echo ""
echo "Creating tenant-wide quota: 30 actions / 2 minutes..."
QUOTA_HTTP=$(curl -s -o /dev/null -w "%{http_code}" -X POST "$ACTEON_URL/v1/quotas" \
  -H "Content-Type: application/json" \
  ${ACTEON_AGENT_KEY:+-H "Authorization: Bearer $ACTEON_AGENT_KEY"} \
  -d "{
    \"id\": \"$QUOTA_ID\",
    \"namespace\": \"agent-swarm\",
    \"tenant\": \"claude-code-agent\",
    \"max_actions\": 30,
    \"window\": \"custom_120\",
    \"overage_behavior\": \"block\",
    \"enabled\": true,
    \"description\": \"Swarm demo: 30 actions per 2 minutes across all agents\"
  }" 2>/dev/null) || true

if [ "$QUOTA_HTTP" = "200" ] || [ "$QUOTA_HTTP" = "201" ]; then
  QUOTA_CREATED=true
  echo "[ok] Quota policy created (will be removed on exit)"
elif [ "$QUOTA_HTTP" = "409" ]; then
  echo "[ok] Quota $QUOTA_ID already exists (will NOT be removed on exit)"
else
  echo "[warn] Quota creation returned HTTP $QUOTA_HTTP -- continuing without quota"
fi

# ── Prepare workspaces ──────────────────────────────────────────────────────

echo ""
echo "Preparing agent workspaces..."
for agent in "${AGENTS[@]}"; do
  WORKSPACE="$SCRIPT_DIR/workspaces/$agent"
  rm -rf "$WORKSPACE"
  cp -r "$EXAMPLE_DIR/dummy-project" "$WORKSPACE"

  # Initialize a fresh git repo in each workspace
  (cd "$WORKSPACE" && git init -q && git add . && git commit -q -m "initial commit") 2>/dev/null
  echo "  [ok] $agent -> workspaces/$agent/"
done

# ── Launch agents ────────────────────────────────────────────────────────────

echo ""
echo "Launching 3 headless Claude Code sessions..."
echo "  (agents share tenant 'claude-code-agent' -- collisions expected)"
echo ""

for agent in "${AGENTS[@]}"; do
  WORKSPACE="$SCRIPT_DIR/workspaces/$agent"
  PROMPT_FILE="$SCRIPT_DIR/prompts/$agent.md"

  if [ ! -f "$PROMPT_FILE" ]; then
    echo "ERROR: Prompt file not found: $PROMPT_FILE"
    exit 1
  fi

  PROMPT=$(cat "$PROMPT_FILE")

  echo "  Starting $agent..."
  (
    cd "$WORKSPACE"
    ACTEON_AGENT_ROLE="$agent" \
    ACTEON_URL="$ACTEON_URL" \
    ${ACTEON_AGENT_KEY:+ACTEON_AGENT_KEY="$ACTEON_AGENT_KEY"} \
    claude -p "$PROMPT" \
      --allowedTools "Bash,Write,Edit,Read,Glob,Grep" \
      2>"$SCRIPT_DIR/workspaces/$agent.stderr.log"
  ) > "$SCRIPT_DIR/workspaces/$agent.stdout.log" 2>&1 &
  PIDS+=($!)
  echo "    PID: ${PIDS[-1]}"
done

# ── Wait for completion ─────────────────────────────────────────────────────

echo ""
echo "Waiting for all agents to finish..."
echo ""
echo "  Watch collisions in real-time (in another terminal):"
echo "    curl -N '$ACTEON_URL/v1/events/stream?namespace=agent-swarm&tenant=claude-code-agent'"
echo "  Or open the Acteon Dashboard at $ACTEON_URL/ui"
echo ""

FAILED=0
for i in "${!AGENTS[@]}"; do
  agent="${AGENTS[$i]}"
  pid="${PIDS[$i]}"
  if wait "$pid" 2>/dev/null; then
    echo "  [done] $agent (PID $pid) completed successfully"
  else
    echo "  [fail] $agent (PID $pid) exited with error"
    FAILED=$((FAILED + 1))
  fi
done

echo ""
if [ "$FAILED" -gt 0 ]; then
  echo "$FAILED agent(s) failed. Check logs in workspaces/*.stderr.log"
else
  echo "All agents completed."
fi

# ── Show collision report ────────────────────────────────────────────────────

echo ""
echo "=== Collision Report ==="
echo ""
bash "$SCRIPT_DIR/show-collisions.sh"
