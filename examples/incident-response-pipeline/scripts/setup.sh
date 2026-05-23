#!/bin/bash
# Creates API-managed resources: quota, retention policy, recurring action,
# payload templates, and template profiles.
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

# ── Template: alert-slack-body ─────────────────────────────────────────────
echo "Creating template: alert-slack-body..."
curl -sf -X POST "$ACTEON_URL/v1/templates" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "alert-slack-body",
    "namespace": "incidents",
    "tenant": "ops-team",
    "body": "ALERT: {{ severity | upper }} — {{ service }}\n\nEnvironment: {{ environment }}\nAlert ID: {{ alert_id }}\n{% if runbook_url %}Runbook: {{ runbook_url }}{% endif %}\n\nReported by Acteon Incident Pipeline",
    "description": "Slack Block Kit message body for alert notifications"
  }' | jq .
echo ""

# ── Template: alert-email-body ────────────────────────────────────────────
echo "Creating template: alert-email-body..."
curl -sf -X POST "$ACTEON_URL/v1/templates" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "alert-email-body",
    "namespace": "incidents",
    "tenant": "ops-team",
    "body": "<h2 style=\"color: {% if severity == '\''critical'\'' %}#dc2626{% elif severity == '\''high'\'' %}#ea580c{% else %}#2563eb{% endif %}\">\n  {{ severity | upper }}: {{ service }}\n</h2>\n<p>Alert <strong>{{ alert_id }}</strong> fired in <em>{{ environment }}</em>.</p>\n{% if details %}<pre>{{ details }}</pre>{% endif %}\n<hr>\n<small>Acteon Incident Response Pipeline</small>",
    "description": "HTML email body for alert notifications"
  }' | jq .
echo ""

# ── Profile: slack-alert ──────────────────────────────────────────────────
echo "Creating profile: slack-alert..."
curl -sf -X POST "$ACTEON_URL/v1/templates/profiles" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "slack-alert",
    "namespace": "incidents",
    "tenant": "ops-team",
    "fields": {
      "text": {"inline": "[{{ severity | upper }}] {{ service }} — {{ alert_id }}"},
      "body": {"$ref": "alert-slack-body"}
    },
    "description": "Slack alert profile with inline subject and body template"
  }' | jq .
echo ""

# ── Profile: email-alert ─────────────────────────────────────────────────
echo "Creating profile: email-alert..."
curl -sf -X POST "$ACTEON_URL/v1/templates/profiles" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "email-alert",
    "namespace": "incidents",
    "tenant": "ops-team",
    "fields": {
      "subject": {"inline": "[{{ severity | upper }}] Alert: {{ service }}"},
      "html_body": {"$ref": "alert-email-body"},
      "reply_to": {"inline": "ops-team@example.com"}
    },
    "description": "Email alert profile with inline subject and HTML body template"
  }' | jq .
echo ""

# ── Preview: render slack-alert template with sample data ─────────────────
echo "Preview: rendering alert-slack-body with sample data..."
curl -sf -X POST "$ACTEON_URL/v1/templates/render" \
  -H "Content-Type: application/json" \
  -d '{
    "template_name": "alert-slack-body",
    "namespace": "incidents",
    "tenant": "ops-team",
    "variables": {
      "severity": "critical",
      "service": "database",
      "environment": "production",
      "alert_id": "ALERT-001",
      "runbook_url": "https://wiki.example.com/runbooks/db-down"
    }
  }' | jq .
echo ""

echo "Setup complete. Resources created:"
echo "  - Quota: 100/hour for incidents:ops-team"
echo "  - Retention: audit 7d, events 1d for incidents:ops-team"
echo "  - Recurring: health-check every 60s via slack-alerts"
echo "  - Template: alert-slack-body (Slack Block Kit message)"
echo "  - Template: alert-email-body (HTML email body)"
echo "  - Profile: slack-alert (inline text + body ref)"
echo "  - Profile: email-alert (inline subject + html_body ref + reply_to)"
