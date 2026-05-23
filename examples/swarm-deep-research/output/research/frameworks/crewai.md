# CrewAI

## Overview

CrewAI is an open-source Python framework for orchestrating autonomous multi-agent systems, founded by João Moura and first released in late 2023. It has grown into one of the most widely adopted agent frameworks, claiming **100,000+ certified developers** through its learning platform at learn.crewai.com.

**Current version**: v1.12.2 (March 26, 2026), with v1.13.0rc1 pre-release (March 27, 2026)
**License**: MIT
**Python**: 3.10–3.13
**LangChain dependency**: Removed — CrewAI is now fully independent

CrewAI's central thesis is that real-world tasks are best solved by teams of specialized AI agents collaborating with defined roles and responsibilities — analogous to how human organizations structure work. It provides first-class abstractions for **role-based agents**, **structured tasks**, and two distinct orchestration models (sequential and hierarchical).

---

## Architecture & Core Concepts

CrewAI uses a **two-layer architecture** that separates structural orchestration from autonomous execution:

```
FLOWS  (structural: state machines, event-driven branching, pipeline logic)
  └── CREWS  (execution: autonomous agent teams tackling complex tasks)
         └── AGENTS + TASKS
```

### Flows

`Flow` is the outer orchestration layer. It defines **how** the overall pipeline progresses — conditional branching, state management, error recovery, and sequential invocation of Crews. Flows emit and listen to events, enabling stateful pipelines without tangling business logic inside agents.

```python
from crewai.flow.flow import Flow, listen, start

class ResearchFlow(Flow):
    @start()
    def collect_requirements(self):
        return self.state.topic

    @listen(collect_requirements)
    def run_research_crew(self, topic):
        crew = ResearchCrew(topic=topic)
        return crew.kickoff()
```

### Crews

A `Crew` is the primary unit of autonomous work. It bundles a list of `Agent`s and `Task`s, runs them through a chosen process model, and returns a final output.

```python
from crewai import Crew, Process

crew = Crew(
    agents=[researcher, analyst, writer],
    tasks=[research_task, analysis_task, writing_task],
    process=Process.sequential,
    verbose=True,
)
result = crew.kickoff()
```

### Agents

Agents are the autonomous actors. The key design choice is that each agent is defined by a **role, goal, and backstory** — a character specification that governs how the LLM interprets its instructions and constrains what it does.

```python
from crewai import Agent

researcher = Agent(
    role="Senior Research Analyst",
    goal="Uncover cutting-edge developments in {topic}",
    backstory="You are an expert at synthesizing complex data into clear insights.",
    tools=[web_search, scraper],
    llm="gpt-4o",
    memory=True,
    allow_delegation=False,
    max_iter=5,
)
```

Key `Agent` fields:
| Field | Purpose |
|---|---|
| `role` | Defines the agent's persona and framing |
| `goal` | What the agent is trying to achieve |
| `backstory` | System-prompt context shaping LLM behavior |
| `tools` | Callable capabilities available to the agent |
| `llm` | Model used (any LiteLLM-compatible provider) |
| `memory` | Whether the agent uses memory |
| `allow_delegation` | Whether the agent can hand subtasks to others |
| `max_iter` | Circuit breaker on agent reasoning loops |

### Tasks

Tasks define the **units of work** assigned to agents. They specify what should be done and what constitutes a valid output.

```python
from crewai import Task

research_task = Task(
    description="Research the latest advances in {topic}. Focus on peer-reviewed sources from the last 12 months.",
    expected_output="A structured summary of 5 key findings with source citations.",
    agent=researcher,
    context=[prior_task],          # inject outputs from other tasks
    guardrails=[validate_citations],
    output_file="findings.md",
)
```

Key `Task` fields:
| Field | Purpose |
|---|---|
| `description` | What the agent must do (supports `{variable}` interpolation) |
| `expected_output` | Defines the acceptance criterion for the output |
| `agent` | Which agent owns this task |
| `context` | List of prior tasks whose outputs are injected as context |
| `guardrails` | Validation callables that can reject and retry outputs |
| `output_file` | Optional path to write result to disk |

### Process Models

**Sequential** — tasks execute in list order. Each task's output is automatically passed as context to the next task. Simple, predictable, easy to reason about.

**Hierarchical** — a manager agent (auto-generated or explicitly configured) receives the task list, delegates to worker agents, and synthesizes their outputs. Worker agents do not communicate with each other directly; all coordination flows through the manager. This is CrewAI's production model for complex, multi-branch workflows.

**Consensual** — planned but not yet released as of March 2026.

---

## Agent Communication Model

CrewAI uses a **hub-and-spoke communication model**. There is no peer-to-peer agent communication. The three mechanisms are:

1. **Task context chaining**: Task outputs are passed to downstream tasks via the `context` parameter. The receiving agent sees the upstream output as additional context in its prompt.

2. **Manager delegation** (Hierarchical process): The manager agent decomposes the goal and dispatches subtasks to worker agents. Workers respond to the manager; they never address each other.

3. **Agent delegation** (`allow_delegation=True`): An agent can ask another agent (by role) to handle a subtask during execution. This still routes through CrewAI's orchestration layer, not directly.

```python
# Hierarchical: manager is auto-generated by the framework
crew = Crew(
    agents=[researcher, coder, reviewer],
    tasks=[main_task],
    process=Process.hierarchical,
    manager_llm="gpt-4o",  # explicit manager model
)
```

The hub-and-spoke constraint is CrewAI's most significant architectural trade-off compared to frameworks like AutoGen that support true peer-to-peer agent conversations. The benefit is predictable, auditable coordination; the cost is reduced flexibility for emergent communication patterns.

---

## Tool & Capability System

### Built-in Tools

CrewAI ships a curated set of tools from the `crewai_tools` package:

- **Web**: `SerperDevTool` (web search), `WebsiteSearchTool` (RAG over URLs), `FirecrawlScrapeWebsiteTool`
- **Files**: `FileReadTool`, `FileWriterTool`, `DirectoryReadTool`
- **Code**: `CodeInterpreterTool`, `CodeDocsSearchTool`
- **Data**: `CSVSearchTool`, `JSONSearchTool`, `PDFSearchTool`
- **APIs**: `GithubSearchTool`, `YoutubeVideoSearchTool`

### Custom Tools

Tools are defined via subclassing `BaseTool` or using the `@tool` decorator:

```python
from crewai.tools import BaseTool

class DatabaseQueryTool(BaseTool):
    name: str = "Database Query"
    description: str = "Query the internal PostgreSQL database."

    def _run(self, query: str) -> str:
        return db.execute(query)

# Or with the decorator:
from crewai import tool

@tool("Market Data Lookup")
def get_market_data(symbol: str) -> str:
    """Fetch current price and volume for a stock symbol."""
    return market_api.quote(symbol)
```

### MCP Integration

CrewAI integrates with the **Model Context Protocol (MCP)**, enabling agents to discover and call tools from any MCP-compatible server:

```python
researcher = Agent(
    role="Research Analyst",
    goal="...",
    backstory="...",
    mcps=["snowflake", "stripe", "github"],  # connects to MCP servers
)
```

This opens access to 1,000+ pre-built integrations. Enterprise tier supports bidirectional MCP (CrewAI agents exposed as MCP servers to other systems).

---

## Memory & Knowledge Management

CrewAI provides a unified `Memory` class with four distinct memory types:

### Memory Types

| Type | Scope | Backend | Use |
|---|---|---|---|
| **Short-term** | Within a single crew run | In-memory (RAG) | Recent context within an execution |
| **Long-term** | Persists across runs | SQLite (default) | Learning from past task outcomes |
| **Entity** | Cross-run entity tracking | Embeddings + store | Remembering facts about named entities |
| **User** | Per-user personalization | Configurable | Adapting behavior to individual users |

```python
from crewai.memory import LongTermMemory, ShortTermMemory, EntityMemory
from crewai.memory.storage.rag_storage import RAGStorage

crew = Crew(
    agents=[...],
    tasks=[...],
    memory=True,                       # enables all memory types
    long_term_memory=LongTermMemory(
        storage=RAGStorage(
            embedder_config={"provider": "openai", "config": {"model": "text-embedding-3-small"}},
            storage_config={"path": "./memory"},
        )
    ),
)
```

### Knowledge Sources

Agents can be initialized with structured knowledge that is chunked and embedded into a vector store:

```python
from crewai.knowledge.source.pdf_knowledge_source import PDFKnowledgeSource
from crewai.knowledge.source.string_knowledge_source import StringKnowledgeSource

docs = PDFKnowledgeSource(file_paths=["product_manual.pdf"])
specs = StringKnowledgeSource(content="API rate limit is 1000 req/min per tenant.")

agent = Agent(
    role="Support Specialist",
    knowledge_sources=[docs, specs],
    ...
)
```

Supported knowledge source types: plain text, PDF, CSV, JSON, Excel, DOCX, and custom URL scrapers. The underlying RAG pipeline uses configurable embedders and supports OpenAI, Cohere, Google, Ollama, and others.

---

## GitHub & Community

| Metric | Value |
|---|---|
| Repository | `crewAIInc/crewAI` |
| Stars | **47,400+** |
| Forks | **6,400+** |
| License | MIT |
| Latest stable | v1.12.2 (March 26, 2026) |
| Python support | 3.10–3.13 |
| Certified devs | 100,000+ (via learn.crewai.com) |
| LangChain dep | Removed — fully standalone |

CrewAI has one of the largest community footprints of any agent framework. The project ships a dedicated learning platform, an enterprise tier (CrewAI+), and a growing ecosystem of pre-built tool integrations via `crewai_tools`. Active bi-weekly community calls and a large Discord server sustain engagement.

---

## Strengths

- **Intuitive role-based abstraction**: The `role/goal/backstory` pattern maps directly to how humans think about team composition, making it easy to design and communicate agent systems.
- **Production-grade memory**: Four-tier memory system with configurable backends — a level above most framework peers.
- **MCP ecosystem access**: Native integration with 1,000+ MCP servers eliminates most custom tooling work.
- **Two-layer architecture**: Separating `Flow` (orchestration logic) from `Crew` (autonomous execution) keeps codebases clean and testable.
- **Rich knowledge management**: Built-in chunking, embedding, and retrieval from PDFs, spreadsheets, and APIs without third-party dependencies.
- **Broad LLM compatibility**: LiteLLM integration means any OpenAI-compatible provider works with minimal configuration.
- **Task guardrails**: Validation callbacks on task outputs allow iterative correction without writing custom retry loops.
- **Community scale**: 100,000+ certified users, active ecosystem, enterprise support.

---

## Weaknesses

- **Hub-and-spoke only**: No peer-to-peer agent communication. Complex conversational agent graphs (the AutoGen model) are not expressible in the framework.
- **Verbose configuration**: Defining a crew for a simple task requires substantially more boilerplate than minimalist frameworks (Swarm, AutoGen with direct API access).
- **Manager agent opacity**: In hierarchical mode, the auto-generated manager is an LLM prompt, not a deterministic router. Debugging routing decisions requires examining raw LLM outputs.
- **Consensual process absent**: The third process model (consensus-based) has been on the roadmap but is not yet released.
- **Enterprise gate on advanced features**: Bidirectional MCP, some integrations, and production-scale memory backends are behind the CrewAI+ enterprise tier.
- **Token overhead**: Each agent carries a full role/goal/backstory prompt. In large crews, this can significantly inflate context usage per task.
- **LLM-dependent coordination**: Delegation and task routing are mediated by LLM outputs, which can produce inconsistent behavior under adversarial prompts or unusual edge cases.

---

## Comparison with acteon-swarm

CrewAI and acteon-swarm occupy different positions in the design space: CrewAI is a high-level **workflow framework** with rich abstractions; acteon-swarm is a lightweight **orchestration primitive** optimized for performance and composability.

| Dimension | CrewAI | acteon-swarm |
|---|---|---|
| **Primary metaphor** | Human team with roles and responsibilities | Emergent collective of concurrent actors |
| **Orchestration style** | Declarative (define agents/tasks/crews, framework runs them) | Imperative (build coordination logic directly) |
| **Communication** | Hub-and-spoke; no peer-to-peer | Concurrent actor model; direct messaging |
| **State management** | Four-tier memory + Flow state | Explicit, minimal shared state |
| **Tool system** | Rich built-in library + MCP + `BaseTool` | Lightweight, typed capability registry |
| **Memory** | Built-in short/long/entity/user memory | Application-managed |
| **Configuration overhead** | High — role, goal, backstory, task spec per agent | Low — compose functions and dispatch |
| **Parallelism** | Sequential or manager-delegated; no native fan-out | Concurrent execution first-class |
| **Target user** | Teams building domain-specific agentic workflows | Engineers building infrastructure-level systems |
| **Enterprise tier** | Yes (CrewAI+) | N/A |

CrewAI's abstraction richness is its greatest strength for product teams who want to ship agentic features quickly without building coordination infrastructure. The cost is opacity — the framework makes many decisions on your behalf, and debugging requires understanding the framework's internals.

acteon-swarm's lower abstraction level gives engineers precise control over scheduling, concurrency, and state transitions. This is better suited to systems where performance, predictability, and operational transparency are primary constraints.

The practical choice: use CrewAI when the **problem is decomposable into roles and tasks** and you want the framework to handle coordination. Use acteon-swarm when you need **fine-grained control over how agents interact** and coordination is itself part of the system design.

---

## References

- [crewAIInc/crewAI — GitHub repository](https://github.com/crewAIInc/crewAI)
- [CrewAI Documentation](https://docs.crewai.com)
- [CrewAI Learning Platform](https://learn.crewai.com)
- [crewai-tools — Built-in tool library](https://github.com/crewAIInc/crewAI-tools)
- [CrewAI Flows introduction](https://docs.crewai.com/concepts/flows)
- [CrewAI Memory system](https://docs.crewai.com/concepts/memory)
- [CrewAI MCP integration](https://docs.crewai.com/concepts/mcp)
- [PyPI: crewai](https://pypi.org/project/crewai/)
