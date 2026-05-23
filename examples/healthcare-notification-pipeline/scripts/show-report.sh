#!/bin/bash
# Queries API endpoints and displays a comprehensive pipeline report.
#
# Usage: bash scripts/show-report.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
NAMESPACE="healthcare"
TENANT="metro-hospital"

echo "╔══════════════════════════════════════════════════════════════════╗"
echo "║        Healthcare Notification Pipeline — Report               ║"
echo "╚══════════════════════════════════════════════════════════════════╝"
echo ""

# ── 1. Audit trail ────────────────────────────────────────────────────────
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

# Show PHI redaction in action
REDACTED=$(echo "$RECORDS" | jq '[.[] | select(.payload != null) | .payload | to_entries[] | select(.value == "[PHI_REDACTED]") | .key] | unique')
if [ "$REDACTED" != "[]" ] && [ "$REDACTED" != "null" ]; then
  echo "PHI-redacted fields in stored payloads: $REDACTED"
  echo ""
fi

# ── 2. Chains ─────────────────────────────────────────────────────────────
echo "━━━ 2. Discharge Workflow Chains ━━━"
CHAINS_RESP=$(curl -sf "$ACTEON_URL/v1/chains?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || CHAINS_RESP='{"chains":[]}'
CHAINS=$(echo "$CHAINS_RESP" | jq '.chains // []')
CHAIN_COUNT=$(echo "$CHAINS" | jq 'length')
echo "Active/completed chains: $CHAIN_COUNT"
if [ "$CHAIN_COUNT" -gt 0 ]; then
  echo "$CHAINS" | jq -r '
    .[] |
    "  [\(.status // "?")] \(.chain_name // "?") — started: \(.started_at // "?") step: \(.current_step_index // 0)"
  ' 2>/dev/null || true
fi
echo ""

# ── 3. Provider Health ────────────────────────────────────────────────────
echo "━━━ 3. Provider Health ━━━"
HEALTH=$(curl -sf "$ACTEON_URL/v1/providers/health" 2>/dev/null) || HEALTH='{}'
echo "$HEALTH" | jq -r '
  to_entries[] |
  "  \(.key): status=\(.value.status // "?") circuit=\(.value.circuit_breaker // "?")"
' 2>/dev/null || echo "  (no health data)"
echo ""

# ── 4. Quotas ─────────────────────────────────────────────────────────────
echo "━━━ 4. Quotas ━━━"
QUOTAS_RESP=$(curl -sf "$ACTEON_URL/v1/quotas?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || QUOTAS_RESP='{"quotas":[]}'
QUOTAS=$(echo "$QUOTAS_RESP" | jq '.quotas // []')
QUOTA_COUNT=$(echo "$QUOTAS" | jq 'length')
echo "Quota policies: $QUOTA_COUNT"
if [ "$QUOTA_COUNT" -gt 0 ]; then
  echo "$QUOTAS" | jq -r '
    .[] |
    "  \(.description // "unnamed"): \(.max_actions // "?")/\(.window // "?") [\(if .enabled then "enabled" else "disabled" end)]"
  ' 2>/dev/null || true
fi
echo ""

# ── 5. Event Groups ──────────────────────────────────────────────────────
echo "━━━ 5. Event Groups ━━━"
GROUPS_RESP=$(curl -sf "$ACTEON_URL/v1/groups" 2>/dev/null) || GROUPS_RESP='{"groups":[]}'
GROUPS=$(echo "$GROUPS_RESP" | jq '.groups // []')
GROUP_COUNT=$(echo "$GROUPS" | jq 'length')
echo "Active groups: $GROUP_COUNT"
if [ "$GROUP_COUNT" -gt 0 ]; then
  echo "$GROUPS" | jq -r '
    .[] |
    "  group=\(.group_key // "?") count=\(.count // 0) created=\(.created_at // "?")"
  ' 2>/dev/null || true
fi
echo ""

# ── 6. Retention Policies ────────────────────────────────────────────────
echo "━━━ 6. Retention Policies ━━━"
RET_RESP=$(curl -sf "$ACTEON_URL/v1/retention?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || RET_RESP='{"policies":[]}'
RETENTION=$(echo "$RET_RESP" | jq '.policies // []')
RET_COUNT=$(echo "$RETENTION" | jq 'length')
echo "Retention policies: $RET_COUNT"
if [ "$RET_COUNT" -gt 0 ]; then
  echo "$RETENTION" | jq -r '
    .[] |
    "  \(.description // "unnamed"): audit=\(.audit_ttl_seconds // "?")s events=\(.event_ttl_seconds // "?")s hold=\(.compliance_hold // false) [\(if .enabled then "enabled" else "disabled" end)]"
  ' 2>/dev/null || true
fi
echo ""

# ── 7. Compliance Status ─────────────────────────────────────────────────
echo "━━━ 7. Compliance Status ━━━"
STATUS=$(curl -sf "$ACTEON_URL/v1/compliance/status" 2>/dev/null) || STATUS='{}'
echo "$STATUS" | jq -r '
  "  Mode:              \(.mode // "none")\n  Sync writes:       \(.sync_audit_writes // false)\n  Immutable audit:   \(.immutable_audit // false)\n  Hash chain:        \(.hash_chain // false)"
' 2>/dev/null || echo "  (compliance status unavailable)"
echo ""

# ── Summary Table ─────────────────────────────────────────────────────────
echo "━━━ Summary ━━━"
echo ""
echo "  Category                        | Expected        | Actual"
echo "  --------------------------------|-----------------|--------"

EXECUTED=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "executed")] | length')
SUPPRESSED=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "suppressed")] | length')
REROUTED=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "rerouted")] | length')
APPROVAL=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "pending_approval")] | length')
CHAINED=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "chain_started")] | length')
GROUPED=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "grouped")] | length')
DEDUPED=$(echo "$RECORDS" | jq '[.[] | select(.outcome == "deduplicated")] | length')

echo "  Executed (safe notifications)   | ~6              | $EXECUTED"
echo "  Suppressed (PHI blocked)        | ~5              | $SUPPRESSED"
echo "  Rerouted (to patient portal)    | ~2              | $REROUTED"
echo "  Pending approval (external PHI) | ~2              | $APPROVAL"
echo "  Chain started (discharge)       | ~2              | $CHAINED"
echo "  Grouped (routine labs)          | ~3              | $GROUPED"
echo "  Deduplicated                    | ~1              | $DEDUPED"
echo ""

echo "═══════════════════════════════════════════════════════════════════"
echo "Report complete."
