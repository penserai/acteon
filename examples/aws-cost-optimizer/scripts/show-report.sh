#!/bin/bash
# Queries API endpoints and displays a cost optimizer report.
#
# Usage: bash examples/aws-cost-optimizer/scripts/show-report.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
NAMESPACE="infra"
TENANT="cost-optimizer"

echo "╔══════════════════════════════════════════════════════════════════╗"
echo "║          AWS Cost Optimizer — Report                           ║"
echo "╚══════════════════════════════════════════════════════════════════╝"
echo ""

# ── 1. Audit trail ──────────────────────────────────────────────────────────
echo "━━━ 1. Audit Trail ━━━"
AUDIT=$(curl -sf "$ACTEON_URL/v1/audit?namespace=$NAMESPACE&tenant=$TENANT&limit=100" 2>/dev/null) || AUDIT='{"records":[]}'
RECORDS=$(echo "$AUDIT" | jq '.records // []')
TOTAL=$(echo "$RECORDS" | jq 'length')

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

echo "Action type breakdown:"
echo "$RECORDS" | jq -r '
  group_by(.action_type) |
  map({action_type: .[0].action_type, count: length}) |
  sort_by(-.count) |
  .[] |
  "  \(.action_type): \(.count)"
'
echo ""

# ── 2. Provider Health ──────────────────────────────────────────────────────
echo "━━━ 2. Provider Health ━━━"
HEALTH=$(curl -sf "$ACTEON_URL/v1/providers/health" 2>/dev/null) || HEALTH='{}'
echo "$HEALTH" | jq -r '
  to_entries[] |
  "  \(.key): status=\(.value.status // "?") circuit=\(.value.circuit_breaker // "?")"
' 2>/dev/null || echo "  (no health data)"
echo ""

# ── 3. Recurring Actions ───────────────────────────────────────────────────
echo "━━━ 3. Recurring Actions ━━━"
RECURRING=$(curl -sf "$ACTEON_URL/v1/recurring?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || RECURRING='[]'
REC_COUNT=$(echo "$RECURRING" | jq 'if type == "array" then length else 0 end')
echo "Recurring actions: $REC_COUNT"
if [ "$REC_COUNT" -gt 0 ]; then
  echo ""
  echo "  Scale-down (off-hours):"
  echo "$RECURRING" | jq -r '
    if type == "array" then
      .[] | select(.description | test("Scale down"; "i")) |
      "    \(.description): cron=\(.cron_expr) executions=\(.execution_count // 0) [\(if .enabled then "active" else "paused" end)]"
    else
      empty
    end
  ' 2>/dev/null || true
  echo ""
  echo "  Scale-up (morning):"
  echo "$RECURRING" | jq -r '
    if type == "array" then
      .[] | select(.description | test("Scale up"; "i")) |
      "    \(.description): cron=\(.cron_expr) executions=\(.execution_count // 0) [\(if .enabled then "active" else "paused" end)]"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

# ── 4. Quotas ──────────────────────────────────────────────────────────────
echo "━━━ 4. Quotas ━━━"
QUOTAS=$(curl -sf "$ACTEON_URL/v1/quotas?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || QUOTAS='[]'
QUOTA_COUNT=$(echo "$QUOTAS" | jq 'if type == "array" then length else 0 end')
echo "Quota policies: $QUOTA_COUNT"
if [ "$QUOTA_COUNT" -gt 0 ]; then
  echo "$QUOTAS" | jq -r '
    if type == "array" then
      .[] |
      "  \(.description // "unnamed"): \(.max_actions // "?")/\(.window // "?") [\(if .enabled then "enabled" else "disabled" end)]"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

# ── 5. Cost Savings Summary ────────────────────────────────────────────────
echo "━━━ 5. Cost Savings Estimate ━━━"
echo "  Daytime capacity:   15 instances (web=4, api=6, workers=5)"
echo "  Off-hours capacity:  2 instances (web=1, api=1, workers=0)"
echo "  Reduction:          13 instances / 87% during off-hours"
echo "  Schedule:           Mon-Fri 7pm-7am EST (12 hrs/day)"
echo "  Weekly off-hours:   60 hrs of reduced capacity"
echo ""

echo "═══════════════════════════════════════════════════════════════════"
echo "Report complete."
