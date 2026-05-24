# Agent operator lifecycle

Every bus-resident agent carries two orthogonal state dimensions:

| Dimension | Source | Question it answers |
|-----------|--------|---------------------|
| **`AgentStatus`** | Heartbeat freshness | *Is the agent alive?* (`Online` / `Idle` / `Dead` / `Unknown`) |
| **`AgentAdminState`** | Operator-set | *Is the agent allowed?* (`Active` / `Suspended` / `Banned`) |

A `Banned` agent that is still heartbeating reads as `Online` + `Banned` — alive but refused at every routing decision. The two states compose; the operator state is the gate.

## The three states

### `Active` (default)

Routable, discoverable, accepts heartbeats, accepts sends. Every agent registered before this feature shipped deserializes as `Active` via `#[serde(default)]` — no migration is required.

### `Suspended`

Reversible park. The agent's row stays in the registry, its heartbeats keep flowing in, but route / discovery / send all refuse with `403 Forbidden`. Optionally time-boxed via `admin_expires_at`: once the expiry passes, the next read of the row treats it as `Active` again (auto-reinstate). The stored value remains `Suspended` until a write touches the row, but `effective_admin_state()` returns `Active`.

Typical use: park a flaky agent for an hour without having to remember to unblock it.

### `Banned`

Operator-banned (compromised, abusive, or decommissioned). Same enforcement as `Suspended` but never auto-reinstates — `admin_expires_at` is silently ignored on `Banned` rows. The record is kept rather than deleted so the audit metadata (`admin_set_by`, `admin_set_at`, `admin_reason`) survives. Reinstating is allowed via the API but is unusual.

## API

### Set admin state

```text
PUT /v1/bus/agents/{namespace}/{tenant}/{agent_id}/admin-state
```

```json
{
  "admin_state": "suspended",
  "reason": "investigating runaway tool calls",
  "expires_at": "2026-05-23T18:00:00Z"
}
```

Field rules:

- `admin_state` — required, one of `active` / `suspended` / `banned`.
- `reason` — optional, max 4 096 bytes. Surfaced to the *caller* on a blocked `send`. Never displayed to the agent owner. Don't put secrets here.
- `expires_at` — optional. **Valid only with `admin_state=suspended`.** Sending it with `active` or `banned` returns `400 Bad Request` so the contract is explicit. On `Suspended`, it triggers auto-reinstate.

Response: the full `Agent` row with the new admin metadata.

Required permission: `ManageAgent` (operator role). This is a moderation surface, not an end-user one.

### Discover by state

```text
GET /v1/bus/agents?admin_state=banned
```

The list endpoint accepts an `admin_state` filter that compares against the **effective** state (i.e. an auto-reinstated `Suspended` row appears under `active`, not `suspended`).

## Enforcement

Every `send_to_agent` call computes the effective admin state and short-circuits to `403 Forbidden` when the agent is not `Active`:

```json
{
  "error": "agent prod/acme/planner-01 is suspended: investigating runaway tool calls"
}
```

The reason is included so the caller knows *why*; `admin_set_by` is deliberately omitted from the error so the operator's identity is not leaked across the trust boundary.

Discovery (`GET /v1/bus/agents/{...}/card` and the `/.well-known/agent-card` walker) hides non-`Active` agents the same way.

## Audit trail

The audit lives on the row itself, not in a separate event log. Every `apply_admin_state` mutation stamps:

- `admin_state` — the new state
- `admin_reason` — operator-supplied free text
- `admin_set_by` — `CallerIdentity.id` of the operator who flipped the state
- `admin_set_at` — wall-clock time of the change
- `admin_expires_at` — only set on `Suspended`

State changes go through `cas_update`, so concurrent operator actions converge on a single winner rather than racing.

## Worked example

Park an agent for one hour while you investigate, then ban it:

```bash
# 1. Suspend with a 1h auto-expire
curl -X PUT \
  -H "Authorization: Bearer $ACTEON_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "admin_state": "suspended",
    "reason": "investigating runaway tool calls",
    "expires_at": "'"$(date -u -v+1H +%Y-%m-%dT%H:%M:%SZ)"'"
  }' \
  "$ACTEON_URL/v1/bus/agents/prod/acme/planner-01/admin-state"

# 2. (an attempted send now fails with 403)
curl -X POST \
  -H "Authorization: Bearer $ACTEON_TOKEN" \
  -d '{"payload": {"task": "summarize"}}' \
  "$ACTEON_URL/v1/bus/agents/prod/acme/planner-01/send"
# -> 403 {"error": "agent prod/acme/planner-01 is suspended: investigating runaway tool calls"}

# 3. Decision is made — ban it permanently
curl -X PUT \
  -H "Authorization: Bearer $ACTEON_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"admin_state": "banned", "reason": "confirmed credential leak"}' \
  "$ACTEON_URL/v1/bus/agents/prod/acme/planner-01/admin-state"
```

## When to use which state

| Situation | Use |
|-----------|-----|
| Routine maintenance window | `Suspended` with `expires_at` |
| Suspected misbehavior, not yet confirmed | `Suspended` (no expiry, manual reinstate) |
| Confirmed compromise / abuse | `Banned` |
| Decommissioning an agent permanently | `Banned` — preserves audit |
| Outright deletion | `DELETE /v1/bus/agents/{ns}/{t}/{id}` — loses audit, prefer `Banned` |

## See also

- [Agentic bus concepts](../concepts/agentic-bus.md) — the agent record and where it lives
- [A2A protocol](a2a.md) — discovery card visibility honors admin state
- [Audit trail](audit-trail.md) — for action-level audit (separate from agent-record audit)
