# AutoGen (Microsoft)

## Overview

AutoGen originated in Microsoft Research as an experimental framework for building LLM applications using multiple conversational agents. Published in September 2023, it quickly became one of the most starred agent frameworks on GitHub — reaching 56,300+ stars — by offering a practical, conversation-centric abstraction over the raw Chat Completions API.

**Current state**: AutoGen exists in four parallel lineages following a governance split in November 2024. The `microsoft/autogen` repository on `main` is now **AutoGen 0.4** — a complete architectural rewrite based on an async actor model with no backward compatibility. The `0.2` branch remains community-maintained (latest: v0.2.38). Original creators Chi Wang and Qingyun Wu departed Microsoft on November 13, 2024, and forked to **AG2** (`ag2ai/ag2`, Apache 2.0) with contributors from Google, IBM, Meta, Penn State, and UW, explicitly preserving the v0.2 API under community governance.

An important packaging confusion: the PyPI packages `autogen`, `pyautogen`, and `ag2` all install **identical AG2 code** — they are the same library under three names. Only `autogen-agentchat` and `autogen-core` correspond to Microsoft's 0.4 rewrite.

**Licenses**: `microsoft/autogen` — MIT (code), CC-BY-4.0 (docs); `ag2ai/ag2` — Apache 2.0.

---

## Architecture & Core Concepts

### v0.2 / AG2 — Conversational Agent Model

The central abstraction is `ConversableAgent`: a stateful, message-receiving agent with a configurable reply chain. Two concrete subclasses handle the vast majority of real workloads:

```python
from autogen import AssistantAgent, UserProxyAgent

assistant = AssistantAgent(
    name="assistant",
    llm_config={"model": "gpt-4o", "api_key": "..."},
)

user_proxy = UserProxyAgent(
    name="user_proxy",
    human_input_mode="NEVER",       # NEVER / TERMINATE / ALWAYS
    code_execution_config={"work_dir": "coding", "use_docker": True},
    max_consecutive_auto_reply=10,
)

user_proxy.initiate_chat(assistant, message="Write a Python script to scrape Hacker News headlines.")
```

- **`AssistantAgent`** — LLM-backed; writes and reviews code, proposes plans, responds to instructions. Does not solicit human input by default.
- **`UserProxyAgent`** — acts as human proxy; auto-extracts and executes code blocks from assistant messages; can ask a real human when `human_input_mode="ALWAYS"`.

### GroupChat

For N-agent coordination, `GroupChat` manages the agent pool and `GroupChatManager` selects speakers:

```python
from autogen import GroupChat, GroupChatManager

group_chat = GroupChat(
    agents=[user_proxy, coder, reviewer, planner],
    messages=[],
    max_round=20,
    speaker_selection_method="auto",  # or "round_robin", "random", or a callable
)

manager = GroupChatManager(groupchat=group_chat, llm_config=llm_config)
user_proxy.initiate_chat(manager, message="Implement and test a binary search tree.")
```

The manager calls the LLM each round to choose the next speaker, given the full conversation history and each agent's description. Custom speaker selection methods can implement FSM-constrained transitions — a directed adjacency matrix that restricts which agents can follow which.

### v0.4 Architecture — Actor Model Rewrite

AutoGen 0.4 (`autogen-core`) abandons the synchronous, callback-based design in favor of a proper **async actor model**:

- Every agent is an independent async actor with a mailbox.
- Agents publish and subscribe to **typed message topics** via an event bus (`SingleThreadedAgentRuntime` or `WorkerAgentRuntime`).
- The `AgentChat` layer (`autogen-agentchat`) re-implements the familiar two-agent and group-chat patterns on top of this runtime.
- Teams replace GroupChat: `RoundRobinGroupChat`, `SelectorGroupChat`, `Swarm`, `MagenticOneGroupChat`.

The 0.4 model enables cross-process and distributed deployments, persistent agents, and true async parallelism — at the cost of a steeper API surface.

---

## Agent Communication Model

In v0.2/AG2, communication is **conversational turn-taking**. Every message exchange is a dialogue entry appended to a shared list:

- **Two-agent**: The canonical pattern. `UserProxyAgent` and `AssistantAgent` exchange messages until a termination condition triggers (`TERMINATE` in reply, max rounds reached, or human says stop).
- **Group chat**: One-at-a-time broadcast model. The manager selects the next speaker; that agent sends a message visible to all others. No peer-to-peer side channels.
- **Nested chats**: An agent can spawn a sub-conversation with a different agent or group during its turn, collect the result, and return it to the outer conversation. Useful for research-then-report patterns.
- **FSM graphs**: A transition dict restricts allowed next-speaker sequences, adding determinism to otherwise LLM-driven routing.

In v0.4, communication shifts to **typed event publishing**. Agents declare `@message_handler` methods for specific message types. The runtime routes messages to handlers based on type and subscription, supporting fan-out, fan-in, and broadcast patterns natively.

---

## Tool & Capability System

Tools are registered as Python functions with JSON schema annotations. In v0.2, they attach to agents via `register_for_llm` (schema registration) and `register_for_execution` (actual callable):

```python
from autogen import register_function

def web_search(query: str) -> str:
    """Search the web and return a summary."""
    return search_api(query)

register_function(
    web_search,
    caller=assistant,       # LLM decides when to call
    executor=user_proxy,    # UserProxy executes it
    description="Search the web for information.",
)
```

**Code execution** is AutoGen's most distinctive capability: `UserProxyAgent` extracts fenced code blocks from assistant messages and runs them in a Docker container (preferred) or a local subprocess. This makes the assistant's code generation self-correcting — execution errors feed back into the conversation automatically.

In v0.4, tools are `FunctionTool` objects with explicit schemas, attached to `AssistantAgent` or custom agents. MCP server tools are supported in AG2 via `MCPTool`.

---

## Memory & Knowledge Management

**Conversation history** is the primary memory mechanism: the full message list persists for the run and is included in every LLM call. This gives all agents full context but grows token cost linearly.

**Teachable agents** (`TeachableAgent` in v0.2/AG2) persist learned facts and preferences to a vector store (ChromaDB by default). After a conversation, the agent extracts "memos" (key-value facts) and stores them; future conversations retrieve relevant memos and prepend them to the system prompt:

```python
from autogen.agentchat.contrib.capabilities import TeachableAgent

teachable = TeachableAgent(
    name="teachable_assistant",
    llm_config=llm_config,
    teach_config={"verbosity": 0, "recall_threshold": 1.5},
)
```

AG2 also exposes `Memory` integrations (ChromaDB, Neo4j via Graphiti) through its capability architecture. Long-term memory in v0.4 is handled outside the core runtime via pluggable `ChatCompletionContext` objects that can truncate or summarize history.

---

## GitHub & Community

| Property | microsoft/autogen | ag2ai/ag2 |
|---|---|---|
| Stars | 56,300+ | 4,300+ |
| Forks | 8,500+ | 568+ |
| License | MIT / CC-BY-4.0 | Apache 2.0 |
| PyPI | `autogen-agentchat`, `autogen-core` | `ag2`, `autogen`, `pyautogen` |

The November 2024 governance split was acrimonious and has created persistent confusion for newcomers. Microsoft's 0.4 rewrite is the official future; AG2 is the continuity path for teams already on v0.2. Many production deployments stay on v0.2/AG2 due to its stability and familiar API. Microsoft Semantic Kernel has embedded the AutoGen 0.4 runtime for enterprise-oriented deployments.

---

## Strengths

- **Battle-tested code execution loop**: The UserProxy ↔ Assistant pattern with Docker-sandboxed execution is mature and well-documented, covering the majority of agentic coding tasks.
- **Rich conversation topologies**: Two-agent, group chat, nested chats, FSM graphs, and swarm patterns out of the box — more structural variety than most frameworks.
- **Teachable / persistent memory**: TeachableAgent and ChromaDB integration make long-term personalization practical without custom plumbing.
- **Human-in-the-loop granularity**: `human_input_mode` on every agent provides fine-grained control from fully automated to always-supervised.
- **Strong research pedigree**: Originated from Microsoft Research; well-cited; substantial empirical work on multi-agent coordination published alongside the framework.
- **0.4 async runtime**: The actor model in v0.4 is architecturally sound for distributed deployments and large agent populations.

---

## Weaknesses

- **Governance fragmentation**: Four active lineages, three PyPI packages pointing to the same code, and a breaking v0.4 rewrite create real adoption risk. Teams must choose a branch and accept that the ecosystem is split.
- **Token cost at scale**: Passing the full conversation history to every LLM call is expensive for long runs. Truncation strategies exist but require tuning.
- **LLM-driven speaker selection is fragile**: GroupChat's default `auto` mode relies on an LLM call to pick the next speaker — an additional latency and failure point in every round.
- **Sequential group chat**: Despite multiple agents, only one speaks per round. No true parallelism in the communication model (v0.2/AG2).
- **Heavy setup for code execution**: Docker sandboxing is the recommended path but adds infrastructure overhead; local execution is faster but unsafe for arbitrary LLM-generated code.
- **0.4 migration cost**: The v0.4 API is entirely incompatible with v0.2. Teams migrating from the most popular version face a full rewrite.

---

## Comparison with acteon-swarm

AutoGen's conversational model and acteon-swarm's orchestration model solve different problems at different layers.

| Dimension | AutoGen (v0.2/AG2) | acteon-swarm |
|---|---|---|
| **Coordination unit** | Dialogue turn in a shared conversation | Task dispatch to concurrent agents |
| **Parallelism** | Sequential (one speaker per round) | Concurrent agent execution |
| **Code execution** | First-class, sandboxed, self-correcting | External tool capability |
| **State model** | Full conversation history per run | Persistent state across sessions |
| **Human-in-loop** | Native `human_input_mode` per agent | Configurable at orchestration layer |
| **Architecture** | Conversation threads / actor events | Swarm task graph |
| **Primary use case** | Code-writing, research, dialogue tasks | Scalable workload distribution |

AutoGen's conversational framing shines for tasks where iterative back-and-forth produces quality: code generation, multi-step research, document review. The self-correcting execution loop — assistant writes code, proxy runs it, error feeds back, assistant fixes — is a tight feedback cycle that is genuinely hard to replicate with pure task-dispatch architectures.

acteon-swarm's strength is orchestrating many parallel agents on decomposed workloads, with durable state and operational reliability. Where AutoGen asks "how do agents reason together?", acteon-swarm asks "how do we deploy agents at scale?" They are complementary rather than competing — AutoGen's agent pairs could be actors in an acteon-swarm topology.

---

## References

- [microsoft/autogen GitHub repository](https://github.com/microsoft/autogen)
- [ag2ai/ag2 GitHub repository](https://github.com/ag2ai/ag2)
- [AutoGen 0.4 documentation](https://microsoft.github.io/autogen/stable/)
- [AG2 documentation](https://ag2.ai/docs/Getting-Started)
- [AutoGen original research paper — Wu et al., 2023](https://arxiv.org/abs/2308.08155)
- [AG2 announcement (Chi Wang, Nov 2024)](https://ag2.ai/blog/announcing-ag2)
- [AutoGen 0.4 migration guide](https://microsoft.github.io/autogen/stable/user-guide/agentchat-user-guide/migration-guide.html)
