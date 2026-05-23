#!/bin/bash
# Demonstrates order lifecycle management via the Acteon event API.
#
# 1. Seeds an initial order via dispatch API
# 2. Transitions: placed → confirmed → shipped → delivered
# 3. Lists events at each stage
#
# Usage: bash scripts/manage-order.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

FINGERPRINT="order-lifecycle-001"
NAMESPACE="ecommerce"
TENANT="acme-store"

echo "=== E-Commerce Order Pipeline: Order Lifecycle ==="
echo ""

# ── Step 1: Seed initial order via dispatch API ────────────────────────────
echo "Step 1: Placing order via dispatch..."
curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "lifecycle-seed-001",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'",
    "provider": "payment-gateway",
    "action_type": "place_order",
    "payload": {"order_id": "'"$FINGERPRINT"'", "customer_email": "lifecycle@example.com", "total_cents": 9999, "currency": "USD", "shipping_country": "US", "items": [{"sku": "LIFECYCLE-ITEM", "qty": 1}], "card_last4": "9999", "billing_zip": "10001"},
    "dedup_key": "order-'"$FINGERPRINT"'"
  }' > /dev/null 2>&1 && echo "  Dispatched order $FINGERPRINT" || echo "  WARNING: dispatch failed. Is the server running?"
echo ""

# ── Step 2: List events (should show placed) ──────────────────────────────
echo "Step 2: Listing events (expect placed)..."
curl -sf "$ACTEON_URL/v1/events?namespace=$NAMESPACE&tenant=$TENANT" | jq . 2>/dev/null || echo "  (no events yet)"
echo ""

# ── Step 3: Transition placed → confirmed ──────────────────────────────────
echo "Step 3: Transitioning $FINGERPRINT: placed → confirmed..."
curl -sf -X PUT "$ACTEON_URL/v1/events/$FINGERPRINT/transition" \
  -H "Content-Type: application/json" \
  -d '{
    "to": "confirmed",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'"
  }' | jq .
echo ""

# ── Step 4: Transition confirmed → shipped ─────────────────────────────────
echo "Step 4: Transitioning $FINGERPRINT: confirmed → shipped..."
curl -sf -X PUT "$ACTEON_URL/v1/events/$FINGERPRINT/transition" \
  -H "Content-Type: application/json" \
  -d '{
    "to": "shipped",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'"
  }' | jq .
echo ""

# ── Step 5: Transition shipped → delivered ─────────────────────────────────
echo "Step 5: Transitioning $FINGERPRINT: shipped → delivered..."
curl -sf -X PUT "$ACTEON_URL/v1/events/$FINGERPRINT/transition" \
  -H "Content-Type: application/json" \
  -d '{
    "to": "delivered",
    "namespace": "'"$NAMESPACE"'",
    "tenant": "'"$TENANT"'"
  }' | jq .
echo ""

# ── Step 6: Final event listing ────────────────────────────────────────────
echo "Step 6: Final event listing..."
curl -sf "$ACTEON_URL/v1/events?namespace=$NAMESPACE&tenant=$TENANT" | jq .
echo ""

echo "=== Order lifecycle complete: placed → confirmed → shipped → delivered ==="
