#!/bin/bash
# Stop hook: sends a Discord notification via Acteon when the Claude Code
# session ends. The notification includes session metadata.
#
# Environment:
#   ACTEON_URL       - Acteon gateway URL (default: http://localhost:8080)
#   ACTEON_AGENT_KEY  - API key for the claude-code-agent tenant
#   ACTEON_AGENT_ROLE - Agent role identifier (default: "coding")
set -e

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
AGENT_ROLE="${ACTEON_AGENT_ROLE:-coding}"

INPUT=$(cat)
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')
TIMESTAMP=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
ACTION_ID=$(uuidgen | tr '[:upper:]' '[:lower:]')

# ── Send completion notification via Acteon (Discord provider) ─────────────
curl -s -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  ${ACTEON_AGENT_KEY:+-H "Authorization: Bearer $ACTEON_AGENT_KEY"} \
  -d "{
    \"id\": \"$ACTION_ID\",
    \"namespace\": \"agent-swarm\",
    \"tenant\": \"claude-code-agent\",
    \"provider\": \"discord\",
    \"action_type\": \"session_complete\",
    \"created_at\": \"$TIMESTAMP\",
    \"payload\": {
      \"content\": \"Claude Code session completed.\",
      \"embeds\": [{
        \"title\": \"Agent Session Complete\",
        \"color\": 5763719,
        \"fields\": [
          {\"name\": \"Session\", \"value\": \"$SESSION_ID\", \"inline\": true},
          {\"name\": \"Timestamp\", \"value\": \"$TIMESTAMP\", \"inline\": true},
          {\"name\": \"Agent\", \"value\": \"claude-code-agent\", \"inline\": true},
          {\"name\": \"Role\", \"value\": \"$AGENT_ROLE\", \"inline\": true}
        ],
        \"footer\": {\"text\": \"Acteon Agent Swarm\"}
      }]
    },
    \"metadata\": {
      \"session_id\": \"$SESSION_ID\",
      \"agent_role\": \"$AGENT_ROLE\",
      \"event\": \"session_complete\"
    },
    \"dedup_key\": \"session-complete-$SESSION_ID\"
  }" > /dev/null 2>&1 || true

# Always allow the session to end (don't block on notification failure)
exit 0
