# Features

Acteon provides a rich set of features for controlling action dispatch. Each feature is implemented as a rule action type and can be combined with any condition.

## Rule-Based Processing

<div class="grid" markdown>

<div class="card" markdown>
### [Deduplication](deduplication.md)
Prevent duplicate processing using configurable keys and TTLs.
</div>

<div class="card" markdown>
### [Suppression](suppression.md)
Block actions matching specific conditions — spam filtering, maintenance windows, etc.
</div>

<div class="card" markdown>
### [Throttling](throttling.md)
Rate-limit actions per tenant, provider, or action type with automatic retry-after hints.
</div>

<div class="card" markdown>
### [Rerouting](rerouting.md)
Dynamically redirect actions to different providers based on priority, load, or content.
</div>

<div class="card" markdown>
### [Payload Modification](modification.md)
Transform action payloads before execution — redaction, enrichment, normalization.
</div>

</div>

## Event Lifecycle

<div class="grid" markdown>

<div class="card" markdown>
### [Event Grouping](event-grouping.md)
Batch related events together for consolidated notifications.
</div>

<div class="card" markdown>
### [State Machines](state-machines.md)
Track event lifecycle through configurable states with automatic timeouts.
</div>

</div>

## Infrastructure

<div class="grid" markdown>

<div class="card" markdown>
### [Circuit Breaker](circuit-breaker.md)
Automatic provider health tracking — stop requests to failing providers, reroute to fallbacks, and recover gracefully.
</div>

<div class="card" markdown>
### [Event Streaming](event-streaming.md)
Real-time SSE event stream for dashboards and monitoring — subscribe to action outcomes as they happen.
</div>

</div>

## Advanced Features

<div class="grid" markdown>

<div class="card" markdown>
### [Human Approvals](approvals.md)
Require human approval before executing sensitive actions.
</div>

<div class="card" markdown>
### [Task Chains](chains.md)
Orchestrate multi-step workflows where each step feeds the next.
</div>

<div class="card" markdown>
### [LLM Guardrails](llm-guardrails.md)
AI-powered content evaluation and action gating.
</div>

<div class="card" markdown>
### [Semantic Routing](semantic-routing.md)
Route actions by meaning using vector embeddings and cosine similarity.
</div>

<div class="card" markdown>
### [Audit Trail](audit-trail.md)
Comprehensive, searchable record of every action and its outcome.
</div>

<div class="card" markdown>
### [Dry-Run Mode](dry-run.md)
Test rules without executing actions — see what *would* happen before it happens.
</div>

<div class="card" markdown>
### [Time-Based Rules](time-based-rules.md)
Apply rules based on time of day, day of week, or date — business hours, weekend routing, maintenance windows.
</div>

<div class="card" markdown>
### [Action Replay](action-replay.md)
Reconstruct and re-dispatch actions from the audit trail — recover from outages, fix suppressed actions, bulk reprocess.
</div>

</div>
