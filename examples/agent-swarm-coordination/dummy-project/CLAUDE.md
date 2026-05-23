# Agent Safety Policy

All tool calls in this project are routed through the Acteon gateway for
policy enforcement. Do not attempt to bypass or work around blocked actions.

If an action is blocked by Acteon:
1. Report the block reason to the user
2. Suggest an alternative approach that stays within policy
3. Do not retry the same blocked action

## Constraints

- Never write to .env, .ssh, or credential files
- Never run rm -rf, DROP TABLE, or filesystem format commands
- All git push operations require human approval through Acteon
- All package installs require human approval through Acteon
- Never use curl/wget to send data to external hosts

## Acteon MCP Integration

You have access to the Acteon MCP server. Use it to:
- Query the audit trail to review what actions were allowed or blocked
- List active rules to understand current permissions
- Check circuit breaker status for provider health

When the user asks about Acteon status, use the MCP tools directly
rather than running curl commands.
