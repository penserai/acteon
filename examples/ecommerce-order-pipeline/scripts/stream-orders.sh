#!/bin/bash
# Demonstrates real-time order event streaming via SSE.
#
# Starts an SSE listener in the background, dispatches 3 orders, then
# shows real-time events flowing through. Cleans up the background
# listener on exit.
#
# Usage: bash scripts/stream-orders.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== E-Commerce Order Pipeline: SSE Streaming Demo ==="
echo ""

# ── Start SSE listener in background ──────────────────────────────────────
echo "Starting SSE listener (namespace=ecommerce)..."
echo "Press Ctrl+C to stop."
echo ""

# Trap to clean up background process
cleanup() {
  if [ -n "${SSE_PID:-}" ]; then
    kill "$SSE_PID" 2>/dev/null || true
    wait "$SSE_PID" 2>/dev/null || true
  fi
  echo ""
  echo "SSE listener stopped."
}
trap cleanup EXIT

# Start SSE stream in background, printing events as they arrive
curl -N -sf "$ACTEON_URL/v1/stream?namespace=ecommerce" 2>/dev/null &
SSE_PID=$!

# Give the SSE connection time to establish
sleep 1

echo ""
echo "── Dispatching 3 orders while streaming ──"
echo ""

CREATED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# Order 1: standard
echo -n "  stream-order-1: "
curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "stream-ord-001",
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "provider": "payment-gateway",
    "action_type": "place_order",
    "payload": {"order_id": "STREAM-001", "customer_email": "stream1@example.com", "total_cents": 2999, "currency": "USD", "shipping_country": "US", "items": [{"sku": "SSE-A", "qty": 1}], "card_last4": "4444", "billing_zip": "10001"},
    "dedup_key": "order-STREAM-001",
    "created_at": "'"$CREATED_AT"'"
  }' | jq -r 'keys[0] // "unknown"' 2>/dev/null || echo "unknown"

sleep 1

# Order 2: another standard
echo -n "  stream-order-2: "
curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "stream-ord-002",
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "provider": "payment-gateway",
    "action_type": "place_order",
    "payload": {"order_id": "STREAM-002", "customer_email": "stream2@example.com", "total_cents": 4550, "currency": "USD", "shipping_country": "CA", "items": [{"sku": "SSE-B", "qty": 2}], "card_last4": "5555", "billing_zip": "M5V1J2"},
    "dedup_key": "order-STREAM-002",
    "created_at": "'"$CREATED_AT"'"
  }' | jq -r 'keys[0] // "unknown"' 2>/dev/null || echo "unknown"

sleep 1

# Order 3: sanctioned region (will be denied)
echo -n "  stream-order-3 (denied): "
curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  -d '{
    "id": "stream-ord-003",
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "provider": "payment-gateway",
    "action_type": "place_order",
    "payload": {"order_id": "STREAM-003", "customer_email": "stream3@example.com", "total_cents": 1500, "currency": "USD", "shipping_country": "IR", "items": [{"sku": "SSE-C", "qty": 1}], "card_last4": "6666", "billing_zip": "00000"},
    "dedup_key": "order-STREAM-003",
    "created_at": "'"$CREATED_AT"'"
  }' | jq -r 'keys[0] // "unknown"' 2>/dev/null || echo "unknown"

echo ""
echo "── Waiting 3s for SSE events to appear above ──"
sleep 3

echo ""
echo "=== SSE streaming demo complete ==="
