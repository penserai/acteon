#!/bin/bash
# Creates LocalStack AWS resources: SNS topic, Lambda functions, EventBridge bus, SQS queues.
#
# Prerequisites:
#   - LocalStack running: docker run --rm -d --name localstack -p 4566:4566 localstack/localstack
#   - awslocal CLI: pip install awscli-local
#
# Usage: bash examples/aws-event-pipeline/scripts/setup.sh
set -euo pipefail

ENDPOINT="http://localhost:4566"
REGION="us-east-1"
ACCOUNT="000000000000"

export AWS_DEFAULT_REGION="$REGION"
export AWS_ACCESS_KEY_ID="test"
export AWS_SECRET_ACCESS_KEY="test"

echo "=== AWS Event Pipeline: LocalStack Setup ==="
echo ""

# ── SNS Topic ──────────────────────────────────────────────────────────────
echo "Creating SNS topic: building-alerts..."
awslocal sns create-topic --name building-alerts | jq .
echo ""

# ── SQS Queues ─────────────────────────────────────────────────────────────
echo "Creating SQS queue: telemetry-metrics..."
awslocal sqs create-queue --queue-name telemetry-metrics | jq .
echo ""

echo "Creating SQS dead-letter queue: telemetry-dlq..."
awslocal sqs create-queue --queue-name telemetry-dlq | jq .
echo ""

# ── EventBridge Bus ────────────────────────────────────────────────────────
echo "Creating EventBridge bus: building-events..."
awslocal events create-event-bus --name building-events | jq .
echo ""

# ── Lambda Functions ───────────────────────────────────────────────────────
# Create a temporary directory for the mock Lambda code
TMPDIR=$(mktemp -d)
trap 'rm -rf "$TMPDIR"' EXIT

# Mock anomaly detector: returns anomaly=true for values > 85
cat > "$TMPDIR/anomaly_detector.py" << 'PYEOF'
import json

def handler(event, context):
    value = float(event.get("value", 0))
    sensor_type = event.get("sensor_type", "unknown")
    anomaly = False
    reason = "normal"

    if sensor_type == "temperature" and value > 85:
        anomaly = True
        reason = "temperature_critical"
    elif sensor_type == "humidity" and (value < 20 or value > 80):
        anomaly = True
        reason = "humidity_out_of_range"
    elif sensor_type == "energy" and value > 1000:
        anomaly = True
        reason = "energy_spike"

    return {
        "statusCode": 200,
        "body": json.dumps({
            "anomaly": anomaly,
            "reason": reason,
            "device_id": event.get("device_id"),
            "sensor_type": sensor_type,
            "value": value
        })
    }
PYEOF

# Mock telemetry normalizer: adds timestamp and standardizes units
cat > "$TMPDIR/normalizer.py" << 'PYEOF'
import json
from datetime import datetime, timezone

UNIT_MAP = {
    "temperature": "celsius",
    "humidity": "percent",
    "energy": "kwh",
    "motion": "binary"
}

def handler(event, context):
    sensor_type = event.get("sensor_type", "unknown")
    return {
        "statusCode": 200,
        "body": json.dumps({
            "device_id": event.get("device_id"),
            "sensor_type": sensor_type,
            "value": event.get("value"),
            "unit": UNIT_MAP.get(sensor_type, event.get("unit", "unknown")),
            "normalized_at": datetime.now(timezone.utc).isoformat(),
            "schema_version": "2.0"
        })
    }
PYEOF

echo "Creating Lambda function: anomaly-detector..."
cd "$TMPDIR" && zip -q anomaly_detector.zip anomaly_detector.py && cd -
awslocal lambda create-function \
  --function-name anomaly-detector \
  --runtime python3.12 \
  --handler anomaly_detector.handler \
  --role "arn:aws:iam::${ACCOUNT}:role/lambda-role" \
  --zip-file "fileb://${TMPDIR}/anomaly_detector.zip" \
  --timeout 30 | jq '{FunctionName, Runtime, State}'
echo ""

echo "Creating Lambda function: telemetry-normalizer..."
cd "$TMPDIR" && zip -q normalizer.zip normalizer.py && cd -
awslocal lambda create-function \
  --function-name telemetry-normalizer \
  --runtime python3.12 \
  --handler normalizer.handler \
  --role "arn:aws:iam::${ACCOUNT}:role/lambda-role" \
  --zip-file "fileb://${TMPDIR}/normalizer.zip" \
  --timeout 30 | jq '{FunctionName, Runtime, State}'
echo ""

# ── DynamoDB Tables ────────────────────────────────────────────────────────
echo "Creating DynamoDB table: acteon_state..."
awslocal dynamodb create-table \
  --table-name acteon_state \
  --attribute-definitions AttributeName=pk,AttributeType=S AttributeName=sk,AttributeType=S \
  --key-schema AttributeName=pk,KeyType=HASH AttributeName=sk,KeyType=RANGE \
  --billing-mode PAY_PER_REQUEST 2>/dev/null | jq '{TableName: .TableDescription.TableName, Status: .TableDescription.TableStatus}' || echo "  (table may already exist)"
echo ""

echo "Creating DynamoDB table: acteon_audit..."
awslocal dynamodb create-table \
  --table-name acteon_audit \
  --attribute-definitions AttributeName=pk,AttributeType=S AttributeName=sk,AttributeType=S \
  --key-schema AttributeName=pk,KeyType=HASH AttributeName=sk,KeyType=RANGE \
  --billing-mode PAY_PER_REQUEST 2>/dev/null | jq '{TableName: .TableDescription.TableName, Status: .TableDescription.TableStatus}' || echo "  (table may already exist)"
echo ""

echo "=== LocalStack setup complete ==="
echo ""
echo "Resources created:"
echo "  - SNS topic: building-alerts"
echo "  - Lambda: anomaly-detector (Python 3.12)"
echo "  - Lambda: telemetry-normalizer (Python 3.12)"
echo "  - EventBridge bus: building-events"
echo "  - SQS queue: telemetry-metrics"
echo "  - SQS queue: telemetry-dlq (dead-letter)"
echo "  - DynamoDB table: acteon_state"
echo "  - DynamoDB table: acteon_audit"
