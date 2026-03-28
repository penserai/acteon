# CAMEL

## Overview

CAMEL (Communicative Agents for "Mind" Exploration) originated as an academic project from King Abdullah University of Science and Technology (KAUST), introduced in the 2023 NeurIPS paper *"CAMEL: Communicative Agents for 'Mind' Exploration of Large Language Model Society"*. It was among the first frameworks to formalize **autonomous multi-agent conversations** driven entirely by role-playing — without a human in the loop after the initial prompt.

The project has since evolved from a research prototype into a general-purpose multi-agent framework under the `camel-ai` organization. The current package (`camel-ai`, PyPI) supports a wide range of LLM backends, tool integrations, and a higher-level workforce abstraction.

- **License**: Apache 2.0
- **Python**: 3.10+
- **Core installation**: `pip install camel-ai`
- **Extended (all tools)**: `pip install 'camel-ai[all]'`

---

## Architecture & Core Concepts

### Role-Playing via Inception Prompting

CAMEL's foundational insight is that two LLM agents can drive each other to task completion if given carefully engineered system prompts at session initialization — a technique called **inception prompting**.

Three coordinated prompts are injected at the start of each session:

| Prompt | Symbol | Purpose |
|--------|--------|---------|
| Task Specifier | PT | Refines an abstract human goal into a concrete, actionable task |
| Assistant System | PA | Assigns the assistant a role and provides context about the user agent |
| User System | PU | Assigns the user agent a role and instructs it to issue step-by-step directives |

Once initialized, the **AI User** agent issues instructions and the **AI Assistant** agent executes them and reports results. This loop continues until a termination condition is met: the assistant signals task completion, the maximum turn count is reached, or an agent issues a refusal.

```python
from camel.societies import RolePlaying

session = RolePlaying(
    assistant_role_name="Python Programmer",
    user_role_name="Stock Trader",
    task_prompt="Develop a trading bot for the stock market",
)

# Agents auto-prompt each other — no human input required
input_msg = session.init_chat()
while True:
    assistant_response, user_response = session.step(input_msg)
    if "CAMEL_TASK_DONE" in user_response.msg.content:
        break
    input_msg = assistant_response.msg
```

The engineered safeguards embedded in the system prompts prevent role-flipping (where the assistant starts directing the user), block harmful outputs, and keep both agents aligned with the original human intent throughout the conversation.

---

## Agent Communication Model

CAMEL's communication is **conversational and turn-based**. Every message is a `BaseMessage` — a typed envelope carrying role, content, and metadata. There is no message bus, event queue, or shared memory between agents; coordination happens entirely through the exchange of natural-language messages within the role-playing session.

```python
from camel.messages import BaseMessage
from camel.types import RoleType

msg = BaseMessage(
    role_name="Python Programmer",
    role_type=RoleType.ASSISTANT,
    content="Here is the implementation of the moving average strategy...",
    meta_dict={},
)
```

For more complex pipelines, CAMEL provides a **Workforce** abstraction that coordinates multiple task-specific agents. A coordinator agent decomposes a high-level goal into subtasks and dispatches them to specialized worker agents, collecting and synthesizing results.

```python
from camel.workforce import Workforce

workforce = Workforce("Research Team")
workforce.add_single_agent_worker("Web Researcher", worker=researcher_agent)
workforce.add_single_agent_worker("Data Analyst", worker=analyst_agent)
result = workforce.process_task(task)
```

This is closer to a task-queue model than the peer-to-peer conversation of the base role-playing setup.

---

## Tool & Capability System

CAMEL organizes external capabilities into **Toolkits** — grouped collections of related tools that can be attached to any `ChatAgent`.

```python
from camel.toolkits import SearchToolkit, CodeExecutionToolkit

agent = ChatAgent(
    system_message=system_msg,
    tools=[*SearchToolkit().get_tools(), *CodeExecutionToolkit().get_tools()],
)
```

Built-in toolkits include:

| Toolkit | Capabilities |
|---------|-------------|
| `SearchToolkit` | Google, DuckDuckGo, Wikipedia, arXiv search |
| `CodeExecutionToolkit` | Sandboxed Python execution (Docker or subprocess) |
| `BrowserToolkit` | Playwright-based web browsing |
| `FileToolkit` | Local file read/write/search |
| `GithubToolkit` | Repo exploration, issue/PR management |
| `MathToolkit` | Symbolic math via SymPy |
| `LinkedInToolkit`, `SlackToolkit` | Social/messaging integrations |

CAMEL also supports **MCP (Model Context Protocol)** for connecting to external tool servers, and integrates with LangChain tools and OpenAI function calling natively.

---

## Memory & Knowledge Management

Each `ChatAgent` maintains its own **context window** managed by a `BaseContextCreator`, which determines how conversation history is trimmed or summarized when approaching token limits. Three strategies are available: `ScoreBasedContextCreator` (retains highest-scored messages), `SummaryContextCreator` (auto-summarizes old turns), and a simple sliding window.

For long-horizon tasks and retrieval, CAMEL provides a **RAG pipeline**:

```python
from camel.storages import QdrantStorage
from camel.retrievers import VectorRetriever

storage = QdrantStorage(vector_dim=1536)
retriever = VectorRetriever(embedding_model=embedding, storage=storage)
retriever.process(content="path/to/docs")
context, score = retriever.query("relevant question")
```

Supported vector backends include Qdrant, Milvus, and pgvector. An `AutoRetriever` wrapper handles both embedding and retrieval in a single call, making RAG setup low-friction for most use cases.

---

## GitHub & Community

| Metric | Value |
|--------|-------|
| Repository | `camel-ai/camel` |
| Stars | ~16,500 |
| Forks | ~1,800 |
| License | Apache 2.0 |
| Contributors | 200+ |
| Community members | 30,000+ |
| OWL extension stars | ~19,300 (`camel-ai/owl`) |

The original NeurIPS 2023 paper has been cited over 500 times. The framework remains actively maintained with 2,160+ commits and regular releases. The `camel-ai/owl` repository demonstrates a production-grade OWL (Optimized Workforce Layout) system built on top of CAMEL, showing the framework's capacity to underpin competitive agent benchmarks.

---

## Strengths

- **Academic foundation**: Grounded in peer-reviewed research with a clear theoretical model, not just engineering heuristics.
- **Role-playing paradigm**: Inception prompting enables fully autonomous agent collaboration without any mid-session human input.
- **Rich toolkit ecosystem**: 20+ built-in toolkits covering search, code execution, browser control, and popular APIs.
- **Flexible LLM backend**: Supports OpenAI, Anthropic, Mistral, Ollama, Gemini, and others through a unified `ModelFactory`.
- **RAG integration**: First-class support for vector storage and retrieval with multiple backend options.
- **Active community**: 30,000+ members, strong academic citation record, and active open-source contributors.
- **Apache 2.0 license**: Permissive, suitable for commercial use.

---

## Weaknesses

- **Conversational overhead**: The role-playing loop generates verbose turn-by-turn dialogue for every task step, increasing token consumption compared to direct function dispatch.
- **Limited parallelism**: The base `RolePlaying` session is inherently sequential. The `Workforce` layer adds concurrency, but it's less mature than purpose-built distributed frameworks (e.g., AgentScope).
- **Role-playing brittleness**: Inception prompting works well for well-scoped tasks but can degrade on open-ended or ambiguous goals — agents may loop, diverge, or produce verbose non-solutions.
- **Steep prompt engineering curve**: Getting quality results requires careful crafting of role names and task prompts; the framework does not abstract this away.
- **Research-first ergonomics**: The API surface reflects academic experimentation priorities; production hardening (retries, timeouts, circuit breakers) requires custom implementation.

---

## Comparison with acteon-swarm

| Dimension | CAMEL | acteon-swarm |
|-----------|-------|--------------|
| **Paradigm** | Role-playing conversation loop | Task-graph / function dispatch |
| **Orchestration** | Peer-to-peer (user ↔ assistant) or Workforce coordinator | Central orchestrator with tool-calling agents |
| **Communication** | Structured natural-language messages | Direct function calls / structured payloads |
| **Parallelism** | Sequential by default; Workforce adds limited concurrency | Native parallel task execution |
| **State management** | Per-agent context window; optional vector RAG | Shared state object passed through task graph |
| **Tool integration** | Toolkit objects; 20+ built-ins + MCP | Lightweight function registration |
| **Primary use case** | Open-ended exploration, academic research, agent behavior studies | Focused task pipelines, production automation |
| **Prompt engineering burden** | High — role names and task prompts matter significantly | Low — orchestration is structural, not conversational |
| **Observability** | Conversation logs; limited structured tracing | Structured task graph; easier to instrument |

**When to choose CAMEL**: Tasks where the emergent behavior of two agents negotiating a solution adds value — creative exploration, research synthesis, or studying agent interaction patterns. The role-playing model also makes it easier to inject domain expertise through role framing.

**When to choose acteon-swarm**: Production pipelines where predictability, parallelism, and low token overhead matter. Direct function dispatch is faster, cheaper, and easier to test than conversational coordination.

---

## References

- Zheng et al., *"CAMEL: Communicative Agents for 'Mind' Exploration of Large Language Model Society"*, NeurIPS 2023. [arXiv:2303.17760](https://arxiv.org/abs/2303.17760)
- CAMEL GitHub: https://github.com/camel-ai/camel
- OWL extension: https://github.com/camel-ai/owl
- CAMEL documentation: https://docs.camel-ai.org
- PyPI: https://pypi.org/project/camel-ai/
