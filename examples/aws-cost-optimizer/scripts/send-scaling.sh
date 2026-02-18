#!/bin/bash
# Sends manual scaling actions to demonstrate the cost optimizer pipeline.
#
# These actions simulate what the recurring cron jobs dispatch automatically.
# Useful for testing without waiting for cron triggers.
#
# Usage: bash examples/aws-cost-optimizer/scripts/send-scaling.sh
# Environment:
#   ACTEON_URL - Acteon gateway URL (default: http://localhost:8080)
set -euo pipefail

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"

echo "=== AWS Cost Optimizer: Manual Scaling Actions ==="
echo ""

# Helper: dispatch an action and show the result.
dispatch() {
  local desc="$1"
  local payload="$2"

  echo ">>> $desc"
  curl -sf -X POST "$ACTEON_URL/v1/dispatch" \
    -H "Content-Type: application/json" \
    -d "$payload" | jq '{action_id, outcome}'
  echo ""
}

# ── Scale-Down Actions (simulate 7pm off-hours) ─────────────────────────────
echo "--- Scale-Down (off-hours) ---"
echo ""

dispatch "Scale down staging-web to 1 instance" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "set_desired_capacity",
  "payload": {
    "auto_scaling_group_name": "staging-web",
    "desired_capacity": 1,
    "honor_cooldown": false,
    "tag": "scale-down"
  }
}'

dispatch "Scale down staging-api to 1 instance" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "set_desired_capacity",
  "payload": {
    "auto_scaling_group_name": "staging-api",
    "desired_capacity": 1,
    "honor_cooldown": false,
    "tag": "scale-down"
  }
}'

dispatch "Scale down staging-workers to 0 instances" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "set_desired_capacity",
  "payload": {
    "auto_scaling_group_name": "staging-workers",
    "desired_capacity": 0,
    "honor_cooldown": false,
    "tag": "scale-down"
  }
}'

# ── Describe: check current state ───────────────────────────────────────────
echo "--- Verify Current State ---"
echo ""

dispatch "Describe all staging ASGs" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "describe_auto_scaling_groups",
  "payload": {
    "auto_scaling_group_names": ["staging-web", "staging-api", "staging-workers"]
  }
}'

# ── Scale-Up Actions (simulate 7am morning) ─────────────────────────────────
echo "--- Scale-Up (morning) ---"
echo ""

dispatch "Scale up staging-web to 4 instances" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "set_desired_capacity",
  "payload": {
    "auto_scaling_group_name": "staging-web",
    "desired_capacity": 4,
    "honor_cooldown": false,
    "tag": "scale-up"
  }
}'

dispatch "Scale up staging-api to 6 instances" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "set_desired_capacity",
  "payload": {
    "auto_scaling_group_name": "staging-api",
    "desired_capacity": 6,
    "honor_cooldown": false,
    "tag": "scale-up"
  }
}'

dispatch "Scale up staging-workers to 5 instances" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "set_desired_capacity",
  "payload": {
    "auto_scaling_group_name": "staging-workers",
    "desired_capacity": 5,
    "honor_cooldown": false,
    "tag": "scale-up"
  }
}'

# ── Update ASG config ───────────────────────────────────────────────────────
echo "--- Update ASG Config ---"
echo ""

dispatch "Update staging-workers max to 12 and health check" '{
  "namespace": "infra",
  "tenant": "cost-optimizer",
  "provider": "cost-asg",
  "action_type": "update_auto_scaling_group",
  "payload": {
    "auto_scaling_group_name": "staging-workers",
    "max_size": 12,
    "health_check_type": "ELB",
    "health_check_grace_period": 120
  }
}'

echo "=== Manual scaling actions complete ==="
echo ""
echo "Actions sent:"
echo "  - 3 scale-down actions (simulate off-hours)"
echo "  - 1 describe action (verify state)"
echo "  - 3 scale-up actions (simulate morning)"
echo "  - 1 update ASG config"
echo ""
echo "Run show-report.sh to see the audit trail."
