#!/bin/bash
# Fires ~21 sample healthcare notifications exercising all Acteon features.
#
# Categories:
#   3 appointment reminders (no PHI)  → executed
#   3 lab results with PHI via SMS    → suppressed (PHI in SMS)
#   2 lab results with PHI unencrypted email → suppressed (unencrypted)
#   2 lab results with PHI encrypted email   → executed
#   2 external referral with PHI      → pending_approval
#   2 PHI rerouted to portal          → rerouted → patient-portal
#   3 routine lab results             → grouped by patient_id
#   2 discharge summaries             → chain_started
#   2 duplicate appointment reminder  → 1 deduplicated
#
# Usage: bash scripts/send-notifications.sh
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
  OUTCOME=$(echo "$RESPONSE" | jq -r 'if type == "object" then keys[0] else . end // "unknown"' 2>/dev/null || echo "unknown")
  echo "$OUTCOME"
}

CREATED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

echo "=== Healthcare Notification Pipeline: Sending Notifications ==="
echo ""

# ── Appointment reminders (3) — no PHI, email → executed ───────────────────
echo "Appointment reminders (no PHI, email):"
dispatch "appt-reminder-001" '{
  "id": "appt-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-1001", "body": "Reminder: Your appointment is scheduled for March 5 at 2:00 PM with Dr. Smith.", "subject": "Appointment Reminder", "encrypted": true},
  "metadata": {"department": "scheduling"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "appt-reminder-002" '{
  "id": "appt-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-1002", "body": "Reminder: Your follow-up visit is on March 8 at 10:00 AM.", "subject": "Appointment Reminder", "encrypted": true},
  "metadata": {"department": "scheduling"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "appt-reminder-003" '{
  "id": "appt-003",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-1003", "body": "Reminder: Annual physical scheduled for March 12 at 9:00 AM.", "subject": "Appointment Reminder", "encrypted": true},
  "metadata": {"department": "scheduling"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Lab results with PHI via SMS (3) → suppressed ─────────────────────────
echo "Lab results with PHI via SMS (should be suppressed):"
dispatch "phi-sms-001" '{
  "id": "phi-sms-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "sms-service",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-2001", "body": "Lab results for MRN: 123456. diagnosis code ICD-10 E11.9. Please review.", "mrn": "123456"},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "phi-sms-002" '{
  "id": "phi-sms-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "sms-service",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-2002", "body": "Your diagnosis has been updated. Contact your provider for medication details.", "ssn": "987-65-4321"},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "phi-sms-003" '{
  "id": "phi-sms-003",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "sms-service",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-2003", "body": "New lab results available. Patient SSN 123-45-6789 on file. Review portal.", "ssn": "123-45-6789"},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Lab results with PHI via unencrypted email (2) → suppressed ────────────
echo "Lab results with PHI via unencrypted email (should be suppressed):"
dispatch "phi-email-unenc-001" '{
  "id": "phi-email-unenc-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-3001", "body": "Lab results: MRN: 654321. ICD-10 J45.20 diagnosis confirmed.", "encrypted": false, "mrn": "654321"},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "phi-email-unenc-002" '{
  "id": "phi-email-unenc-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-3002", "body": "Updated diagnosis and medication plan attached. Review immediately.", "encrypted": false, "diagnosis": "Type 2 Diabetes"},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Lab results with PHI via encrypted email (2) → executed ────────────────
echo "Lab results with PHI via encrypted email (should execute):"
dispatch "phi-email-enc-001" '{
  "id": "phi-email-enc-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-4001", "body": "Your lab results are ready. Please check your patient portal for details.", "subject": "Lab Results Available", "encrypted": true},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "phi-email-enc-002" '{
  "id": "phi-email-enc-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-4002", "body": "Updated results from your recent visit are available in the portal.", "subject": "Updated Results", "encrypted": true},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── External referral with PHI (2) → pending_approval ─────────────────────
echo "External referral with PHI (should require approval):"
dispatch "ext-phi-001" '{
  "id": "ext-phi-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "referral",
  "payload": {"patient_id": "PT-5001", "body": "Referral for specialist consultation. Patient MRN: 789012. diagnosis: chronic condition.", "recipient_type": "external", "encrypted": true, "mrn": "789012"},
  "metadata": {"department": "referrals"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "ext-phi-002" '{
  "id": "ext-phi-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "referral",
  "payload": {"patient_id": "PT-5002", "body": "Transfer records for patient SSN 234-56-7890. diagnosis review needed.", "recipient_type": "external", "encrypted": true, "ssn": "234-56-7890"},
  "metadata": {"department": "referrals"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── PHI rerouted to patient portal (2) → rerouted ─────────────────────────
echo "PHI notifications rerouted to portal (should be rerouted):"
dispatch "reroute-001" '{
  "id": "reroute-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "sms-service",
  "action_type": "notification",
  "payload": {"patient_id": "PT-6001", "body": "Your lab result is ready. View your complete medical record in the portal."},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "reroute-002" '{
  "id": "reroute-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-6002", "body": "Your latest test result has been posted. Check the portal for details."},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Routine lab results (3) → grouped by patient_id ───────────────────────
echo "Routine lab results (should be grouped):"
dispatch "routine-lab-001" '{
  "id": "routine-lab-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "patient-portal",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-7001", "test_name": "CBC", "priority": "routine", "body": "Complete blood count results available."},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "routine-lab-002" '{
  "id": "routine-lab-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "patient-portal",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-7001", "test_name": "BMP", "priority": "routine", "body": "Basic metabolic panel results available."},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "routine-lab-003" '{
  "id": "routine-lab-003",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "patient-portal",
  "action_type": "lab_result",
  "payload": {"patient_id": "PT-7002", "test_name": "Lipid Panel", "priority": "routine", "body": "Lipid panel results available."},
  "metadata": {"department": "lab"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Discharge summaries (2) → chain_started ───────────────────────────────
echo "Discharge summaries (should start chain):"
dispatch "discharge-001" '{
  "id": "discharge-001",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "patient-portal",
  "action_type": "discharge_summary",
  "payload": {"patient_id": "PT-8001", "discharge_date": "2026-02-17", "pcp_name": "Dr. Johnson", "body": "Patient discharged with follow-up instructions."},
  "metadata": {"department": "discharge"},
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "discharge-002" '{
  "id": "discharge-002",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "patient-portal",
  "action_type": "discharge_summary",
  "payload": {"patient_id": "PT-8002", "discharge_date": "2026-02-17", "pcp_name": "Dr. Williams", "body": "Patient discharged after observation period."},
  "metadata": {"department": "discharge"},
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

# ── Duplicate appointment reminders (2) → 1 deduplicated ─────────────────
echo "Duplicate appointment reminders (second should be deduplicated):"
dispatch "dup-appt-001a" '{
  "id": "dup-appt-001a",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-9001", "body": "Reminder: Your appointment is on March 15 at 3:00 PM.", "subject": "Appointment Reminder", "encrypted": true},
  "metadata": {"department": "scheduling"},
  "dedup_key": "appt-reminder-PT-9001-march15",
  "created_at": "'"$CREATED_AT"'"
}'

dispatch "dup-appt-001b" '{
  "id": "dup-appt-001b",
  "namespace": "healthcare",
  "tenant": "metro-hospital",
  "provider": "secure-email",
  "action_type": "notification",
  "payload": {"patient_id": "PT-9001", "body": "Reminder: Your appointment is on March 15 at 3:00 PM.", "subject": "Appointment Reminder", "encrypted": true},
  "metadata": {"department": "scheduling"},
  "dedup_key": "appt-reminder-PT-9001-march15",
  "created_at": "'"$CREATED_AT"'"
}'
echo ""

echo "=== Done: ~21 notifications dispatched ==="
echo ""
echo "Expected outcomes:"
echo "  - 6 executed (3 appointment reminders + 2 encrypted email + 1 first dedup)"
echo "  - 3 suppressed (PHI in SMS)"
echo "  - 2 suppressed (PHI in unencrypted email)"
echo "  - 2 pending_approval (external PHI referral)"
echo "  - 2 rerouted to patient-portal"
echo "  - 3 grouped (routine lab results by patient_id)"
echo "  - 2 chain_started (discharge workflows)"
echo "  - 1 deduplicated (duplicate appointment reminder)"
echo ""
echo "Run 'bash scripts/verify-compliance.sh' to check HIPAA compliance."
echo "Run 'bash scripts/show-report.sh' to see full results."
