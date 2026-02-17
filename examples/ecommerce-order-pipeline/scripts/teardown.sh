#!/bin/bash
# Cleans up API-created resources (quotas, retention policies).
#
# Usage: bash scripts/teardown.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
NAMESPACE="ecommerce"
TENANT="acme-store"

echo "=== E-Commerce Order Pipeline: Teardown ==="
echo ""

# ── Delete quotas ──────────────────────────────────────────────────────────
echo "Deleting quotas..."
QUOTAS=$(curl -sf "$ACTEON_URL/v1/quotas?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || QUOTAS='[]'
echo "$QUOTAS" | jq -r 'if type == "array" then .[].id else empty end' 2>/dev/null | while read -r ID; do
  echo "  Deleting quota $ID..."
  curl -sf -X DELETE "$ACTEON_URL/v1/quotas/$ID" > /dev/null 2>&1 && echo "    done" || echo "    failed"
done
echo ""

# ── Delete retention policies ──────────────────────────────────────────────
echo "Deleting retention policies..."
RETENTION=$(curl -sf "$ACTEON_URL/v1/retention?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || RETENTION='[]'
echo "$RETENTION" | jq -r 'if type == "array" then .[].id else empty end' 2>/dev/null | while read -r ID; do
  echo "  Deleting retention policy $ID..."
  curl -sf -X DELETE "$ACTEON_URL/v1/retention/$ID" > /dev/null 2>&1 && echo "    done" || echo "    failed"
done
echo ""

echo "=== Teardown complete ==="
