#!/bin/bash
# Demonstrates event lifecycle management via the Acteon API.
#
# 1. Seeds an initial event via psql INSERT
# 2. Transitions: open → acknowledged → investigating → resolved
# 3. Lists events at each stage
#
# Usage: bash scripts/manage-incident.sh
# Environment:
#   ACTEON_URL  - Acteon gateway URL (default: http://localhost:8080)
#   DATABASE_URL - PostgreSQL URL (default: postgres://localhost:5432/acteon)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
DATABASE_URL="${DATABASE_URL:-postgres://localhost:5432/acteon}"

FINGERPRINT="incident-db-001"
NAMESPACE="incidents"
TENANT="ops-team"

echo "=== Incident Response Pipeline: Event Lifecycle ==="
echo ""

# ── Step 1: Seed initial event via psql ──────────────────────────────────────
echo "Step 1: Seeding initial event (state=open) via psql..."
STATE_KEY="${NAMESPACE}:${TENANT}:event_state:${FINGERPRINT}"
UPDATED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
STATE_VALUE='{"state":"open","fingerprint":"'"$FINGERPRINT"'","updated_at":"'"$UPDATED_AT"'"}'

psql "$DATABASE_URL" -c "
  INSERT INTO acteon_state (key, value, updated_at)
  VALUES ('$STATE_KEY', '$STATE_VALUE', NOW())
  ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = NOW();
" 2>/dev/null && echo "  Seeded: $FINGERPRINT (state=open)" || {
  echo "  WARNING: psql failed. Is PostgreSQL running?"
  echo "  Trying to create the event via a dispatch instead..."
  # Fallback: dispatch an action that will create state
  curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
    -H "Content-Type: application/json" \
    -d '{
      "id": "seed-event-001",
      "namespace": "'"$NAMESPACE"'",
      "tenant": "'"$TENANT"'",
      "provider": "slack-alerts",
      "action_type": "alert",
      "payload": {"alert_id": "'"$FINGERPRINT"'", "severity": "critical", "service": "database", "environment": "production"}
    }' > /dev/null 2>&1
  echo "  Dispatched alert as fallback."
}
echo ""

# ── Step 2: List events (should show open) ───────────────────────────────────
echo "Step 2: Listing events (expect open)..."
curl -sf "$ACTEON_URL/v1/events?namespace=$NAMESPACE&tenant=$TENANT" | jq . 2>/dev/null || echo "  (no events yet)"
echo ""

# ── Step 3: Transition open → acknowledged ───────────────────────────────────
echo "Step 3: Transitioning $FINGERPRINT: open → acknowledged..."
curl -sf -X PUT "$ACTEON_URL/v1/events/$FINGERPRINT/transition" \
  -H "Content-Type: application/json" \
  -d '{
    "to": "acknowledged",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'"
  }' | jq .
echo ""

# ── Step 4: Transition acknowledged → investigating ──────────────────────────
echo "Step 4: Transitioning $FINGERPRINT: acknowledged → investigating..."
curl -sf -X PUT "$ACTEON_URL/v1/events/$FINGERPRINT/transition" \
  -H "Content-Type: application/json" \
  -d '{
    "to": "investigating",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'"
  }' | jq .
echo ""

# ── Step 5: Transition investigating → resolved ─────────────────────────────
echo "Step 5: Transitioning $FINGERPRINT: investigating → resolved..."
curl -sf -X PUT "$ACTEON_URL/v1/events/$FINGERPRINT/transition" \
  -H "Content-Type: application/json" \
  -d '{
    "to": "resolved",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'"
  }' | jq .
echo ""

# ── Step 6: Final event listing ──────────────────────────────────────────────
echo "Step 6: Final event listing..."
curl -sf "$ACTEON_URL/v1/events?namespace=$NAMESPACE&tenant=$TENANT" | jq .
echo ""

echo "=== Event lifecycle complete: open → acknowledged → investigating → resolved ==="
