# Swarm (OpenAI)

## Overview

OpenAI Swarm was released on October 12, 2024 as an **experimental, educational framework** for orchestrating multi-agent LLM systems. It was never positioned as a production library — the README explicitly described it as "an educational framework exploring ergonomic interfaces for multi-agent systems." It was not published to PyPI; installation required cloning directly from GitHub (`openai/swarm`).

As of March 2025, Swarm has been **superseded by the OpenAI Agents SDK**, which productionizes the same concepts with persistence, tracing, guardrails, and a supported release cycle. The Swarm repo now carries a prominent migration notice. Despite this, the repo accumulated 21,200+ stars and 2,300+ forks, signaling strong community interest in its minimalist approach.

**Design philosophy**: Swarm's defining bet is that multi-agent orchestration should be reducible to two primitives — *routines* and *handoffs* — both expressible in plain Python and plain English without any new abstractions beyond the OpenAI Chat API. The framework adds fewer than 1,000 lines of code on top of the standard `openai` client.

---

## Architecture & Core Concepts

### Agents

An `Agent` is a named bundle of three things: a model, a system prompt (instructions), and a list of Python functions it can call.

```python
from swarm import Agent

billing_agent = Agent(
    name="Billing Support",
    model="gpt-4o",
    instructions="You handle billing questions. Be concise and helpful.",
    functions=[check_invoice, transfer_to_refunds],
)
```

Instructions can be a plain string or a callable that receives `context_variables` and returns a string, enabling dynamic system prompts:

```python
def instructions(context_variables):
    name = context_variables.get("user_name", "customer")
    return f"You are helping {name}. Be personalized and direct."
```

### Routines

A "routine" is not a class or special construct — it is the agent's `instructions` field describing a sequence of steps in natural language. The LLM treats the system prompt as a program and follows it:

```
1. Greet the customer and ask for their order number.
2. Look up the order using get_order_status().
3. If the item is damaged, transfer to the refunds agent.
4. Otherwise, confirm the resolution and close the ticket.
```

This is the framework's most opinionated design choice: structured control flow encoded in prose, interpreted by the model itself.

### Handoffs

A handoff occurs when a tool function returns an `Agent` object. Swarm's run loop detects this and switches the active agent for subsequent turns, while preserving the full message history:

```python
refunds_agent = Agent(name="Refunds", instructions="Process refund requests.", functions=[issue_refund])

def transfer_to_refunds():
    """Transfer the conversation to the refunds specialist."""
    return refunds_agent

triage_agent = Agent(
    name="Triage",
    instructions="Route customers to the right department.",
    functions=[transfer_to_refunds],
)
```

The function docstring becomes the tool description the LLM sees. Returning an `Agent` is the entire handoff mechanism — no message bus, no pub/sub, no coordinator.

### Context Variables

A shared `dict` is threaded through the entire run, available to both instruction callables and tool functions:

```python
def get_account_balance(context_variables: dict) -> str:
    user_id = context_variables["user_id"]
    return fetch_balance(user_id)

result = client.run(
    agent=billing_agent,
    messages=[{"role": "user", "content": "What's my balance?"}],
    context_variables={"user_id": "u_12345", "user_name": "Alice"},
)
```

Swarm auto-injects `context_variables` when a function declares the parameter. Functions can return a `Result` object to update context variables mid-run:

```python
from swarm.types import Result

def authenticate_user(user_id: str) -> Result:
    return Result(
        value="User authenticated.",
        context_variables={"authenticated": True, "user_id": user_id},
    )
```

---

## Agent Communication Model

Swarm uses **direct function-based handoffs** with no intermediary. There is no message bus, no shared state object, no coordinator agent, and no event system. Communication flow is:

1. Active agent receives messages and calls tools via the Chat Completions API.
2. If a tool returns an `Agent`, the run loop replaces the active agent.
3. The full conversation history is passed to the new agent unchanged.
4. If a tool returns a `Result`, its `context_variables` dict is merged into the shared context.

This model is intentionally linear: one agent is active at a time. Parallel agent execution is out of scope. The simplicity makes the control flow easy to trace and debug, which aligns with the educational framing.

---

## Tool & Capability System

Tools are ordinary Python functions. Swarm introspects their type annotations and docstrings to generate JSON schemas for the OpenAI function-calling API automatically:

```python
def check_order_status(order_id: str, context_variables: dict) -> str:
    """Check the current status of an order by its ID."""
    order = db.get_order(order_id)
    return f"Order {order_id} is {order.status}."
```

Return value conventions:
- **`str`** — added to the conversation as a tool result.
- **`Agent`** — triggers a handoff to that agent.
- **`Result`** — carries a string value, optional agent handoff, and optional context variable updates.

There is no plugin registry, no capability graph, no tool versioning. Functions are attached directly to agents at construction time.

---

## Memory & Knowledge Management

Swarm has **no memory system**. This is a deliberate design choice, not an omission. The framework is stateless between `client.run()` calls: no persistent conversation storage, no vector retrieval, no user profiles.

Within a single run, shared state is managed via:
- **Message history** — the full conversation list passed between agents.
- **Context variables** — a flat `dict` available to all functions and instruction callables in the run.

If cross-session memory is needed, the calling application must manage it externally and inject it via `context_variables` or `messages`. The framework makes no prescription about how to do this.

---

## GitHub & Community

| Metric | Value |
|--------|-------|
| Repository | `openai/swarm` |
| Stars | 21,200+ |
| Forks | 2,300+ |
| Released | October 12, 2024 |
| License | MIT |
| Status | Superseded by OpenAI Agents SDK (March 2025) |

The codebase is ~1,000 lines across a handful of files. Community forks have extended it with persistence layers, async support, streaming, and alternative LLM backends. The conceptual vocabulary (agents, handoffs, routines) carried directly into the production Agents SDK.

---

## Strengths

- **Extreme simplicity**: The entire framework fits in a single reading session. No new abstractions beyond Python functions and dicts.
- **Transparent control flow**: One active agent at a time; handoffs are explicit function returns. Easy to trace, test, and debug.
- **Native Python tooling**: No DSL, no config files, no decorators required. Tools are just functions.
- **Low coupling**: Agents are independent; dependencies are injected via context variables, not global state.
- **Educational clarity**: Demonstrates that multi-agent orchestration doesn't require complex infrastructure.

---

## Weaknesses

- **No production support**: Explicitly experimental; superseded and unmaintained.
- **No parallelism**: Single active agent; no concurrent tool execution or fan-out patterns.
- **No persistence**: Stateless by design; session continuity requires external implementation.
- **No observability**: No tracing, logging hooks, or built-in retry logic.
- **Fragile control flow**: Routing logic lives in natural-language instructions; susceptible to LLM interpretation drift.
- **OpenAI-only**: Hardcoded to OpenAI's Chat Completions API; no multi-provider support without forking.

---

## Comparison with acteon-swarm

Both projects use the "swarm" metaphor but differ sharply in scope and philosophy.

| Dimension | OpenAI Swarm | acteon-swarm |
|---|---|---|
| **Metaphor** | Handoff-based routing — one agent active at a time | Emergent collective — agents as concurrent actors |
| **Orchestration** | Implicit, via LLM routing decisions | Explicit, structured coordination layer |
| **State** | Stateless; context variables as ephemeral dict | Persistent state management across sessions |
| **Parallelism** | None — sequential agent chain | Concurrent agent execution |
| **Scope** | Educational proof-of-concept | Production system |
| **Tool binding** | Python functions, introspected at runtime | Typed capability registry |
| **Primary use case** | Teaching multi-agent concepts | Running real workloads |

OpenAI Swarm proves the minimalist thesis: handoffs and routines are sufficient primitives for many agent workflows. acteon-swarm takes the next step — applying the swarm metaphor to systems that need durability, scale, and operational reliability. Where Swarm asks "how simple can this be?", acteon-swarm asks "how far can this scale?".

---

## References

- [openai/swarm GitHub repository](https://github.com/openai/swarm)
- [OpenAI Swarm announcement blog post](https://openai.com/index/new-tools-to-build-agents/) (October 2024)
- [OpenAI Agents SDK (successor)](https://openai.github.io/openai-agents-python/)
- [Swarm README — routines and handoffs design notes](https://github.com/openai/swarm/blob/main/README.md)
- [OpenAI function calling documentation](https://platform.openai.com/docs/guides/function-calling)
