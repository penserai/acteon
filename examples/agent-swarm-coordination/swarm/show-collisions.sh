#!/bin/bash
# Queries the Acteon audit trail and displays a collision report showing how
# three concurrent agents interacted through shared state.
#
# Environment:
#   ACTEON_URL       - Acteon gateway URL (default: http://localhost:8080)
#   ACTEON_AGENT_KEY - API key for the claude-code-agent tenant
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

# ── Fetch audit records ──────────────────────────────────────────────────────

AUDIT=$(curl -sf "$ACTEON_URL/v1/audit?namespace=agent-swarm&tenant=claude-code-agent&limit=200" \
  ${ACTEON_AGENT_KEY:+-H "Authorization: Bearer $ACTEON_AGENT_KEY"} \
  2>/dev/null) || {
  echo "ERROR: Could not query Acteon audit trail at $ACTEON_URL"
  exit 1
}

# The response is an AuditPage with a .records array
RECORDS=$(echo "$AUDIT" | jq '.records // []')
TOTAL=$(echo "$RECORDS" | jq 'length')

if [ "$TOTAL" -eq 0 ]; then
  echo "No audit records found. Did the agents run?"
  exit 0
fi

# ── Overall summary ──────────────────────────────────────────────────────────

echo "Total dispatches: $TOTAL"
echo ""

echo "Outcome breakdown:"
echo "$RECORDS" | jq -r '
  group_by(.outcome) |
  map({outcome: .[0].outcome, count: length}) |
  sort_by(-.count) |
  .[] |
  "  \(.outcome): \(.count)"
'

echo ""

# ── Per-agent breakdown ──────────────────────────────────────────────────────

echo "Per-agent breakdown:"
echo "$RECORDS" | jq -r '
  group_by(.metadata.agent_role // "unknown") |
  map({
    agent: .[0].metadata.agent_role // "unknown",
    total: length,
    outcomes: (group_by(.outcome) | map({outcome: .[0].outcome, count: length}) | from_entries)
  }) |
  sort_by(.agent) |
  .[] |
  "  \(.agent) (\(.total) actions): \(.outcomes | to_entries | map("\(.key)=\(.value)") | join(", "))"
'

echo ""

# ── Collision events ─────────────────────────────────────────────────────────

COLLISIONS=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "throttled" or .outcome == "deduplicated" or .outcome == "quota_exceeded" or .outcome == "suppressed" or .outcome == "rerouted")]')
COLLISION_COUNT=$(echo "$COLLISIONS" | jq 'length')

echo "Collision events ($COLLISION_COUNT total):"
if [ "$COLLISION_COUNT" -gt 0 ]; then
  echo "$COLLISIONS" | jq -r '
    sort_by(.dispatched_at) |
    .[] |
    "  [\(.dispatched_at // "?")] \(.outcome) | agent=\(.metadata.agent_role // "?") | rule=\(.matched_rule // "n/a") | type=\(.action_type // "?")"
  '
else
  echo "  (none -- try running more agents or lowering throttle limits)"
fi

echo ""

# ── Top rules fired ─────────────────────────────────────────────────────────

echo "Top rules fired:"
echo "$RECORDS" | jq -r '
  [.[] | select(.matched_rule != null)] |
  group_by(.matched_rule) |
  map({rule: .[0].matched_rule, count: length}) |
  sort_by(-.count) |
  .[:10] |
  .[] |
  "  \(.rule): \(.count) times"
'
