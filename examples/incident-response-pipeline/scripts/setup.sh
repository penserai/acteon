#!/bin/bash
# Creates API-managed resources: quota, retention policy, and recurring action.
#
# Usage: bash scripts/setup.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== Incident Response Pipeline: Setup ==="
echo ""

# ── Quota: 100 actions/hour for ops-team ─────────────────────────────────────
echo "Creating quota (100 actions/hour)..."
curl -sf -X POST "$ACTEON_URL/v1/quotas" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "incidents",
    "tenant": "ops-team",
    "max_actions": 100,
    "window": "hourly",
    "overage_behavior": "block",
    "enabled": true,
    "description": "100 actions per hour for ops-team"
  }' | jq .
echo ""

# ── Retention: audit 7 days, events 1 day ────────────────────────────────────
echo "Creating retention policy (audit 7d, events 1d)..."
curl -sf -X POST "$ACTEON_URL/v1/retention" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "incidents",
    "tenant": "ops-team",
    "enabled": true,
    "audit_ttl_seconds": 604800,
    "event_ttl_seconds": 86400,
    "compliance_hold": false,
    "description": "7-day audit, 1-day events"
  }' | jq .
echo ""

# ── Recurring: health check every minute ─────────────────────────────────────
echo "Creating recurring action (health check every 60s)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "incidents",
    "tenant": "ops-team",
    "cron_expr": "* * * * *",
    "timezone": "UTC",
    "enabled": true,
    "action_template": {
      "provider": "slack-alerts",
      "action_type": "alert",
      "payload": {
        "severity": "low",
        "service": "health-monitor",
        "alert_id": "health-check-{{execution_time}}"
      }
    },
    "description": "Health check alert every minute"
  }' | jq .
echo ""

echo "Setup complete. Resources created:"
echo "  - Quota: 100/hour for incidents:ops-team"
echo "  - Retention: audit 7d, events 1d for incidents:ops-team"
echo "  - Recurring: health-check every 60s via slack-alerts"
