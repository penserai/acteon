#!/bin/bash
# Creates API-managed resources: recurring scale-down/scale-up actions and quota.
#
# This creates 6 recurring actions:
#   - 3 scale-down actions (Mon-Fri 7pm EST → reduce to minimum capacity)
#   - 3 scale-up actions   (Mon-Fri 7am EST → restore daytime capacity)
#
# Usage: bash examples/aws-cost-optimizer/scripts/setup-api.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== AWS Cost Optimizer: API Setup ==="
echo ""

# ── Quota: 100 scaling actions/day ──────────────────────────────────────────
echo "Creating quota (100 actions/day)..."
curl -sf -X POST "$ACTEON_URL/v1/quotas" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "max_actions": 100,
    "window": "daily",
    "overage_behavior": "block",
    "enabled": true,
    "description": "100 scaling actions per day for cost-optimizer"
  }' | jq .
echo ""

# ── Scale-Down: staging-web (7pm EST, Mon-Fri) ─────────────────────────────
echo "Creating recurring action: scale-down staging-web (7pm Mon-Fri)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "cron_expr": "0 19 * * 1-5",
    "timezone": "America/New_York",
    "enabled": true,
    "action_template": {
      "provider": "cost-asg",
      "action_type": "set_desired_capacity",
      "payload": {
        "auto_scaling_group_name": "staging-web",
        "desired_capacity": 1,
        "honor_cooldown": false,
        "tag": "scale-down"
      }
    },
    "description": "Scale down staging-web to 1 instance at 7pm EST"
  }' | jq .
echo ""

# ── Scale-Down: staging-api (7pm EST, Mon-Fri) ─────────────────────────────
echo "Creating recurring action: scale-down staging-api (7pm Mon-Fri)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "cron_expr": "0 19 * * 1-5",
    "timezone": "America/New_York",
    "enabled": true,
    "action_template": {
      "provider": "cost-asg",
      "action_type": "set_desired_capacity",
      "payload": {
        "auto_scaling_group_name": "staging-api",
        "desired_capacity": 1,
        "honor_cooldown": false,
        "tag": "scale-down"
      }
    },
    "description": "Scale down staging-api to 1 instance at 7pm EST"
  }' | jq .
echo ""

# ── Scale-Down: staging-workers (7pm EST, Mon-Fri) ─────────────────────────
echo "Creating recurring action: scale-down staging-workers (7pm Mon-Fri)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "cron_expr": "0 19 * * 1-5",
    "timezone": "America/New_York",
    "enabled": true,
    "action_template": {
      "provider": "cost-asg",
      "action_type": "set_desired_capacity",
      "payload": {
        "auto_scaling_group_name": "staging-workers",
        "desired_capacity": 0,
        "honor_cooldown": false,
        "tag": "scale-down"
      }
    },
    "description": "Scale down staging-workers to 0 instances at 7pm EST"
  }' | jq .
echo ""

# ── Scale-Up: staging-web (7am EST, Mon-Fri) ───────────────────────────────
echo "Creating recurring action: scale-up staging-web (7am Mon-Fri)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "cron_expr": "0 7 * * 1-5",
    "timezone": "America/New_York",
    "enabled": true,
    "action_template": {
      "provider": "cost-asg",
      "action_type": "set_desired_capacity",
      "payload": {
        "auto_scaling_group_name": "staging-web",
        "desired_capacity": 4,
        "honor_cooldown": false,
        "tag": "scale-up"
      }
    },
    "description": "Scale up staging-web to 4 instances at 7am EST"
  }' | jq .
echo ""

# ── Scale-Up: staging-api (7am EST, Mon-Fri) ───────────────────────────────
echo "Creating recurring action: scale-up staging-api (7am Mon-Fri)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "cron_expr": "0 7 * * 1-5",
    "timezone": "America/New_York",
    "enabled": true,
    "action_template": {
      "provider": "cost-asg",
      "action_type": "set_desired_capacity",
      "payload": {
        "auto_scaling_group_name": "staging-api",
        "desired_capacity": 6,
        "honor_cooldown": false,
        "tag": "scale-up"
      }
    },
    "description": "Scale up staging-api to 6 instances at 7am EST"
  }' | jq .
echo ""

# ── Scale-Up: staging-workers (7am EST, Mon-Fri) ───────────────────────────
echo "Creating recurring action: scale-up staging-workers (7am Mon-Fri)..."
curl -sf -X POST "$ACTEON_URL/v1/recurring" \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "infra",
    "tenant": "cost-optimizer",
    "cron_expr": "0 7 * * 1-5",
    "timezone": "America/New_York",
    "enabled": true,
    "action_template": {
      "provider": "cost-asg",
      "action_type": "set_desired_capacity",
      "payload": {
        "auto_scaling_group_name": "staging-workers",
        "desired_capacity": 5,
        "honor_cooldown": false,
        "tag": "scale-up"
      }
    },
    "description": "Scale up staging-workers to 5 instances at 7am EST"
  }' | jq .
echo ""

echo "=== API setup complete ==="
echo ""
echo "Resources created:"
echo "  - Quota: 100/day for infra:cost-optimizer"
echo "  - Recurring: 3 scale-down actions (Mon-Fri 7pm EST)"
echo "    - staging-web:     4 -> 1 instance"
echo "    - staging-api:     6 -> 1 instance"
echo "    - staging-workers: 5 -> 0 instances"
echo "  - Recurring: 3 scale-up actions (Mon-Fri 7am EST)"
echo "    - staging-web:     1 -> 4 instances"
echo "    - staging-api:     1 -> 6 instances"
echo "    - staging-workers: 0 -> 5 instances"
