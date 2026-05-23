# Healthcare Notification Pipeline

A HIPAA-compliant healthcare notification system demonstrating 15 Acteon features. Metro General Hospital sends patient notifications (appointment reminders, lab results, discharge summaries) through multiple channels. HIPAA rules detect and block Protected Health Information (PHI) over insecure channels, while SOC2/HIPAA compliance ensures a tamper-evident hash-chained audit trail of every notification attempt — including blocked ones.

## Features Exercised

| # | Feature | How |
|---|---------|-----|
| 1 | **HIPAA compliance mode** | `mode = "hipaa"` — enables sync writes + immutable audit + hash chain |
| 2 | **Hash chain integrity** | Verified via `POST /v1/audit/verify` in verify-compliance.sh |
| 3 | **Immutable audit records** | HIPAA mode prevents deletion of audit records |
| 4 | **Synchronous audit writes** | Every notification attempt recorded inline before response |
| 5 | **Audit field redaction** | PHI fields (mrn, ssn, diagnosis, medication) redacted in stored payloads |
| 6 | **Suppress** | Block PHI in SMS and unencrypted email |
| 7 | **Reroute** | PHI notifications redirected to secure patient portal |
| 8 | **Request approval** | External PHI sharing requires compliance officer sign-off |
| 9 | **Chains** | discharge-workflow: notify-patient → notify-pcp → schedule-followup |
| 10 | **Deduplication** | Duplicate appointment reminders within 5-minute window |
| 11 | **Grouping** | Routine lab results batched by patient_id (60s window) |
| 12 | **Quotas** | 200 notifications/hour per hospital |
| 13 | **Retention + compliance hold** | 90-day audit, 7-day events, no auto-deletion |
| 14 | **Circuit breaker + fallback** | secure-email → compliance-log on failure |
| 15 | **Modify/enrich** | Add `hipaa_compliant` and `audit_required` metadata to all actions |

## Prerequisites

- PostgreSQL (for durable state + audit with hash chaining)
- `jq` (for script output formatting)

## Quick Start

```bash
# 1. Start PostgreSQL
docker compose --profile postgres up -d

# 2. Run database migrations
scripts/migrate.sh -c examples/healthcare-notification-pipeline/acteon.toml

# 3. Start Acteon with HIPAA compliance mode
cargo run -p acteon-server --features postgres -- \
  -c examples/healthcare-notification-pipeline/acteon.toml

# 4. Setup API resources (retention policy + quota)
cd examples/healthcare-notification-pipeline
bash scripts/setup.sh

# 5. Fire ~21 sample notifications
bash scripts/send-notifications.sh

# 6. Verify HIPAA compliance and hash chain
bash scripts/verify-compliance.sh

# 7. View comprehensive report
bash scripts/show-report.sh
```

## File Structure

```
healthcare-notification-pipeline/
├── acteon.toml              # Server config (HIPAA mode, audit redaction, chains, circuit breakers)
├── rules/
│   ├── hipaa.yaml           # PHI detection: suppress, reroute, require approval
│   ├── routing.yaml         # Dedup, enrich HIPAA headers, group labs, chain discharge
│   └── safety.yaml          # Catch-all deny
├── scripts/
│   ├── setup.sh             # Create retention policy + quota via API
│   ├── send-notifications.sh # Fire ~21 sample notifications
│   ├── verify-compliance.sh # Check HIPAA mode + verify hash chain integrity
│   └── show-report.sh       # Query audit/chains/health/quotas/groups + summary table
└── README.md
```

## Architecture

```
                    ┌──────────────────────────┐
  Hospital EHR ───►│   Acteon Gateway          │
  (appointments,    │                          │
   lab results,     │  HIPAA Rules Engine      │
   discharges)      │  ┌─block PHI in SMS────┐ │     ┌────────────────┐
                    │  ├─block PHI unencrypt──┤ │────►│ secure-email    │──┐ fallback
                    │  ├─approval ext PHI─────┤ │     ├────────────────┤  │
                    │  ├─reroute PHI→portal───┤ │────►│ patient-portal  │  │
                    │  ├─dedup notifications──┤ │     ├────────────────┤  │
                    │  ├─enrich HIPAA hdrs────┤ │────►│ sms-service     │  │
                    │  ├─group routine labs───┤ │     ├────────────────┤  │
                    │  └─chain discharge──────┘ │────►│ pager-service   │  │
                    │                          │     ├────────────────┤  │
                    │  Compliance Layer         │     │ compliance-log  │◄─┘
                    │  ├─hash chain (SHA-256)  │     └────────────────┘
                    │  ├─sync audit writes     │
                    │  ├─immutable records     │
                    │  └─PHI field redaction   │
                    │                          │
                    │  Background Jobs          │
                    │  ├─group flush (60s)     │
                    │  ├─retention reaper      │
                    │  └─timeout processing    │
                    └──────────────────────────┘
```

## Discharge Workflow Chain

The `discharge-workflow` chain handles patient discharge notifications:

```
notify-patient (patient-portal)
    │
    ├─ body.logged == true ──► notify-pcp (secure-email, encrypted)
    │                              │
    │                              ├─ body.logged == true ──► schedule-followup (sms-service)
    │                              │
    │                              └─ (no match) ──► end
    │
    └─ (no match) ──► notify-pcp (next step)
```

## Expected Outcomes from `send-notifications.sh`

| Notifications | Count | Expected Outcome |
|---------------|-------|-----------------|
| Appointment reminders (no PHI) | 3 | `executed` |
| Lab results with PHI via SMS | 3 | `suppressed` (PHI in SMS) |
| Lab results with PHI unencrypted email | 2 | `suppressed` (unencrypted) |
| Lab results with PHI via encrypted email | 2 | `executed` |
| External referral with PHI | 2 | `pending_approval` |
| PHI rerouted to portal | 2 | `rerouted` → patient-portal |
| Routine lab results | 3 | `grouped` by patient_id |
| Discharge summaries | 2 | `chain_started` |
| Duplicate appointment | 2 | 1 `deduplicated` |

## Circuit Breaker Demo

The `secure-email` provider has a circuit breaker configured to trip after 2 failures, falling back to `compliance-log`. Since `secure-email` is a `log` provider in this demo it won't fail, but the configuration demonstrates the pattern for production deployments where the email gateway might be unreachable.

## Notes

- **Log providers** return `{"provider": "<name>", "logged": true}`. Chain branching uses `body.logged == true` as a condition.
- **PHI detection** uses regex patterns for SSN (`\d{3}-\d{2}-\d{4}`), MRN (`MRN:\d{6,}`), and ICD codes (`ICD-\d{1,2}[.\d]*`), plus keyword matching for `diagnosis` and `medication`.
- **Audit redaction** replaces PHI fields (mrn, ssn, date_of_birth, diagnosis, medication, insurance_id) with `[PHI_REDACTED]` in stored payloads. The original data is never persisted.
- **Compliance hold** on the retention policy prevents the retention reaper from auto-deleting audit records, even after the 90-day TTL — ensuring HIPAA-mandated record retention.
- **Quota window** uses `"hourly"` string format. Other options: `"daily"`, `"weekly"`, `"monthly"`.
