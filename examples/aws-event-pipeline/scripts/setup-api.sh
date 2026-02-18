#!/bin/bash
# Creates API-managed resources: quota, retention policy, and recurring action.
#
# Usage: bash examples/aws-event-pipeline/scripts/setup-api.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== AWS Event Pipeline: API Setup ==="
echo ""

# ── Quota: 500 actions/hour for smartbuilding-hq ───────────────────────────
echo "Creating quota (500 actions/hour)..."
curl -sf -X POST "$ACTEON_URL/v1/quotas" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "max_actions": 500,
    "window": "hourly",
    "overage_behavior": "block",
    "enabled": true,
    "description": "500 actions per hour for smartbuilding-hq"
  }' | jq .
echo ""

# ── Retention: audit 7 days, events 2 days ─────────────────────────────────
echo "Creating retention policy (audit 7d, events 2d)..."
curl -sf -X POST "$ACTEON_URL/v1/retention" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "enabled": true,
    "audit_ttl_seconds": 604800,
    "event_ttl_seconds": 172800,
    "compliance_hold": false,
    "description": "7-day audit, 2-day events"
  }' | jq .
echo ""

# ── Recurring: device heartbeat check every 5 minutes ─────────────────────
echo "Creating recurring action (heartbeat every 5 min)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "cron_expr": "*/5 * * * *",
    "timezone": "UTC",
    "enabled": true,
    "action_template": {
      "provider": "event-bus",
      "action_type": "device_lifecycle",
      "payload": {
        "event": "heartbeat_check",
        "source": "acteon.scheduler",
        "check_id": "heartbeat-{{execution_time}}"
      }
    },
    "description": "Device heartbeat check every 5 minutes"
  }' | jq .
echo ""

echo "Setup complete. Resources created:"
echo "  - Quota: 500/hour for iot:smartbuilding-hq"
echo "  - Retention: audit 7d, events 2d for iot:smartbuilding-hq"
echo "  - Recurring: heartbeat check every 5 min via event-bus"
