#!/bin/bash
# Creates LocalStack Auto Scaling Groups for the cost optimizer demo.
#
# Prerequisites:
#   - LocalStack running: docker run --rm -d --name localstack -p 4566:4566 localstack/localstack
#   - awslocal CLI: pip install awscli-local
#
# Usage: bash examples/aws-cost-optimizer/scripts/setup.sh
set -euo pipefail

ENDPOINT="http://localhost:4566"
REGION="us-east-1"

export AWS_DEFAULT_REGION="$REGION"
export AWS_ACCESS_KEY_ID="test"
export AWS_SECRET_ACCESS_KEY="test"

echo "=== AWS Cost Optimizer: LocalStack Setup ==="
echo ""

# ── Launch Configuration (required to create ASGs) ──────────────────────────
echo "Creating launch configuration: cost-opt-lc..."
awslocal autoscaling create-launch-configuration \
  --launch-configuration-name cost-opt-lc \
  --image-id ami-12345678 \
  --instance-type t3.medium 2>/dev/null || echo "  (may already exist)"
echo ""

# ── Auto Scaling Group: staging-web ─────────────────────────────────────────
echo "Creating ASG: staging-web (min=1, max=6, desired=4)..."
awslocal autoscaling create-auto-scaling-group \
  --auto-scaling-group-name staging-web \
  --launch-configuration-name cost-opt-lc \
  --min-size 1 \
  --max-size 6 \
  --desired-capacity 4 \
  --availability-zones "${REGION}a" 2>/dev/null || echo "  (may already exist)"
echo ""

# ── Auto Scaling Group: staging-api ─────────────────────────────────────────
echo "Creating ASG: staging-api (min=1, max=8, desired=6)..."
awslocal autoscaling create-auto-scaling-group \
  --auto-scaling-group-name staging-api \
  --launch-configuration-name cost-opt-lc \
  --min-size 1 \
  --max-size 8 \
  --desired-capacity 6 \
  --availability-zones "${REGION}a" 2>/dev/null || echo "  (may already exist)"
echo ""

# ── Auto Scaling Group: staging-workers ─────────────────────────────────────
echo "Creating ASG: staging-workers (min=0, max=10, desired=5)..."
awslocal autoscaling create-auto-scaling-group \
  --auto-scaling-group-name staging-workers \
  --launch-configuration-name cost-opt-lc \
  --min-size 0 \
  --max-size 10 \
  --desired-capacity 5 \
  --availability-zones "${REGION}a" 2>/dev/null || echo "  (may already exist)"
echo ""

# ── Verify ──────────────────────────────────────────────────────────────────
echo "Verifying Auto Scaling Groups..."
awslocal autoscaling describe-auto-scaling-groups \
  --auto-scaling-group-names staging-web staging-api staging-workers \
  | jq '.AutoScalingGroups[] | {Name: .AutoScalingGroupName, Min: .MinSize, Max: .MaxSize, Desired: .DesiredCapacity}'
echo ""

echo "=== LocalStack setup complete ==="
echo ""
echo "Resources created:"
echo "  - Launch config: cost-opt-lc (t3.medium)"
echo "  - ASG: staging-web       (desired=4, min=1, max=6)"
echo "  - ASG: staging-api       (desired=6, min=1, max=8)"
echo "  - ASG: staging-workers   (desired=5, min=0, max=10)"
echo ""
echo "Total daytime capacity: 15 instances"
echo "Off-hours capacity:      2 instances (web=1, api=1, workers=0)"
echo "Estimated savings:      ~87% overnight compute reduction"
