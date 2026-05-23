#!/bin/bash
# Cleans up API-created resources (quotas, retention policies, templates, profiles).
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

# ── Delete template profiles ────────────────────────────────────────────────
echo "Deleting template profiles..."
PROFILES=$(curl -sf "$ACTEON_URL/v1/templates/profiles?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || PROFILES='{"profiles":[]}'
echo "$PROFILES" | jq -r '.profiles[]?.id // empty' 2>/dev/null | while read -r ID; do
  echo "  Deleting profile $ID..."
  curl -sf -X DELETE "$ACTEON_URL/v1/templates/profiles/$ID" > /dev/null 2>&1 && echo "    done" || echo "    failed"
done
echo ""

# ── Delete templates ────────────────────────────────────────────────────────
echo "Deleting templates..."
TEMPLATES=$(curl -sf "$ACTEON_URL/v1/templates?namespace=$NAMESPACE&tenant=$TENANT" 2>/dev/null) || TEMPLATES='{"templates":[]}'
echo "$TEMPLATES" | jq -r '.templates[]?.id // empty' 2>/dev/null | while read -r ID; do
  echo "  Deleting template $ID..."
  curl -sf -X DELETE "$ACTEON_URL/v1/templates/$ID" > /dev/null 2>&1 && echo "    done" || echo "    failed"
done
echo ""

echo "=== Teardown complete ==="
