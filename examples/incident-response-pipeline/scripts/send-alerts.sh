#!/bin/bash
# Fires 20 sample alerts exercising all Acteon features.
#
# Categories:
#   2 critical  → triggers chain + sub-chain
#   3 high      → triggers chain (simpler path)
#   5 low       → grouped by service, 30s batch
#   5 storm     → rapid-fire, hits throttle at ~20/min
#   3 duplicate → same dedup_key, deduplicated
#   2 test-env  → environment=test, suppressed
#
# Usage: bash scripts/send-alerts.sh
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
  # Extract the outcome key from the response
  OUTCOME=$(echo "$RESPONSE" | jq -r 'keys[0] // "unknown"' 2>/dev/null || echo "unknown")
  echo "$OUTCOME"
}

CREATED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

echo "=== Incident Response Pipeline: Sending Alerts ==="
echo ""

# ── Critical alerts (2) → chain + sub-chain ──────────────────────────────────
echo "Critical alerts (chain + sub-chain):"
dispatch "critical-db-001" '{
  "id": "crit-db-001",
  "namespace": "incidents",
  "tenant": "ops-team",
  "provider": "pagerduty",
  "action_type": "alert",
  "payload": {"alert_id": "ALERT-001", "severity": "critical", "service": "database", "environment": "production", "api_key": "pg-key-12345"},
  "metadata": {"source": "datadog"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "critical-api-002" '{
  "id": "crit-api-002",
  "namespace": "incidents",
  "tenant": "ops-team",
  "provider": "pagerduty",
  "action_type": "alert",
  "payload": {"alert_id": "ALERT-002", "severity": "critical", "service": "api-gateway", "environment": "production", "webhook_url": "https://hooks.example.com/abc"},
  "metadata": {"source": "prometheus"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── High-severity alerts (3) → chain, simpler path ──────────────────────────
echo "High-severity alerts (chain):"
for i in 1 2 3; do
  dispatch "high-cache-00$i" '{
    "id": "high-cache-00'"$i"'",
    "namespace": "incidents",
    "tenant": "ops-team",
    "provider": "pagerduty",
    "action_type": "alert",
    "payload": {"alert_id": "ALERT-10'"$i"'", "severity": "high", "service": "cache-layer", "environment": "production"},
    "metadata": {"source": "grafana"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Low-severity alerts (5) → grouped by service, 30s batch ─────────────────
echo "Low-severity alerts (grouped):"
SERVICES=("cdn" "search" "cdn" "auth" "search")
for i in 0 1 2 3 4; do
  SVC="${SERVICES[$i]}"
  dispatch "low-$SVC-$i" '{
    "id": "low-'"$SVC"'-00'"$i"'",
    "namespace": "incidents",
    "tenant": "ops-team",
    "provider": "slack-alerts",
    "action_type": "alert",
    "payload": {"alert_id": "ALERT-20'"$i"'", "severity": "low", "service": "'"$SVC"'", "environment": "production"},
    "metadata": {"source": "cloudwatch"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Storm alerts (5) → rapid-fire, hits throttle limit ──────────────────────
echo "Storm alerts (throttle test):"
for i in $(seq 1 5); do
  dispatch "storm-$i" '{
    "id": "storm-'"$i"'",
    "namespace": "incidents",
    "tenant": "ops-team",
    "provider": "slack-alerts",
    "action_type": "alert",
    "payload": {"alert_id": "STORM-'"$i"'", "severity": "medium", "service": "payment-service", "environment": "production"},
    "metadata": {"source": "synthetic-storm"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Duplicate alerts (3) → same dedup_key, deduplicated ─────────────────────
echo "Duplicate alerts (dedup test):"
for i in 1 2 3; do
  dispatch "dup-$i" '{
    "id": "dup-net-00'"$i"'",
    "namespace": "incidents",
    "tenant": "ops-team",
    "provider": "email-alerts",
    "action_type": "alert",
    "payload": {"alert_id": "ALERT-300", "severity": "high", "service": "network", "environment": "production"},
    "metadata": {"source": "nagios"},
    "dedup_key": "network-alert-300",
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Test-environment alerts (2) → suppressed ─────────────────────────────────
echo "Test-environment alerts (suppressed):"
for i in 1 2; do
  dispatch "test-env-$i" '{
    "id": "test-env-00'"$i"'",
    "namespace": "incidents",
    "tenant": "ops-team",
    "provider": "slack-alerts",
    "action_type": "alert",
    "payload": {"alert_id": "TEST-00'"$i"'", "severity": "critical", "service": "staging-db", "environment": "test"},
    "metadata": {"source": "ci-pipeline"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

echo "=== Done: 20 alerts dispatched ==="
echo ""
echo "Expected outcomes:"
echo "  - 2 suppressed (test environment)"
echo "  - 2-3 deduplicated (same dedup_key)"
echo "  - 5 grouped (low severity, batched 30s)"
echo "  - Some throttled (if >20/min reached)"
echo "  - 5+ chain_started (critical + high severity)"
echo "  - Remaining executed"
echo ""
echo "Run 'bash scripts/show-report.sh' to see results."
