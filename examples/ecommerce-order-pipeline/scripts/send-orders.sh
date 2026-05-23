#!/bin/bash
# Dispatches 15 sample orders exercising all Acteon features.
#
# Categories:
#   3 standard       → executed (pass dedup + throttle gates)
#   2 high-value     → requires approval (>$500)
#   2 sanctioned     → denied (NK, SY shipping)
#   2 duplicate      → same dedup_key, 2nd deduplicated
#   3 rapid-fire     → executed (new dedup_key each, under throttle limit)
#   3 after-hours    → scheduled if outside 9-17 ET Mon-Fri
#
# Usage: bash scripts/send-orders.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

dispatch() {
  local label="$1"
  shift
  echo -n "  $label: "
  RESPONSE=$(curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
    -H "Content-Type: application/json" \
    -d "$1" 2>&1) || { echo "FAILED"; return; }
  OUTCOME=$(echo "$RESPONSE" | jq -r 'keys[0] // "unknown"' 2>/dev/null || echo "unknown")
  echo "$OUTCOME"
}

CREATED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

echo "=== E-Commerce Order Pipeline: Sending Orders ==="
echo ""

# ── Standard orders (3) → chain: order-processing ──────────────────────────
echo "Standard orders (chain):"
dispatch "order-std-001" '{
  "id": "ord-std-001",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-1001", "customer_email": "alice@example.com", "total_cents": 5990, "currency": "USD", "shipping_country": "US", "items": [{"sku": "WIDGET-A", "qty": 2}], "card_last4": "4242", "billing_zip": "10001"},
  "template": "order-confirmation",
  "dedup_key": "order-ORD-1001",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-std-002" '{
  "id": "ord-std-002",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-1002", "customer_email": "bob@example.com", "total_cents": 12500, "currency": "USD", "shipping_country": "CA", "items": [{"sku": "GADGET-B", "qty": 1}], "card_last4": "1234", "billing_zip": "M5V3L9"},
  "template": "order-confirmation",
  "dedup_key": "order-ORD-1002",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-std-003" '{
  "id": "ord-std-003",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-1003", "customer_email": "carol@example.com", "total_cents": 19999, "currency": "USD", "shipping_country": "GB", "items": [{"sku": "TOOL-C", "qty": 3}, {"sku": "PART-D", "qty": 1}], "card_last4": "5678", "billing_zip": "SW1A1AA"},
  "template": "order-confirmation",
  "dedup_key": "order-ORD-1003",
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── High-value orders (2) → pending approval (>$500) ──────────────────────
echo "High-value orders (approval required):"
dispatch "order-hv-004" '{
  "id": "ord-hv-004",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-2001", "customer_email": "dave@example.com", "total_cents": 75000, "currency": "USD", "shipping_country": "US", "items": [{"sku": "PREMIUM-E", "qty": 1}], "card_last4": "9012", "billing_zip": "90210"},
  "dedup_key": "order-ORD-2001",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-hv-005" '{
  "id": "ord-hv-005",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-2002", "customer_email": "eve@example.com", "total_cents": 120000, "currency": "USD", "shipping_country": "DE", "items": [{"sku": "LUXURY-F", "qty": 2}], "card_last4": "3456", "billing_zip": "10115"},
  "dedup_key": "order-ORD-2002",
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Sanctioned region orders (2) → denied ─────────────────────────────────
echo "Sanctioned region orders (denied):"
dispatch "order-deny-006" '{
  "id": "ord-deny-006",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-3001", "customer_email": "user1@example.com", "total_cents": 2500, "currency": "USD", "shipping_country": "NK", "items": [{"sku": "ITEM-G", "qty": 1}], "card_last4": "7890", "billing_zip": "00000"},
  "dedup_key": "order-ORD-3001",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-deny-007" '{
  "id": "ord-deny-007",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-3002", "customer_email": "user2@example.com", "total_cents": 4900, "currency": "USD", "shipping_country": "SY", "items": [{"sku": "ITEM-H", "qty": 2}], "card_last4": "2345", "billing_zip": "00000"},
  "dedup_key": "order-ORD-3002",
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Duplicate orders (2) → 2nd is deduplicated ────────────────────────────
echo "Duplicate orders (dedup test):"
dispatch "order-dup-008a" '{
  "id": "ord-dup-008a",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-4001", "customer_email": "frank@example.com", "total_cents": 8990, "currency": "USD", "shipping_country": "US", "items": [{"sku": "DUP-ITEM", "qty": 1}], "card_last4": "6789", "billing_zip": "30301"},
  "dedup_key": "order-ORD-4001",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-dup-008b" '{
  "id": "ord-dup-008b",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-4001", "customer_email": "frank@example.com", "total_cents": 8990, "currency": "USD", "shipping_country": "US", "items": [{"sku": "DUP-ITEM", "qty": 1}], "card_last4": "6789", "billing_zip": "30301"},
  "dedup_key": "order-ORD-4001",
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Rapid-fire orders (3) → tests throttle (10/min per merchant) ──────────
echo "Rapid-fire orders (throttle test):"
for i in 1 2 3; do
  dispatch "order-rapid-$i" '{
    "id": "ord-rapid-00'"$i"'",
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "provider": "payment-gateway",
    "action_type": "place_order",
    "payload": {"order_id": "ORD-500'"$i"'", "customer_email": "rapid'"$i"'@example.com", "total_cents": 1500, "currency": "USD", "shipping_country": "US", "items": [{"sku": "BULK-'"$i"'", "qty": 1}], "card_last4": "000'"$i"'", "billing_zip": "2000'"$i"'"},
    "dedup_key": "order-ORD-500'"$i"'",
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── After-hours orders (3) → scheduled if outside 9-17 ET Mon-Fri ─────────
echo "After-hours orders (scheduled if outside business hours):"
dispatch "order-late-013" '{
  "id": "ord-late-013",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-6001", "customer_email": "night1@example.com", "total_cents": 3490, "currency": "USD", "shipping_country": "US", "items": [{"sku": "LATE-A", "qty": 1}], "card_last4": "1111", "billing_zip": "60601"},
  "dedup_key": "order-ORD-6001",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-late-014" '{
  "id": "ord-late-014",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-6002", "customer_email": "night2@example.com", "total_cents": 7250, "currency": "USD", "shipping_country": "US", "items": [{"sku": "LATE-B", "qty": 2}], "card_last4": "2222", "billing_zip": "60602"},
  "dedup_key": "order-ORD-6002",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "order-late-015" '{
  "id": "ord-late-015",
  "namespace": "ecommerce",
  "tenant": "acme-store",
  "provider": "payment-gateway",
  "action_type": "place_order",
  "payload": {"order_id": "ORD-6003", "customer_email": "night3@example.com", "total_cents": 4100, "currency": "USD", "shipping_country": "CA", "items": [{"sku": "LATE-C", "qty": 1}], "card_last4": "3333", "billing_zip": "V6B1A1"},
  "dedup_key": "order-ORD-6003",
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

echo "=== Done: 15 orders dispatched ==="
echo ""
echo "Expected outcomes (first-match-wins rule evaluation):"
echo "  - 2 suppressed (sanctioned regions: NK, SY — deny rule, priority 1)"
echo "  - 2 pending_approval (high-value orders >$500 — approval rule, priority 3)"
echo "  - 3 scheduled (if run outside 9-17 ET Mon-Fri — schedule rule, priority 10)"
echo "  - 1 deduplicated (duplicate ORD-4001 — dedup rule, priority 15)"
echo "  - Remaining: executed (first-time orders pass dedup, then throttle gate)"
echo ""
echo "Run 'bash scripts/show-report.sh' to see results."
