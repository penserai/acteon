#!/bin/bash
# Creates API-managed resources (quotas, retention, templates, profiles) for the e-commerce pipeline.
#
# Usage: bash scripts/setup.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== E-Commerce Order Pipeline: Setup ==="
echo ""

# ── Quota: 200 orders/hour per merchant ────────────────────────────────────
echo "Creating quota (200 orders/hour)..."
curl -sf -X POST "$ACTEON_URL/v1/quotas" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "max_actions": 200,
    "window": "hourly",
    "overage_behavior": "block",
    "enabled": true,
    "description": "200 orders per hour for acme-store"
  }' | jq .
echo ""

# ── Retention: audit 14 days, events 3 days ────────────────────────────────
echo "Creating retention policy (audit 14d, events 3d)..."
curl -sf -X POST "$ACTEON_URL/v1/retention" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "enabled": true,
    "audit_ttl_seconds": 1209600,
    "event_ttl_seconds": 259200,
    "compliance_hold": false,
    "description": "14-day audit, 3-day events"
  }' | jq .
echo ""

# ── Templates: order confirmation + fraud alert ──────────────────────────
echo "Creating template: order-confirmation-body..."
curl -sf -X POST "$ACTEON_URL/v1/templates" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "name": "order-confirmation-body",
    "body": "<h2>Order Confirmed: {{ order_id }}</h2>\n<p>Thank you for your order!</p>\n<table style=\"border-collapse: collapse; width: 100%;\">\n  <tr style=\"background: #f3f4f6;\">\n    <th style=\"padding: 8px; text-align: left;\">Item</th>\n    <th style=\"padding: 8px; text-align: right;\">Qty</th>\n  </tr>\n  {% for item in items %}\n  <tr>\n    <td style=\"padding: 8px;\">{{ item.sku }}</td>\n    <td style=\"padding: 8px; text-align: right;\">{{ item.qty }}</td>\n  </tr>\n  {% endfor %}\n</table>\n<p><strong>Total:</strong> ${{ \"%.2f\" | format(total_cents / 100) }}</p>\n<p>Shipping to: {{ shipping_country }}</p>\n<hr>\n<small>Acme Store — Powered by Acteon</small>",
    "description": "HTML email body for order confirmations"
  }' | jq .
echo ""

echo "Creating template: fraud-alert-body..."
curl -sf -X POST "$ACTEON_URL/v1/templates" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "name": "fraud-alert-body",
    "body": "FRAUD REVIEW REQUIRED\n\nOrder: {{ order_id }}\nAmount: ${{ \"%.2f\" | format(total_cents / 100) }}\nRegion: {{ shipping_country }}\nReason: {{ reason }}\n\nPlease review in the compliance dashboard within 24 hours.",
    "description": "Plain-text fraud review notification"
  }' | jq .
echo ""

# ── Profiles: order confirmation + fraud review ─────────────────────────
echo "Creating profile: order-confirmation..."
curl -sf -X POST "$ACTEON_URL/v1/templates/profiles" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "name": "order-confirmation",
    "fields": {
      "subject": {"inline": "Order {{ order_id }} confirmed — Acme Store"},
      "html_body": {"$ref": "order-confirmation-body"},
      "reply_to": {"inline": "orders@acme-store.example.com"}
    },
    "description": "Order confirmation email profile"
  }' | jq .
echo ""

echo "Creating profile: fraud-review-notification..."
curl -sf -X POST "$ACTEON_URL/v1/templates/profiles" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "name": "fraud-review-notification",
    "fields": {
      "subject": {"inline": "[ACTION REQUIRED] Fraud review: {{ order_id }}"},
      "body": {"$ref": "fraud-alert-body"},
      "priority": {"inline": "high"}
    },
    "description": "Fraud review notification profile"
  }' | jq .
echo ""

# ── Render preview ──────────────────────────────────────────────────────
echo "Rendering template preview (order-confirmation-body)..."
curl -sf -X POST "$ACTEON_URL/v1/templates/render" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "ecommerce",
    "tenant": "acme-store",
    "template": "order-confirmation-body",
    "data": {
      "order_id": "ORD-PREVIEW",
      "items": [{"sku": "WIDGET-A", "qty": 2}, {"sku": "GADGET-B", "qty": 1}],
      "total_cents": 12990,
      "shipping_country": "US"
    }
  }' | jq .
echo ""

echo "=== Setup complete ==="
