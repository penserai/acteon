#!/bin/bash
# Creates API-managed resources (quotas + retention) for the e-commerce pipeline.
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

echo "=== Setup complete ==="
