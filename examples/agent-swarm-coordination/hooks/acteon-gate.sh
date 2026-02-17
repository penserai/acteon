#!/bin/bash
# PreToolUse hook: routes Claude Code tool calls through Acteon for policy
# enforcement. Reads tool call JSON from stdin, dispatches to Acteon, and
# exits 0 (allow) or 2 (block) based on the outcome.
#
# Environment:
#   ACTEON_URL       - Acteon gateway URL (default: http://localhost:8080)
#   ACTEON_AGENT_KEY  - API key for the claude-code-agent tenant
#   ACTEON_AGENT_ROLE - Agent role identifier (default: "coding")
set -e

ACTEON_URL="${ACTEON_URL:-http://localhost:8080}"
AGENT_ROLE="${ACTEON_AGENT_ROLE:-coding}"

INPUT=$(cat)
TOOL_NAME=$(echo "$INPUT" | jq -r '.tool_name')
TOOL_INPUT=$(echo "$INPUT" | jq -c '.tool_input // {}')
SESSION_ID=$(echo "$INPUT" | jq -r '.session_id')

# ── Map tool names to action types ─────────────────────────────────────────
case "$TOOL_NAME" in
  Bash)
    ACTION_TYPE="execute_command"
    ;;
  Write)
    ACTION_TYPE="write_file"
    ;;
  Edit)
    ACTION_TYPE="write_file"
    ;;
  WebFetch|WebSearch)
    ACTION_TYPE="web_access"
    ;;
  Task)
    ACTION_TYPE="spawn_agent"
    ;;
  *)
    # Read, Grep, Glob, and other read-only tools pass through without checking
    exit 0
    ;;
esac

# ── Build dedup key ────────────────────────────────────────────────────────
# For write_file actions, use a file-path-based key so cross-session writes
# to the same file are deduplicated. Other actions use session-scoped keys.
if [ "$ACTION_TYPE" = "write_file" ]; then
  FILE_PATH=$(echo "$TOOL_INPUT" | jq -r '.file_path // .path // ""')
  if [ -n "$FILE_PATH" ]; then
    FILE_PATH_HASH=$(echo -n "$FILE_PATH" | md5sum 2>/dev/null | cut -d' ' -f1 || echo -n "$FILE_PATH" | md5 2>/dev/null)
    DEDUP_KEY="write-${FILE_PATH_HASH:-none}"
  else
    DEDUP_HASH=$(echo -n "$TOOL_INPUT" | md5sum 2>/dev/null | cut -d' ' -f1 || echo -n "$TOOL_INPUT" | md5 2>/dev/null)
    DEDUP_KEY="$SESSION_ID-$ACTION_TYPE-${DEDUP_HASH:-none}"
  fi
else
  DEDUP_HASH=$(echo -n "$TOOL_INPUT" | md5sum 2>/dev/null | cut -d' ' -f1 || echo -n "$TOOL_INPUT" | md5 2>/dev/null)
  DEDUP_KEY="$SESSION_ID-$ACTION_TYPE-${DEDUP_HASH:-none}"
fi

# ── Generate action ID and timestamp ─────────────────────────────────────
ACTION_ID=$(uuidgen | tr '[:upper:]' '[:lower:]')
CREATED_AT=$(date -u +"%Y-%m-%dT%H:%M:%SZ")

# ── Dispatch to Acteon ─────────────────────────────────────────────────────
RESPONSE=$(curl -s -w "\n%{http_code}" -X POST "$ACTEON_URL/v1/dispatch" \
  -H "Content-Type: application/json" \
  ${ACTEON_AGENT_KEY:+-H "Authorization: Bearer $ACTEON_AGENT_KEY"} \
  -d "{
    \"id\": \"$ACTION_ID\",
    \"namespace\": \"agent-swarm\",
    \"tenant\": \"claude-code-agent\",
    \"provider\": \"claude-code\",
    \"action_type\": \"$ACTION_TYPE\",
    \"payload\": $TOOL_INPUT,
    \"metadata\": {
      \"tool_name\": \"$TOOL_NAME\",
      \"session_id\": \"$SESSION_ID\",
      \"agent_role\": \"$AGENT_ROLE\"
    },
    \"created_at\": \"$CREATED_AT\",
    \"dedup_key\": \"$DEDUP_KEY\"
  }" 2>/dev/null) || {
    # Fail closed: if Acteon is unreachable, block the action
    echo "Acteon gateway unreachable at $ACTEON_URL -- blocking for safety" >&2
    exit 2
  }

# ── Parse response ─────────────────────────────────────────────────────────
HTTP_CODE=$(echo "$RESPONSE" | tail -1)
BODY=$(echo "$RESPONSE" | sed '$d')

# Non-2xx from Acteon: block
if [[ "$HTTP_CODE" -lt 200 || "$HTTP_CODE" -ge 300 ]]; then
  echo "Acteon returned HTTP $HTTP_CODE -- blocking action" >&2
  exit 2
fi

# Response is a Rust enum: {"Executed":{...}}, {"Suppressed":{...}}, etc.
OUTCOME=$(echo "$BODY" | jq -r 'keys[0]')

case "$OUTCOME" in
  Executed|Deduplicated)
    # Action permitted
    exit 0
    ;;
  PendingApproval)
    APPROVAL_ID=$(echo "$BODY" | jq -r '.PendingApproval.approval_id // "unknown"')
    APPROVE_URL=$(echo "$BODY" | jq -r '.PendingApproval.approve_url // ""')
    echo "Action held for human approval (ID: $APPROVAL_ID)" >&2
    if [ -n "$APPROVE_URL" ]; then
      echo "Approve: $APPROVE_URL" >&2
    fi
    echo "Waiting for approval -- the action has been paused." >&2
    exit 2
    ;;
  Suppressed)
    RULE=$(echo "$BODY" | jq -r '.Suppressed.rule // "unknown rule"')
    echo "BLOCKED by Acteon rule '$RULE'" >&2
    exit 2
    ;;
  Throttled)
    RETRY=$(echo "$BODY" | jq -r '.Throttled.retry_after.secs // "unknown"')
    echo "Rate limited -- retry after ${RETRY}s" >&2
    exit 2
    ;;
  Rerouted)
    TARGET=$(echo "$BODY" | jq -r '.Rerouted.target_provider // "unknown"')
    echo "Action rerouted to '$TARGET'" >&2
    exit 0
    ;;
  QuotaExceeded)
    echo "Tenant quota exceeded -- action blocked" >&2
    exit 2
    ;;
  *)
    echo "Unexpected Acteon outcome: $OUTCOME -- blocking for safety" >&2
    exit 2
    ;;
esac
