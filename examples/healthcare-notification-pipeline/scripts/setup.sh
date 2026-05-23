#!/bin/bash
# Creates API-managed resources: retention policy and quota.
# Idempotent — skips creation if resources already exist.
#
# Usage: bash scripts/setup.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== Healthcare Notification Pipeline: Setup ==="
echo ""

# ── Retention: 90-day audit, 7-day state, compliance hold ──────────────────
EXISTING_RET=$(curl -sf "$ACTEON_URL/v1/retention?namespace=healthcare&tenant=metro-hospital" 2>/dev/null | jq '.count // 0') || EXISTING_RET=0
if [ "$EXISTING_RET" -gt 0 ]; then
  echo "Retention policy already exists, skipping."
else
  echo "Creating retention policy (audit 90d, events 7d, compliance hold)..."
  curl -sf -X POST "$ACTEON_URL/v1/retention" \
    -H "Content-Type: application/json" \
    -d '{
      "namespace": "healthcare",
      "tenant": "metro-hospital",
      "enabled": true,
      "audit_ttl_seconds": 7776000,
      "event_ttl_seconds": 604800,
      "compliance_hold": true,
      "description": "HIPAA: 90-day audit, 7-day events, no auto-deletion"
    }' | jq .
fi
echo ""

# ── Quota: 200 actions/hour for metro-hospital ────────────────────────────
EXISTING_QUOTA=$(curl -sf "$ACTEON_URL/v1/quotas?namespace=healthcare&tenant=metro-hospital" 2>/dev/null | jq '.count // 0') || EXISTING_QUOTA=0
if [ "$EXISTING_QUOTA" -gt 0 ]; then
  echo "Quota policy already exists, skipping."
else
  echo "Creating quota (200 actions/hour)..."
  curl -sf -X POST "$ACTEON_URL/v1/quotas" \
    -H "Content-Type: application/json" \
    -d '{
      "namespace": "healthcare",
      "tenant": "metro-hospital",
      "max_actions": 200,
      "window": "hourly",
      "overage_behavior": "block",
      "enabled": true,
      "description": "200 notifications per hour for metro-hospital"
    }' | jq .
fi
echo ""

echo "Setup complete. Resources:"
echo "  - Retention: audit 90d, events 7d, compliance_hold=true for healthcare:metro-hospital"
echo "  - Quota: 200/hour for healthcare:metro-hospital"
