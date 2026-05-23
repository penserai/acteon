#!/bin/bash
# Queries 8 API endpoints and displays a comprehensive pipeline report.
#
# Usage: bash examples/aws-event-pipeline/scripts/show-report.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
NAMESPACE="iot"
TENANT="smartbuilding-hq"

echo "╔══════════════════════════════════════════════════════════════════╗"
echo "║          AWS Event Pipeline — Report                           ║"
echo "╚══════════════════════════════════════════════════════════════════╝"
echo ""

# ── 1. Audit trail ──────────────────────────────────────────────────────────
echo "━━━ 1. Audit Trail ━━━"
AUDIT=$(curl -sf "$ACTEON_URL/v1/audit?namespace=$NAMESPACE&tenant=$TENANT&limit=200" 2>/dev/null) || AUDIT='{"records":[]}'
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

# Show redaction in action (if any payloads stored)
REDACTED=$(echo "$RECORDS" | jq '[.[] | select(.payload != null) | .payload | to_entries[] | select(.value == "[REDACTED]") | .key] | unique')
if [ "$REDACTED" != "[]" ] && [ "$REDACTED" != "null" ]; then
  echo "Redacted fields found in payloads: $REDACTED"
  echo ""
fi

# ── 2. Chains ───────────────────────────────────────────────────────────────
echo "━━━ 2. Chains ━━━"
CHAINS=$(curl -sf "$ACTEON_URL/v1/chains?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || CHAINS='[]'
CHAIN_COUNT=$(echo "$CHAINS" | jq 'if type == "array" then length else 0 end')
echo "Active/completed chains: $CHAIN_COUNT"
if [ "$CHAIN_COUNT" -gt 0 ]; then
  echo "$CHAINS" | jq -r '
    if type == "array" then
      .[] |
      "  [\(.status // "?")] \(.chain_name // "?") — started: \(.started_at // "?") step: \(.current_step_index // 0)"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

# ── 3. Events ───────────────────────────────────────────────────────────────
echo "━━━ 3. Events ━━━"
EVENTS=$(curl -sf "$ACTEON_URL/v1/events?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || EVENTS='[]'
EVENT_COUNT=$(echo "$EVENTS" | jq 'if type == "array" then length else 0 end')
echo "Tracked events: $EVENT_COUNT"
if [ "$EVENT_COUNT" -gt 0 ]; then
  echo "$EVENTS" | jq -r '
    if type == "array" then
      .[] |
      "  [\(.state // "?")] fingerprint=\(.fingerprint // "?") updated=\(.updated_at // "?")"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

# ── 4. Provider Health ────────────────────────────────────────────────────
echo "━━━ 4. Provider Health ━━━"
HEALTH=$(curl -sf "$ACTEON_URL/v1/providers/health" 2>/dev/null) || HEALTH='{}'
echo "$HEALTH" | jq -r '
  to_entries[] |
  "  \(.key): status=\(.value.status // "?") circuit=\(.value.circuit_breaker // "?")"
' 2>/dev/null || echo "  (no health data)"
echo ""

# ── 5. Quotas ─────────────────────────────────────────────────────────────
echo "━━━ 5. Quotas ━━━"
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

# ── 6. Groups ─────────────────────────────────────────────────────────────
echo "━━━ 6. Event Groups ━━━"
GROUPS=$(curl -sf "$ACTEON_URL/v1/groups" 2>/dev/null) || GROUPS='[]'
GROUP_COUNT=$(echo "$GROUPS" | jq 'if type == "array" then length else 0 end')
echo "Active groups: $GROUP_COUNT"
if [ "$GROUP_COUNT" -gt 0 ]; then
  echo "$GROUPS" | jq -r '
    if type == "array" then
      .[] |
      "  group=\(.group_key // "?") count=\(.count // 0) created=\(.created_at // "?")"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

# ── 7. Recurring Actions ──────────────────────────────────────────────────
echo "━━━ 7. Recurring Actions ━━━"
RECURRING=$(curl -sf "$ACTEON_URL/v1/recurring?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || RECURRING='[]'
REC_COUNT=$(echo "$RECURRING" | jq 'if type == "array" then length else 0 end')
echo "Recurring actions: $REC_COUNT"
if [ "$REC_COUNT" -gt 0 ]; then
  echo "$RECURRING" | jq -r '
    if type == "array" then
      .[] |
      "  \(.description // "unnamed"): cron=\(.cron_expr // "?") executions=\(.execution_count // 0) [\(if .enabled then "active" else "paused" end)]"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

# ── 8. Retention Policies ─────────────────────────────────────────────────
echo "━━━ 8. Retention Policies ━━━"
RETENTION=$(curl -sf "$ACTEON_URL/v1/retention?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || RETENTION='[]'
RET_COUNT=$(echo "$RETENTION" | jq 'if type == "array" then length else 0 end')
echo "Retention policies: $RET_COUNT"
if [ "$RET_COUNT" -gt 0 ]; then
  echo "$RETENTION" | jq -r '
    if type == "array" then
      .[] |
      "  \(.description // "unnamed"): audit=\(.audit_ttl_seconds // "?")s events=\(.event_ttl_seconds // "?")s [\(if .enabled then "enabled" else "disabled" end)]"
    else
      empty
    end
  ' 2>/dev/null || true
fi
echo ""

echo "═══════════════════════════════════════════════════════════════════"
echo "Report complete."
