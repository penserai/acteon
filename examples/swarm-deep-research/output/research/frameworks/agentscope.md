# AgentScope (Alibaba)

## Overview

AgentScope is an open-source multi-agent framework developed by Alibaba's ModelScope team, first released in early 2024. Its defining feature is **native distributed execution** — agents can run across multiple processes or machines without framework-level changes to agent code. The project migrated from `modelscope/agentscope` to the `agentscope-ai` GitHub org in 2025-2026, reflecting its growth beyond the ModelScope ecosystem.

- **License**: Apache 2.0
- **Language**: Python 3.10+
- **Install**: `pip install agentscope`
- **Current status**: Actively maintained — biweekly community meetings since January 2026, 246+ commits on main

The framework targets production deployments where fault tolerance, scalability, and heterogeneous model support matter. It is less opinionated about workflow than MetaGPT and more infrastructure-focused than OpenAI Swarm.

---

## Architecture & Core Concepts

### Message (`Msg`)

The fundamental data unit. Every agent interaction is a `Msg` object — a typed dict with mandatory `name` and `content` fields, optional `role`, and an optional `url` for multi-modal payloads. Multi-modal data uses **lazy loading**: only the URL is stored; actual bytes are fetched on demand.

```python
from agentscope.message import Msg

msg = Msg(name="user", content="Summarize this document", role="user", url="file:///docs/report.pdf")
```

### Agent

Two mandatory interfaces define every agent:

- `reply(msg) -> Msg` — receive a message, return a response
- `observe(msg)` — receive a message passively without generating a response

This split enables passive listeners (monitors, loggers, aggregators) alongside active participants. Custom agents subclass `AgentBase` and override `reply()`:

```python
from agentscope.agents import AgentBase

class SummarizerAgent(AgentBase):
    def reply(self, x: Msg) -> Msg:
        prompt = self.model.format(self.sys_prompt, x)
        response = self.model(prompt)
        return Msg(self.name, response.text, role="assistant")
```

### Pipelines

AgentScope provides five composable pipeline primitives in `agentscope.pipelines`:

| Pipeline | Behavior |
|----------|----------|
| `sequential` | Execute agents in order, output of each feeds the next |
| `parallel` | Execute agents concurrently, collect all outputs |
| `if-else` | Conditional branching based on a predicate |
| `switch` | Multi-way dispatch on a value |
| `loop` | Repeat until a condition is met |

These can be nested arbitrarily to build complex DAG workflows without a separate orchestration DSL.

### Message Hub (`MsgHub`)

`MsgHub` implements group broadcast: when any participant calls `reply()`, the resulting message is routed to all other participants via `observe()`. This is the primary primitive for multi-party conversations — debates, voting rounds, panel discussions.

```python
from agentscope.msghub import msghub

with msghub(participants=[agent_a, agent_b, agent_c]) as hub:
    hub.broadcast(Msg("moderator", "State your position", role="user"))
    for agent in [agent_a, agent_b, agent_c]:
        agent.reply(None)  # each replies; others observe automatically
```

---

## Agent Communication Model

Communication is **message-passing only** — no shared mutable state between agents. Agents are isolated; coordination happens through `Msg` objects or `MsgHub` broadcast.

For distributed deployments, AgentScope uses an **actor-based model over gRPC**:

- Each agent can be promoted to a **distributed agent** by calling `.to_dist()`, which spawns it in a separate process (or remote machine) and replaces the local object with a transparent proxy
- Remote procedure calls are synchronous by default; async variants exist for fire-and-forget patterns
- A **server process** manages agent lifecycle, message routing, and health checks

```python
# Local agent
agent = MyAgent(name="worker")

# Distributed agent — same interface, runs in separate process
agent = MyAgent(name="worker").to_dist(host="10.0.0.5", port=12345)

# Caller code is identical either way
response = agent.reply(msg)
```

This transparency means the same agent code runs locally during development and across a cluster in production.

---

## Tool & Capability System

### Service Functions

Services are the primary tool abstraction — plain Python functions decorated or registered to produce `ServiceResponse` objects:

```python
from agentscope.service import ServiceResponse, ServiceExecStatus

def web_search(query: str) -> ServiceResponse:
    results = _do_search(query)
    return ServiceResponse(status=ServiceExecStatus.SUCCESS, content=results)
```

### Built-in Services

AgentScope ships a substantial standard library of services:

| Category | Services |
|----------|----------|
| Web | `bing_search`, `google_search`, `arxiv_search`, `download_from_url` |
| Code | `execute_python_code`, `exec_shell` |
| File I/O | `read_text_file`, `write_text_file`, `list_directory_content` |
| Data | `query_mysql`, `query_mongodb` |
| Communication | `send_email`, `post_message` |

### Tool Integration

Services are wrapped into **Tools** — structured descriptors (name, description, parameter schema) consumed by the LLM's native tool-use interface:

```python
from agentscope.service import ServiceToolkit

toolkit = ServiceToolkit()
toolkit.add(web_search, query="user-provided")
toolkit.add(execute_python_code)

agent = ReActAgent(name="analyst", model_config_name="gpt-4o", service_toolkit=toolkit)
```

The `ReActAgent` handles the reasoning loop (observe → think → act → observe) automatically.

---

## Memory & Knowledge Management

AgentScope provides a pluggable `MemoryBase` interface. The default `TemporaryMemory` stores conversation history in-process. Agents access memory via:

```python
agent.memory.add(msg)            # store
agent.memory.get_memory(k=5)     # retrieve last 5 messages
agent.memory.delete_by_func(fn)  # prune by custom predicate
```

For knowledge-base integration, AgentScope wraps common vector stores (FAISS, Milvus) through a `KnowledgeBank` abstraction. Documents are chunked, embedded, and retrieved via similarity search — the retrieval results are then injected into the agent's context window before calling the LLM.

Memory is local to each agent instance by default. In distributed mode, sharing memory across agents requires explicit coordination through services or a shared external store.

---

## GitHub & Community

| Metric | Value |
|--------|-------|
| Primary repo | `agentscope-ai/agentscope` |
| Stars | ~21,400 (agentscope-ai org) + ~6,400 (modelscope mirror) |
| Forks | ~2,100 |
| License | Apache 2.0 |
| Backing | Alibaba / ModelScope team |
| Community | Biweekly open meetings, DingTalk + Discord |
| Related projects | `agentscope-runtime`, `agentscope-java`, `ReMe` (memory), `CoPaw` (robotics) |

Active development with Alibaba engineering resources behind it. The Java port (`agentscope-java`) suggests enterprise adoption across polyglot environments.

---

## Strengths

- **True distributed execution**: `.to_dist()` is transparent — no rewrite needed to go from local to multi-machine
- **gRPC transport**: low overhead, typed, language-agnostic (Java port exists)
- **Rich built-in services**: web search, code execution, DB queries out of the box
- **Multi-modal first**: `Msg.url` + lazy loading handles images, audio, documents without custom plumbing
- **Composable pipelines**: five primitives cover most workflow topologies without a custom DSL
- **Apache 2.0**: no license friction for commercial use
- **Active Alibaba backing**: sustained engineering investment, not a research prototype

---

## Weaknesses

- **Python 3.10+ required**: rules out environments locked to older Python
- **gRPC dependency**: operational complexity — certificates, service discovery, firewall rules in production
- **Memory is per-agent**: cross-agent knowledge sharing requires explicit external storage
- **Less opinionated**: no built-in role hierarchy or SOP templates (unlike MetaGPT); more infrastructure than workflow
- **Documentation gaps**: distributed deployment docs are less complete than local usage docs
- **Smaller Western mindshare**: primarily adopted in Chinese enterprise contexts; English community is smaller than LangGraph or CrewAI

---

## Comparison with acteon-swarm

| Dimension | AgentScope | acteon-swarm |
|-----------|------------|--------------|
| **Target scale** | Multi-machine distributed clusters | Single-process or modest multi-agent |
| **Transport** | gRPC | In-process / lightweight message passing |
| **Setup overhead** | High — gRPC servers, `.to_dist()` configuration | Low — pure Python, minimal deps |
| **Fault tolerance** | Built-in — actor supervision, remote restart | Not a primary concern |
| **Workflow model** | Pipeline primitives (sequential, parallel, if-else, loop) | Orchestrator-defined control flow |
| **LLM coupling** | Model-agnostic; supports local + API models | Depends on orchestrator design |
| **Multi-modal** | Native (`Msg.url`, lazy loading) | Not native |
| **Best fit** | Enterprise deployments needing horizontal scaling | Research, lightweight orchestration, rapid iteration |

**Key tradeoff**: AgentScope's distribution model is a genuine capability advantage for workloads that benefit from parallelism or isolation (long-running agents, heterogeneous model endpoints, fault-isolated subtasks). acteon-swarm's lightweight design wins on iteration speed, operational simplicity, and debuggability. For a research prototype exploring agent coordination patterns, acteon-swarm's lower overhead is appropriate; for a production system serving thousands of parallel sessions, AgentScope's infrastructure investment pays off.

---

## References

- GitHub: https://github.com/agentscope-ai/agentscope
- PyPI: https://pypi.org/project/agentscope/
- Documentation: https://agentscope.readthedocs.io/
- Paper: "AgentScope: A Flexible yet Robust Multi-Agent Platform" (arXiv 2402.14034)
- Distributed tutorial: https://agentscope.readthedocs.io/en/latest/tutorial/distribute.html
- Service toolkit docs: https://agentscope.readthedocs.io/en/latest/tutorial/service.html
