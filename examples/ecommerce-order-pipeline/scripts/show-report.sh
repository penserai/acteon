#!/bin/bash
# Queries API endpoints and displays a comprehensive order pipeline report.
#
# Usage: bash scripts/show-report.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
NAMESPACE="ecommerce"
TENANT="acme-store"

echo "╔══════════════════════════════════════════════════════════════════╗"
echo "║          E-Commerce Order Pipeline — Report                     ║"
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

# Show redaction in action
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
echo "━━━ 3. Events (Order Lifecycle) ━━━"
EVENTS=$(curl -sf "$ACTEON_URL/v1/events?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || EVENTS='[]'
EVENT_COUNT=$(echo "$EVENTS" | jq 'if type == "array" then length else 0 end')
echo "Tracked orders: $EVENT_COUNT"
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

# ── 4. Provider Health ──────────────────────────────────────────────────────
echo "━━━ 4. Provider Health ━━━"
HEALTH=$(curl -sf "$ACTEON_URL/v1/providers/health" 2>/dev/null) || HEALTH='{}'
echo "$HEALTH" | jq -r '
  to_entries[] |
  "  \(.key): status=\(.value.status // "?") circuit=\(.value.circuit_breaker // "?")"
' 2>/dev/null || echo "  (no health data)"
echo ""

# ── 5. Quotas ───────────────────────────────────────────────────────────────
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

# ── 6. Retention Policies ─────────────────────────────────────────────────
echo "━━━ 6. Retention Policies ━━━"
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

# ── 7. Circuit Breakers ───────────────────────────────────────────────────
echo "━━━ 7. Circuit Breakers ━━━"
CBS=$(curl -sf "$ACTEON_URL/v1/circuit-breakers" 2>/dev/null) || CBS='{}'
echo "$CBS" | jq -r '
  if type == "object" then
    to_entries[] |
    "  \(.key): \(.value // "?")"
  elif type == "array" then
    .[] |
    "  \(.provider // .name // "?"): \(.status // .state // "?")"
  else
    "  (no circuit breaker data)"
  end
' 2>/dev/null || echo "  (no circuit breaker data)"
echo ""

echo "═══════════════════════════════════════════════════════════════════"
echo "Report complete."
