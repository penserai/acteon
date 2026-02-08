# Acteon Admin UI -- Wireframes & View Specifications

> Detailed layout, component hierarchy, data bindings, interaction flows, and state designs
> for every view in the Acteon Admin UI.

---

## Table of Contents

1. [Dashboard](#1-dashboard)
2. [Rules](#2-rules)
3. [Providers](#3-providers)
4. [Actions (Audit Trail)](#4-actions-audit-trail)
5. [Chains](#5-chains)
6. [Approvals](#6-approvals)
7. [Event Stream](#7-event-stream)
8. [Scheduled Actions](#8-scheduled-actions)
9. [Dead-Letter Queue](#9-dead-letter-queue)
10. [Settings](#10-settings)
11. [Global Search (Command Palette)](#11-global-search-command-palette)
12. [Dispatch](#12-dispatch)
13. [Events (State Machines)](#13-events-state-machines)
14. [Groups](#14-groups)
15. [Login](#15-login)

---

## 1. Dashboard

### Purpose
Real-time operational overview. The first thing an operator sees -- must answer "is the system healthy?" in under 2 seconds.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Dashboard                    [Last 1h v]  [Refresh]     |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | +--------+ +--------+ +--------+ +--------+              |
|            | |Dispatch| |Executed| | Failed | | Dedup  |              |
|            | | 12,847 | | 12,102 | |   243  | |   502  |              |
|            | |[spark] | |[spark] | |[spark] | |[spark] |              |
|            | +--------+ +--------+ +--------+ +--------+              |
|            |                                                           |
|            | +--------+ +--------+ +--------+ +--------+              |
|            | |Suppress| |Pending | |Circuit | |Schedul |              |
|            | |   147  | |Approval| | Open   | |  ed    |              |
|            | |[spark] | |    3   | |    1   | |   22   |              |
|            | +--------+ +--------+ +--------+ +--------+              |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | |       Actions Over Time (stacked area chart)          | |
|            | |  [executed] [failed] [suppressed] [dedup] [throttle] | |
|            | |                                                      | |
|            | |  12k ____                                            | |
|            | |      |    \___                                       | |
|            | |   8k |        \___________                           | |
|            | |      |                    \___                       | |
|            | |   4k |________________________\___                   | |
|            | |      |____________________________|                  | |
|            | |      10:00    10:15    10:30    10:45                | |
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | +---------------------------+ +-------------------------+ |
|            | | PROVIDER HEALTH           | | RECENT ACTIVITY         | |
|            | |                           | |                         | |
|            | | [G] email     Closed      | | 10:42 Dispatched email  | |
|            | | [G] webhook   Closed      | | 10:42 ChainAdvanced    | |
|            | | [R] sms       OPEN        | | 10:41 Executed webhook  | |
|            | | [Y] slack     Half-Open   | | 10:41 ApprovalRequired  | |
|            | |                           | | 10:40 GroupFlushed      | |
|            | |                           | | 10:40 Dispatched sms    | |
|            | |                           | | 10:39 Executed email    | |
|            | +---------------------------+ +-------------------------+ |
|            |                                                           |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Region | Component | Data Source |
|--------|-----------|-------------|
| Top row (4 cards) | `StatCard` x4 | `GET /metrics` -- `dispatched`, `executed`, `failed`, `deduplicated` |
| Second row (4 cards) | `StatCard` x4 | `GET /metrics` -- `suppressed`, `pending_approval`, `circuit_open`, `scheduled` |
| Time-series chart | `StackedAreaChart` | `GET /metrics` polled every 5s, client-side time-series buffer (last 60 data points) |
| Provider Health | `Card` > `ProviderHealthRow` x N | `GET /admin/circuit-breakers` |
| Recent Activity | `Timeline` / `ActivityFeed` | `GET /v1/stream` SSE connection |

### Data Bindings

- **Stat cards**: Poll `GET /metrics` every 5s. Each card shows the current counter value. Sparkline derived from client-side 60-point rolling buffer (5s intervals = 5 minutes of history).
- **Time-series chart**: Same polling data, accumulated on client. Stacked by outcome category. Time range selector adjusts both chart and stat card period.
- **Provider health**: Poll `GET /admin/circuit-breakers` every 15s. Show only when circuit breakers are enabled.
- **Activity feed**: SSE connection to `GET /v1/stream?namespace=*&tenant=*`. Last 20 events. Auto-scrolls.

### Interaction Flows

- **Click stat card**: Navigates to relevant filtered view (e.g., clicking "Failed" goes to Audit Trail filtered by `outcome=failed`).
- **Click provider card**: Navigates to Circuit Breaker detail for that provider.
- **Click activity event**: Opens side panel with event/action detail.
- **Time range selector**: Dropdown (Last 5m, 15m, 1h, 6h, 24h). Changes chart time window and stat card delta calculation.
- **Refresh button**: Force poll all data sources.

### States

**Empty state**: First-time setup. Cards show `0` values. Chart shows empty state message: "No actions dispatched yet. Dispatch your first action to see metrics here." Provider health section shows "No providers configured" if no providers, or "Circuit breakers disabled" if feature is off. Activity feed shows "Waiting for events... Connect an SSE stream to see live activity."

**Loading state**: 8 skeleton stat cards (shimmer), chart area skeleton (rectangle shimmer), two skeleton card containers for bottom row.

**Error state**: If `/metrics` fails: inline alert at top "Unable to load metrics. Check server connectivity." with retry button. Individual sections degrade gracefully -- if SSE disconnects, activity feed shows "Disconnected" badge.

### Responsive Behavior

- **Tablet**: Stat cards 2 per row (4 rows). Provider Health and Activity Feed stack vertically.
- **Mobile**: Stat cards 1 per row (scrollable horizontal carousel alternative). Chart collapses to smaller height. Bottom sections stack.

---

## 2. Rules

### Purpose
View, search, and manage routing rules. Split into list view and editor view.

### List View Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Rules                              [Reload Rules]       |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [Search rules...]  [Action Type v]  [Enabled v]  [Src v]|
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | Pri | Name           | Action  |Enabled|Source|Ver   | |
|            | |-----|----------------|---------|-------|------|------| |
|            | |  1  | block-pii      | Deny    | [ON]  | yaml |  3  | |
|            | |  5  | dedup-email    | Dedup   | [ON]  | yaml |  1  | |
|            | | 10  | reroute-sms    | Reroute | [OFF] | api  |  2  | |
|            | | 15  | throttle-api   | Throttle| [ON]  | yaml |  1  | |
|            | | 20  | approval-large | ReqAppr | [ON]  | yaml |  1  | |
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | Showing 1-20 of 20 rules                                  |
+----------------------------------------------------------------------+
```

### Editor View Layout (Split Pane)

```
+----------------------------------------------------------------------+
| [Sidebar]  | Rules / block-pii                    [Save] [Discard]   |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | +------------------------+|+----------------------------+ |
|            | | [YAML] [CEL]           ||| PARSED RULE                | |
|            | |                        |||                            | |
|            | | name: block-pii        ||| Name: block-pii            | |
|            | | priority: 1            ||| Priority: 1                | |
|            | | description: Block     ||| Description: Block PII     | |
|            | |   PII in payloads      ||| Action: Deny               | |
|            | | condition:             ||| Condition:                 | |
|            | |   field: payload.type  |||   payload.type == "pii"    | |
|            | |   operator: eq         ||| Enabled: Yes               | |
|            | |   value: "pii"         ||| Metadata: {}               | |
|            | | action: deny           |||                            | |
|            | | enabled: true          |||----------------------------| |
|            | |                        ||| DRY-RUN TEST               | |
|            | |                        ||| +------------------------+ | |
|            | |                        ||| | { "namespace": "prod", | | |
|            | |                        ||| |   "tenant": "acme",    | | |
|            | |                        ||| |   "action_type": "em", | | |
|            | |                        ||| |   "payload": {...}     | | |
|            | |                        ||| | }                      | | |
|            | |                        ||| +------------------------+ | |
|            | |                        |||       [Run Dry-Run]        | |
|            | |                        |||                            | |
|            | |                        ||| Verdict: DENY              | |
|            | |                        ||| Matched: block-pii         | |
|            | +------------------------+|+----------------------------+ |
+----------------------------------------------------------------------+
```

### Component Hierarchy

**List View**:
| Component | Props/Data |
|-----------|------------|
| `PageHeader` | title="Rules", action=`<ReloadButton>` |
| `FilterBar` | search text, action type select, enabled toggle filter, source select |
| `DataTable` | columns: priority, name, action, enabled (toggle), source, version |
| `ToggleSwitch` (inline in table) | Calls `PUT /v1/rules/{name}/enabled` |

**Editor View**:
| Component | Props/Data |
|-----------|------------|
| `Breadcrumbs` | Rules / {rule_name} |
| `SplitPane` | left: `RuleEditor`, right: `RulePreview` + `DryRunPanel` |
| `RuleEditor` | `TabGroup` (YAML/CEL), `CodeBlock` with syntax highlighting |
| `RulePreview` | Parsed rule fields displayed as key-value pairs |
| `DryRunPanel` | JSON input, Run button, verdict display |

### Data Bindings

- **List**: `GET /v1/rules` -- returns all rules with summary fields.
- **Toggle**: `PUT /v1/rules/{name}/enabled` with `{ enabled: true/false }`.
- **Reload**: `POST /v1/rules/reload` with optional directory override.
- **Dry-run**: `POST /v1/dispatch?dry_run=true` with the test payload.

### Interaction Flows

- **Click rule row**: Opens editor view for that rule (or side panel for quick view).
- **Toggle enabled**: Optimistic update, calls API, reverts on error with toast.
- **Reload Rules**: Shows confirmation toast "Reloading rules from disk..." then success/error toast.
- **Run Dry-Run**: Sends test payload to dispatch API with `dry_run=true`, displays verdict and matched rule in result area.
- **YAML/CEL tab switch**: Switches syntax mode in the editor. Content persists per tab.

### States

**Empty**: "No rules loaded. Add YAML rule files to your rules directory and click Reload, or create rules via the API."

**Loading**: Table skeleton (5 rows).

**Error**: "Failed to load rules. Server returned: {error}." Retry button.

### Responsive

- **Tablet**: Split pane stacks vertically (editor on top, preview below).
- **Mobile**: List view only -- click navigates to full-page detail (no split pane).

---

## 3. Providers

### Purpose
View registered providers and their health status including circuit breaker state.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Providers                                               |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | +------------------+ +------------------+ +--------------+|
|            | | email            | | webhook          | | sms          ||
|            | | Type: webhook    | | Type: webhook    | | Type: webhook||
|            | | URL: http://...  | | URL: http://...  | | URL: http://|||
|            | |                  | |                  | |              ||
|            | | Circuit: CLOSED  | | Circuit: CLOSED  | | Circuit: OPEN||
|            | |   [G]           | |   [G]           | |   [R]        ||
|            | |                  | |                  | |              ||
|            | | [View Detail]    | | [View Detail]    | | [View Detail]||
|            | +------------------+ +------------------+ +--------------+|
|            |                                                           |
|            | +------------------+                                      |
|            | | log              |                                      |
|            | | Type: log        |                                      |
|            | | (no URL)         |                                      |
|            | |                  |                                      |
|            | | Circuit: N/A     |                                      |
|            | | [View Detail]    |                                      |
|            | +------------------+                                      |
+----------------------------------------------------------------------+
```

### Provider Detail (Side Panel)

```
+-----------------------------------------------------------+
| email                                              [x]    |
|-----------------------------------------------------------|
| [Overview] [Circuit Breaker]                              |
|                                                           |
| CONFIGURATION                                             |
| Type:    webhook                                          |
| URL:     http://localhost:9999/webhook                     |
| Headers: Authorization: Bea***en                          |
|                                                           |
| CIRCUIT BREAKER                                           |
|   State: Closed (healthy)                                 |
|   Failure Threshold: 5                                    |
|   Success Threshold: 2                                    |
|   Recovery Timeout: 60s                                   |
|   Fallback: sms-fallback                                  |
|                                                           |
|   +---State Diagram---+                                   |
|   |  [CLOSED] -> [OPEN] -> [HALF-OPEN] -> [CLOSED]      |
|   +--------------------+                                  |
|                                                           |
|   [Trip Circuit]  [Reset Circuit]                         |
+-----------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Providers" |
| `CardGrid` | Grid of `ProviderCard` components, responsive 3-col / 2-col / 1-col |
| `ProviderCard` | Provider name, type badge, URL (truncated), circuit state indicator |
| `Drawer` (detail) | Full config, circuit breaker widget, trip/reset buttons |
| `CircuitBreakerWidget` | State diagram SVG, counters, config knobs |

### Data Bindings

- **Provider list**: Loaded from server configuration (no dedicated provider list API -- derived from config or extracted from circuit breaker endpoint).
- **Circuit breakers**: `GET /admin/circuit-breakers` -- list all with state.
- **Trip/Reset**: `POST /admin/circuit-breakers/{provider}/trip|reset`.

### Interaction Flows

- **Click provider card**: Opens side panel with detail.
- **Trip Circuit**: Confirmation dialog "Force-open circuit for {provider}? All actions will be rejected or routed to fallback." Calls trip endpoint.
- **Reset Circuit**: Confirmation dialog "Force-close circuit for {provider}? Normal operation will resume." Calls reset endpoint.

### States

**Empty**: "No providers configured. Add providers to your `acteon.toml` configuration."

**Circuit breakers disabled**: Cards show without circuit state -- label reads "Circuit breakers not enabled."

### Responsive

- **Tablet**: 2-column card grid.
- **Mobile**: 1-column card stack.

---

## 4. Actions (Audit Trail)

### Purpose
Search, filter, inspect, and replay past action dispatches. The main operational investigation view.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Audit Trail                          [Bulk Replay]      |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [Search ID...]  [namespace v] [tenant v] [action_type v]  |
|            | [outcome v] [verdict v] [provider v] [rule v] [from..to] |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | action_id | ns   | tenant| type  |verdict|outcome|dur | |
|            | |-----------|------|-------|-------|-------|-------|----| |
|            | | abc-123   | prod | acme  | email | allow |Execut| 42 | |
|            | | def-456   | prod | acme  | sms   | deny  |Suppr | 12 | |
|            | | ghi-789   | stg  | beta  | email | allow |Fail  |230 | |
|            | | jkl-012   | prod | acme  | hook  | allow |Execut| 55 | |
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | Showing 1-50 of 2,847       [< 1 2 3 ... 57 >]          |
+----------------------------------------------------------------------+
```

### Detail Side Panel

```
+-----------------------------------------------------------+
| Action abc-123                         [Replay] [x] [>>]  |
|-----------------------------------------------------------|
| [Overview] [Payload] [Verdict] [Outcome]                  |
|                                                           |
| OVERVIEW                                                  |
| Action ID:    abc-123                                     |
| Namespace:    prod                                        |
| Tenant:       acme                                        |
| Provider:     email                                       |
| Action Type:  send-notification                           |
| Verdict:      Allow (rule: default-allow)                 |
| Outcome:      Executed                                    |
| Duration:     42ms                                        |
| Dispatched:   2026-02-08 10:42:01 UTC                     |
| Completed:    2026-02-08 10:42:01 UTC                     |
| Caller:       api-key:service-account                     |
| Auth Method:  api_key                                     |
|                                                           |
| PAYLOAD                                                   |
| +-------------------------------------------------------+ |
| | {                                                     | |
| |   "to": "user@example.com",                          | |
| |   "subject": "Welcome to Acme",                      | |
| |   "body": "Hello, welcome..."                        | |
| | }                                                     | |
| +-------------------------------------------------------+ |
|                                                           |
| METADATA                                                  |
| priority: high                                            |
| source: onboarding-flow                                   |
+-----------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Audit Trail", action=`<BulkReplayButton>` |
| `FilterBar` | Text search for action_id, dropdowns for namespace/tenant/action_type/outcome/verdict/provider/matched_rule, date range picker |
| `DataTable` | Columns: action_id (mono), namespace, tenant, action_type, verdict (badge), outcome (badge), duration_ms, dispatched_at |
| `Drawer` (detail) | `TabGroup` (Overview/Payload/Verdict/Outcome), `JsonViewer` for payload, `Badge` for verdict/outcome |
| `ReplayButton` | On individual records and bulk |

### Data Bindings

- **List**: `GET /v1/audit` with query params from filters. Paginated (limit/offset).
- **Detail**: `GET /v1/audit/{action_id}` for full record.
- **Replay**: `POST /v1/audit/{action_id}/replay` for single. `POST /v1/audit/replay` with current filters for bulk.

### Interaction Flows

- **Click row**: Opens side panel with full audit record detail.
- **Replay button (single)**: Confirmation modal "Replay action abc-123? This will dispatch a new action with the original payload." Shows `ReplayResult` in a toast on completion.
- **Bulk Replay**: Confirmation modal showing filter summary and estimated count. "Replay 47 matching actions?" Shows `ReplaySummary` modal on completion with success/fail/skip counts.
- **Filter changes**: Debounced (300ms) re-fetch with updated query params. URL updates for deep linking.
- **Date range**: Calendar picker for absolute dates, or relative presets (Last 5m, 15m, 1h, 6h, 24h, 7d).
- **Click `[>>]` in panel**: Navigates to full-page detail view.

### States

**Empty (no filters)**: "No audit records found. Actions are recorded when `audit.enabled = true` in server configuration. Dispatch actions to start seeing records here."

**Empty (with filters)**: "No records match your filters. Try adjusting the time range or removing some filters." With a "Clear all filters" button.

**Loading**: 5-row table skeleton. Filter bar shows shimmer in dropdowns while options load.

**Error**: "Failed to load audit records: {error}. Check that the audit backend is running and accessible." Retry button.

### Responsive

- **Tablet**: Table columns reduced -- hide `duration_ms` and `provider`. Full columns accessible via column selector.
- **Mobile**: Card-based list. Each card shows action_id, outcome badge, action_type, timestamp. Tap for full-screen detail.

---

## 5. Chains

### Purpose
Monitor chain executions and visualize the DAG flow with conditional branching. This is THE wow-factor view.

### List View Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Chains                                                  |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [namespace v] [tenant v] [status v]                       |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | chain_id  | name       |status  |progress| started    | |
|            | |-----------|------------|--------|--------|------------| |
|            | | abc-123   | process-   |Running |  2/5   | 10:42:01   | |
|            | |           | order      |[blue]  |[====_] |            | |
|            | | def-456   | onboard-   |Complet |  4/4   | 10:38:22   | |
|            | |           | user       |[green] |[=====] |            | |
|            | | ghi-789   | alert-     |Failed  |  2/3   | 10:35:10   | |
|            | |           | pipeline   |[red]   |[===X ] |            | |
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | Showing 1-25 of 142        [< 1 2 3 ... 6 >]            |
+----------------------------------------------------------------------+
```

### Detail View (DAG) Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Chains / process-order / abc-123       [Cancel Chain]   |
|            |   [Running badge]  Started: 10:42:01   Expires: 11:42   |
|            |-----------------------------------------------------------+
|            |                                                           |
|            |  Execution Path: validate -> enrich -> notify             |
|            |                                                           |
|            |                  +------------+                           |
|            |                  | validate   |                           |
|            |                  | [COMPLETED]|                           |
|            |                  +-----+------+                           |
|            |                   /    |    \                             |
|            |         success=true   |   success=false                 |
|            |                /       |        \                        |
|            |    +----------+  +-----+-----+ +----------+             |
|            |    | enrich   |  | escalate  | | reject   |             |
|            |    | [ACTIVE] |  | [pending] | | [pending]|             |
|            |    +----+-----+  +-----------+ +----------+             |
|            |         |                                                |
|            |    +----+-----+                                         |
|            |    | notify   |                                         |
|            |    | [pending]|                                         |
|            |    +----------+                                         |
|            |                                                           |
|            |  [Fit] [Zoom +] [Zoom -]                                |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | STEP DETAIL: enrich (active)                          | |
|            | | Provider: webhook    Status: In Progress               | |
|            | | Started: 10:42:02                                     | |
|            | | Response: (pending...)                                 | |
|            | +------------------------------------------------------+ |
+----------------------------------------------------------------------+
```

### Component Hierarchy

**List View**:
| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Chains" |
| `FilterBar` | namespace (required), tenant (required), status dropdown |
| `DataTable` | Columns: chain_id (mono), name, status (badge), progress (progress bar), started_at, updated_at |

**Detail View**:
| Component | Data Source |
|-----------|-------------|
| `Breadcrumbs` | Chains / {chain_name} / {chain_id} |
| `PageHeader` | chain_name, status badge, started_at, expires countdown |
| `ExecutionPathBadges` | Ordered badges from `execution_path` array |
| `DAGVisualizer` | Steps as nodes, branches as edges, execution_path highlighted |
| `StepDetailPanel` | Below DAG or as side panel -- shows selected step details |
| `CancelButton` | Only for status=running chains |

### Data Bindings

- **List**: `GET /v1/chains?namespace=X&tenant=Y` with optional status filter.
- **Detail**: `GET /v1/chains/{chain_id}` -- returns `ChainDetailResponse` with steps and execution_path.
- **Cancel**: `POST /v1/chains/{chain_id}/cancel` with namespace, tenant, optional reason.

### DAG Visualizer Data Mapping

```
ChainDetailResponse.steps  -->  Nodes (one per step)
  step.name                -->  Node label
  step.status              -->  Node color (completed=green, active=blue, pending=gray, failed=red, skipped=gray-dashed)
  step.response_body       -->  Node click detail
  step.error               -->  Node error indicator

ChainStepConfig.branches   -->  Edges with labels
  branch.target            -->  Edge destination
  branch.field + operator  -->  Edge label (e.g., "success == true")
ChainStepConfig.default_next -> Default edge (no label, or "default")

ChainDetailResponse.execution_path  -->  Highlighted edge path (thicker, primary color)
```

### Interaction Flows

- **Click chain in list**: Navigate to DAG detail view.
- **Click DAG node**: Step detail panel appears below DAG (or side panel), showing step name, provider, status, response body (JSON viewer), error, completion time.
- **Zoom/Pan**: Mouse wheel for zoom, drag for pan, Fit button to auto-fit.
- **Cancel chain**: Confirmation dialog "Cancel chain process-order (abc-123)? Reason (optional): [input]." Calls cancel endpoint.
- **Execution path display**: Ordered list of step names rendered as connected badges at top of detail view, showing the actual branch path taken.

### States

**Empty (no chains)**: "No chain executions found. Chain executions are created when a rule triggers a `Chain` action. Configure chain definitions in your server config."

**Loading**: Table skeleton for list. For detail: skeleton node placeholders in DAG area.

**Error**: "Failed to load chain data: {error}." Retry button.

**Chain in progress**: Active step pulses. Execution path grows as steps complete. Auto-refresh every 2s for running chains.

**Chain completed**: All executed nodes green, non-executed nodes gray/dashed. "Completed" badge. No auto-refresh.

**Chain failed**: Failed node red with error icon. Steps after failure gray/dashed. Error detail in step panel.

### Responsive

- **Tablet**: DAG takes full width, step detail moves to below the DAG (stacked).
- **Mobile**: DAG replaced with step list (vertical cards in execution order). Each card shows step name, status badge, response preview. Branch conditions shown as labels between cards.

---

## 6. Approvals

### Purpose
Queue of actions awaiting human approval. Operators approve or reject from this view.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Approvals                       [3 pending]             |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [namespace v] [tenant v]                                  |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | [PendingApproval]                          2m ago     | |
|            | |                                                      | |
|            | | Action: send-notification (email)                    | |
|            | | Rule: pii-review-required                            | |
|            | | Message: "Contains PII - requires review"            | |
|            | |                                                      | |
|            | | ns: prod  |  tenant: acme  |  Expires: 23m left     | |
|            | |                                                      | |
|            | | [v Show payload]                                     | |
|            | |                                                      | |
|            | |                     [Reject (red)]  [Approve (green)]| |
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | [PendingApproval]                          8m ago     | |
|            | |                                                      | |
|            | | Action: bulk-update (webhook)                        | |
|            | | Rule: large-batch-review                             | |
|            | | Message: "Batch size > 1000 requires approval"       | |
|            | |                                                      | |
|            | | ns: prod  |  tenant: acme  |  Expires: 17m left     | |
|            | |                                                      | |
|            | | [v Show payload]                                     | |
|            | |                                                      | |
|            | |                     [Reject (red)]  [Approve (green)]| |
|            | +------------------------------------------------------+ |
|            |                                                           |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Approvals", subtitle="{N} pending" |
| `FilterBar` | namespace dropdown, tenant dropdown |
| `ApprovalCard` x N | Card-based layout, one per pending approval |
| `Badge` | Status badge (Pending/Approved/Rejected/Expired) |
| `CountdownTimer` | Live countdown from `expires_at` |
| `JsonViewer` (collapsible) | Action payload preview |
| `Button` (Approve) | Success variant, large size |
| `Button` (Reject) | Danger variant, large size |

### Data Bindings

- **List**: `GET /v1/approvals?namespace=X&tenant=Y` -- returns pending approvals.
- **Approve**: `POST /v1/approvals/{ns}/{tenant}/{id}/approve`.
- **Reject**: `POST /v1/approvals/{ns}/{tenant}/{id}/reject`.
- **Status check**: `GET /v1/approvals/{ns}/{tenant}/{id}` for individual status.

### Interaction Flows

- **Approve**: Click green button. Brief loading state (button shows spinner). On success: card transitions to approved state (green border, "Approved" badge, buttons removed). Toast: "Action approved."
- **Reject**: Click red button. Optional rejection reason input (inline or modal). On success: card transitions to rejected state. Toast: "Action rejected."
- **Expand payload**: Toggles `JsonViewer` open/closed within the card.
- **Countdown**: Live updating timer. Changes to amber at < 5m, red at < 1m. At 0: card auto-transitions to "Expired" state.

### States

**Empty**: "No pending approvals. Actions requiring approval will appear here when a `RequestApproval` rule matches."

**Loading**: 2-3 skeleton cards.

**All resolved**: Cards show with Approved/Rejected/Expired badges. Option to filter by status to see history.

### Responsive

- **Mobile**: Cards stack full-width. Approve/Reject buttons become full-width, stacked vertically (Approve on top).

---

## 7. Event Stream

### Purpose
Live SSE event viewer for real-time monitoring.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Event Stream                                            |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [G Connected]  [ns v] [tenant v] [type v]  [|| Pause]    |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | 10:42:03 [ActionDispatched] email  prod/acme          | |
|            | | 10:42:02 [ChainAdvanced]   chain-abc validate         | |
|            | | 10:42:01 [ApprovalRequired] approval-xyz pii-check   | |
|            | | 10:41:58 [ActionDispatched] webhook prod/acme         | |
|            | | 10:41:55 [GroupFlushed]    group-def alert-group      | |
|            | | 10:41:52 [ActionDispatched] sms    prod/acme         | |
|            | | 10:41:50 [Timeout]        event-123 pending->expired | |
|            | | 10:41:48 [ScheduledActionDue] task-456 email         | |
|            | | ...                                                  | |
|            | +------------------------------------------------------+ |
|            |                                                           |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Event Stream" |
| `ConnectionStatusBadge` | SSE connection state (connected/reconnecting/disconnected) |
| `FilterBar` | namespace, tenant, action_type, event_type dropdowns |
| `PauseResumeButton` | Toggles event buffering |
| `LiveEventFeed` | Scrolling list of `EventRow` components |
| `EventRow` | Timestamp (mono), event type badge, summary text |

### Data Bindings

- **SSE Connection**: `GET /v1/stream?namespace=X&tenant=Y&action_type=Z&event_type=W`
- **Resume**: Uses `Last-Event-ID` header to catch up on missed events.

### Interaction Flows

- **Pause**: Stops consuming from SSE (events buffer on client). Button shows "Resume" with buffered count badge.
- **Resume**: Flushes buffered events into feed. Reconnects if disconnected.
- **Click event**: Expands inline to show full event detail (action_id, full fields).
- **Filter change**: Disconnects and reconnects SSE with new query params.
- **Auto-scroll**: Feed auto-scrolls to top (newest first). Scrolling down pauses auto-scroll. "Jump to latest" floating button appears.

### States

**Connecting**: Spinner with "Connecting to event stream..."

**Connected**: Green dot + "Connected" badge. Events flowing.

**Reconnecting**: Yellow dot + "Reconnecting..." badge. Animation blink.

**Disconnected**: Red dot + "Disconnected" badge. "Reconnect" button. Error message if applicable.

**No events**: "Waiting for events... The stream is connected but no events have arrived yet."

### Responsive

- **Mobile**: Simplified event rows (badge + summary only, timestamp in relative form). Filter bar collapses to icon button that opens filter sheet.

---

## 8. Scheduled Actions

### Purpose
View and manage actions scheduled for future dispatch.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Scheduled Actions                                       |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [namespace v] [tenant v]                                  |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | |action_id |action_type|ns    |dispatch_at |countdown|  | |
|            | |----------|-----------|------|------------|---------|--| |
|            | | abc-123  | email     | prod | 11:00:00   | 18m     |[x]|
|            | | def-456  | webhook   | prod | 11:15:00   | 33m     |[x]|
|            | | ghi-789  | sms       | stg  | 12:00:00   | 78m     |[x]|
|            | +------------------------------------------------------+ |
|            |                                                           |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Scheduled Actions" |
| `FilterBar` | namespace, tenant dropdowns |
| `DataTable` | Columns: action_id (mono), action_type, namespace, dispatch_at, countdown (live), cancel button |
| `CountdownTimer` | Live countdown to `dispatch_at` |
| `IconButton` (cancel) | Danger ghost variant, X icon |

### Data Bindings

- Scheduled actions are stored in state store with `KeyKind::ScheduledAction`. Currently no dedicated list API -- data may need to be sourced from audit records with outcome `Scheduled`, or a new API endpoint.
- Countdown is client-side computed from `dispatch_at` - `now`.

### Interaction Flows

- **Cancel**: Confirmation dialog "Cancel scheduled action {action_id}?" Removes from schedule.
- **Sort**: Default sorted by `dispatch_at` ascending (soonest first).
- **Click row**: Side panel with full action detail (payload, metadata).

### States

**Empty**: "No scheduled actions. Actions dispatched with `starts_at` will appear here until their scheduled time."

---

## 9. Dead-Letter Queue

### Purpose
View and manage actions that exhausted all retry attempts.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Dead-Letter Queue                     [Drain All]       |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | DLQ Status: [Enabled badge]  Entries: 47                  |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | action_id | provider | action_type | error    |attempts||
|            | |-----------|----------|-------------|----------|--------||
|            | | abc-123   | webhook  | email       | timeout  |   3   ||
|            | | def-456   | sms      | sms-notif   | 503      |   3   ||
|            | | ghi-789   | webhook  | webhook     | refused  |   3   ||
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | [Retry Selected (2)]  [Dismiss Selected (2)]              |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Dead-Letter Queue", action=`<DrainButton>` |
| `StatCard` (inline) | DLQ enabled status + entry count from `GET /v1/dlq/stats` |
| `DataTable` | Columns: action_id, provider, action_type, error (truncated), attempts, timestamp. Row selection checkboxes. |
| `BulkActionBar` | Appears when rows selected: "Retry Selected (N)", "Dismiss Selected (N)" |
| `DrainButton` | Danger variant, calls `POST /v1/dlq/drain` |

### Data Bindings

- **Stats**: `GET /v1/dlq/stats` -- enabled flag and count.
- **Drain**: `POST /v1/dlq/drain` -- removes all entries.
- **Entry list**: Currently not available via API (DLQ entries are in-memory). The UI shows stats and drain capability. Individual entry listing would require a new API endpoint.

### Interaction Flows

- **Drain All**: Confirmation dialog "Permanently drain all 47 DLQ entries? This cannot be undone." On confirm: calls drain API. Success toast with count.
- **Click row**: Side panel with full action detail, error details, and retry count.
- **Retry**: Re-dispatches action payload (replay functionality).
- **Dismiss**: Removes from DLQ without retry.

### States

**Empty**: "Dead-letter queue is empty. Failed actions that exhaust retry attempts will appear here."

**DLQ disabled**: "Dead-letter queue is disabled. Enable it with `executor.dlq_enabled = true` in your server configuration."

---

## 10. Settings

### Purpose
View server configuration, manage theme, display auth information.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Settings                                                |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [General] [Theme] [Rate Limits] [Auth] [LLM] [Telemetry] |
|            |                                                           |
|            | GENERAL                                                   |
|            | +------------------------------------------------------+ |
|            | | Server                                               | |
|            | |   Host:       127.0.0.1                              | |
|            | |   Port:       8080                                    | |
|            | |   External:   https://acteon.acme.com                 | |
|            | |                                                      | |
|            | | State Backend                                        | |
|            | |   Type:       redis                                  | |
|            | |   URL:        redis://localhost:6379                  | |
|            | |   Prefix:     acteon                                 | |
|            | |                                                      | |
|            | | Audit Backend                                        | |
|            | |   Enabled:    Yes                                    | |
|            | |   Type:       postgres                               | |
|            | |   TTL:        30 days                                | |
|            | |   Payload:    Stored (redaction off)                 | |
|            | |                                                      | |
|            | | Executor                                             | |
|            | |   Max Retries: 3                                    | |
|            | |   Timeout:    30s                                    | |
|            | |   Concurrent: 10                                    | |
|            | |   DLQ:        Enabled                               | |
|            | +------------------------------------------------------+ |
|            |                                                           |
+----------------------------------------------------------------------+
```

### Sub-Views (Tabs)

**General**: Server config, state backend, audit backend, executor settings. Read-only display of `acteon.toml` values.

**Theme**: Dark/Light/System toggle. Preview of both modes side by side.

**Rate Limits**: Display of rate limit tiers, default limits, per-caller overrides, per-tenant overrides. Read-only from config.

**Auth & Users**: Current user session info (role, grants, auth method). List of configured users (name, role -- no password display). List of API keys (name, role). Read-only.

**LLM Guardrail**: Enabled/disabled, endpoint, model, global policy, per-action-type policies. Read-only.

**Telemetry**: Enabled/disabled, endpoint, service name, sample ratio, protocol. Read-only.

**Providers**: Same as the Providers view but in a settings context (list + config). Read-only.

**Background Tasks**: Which background tasks are enabled, their intervals. Read-only.

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Settings" |
| `TabGroup` | Sections: General, Theme, Rate Limits, Auth, LLM, Telemetry, Background |
| `SettingsSection` | Labeled key-value display groups |
| `KeyValueRow` | Label (left, muted) + Value (right, code font for technical values) |
| `ThemeToggle` | Three-state: System, Light, Dark with preview |

### Data Bindings

- All settings data comes from `GET /health` (embedded metrics and config status indicators).
- Detailed config display requires a dedicated admin config endpoint or is loaded from server-side config at build time.
- Current user identity from JWT decoded on client or from a `/v1/auth/me` endpoint.

### Responsive

- **Mobile**: Tabs become scrollable horizontal pills. Key-value rows stack vertically (label above value).

---

## 11. Global Search (Command Palette)

### Purpose
Universal quick-access to navigation, search, and actions from anywhere in the app.

### Layout

```
        +-------------------------------------------+
        | [Q]  Type a command or search...           |
        |-------------------------------------------|
        |                                           |
        | NAVIGATION                                 |
        |   [icon] Dashboard                  Cmd+1 |
        |   [icon] Rules                      Cmd+2 |
        |   [icon] Chains                     Cmd+3 |
        |   [icon] Audit Trail                Cmd+4 |
        |   [icon] Approvals                  Cmd+5 |
        |                                           |
        | ACTIONS                                    |
        |   [+] Dispatch Action               Cmd+D |
        |   [R] Reload Rules                  Cmd+R |
        |   [T] Toggle Theme                         |
        |                                           |
        | RECENT                                     |
        |   chain-abc-123 (Chain)                    |
        |   block-pii (Rule)                         |
        |   action def-456 (Audit)                   |
        +-------------------------------------------+
```

When user types:

```
        +-------------------------------------------+
        | [Q]  process-order                         |
        |-------------------------------------------|
        |                                           |
        | CHAINS                                     |
        |   process-order (chain definition)         |
        |   process-order/abc-123 (running)          |
        |   process-order/def-456 (completed)        |
        |                                           |
        | RULES                                      |
        |   (no results)                             |
        |                                           |
        | ACTIONS                                    |
        |   process-order action ghi-789             |
        +-------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `CommandPaletteModal` | Overlay with backdrop |
| `SearchInput` | Auto-focused text input |
| `ResultCategory` x N | Grouped results by type |
| `ResultItem` | Icon, label, metadata, shortcut hint |

### Data Bindings

- **Navigation items**: Static list of all sidebar routes.
- **Actions**: Static list of available commands.
- **Recent**: Client-side `localStorage` history of last 10 accessed items.
- **Search results**: Fuzzy match across cached rules (`GET /v1/rules`), recent chains, recent audit records. For deeper search: async API calls to `/v1/audit?action_id=query` and `/v1/chains?namespace=*`.

### Interaction Flows

- **Open**: `Cmd+K` (Mac) / `Ctrl+K` (Win/Linux), or click the Cmd+K hint in header.
- **Navigate**: Arrow keys move highlight. Enter activates highlighted item.
- **Close**: `Escape`, click backdrop, or successful action.
- **Type to filter**: Instant fuzzy matching. Categories with no matches are hidden.

---

## 12. Dispatch

### Purpose
Manually dispatch actions through the gateway pipeline for testing and operations.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Dispatch Action                                         |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | | Namespace*:  [prod         v]                         | |
|            | | Tenant*:     [acme         v]                         | |
|            | | Provider*:   [email        v]                         | |
|            | | Action Type*:[send-notification    ]                  | |
|            | |                                                      | |
|            | | Payload (JSON)*:                                     | |
|            | | +--------------------------------------------------+ | |
|            | | | {                                                | | |
|            | | |   "to": "user@example.com",                     | | |
|            | | |   "subject": "Test",                            | | |
|            | | |   "body": "Hello world"                         | | |
|            | | | }                                                | | |
|            | | +--------------------------------------------------+ | |
|            | |                                                      | |
|            | | Metadata (key-value):                                | |
|            | |   [key: priority ] [val: high  ] [+ Add]            | |
|            | |                                                      | |
|            | | Dedup Key:    [optional-dedup-key     ]              | |
|            | | Schedule At:  [                       ] (optional)   | |
|            | |                                                      | |
|            | |   [x] Dry Run                                       | |
|            | |                                                      | |
|            | |                                [Dispatch Action]     | |
|            | +------------------------------------------------------+ |
|            |                                                           |
|            | RESULT                                                    |
|            | +------------------------------------------------------+ |
|            | | Action ID: abc-def-123                               | |
|            | | Outcome:   [Executed badge]                          | |
|            | | Details:   Provider responded with 200 OK            | |
|            | | [View in Audit Trail]                                | |
|            | +------------------------------------------------------+ |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `PageHeader` | title="Dispatch Action" |
| `Form` | namespace (select/text), tenant (select/text), provider (select from registered), action_type (text), payload (JSON editor), metadata (key-value rows), dedup_key (text), schedule_at (datetime picker), dry_run (checkbox) |
| `JsonEditor` | Code editor for payload with syntax validation |
| `DispatchResultCard` | Outcome badge, action_id, details JSON viewer |
| `Button` (Dispatch) | Primary variant, loading state during dispatch |

### Data Bindings

- **Provider list**: Derived from config or circuit breaker list.
- **Dispatch**: `POST /v1/dispatch` (or `POST /v1/dispatch?dry_run=true`).
- **Response**: `DispatchResponse` rendered in result card.

### Interaction Flows

- **Fill form**: Required fields marked with `*`. JSON payload validated on input.
- **Dispatch**: Calls API. Button shows loading state. Result card appears below with outcome.
- **Dry Run**: Same form but with `dry_run=true`. Result shows verdict and matched rule.
- **View in Audit Trail**: Link to audit detail for the dispatched action_id.

### States

**Initial**: Empty form with placeholders. Result section hidden.

**Dispatching**: Button loading, form disabled.

**Result**: Result card appears with slide-down animation. Success or error outcome.

**Validation error**: Inline errors on invalid fields (e.g., invalid JSON, missing required fields).

---

## 13. Events (State Machines)

### Purpose
View and manage events tracked by state machines, with manual transition capability.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Events                                                   |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [namespace v] [tenant v] [state machine v] [state v]      |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | |fingerprint      |state machine|current  |updated_at  | |
|            | |-----------------|-------------|---------|------------| |
|            | | user-123-email  | onboarding  | active  | 10:42:01   | |
|            | | order-456-pay   | payment     | pending | 10:38:22   | |
|            | | alert-789       | incident    | firing  | 10:35:10   | |
|            | +------------------------------------------------------+ |
+----------------------------------------------------------------------+
```

### Event Detail Side Panel

```
+-----------------------------------------------------------+
| Event: user-123-email                              [x]    |
|-----------------------------------------------------------|
| State Machine: onboarding                                 |
| Current State: [active badge]                             |
| Last Updated: 2026-02-08 10:42:01 UTC                     |
| Transitioned By: action                                   |
|                                                           |
| TRANSITION HISTORY                                        |
|   active   <- pending   (action, 10:42:01)               |
|   pending  <- created   (action, 10:38:00)               |
|   created  <- (initial) (system, 10:35:00)               |
|                                                           |
| MANUAL TRANSITION                                         |
| Target State: [resolved v]                                |
|               [Transition]                                |
|                                                           |
| STATE MACHINE DIAGRAM                                     |
|   [created] -> [pending] -> [active] -> [resolved]       |
|                    |                        ^             |
|                    +-- timeout 300s --------+             |
+-----------------------------------------------------------+
```

### Data Bindings

- **List**: `GET /v1/events?namespace=X&tenant=Y`
- **Detail**: `GET /v1/events/{fingerprint}`
- **Transition**: `POST /v1/events/{fingerprint}/transition` with target state

---

## 14. Groups

### Purpose
View and manage batched event groups.

### Layout

```
+----------------------------------------------------------------------+
| [Sidebar]  | Event Groups                                            |
|            |-----------------------------------------------------------+
|            |                                                           |
|            | [namespace v] [tenant v] [state v]                        |
|            |                                                           |
|            | +------------------------------------------------------+ |
|            | |group_key  |labels         |events|state  |notify_at  | |
|            | |-----------|---------------|------|-------|-----------|  |
|            | | a1b2c3... | env:prod,     |  12  |Pending| 10:45:00  | |
|            | |           | svc:email     |      |       |           | |
|            | | d4e5f6... | env:staging,  |   3  |Notif. | 10:40:00  | |
|            | |           | svc:sms       |      |       |           | |
|            | +------------------------------------------------------+ |
+----------------------------------------------------------------------+
```

### Group Detail Side Panel

```
+-----------------------------------------------------------+
| Group: a1b2c3...                     [Flush] [Delete] [x] |
|-----------------------------------------------------------|
| State: [Pending badge]                                    |
| Events: 12                                                |
| Notify At: 10:45:00 (3m remaining)                        |
| Created: 10:40:00                                         |
| Labels: env=prod, svc=email                               |
|                                                           |
| EVENTS (12)                                               |
| +-------------------------------------------------------+ |
| | 1. action-abc  email  10:42:01  [v payload]           | |
| | 2. action-def  email  10:41:30  [v payload]           | |
| | 3. action-ghi  email  10:41:15  [v payload]           | |
| | ...                                                    | |
| +-------------------------------------------------------+ |
+-----------------------------------------------------------+
```

### Data Bindings

- **List**: `GET /v1/groups?namespace=X&tenant=Y`
- **Detail**: `GET /v1/groups/{group_key}`
- **Flush**: `POST /v1/groups/{group_key}` (triggers immediate flush)
- **Delete**: `DELETE /v1/groups/{group_key}`

### Interaction Flows

- **Flush**: Confirmation dialog "Flush group {key}? This will trigger the group notification immediately."
- **Delete**: Confirmation dialog "Delete group {key}? This will discard all 12 grouped events. This cannot be undone."

---

## 15. Login

### Purpose
Authentication gate when `auth.enabled = true`.

### Layout

```
+----------------------------------------------------------------------+
|                                                                      |
|                                                                      |
|                    +---------------------------+                     |
|                    |      [Acteon Logo]        |                     |
|                    |                           |                     |
|                    | Username:                 |                     |
|                    | [admin@acme.com      ]    |                     |
|                    |                           |                     |
|                    | Password:                 |                     |
|                    | [**************     ]     |                     |
|                    |                           |                     |
|                    |        [Sign In]          |                     |
|                    |                           |                     |
|                    | [error: Invalid creds]    |                     |
|                    +---------------------------+                     |
|                                                                      |
+----------------------------------------------------------------------+
```

### Component Hierarchy

| Component | Data Source |
|-----------|-------------|
| `LoginCard` | Centered card with logo, form, and error area |
| `TextInput` (username) | Email/username input |
| `TextInput` (password) | Password input, masked |
| `Button` (Sign In) | Primary variant, loading during auth |
| `InlineAlert` (error) | Shows on auth failure |

### Data Bindings

- **Login**: `POST /v1/auth/login` with `{ username, password }`.
- **Response**: JWT token stored in `httpOnly` cookie or `localStorage`.
- **Redirect**: On success, redirect to Dashboard (or to the originally requested URL).

### States

**Default**: Empty form, no errors.

**Authenticating**: Button loading, inputs disabled.

**Error**: "Invalid username or password. Please try again." Red inline alert below form.

**Auth disabled**: Login page is not rendered. App loads directly to Dashboard.

### Responsive

- **Mobile**: Login card fills full width with padding. Touch-friendly input sizes (lg variant).
