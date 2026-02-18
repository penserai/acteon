#!/bin/bash
# Verifies HIPAA compliance mode and audit hash chain integrity.
#
# Usage: bash scripts/verify-compliance.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "╔══════════════════════════════════════════════════════════════════╗"
echo "║       Healthcare Pipeline — Compliance Verification            ║"
echo "╚══════════════════════════════════════════════════════════════════╝"
echo ""

# ── 1. Compliance Status ──────────────────────────────────────────────────
echo "━━━ 1. Compliance Mode Status ━━━"
STATUS=$(curl -sf "$ACTEON_URL/v1/compliance/status" 2>/dev/null) || STATUS='{}'

MODE=$(echo "$STATUS" | jq -r '.mode // "unknown"')
SYNC=$(echo "$STATUS" | jq -r '.sync_audit_writes // false')
IMMUTABLE=$(echo "$STATUS" | jq -r '.immutable_audit // false')
HASH=$(echo "$STATUS" | jq -r '.hash_chain // false')

echo "  Mode:               $MODE"
echo "  Sync audit writes:  $SYNC"
echo "  Immutable audit:    $IMMUTABLE"
echo "  Hash chain:         $HASH"
echo ""

if [ "$MODE" = "hipaa" ] && [ "$SYNC" = "true" ] && [ "$IMMUTABLE" = "true" ] && [ "$HASH" = "true" ]; then
  echo "  [PASS] HIPAA compliance mode is fully active"
else
  echo "  [WARN] HIPAA compliance mode is NOT fully configured"
fi
echo ""

# ── 2. Hash Chain Verification ────────────────────────────────────────────
echo "━━━ 2. Audit Hash Chain Verification ━━━"
VERIFY=$(curl -sf -X POST "$ACTEON_URL/v1/audit/verify" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "healthcare",
    "tenant": "metro-hospital"
  }' 2>/dev/null) || VERIFY='{"error": "verification endpoint unavailable"}'

VALID=$(echo "$VERIFY" | jq -r '.valid // "N/A"')
RECORDS=$(echo "$VERIFY" | jq -r '.records_checked // 0')
FIRST=$(echo "$VERIFY" | jq -r '.first_record_id // "N/A"')
LAST=$(echo "$VERIFY" | jq -r '.last_record_id // "N/A"')
BROKEN=$(echo "$VERIFY" | jq -r '.first_broken_at // "none"')

echo "  Chain valid:        $VALID"
echo "  Records checked:    $RECORDS"
echo "  First record:       $FIRST"
echo "  Last record:        $LAST"
echo "  First broken at:    $BROKEN"
echo ""

if [ "$VALID" = "true" ]; then
  echo "  [PASS] Audit hash chain is intact — $RECORDS records verified"
elif [ "$VALID" = "false" ]; then
  echo "  [FAIL] Hash chain broken at record: $BROKEN"
else
  echo "  [INFO] No records to verify or hash chaining not enabled"
fi
echo ""

# ── 3. Retention Policy ───────────────────────────────────────────────────
echo "━━━ 3. Retention Policy ━━━"
RETENTION=$(curl -sf "$ACTEON_URL/v1/retention?namespace=healthcare&tenant=metro-hospital" 2>/dev/null) || RETENTION='{"policies":[]}'
POLICIES=$(echo "$RETENTION" | jq '.policies // []')
RET_COUNT=$(echo "$POLICIES" | jq 'length')

if [ "$RET_COUNT" -gt 0 ]; then
  echo "$POLICIES" | jq -r '
    .[] |
    "  Description:      \(.description // "unnamed")\n  Audit TTL:         \((.audit_ttl_seconds // 0) / 86400 | floor)d\n  Event TTL:         \((.event_ttl_seconds // 0) / 86400 | floor)d\n  Compliance hold:   \(.compliance_hold // false)"
  ' 2>/dev/null || true

  HOLD=$(echo "$POLICIES" | jq -r '.[0].compliance_hold // false')
  if [ "$HOLD" = "true" ]; then
    echo ""
    echo "  [PASS] Compliance hold is active — no auto-deletion"
  fi
else
  echo "  No retention policies found. Run scripts/setup.sh first."
fi
echo ""

echo "═══════════════════════════════════════════════════════════════════"
echo "Compliance verification complete."
