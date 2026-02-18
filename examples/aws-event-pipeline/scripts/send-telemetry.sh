#!/bin/bash
# Fires ~20 sample telemetry events exercising all Acteon features with AWS providers.
#
# Categories:
#   2 critical temp    → rerouted to SNS (>85°C threshold)
#   2 intrusion        → rerouted to SNS (motion sensor)
#   3 normal temp      → chain: normalize → detect → archive
#   3 humidity         → grouped by floor, 30s batch
#   2 energy           → scheduled with 60s delay
#   3 rapid-fire       → hits throttle at 30/min
#   2 duplicates       → same dedup_key, deduplicated
#   2 test devices     → environment=test, suppressed
#   1 device lifecycle → rerouted to EventBridge
#   1 unsigned firmware → denied
#
# Usage: bash examples/aws-event-pipeline/scripts/send-telemetry.sh
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

echo "=== AWS Event Pipeline: Sending Telemetry ==="
echo ""

# ── Critical temperature (2) → rerouted to SNS ────────────────────────────
echo "Critical temperature alerts (SNS fan-out):"
dispatch "temp-crit-floor3" '{
  "id": "temp-crit-001",
  "namespace": "iot",
  "tenant": "smartbuilding-hq",
  "provider": "metrics-queue",
  "action_type": "sensor_reading",
  "payload": {"device_id": "temp-sensor-301", "sensor_type": "temperature", "value": 92, "unit": "celsius", "floor": "3", "zone": "server-room"},
  "metadata": {"source": "building-iot"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "temp-crit-floor5" '{
  "id": "temp-crit-002",
  "namespace": "iot",
  "tenant": "smartbuilding-hq",
  "provider": "metrics-queue",
  "action_type": "sensor_reading",
  "payload": {"device_id": "temp-sensor-501", "sensor_type": "temperature", "value": 88, "unit": "celsius", "floor": "5", "zone": "electrical-room"},
  "metadata": {"source": "building-iot"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Intrusion events (2) → rerouted to SNS ────────────────────────────────
echo "Intrusion alerts (SNS fan-out):"
dispatch "motion-lobby-001" '{
  "id": "motion-001",
  "namespace": "iot",
  "tenant": "smartbuilding-hq",
  "provider": "alert-fanout",
  "action_type": "sensor_reading",
  "payload": {"device_id": "motion-sensor-101", "sensor_type": "motion", "event": "intrusion", "floor": "1", "zone": "lobby"},
  "metadata": {"source": "security-system"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "motion-parking-002" '{
  "id": "motion-002",
  "namespace": "iot",
  "tenant": "smartbuilding-hq",
  "provider": "alert-fanout",
  "action_type": "sensor_reading",
  "payload": {"device_id": "motion-sensor-B01", "sensor_type": "motion", "event": "intrusion", "floor": "B1", "zone": "parking"},
  "metadata": {"source": "security-system"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Normal temperature readings (3) → telemetry processing chain ──────────
echo "Normal temperature readings (chain):"
for i in 1 2 3; do
  dispatch "temp-normal-$i" '{
    "id": "temp-norm-00'"$i"'",
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "provider": "metrics-queue",
    "action_type": "sensor_reading",
    "payload": {"device_id": "temp-sensor-20'"$i"'", "sensor_type": "temperature", "value": '"$((20 + i))"', "unit": "celsius", "floor": "2", "zone": "office"},
    "metadata": {"source": "building-iot"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Humidity readings (3) → grouped by floor, 30s batch ───────────────────
echo "Humidity readings (grouped by floor):"
FLOORS=("2" "3" "2")
for i in 0 1 2; do
  FLOOR="${FLOORS[$i]}"
  dispatch "humidity-floor$FLOOR-$i" '{
    "id": "humid-00'"$i"'",
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "provider": "metrics-queue",
    "action_type": "sensor_reading",
    "payload": {"device_id": "humid-sensor-'"$FLOOR"'0'"$i"'", "sensor_type": "humidity", "value": '"$((45 + i * 5))"', "unit": "percent", "floor": "'"$FLOOR"'", "zone": "hvac"},
    "metadata": {"source": "building-iot"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Energy readings (2) → scheduled with 60s delay ────────────────────────
echo "Energy readings (scheduled, 60s delay):"
for i in 1 2; do
  dispatch "energy-meter-$i" '{
    "id": "energy-00'"$i"'",
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "provider": "metrics-queue",
    "action_type": "sensor_reading",
    "payload": {"device_id": "energy-meter-'"$i"'", "sensor_type": "energy", "value": '"$((250 + i * 50))"', "unit": "kwh", "floor": "'"$i"'", "zone": "main-panel"},
    "metadata": {"source": "building-iot"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Rapid-fire readings (3) → hits throttle limit ─────────────────────────
echo "Rapid-fire readings (throttle test):"
for i in $(seq 1 3); do
  dispatch "rapid-$i" '{
    "id": "rapid-'"$i"'",
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "provider": "metrics-queue",
    "action_type": "sensor_reading",
    "payload": {"device_id": "temp-sensor-999", "sensor_type": "temperature", "value": 22, "unit": "celsius", "floor": "1", "zone": "corridor"},
    "metadata": {"source": "rapid-test"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Duplicate readings (2) → same dedup_key, deduplicated ─────────────────
echo "Duplicate readings (dedup test):"
for i in 1 2; do
  dispatch "dup-$i" '{
    "id": "dup-temp-00'"$i"'",
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "provider": "metrics-queue",
    "action_type": "sensor_reading",
    "payload": {"device_id": "temp-sensor-400", "sensor_type": "temperature", "value": 25, "unit": "celsius", "floor": "4", "zone": "lab"},
    "metadata": {"source": "building-iot"},
    "dedup_key": "temp-sensor-400-reading",
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Test device readings (2) → suppressed ─────────────────────────────────
echo "Test device readings (suppressed):"
for i in 1 2; do
  dispatch "test-dev-$i" '{
    "id": "test-dev-00'"$i"'",
    "namespace": "iot",
    "tenant": "smartbuilding-hq",
    "provider": "metrics-queue",
    "action_type": "sensor_reading",
    "payload": {"device_id": "test-sensor-00'"$i"'", "sensor_type": "temperature", "value": 99, "unit": "celsius", "floor": "0", "zone": "lab", "environment": "test"},
    "metadata": {"source": "test-harness"},
    "created_at": "'"$CREATED_AT"'"
  }'
done
echo ""

# ── Device lifecycle event (1) → rerouted to EventBridge ──────────────────
echo "Device lifecycle event (EventBridge):"
dispatch "device-online-001" '{
  "id": "lifecycle-001",
  "namespace": "iot",
  "tenant": "smartbuilding-hq",
  "provider": "event-bus",
  "action_type": "device_lifecycle",
  "payload": {"device_id": "temp-sensor-301", "event": "online", "firmware_version": "3.2.1", "uptime_seconds": 0},
  "metadata": {"source": "device-manager"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Unsigned firmware update (1) → denied ──────────────────────────────────
echo "Unsigned firmware update (denied):"
dispatch "firmware-unsigned-001" '{
  "id": "firmware-001",
  "namespace": "iot",
  "tenant": "smartbuilding-hq",
  "provider": "metrics-queue",
  "action_type": "firmware_update",
  "payload": {"device_id": "temp-sensor-301", "version": "3.3.0", "signed": false, "size_bytes": 1048576},
  "metadata": {"source": "ota-service"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

echo "=== Done: ~21 telemetry events dispatched ==="
echo ""
echo "Expected outcomes:"
echo "  - 2 suppressed (test devices)"
echo "  - 1 denied (unsigned firmware)"
echo "  - 1 deduplicated (same dedup_key)"
echo "  - 3 grouped (humidity, batched by floor 30s)"
echo "  - 2 scheduled (energy, 60s delay)"
echo "  - Some throttled (if >30/min reached)"
echo "  - 2 rerouted to SNS (critical temperature)"
echo "  - 2 rerouted to SNS (intrusion)"
echo "  - 3+ chain_started (normal sensor readings)"
echo "  - 1 rerouted to EventBridge (device lifecycle)"
echo ""
echo "Run 'bash examples/aws-event-pipeline/scripts/show-report.sh' to see results."
