#!/bin/bash
# Demonstrates event lifecycle management via the Acteon API.
#
# 1. Seeds an initial event via dispatch API
# 2. Transitions: open → acknowledged → investigating → resolved
# 3. Lists events at each stage
#
# Usage: bash scripts/manage-incident.sh
# Environment:
#   ACTEON_URL  - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

FINGERPRINT="incident-db-001"
NAMESPACE="incidents"
TENANT="ops-team"

echo "=== Incident Response Pipeline: Event Lifecycle ==="
echo ""

# ── Step 1: Seed initial event via dispatch API ──────────────────────────────
echo "Step 1: Seeding initial event via dispatch..."
curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "seed-event-001",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'",
    "provider": "slack-alerts",
    "action_type": "alert",
    "payload": {"alert_id": "'"$FINGERPRINT"'", "severity": "critical", "service": "database", "environment": "production"}
  }' > /dev/null 2>&1 && echo "  Dispatched alert for $FINGERPRINT" || echo "  WARNING: dispatch failed. Is the server running?"
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
