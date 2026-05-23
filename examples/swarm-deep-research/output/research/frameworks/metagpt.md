# MetaGPT

## Overview

MetaGPT is an open-source multi-agent framework built around a central metaphor: **a software company as a multi-agent system**. Given a single natural-language requirement, it coordinates a team of specialized LLM agents—each playing a defined company role—to produce a complete software artifact including PRDs, system designs, code, and tests.

- **Original repo**: `geekan/MetaGPT` (now `FoundationAgents/MetaGPT`)
- **Stars**: ~66,400 (one of the most-starred agent frameworks on GitHub)
- **License**: MIT
- **Latest release**: v0.8.2 (March 2024); v0.8.0 introduced Data Interpreter and RAG integration
- **Python requirement**: 3.9–3.11
- **Academic origins**: Published as a research paper ("MetaGPT: Meta Programming for Multi-Agent Collaborative Framework") with a key insight summarized as: `Code = SOP(Team)`

The framework's distinguishing premise is that **Standard Operating Procedures (SOPs)** — not free-form chat — are the right abstraction for coordinating agents on complex, multi-step engineering tasks.

---

## Architecture & Core Concepts

### Role-Based Agents

MetaGPT defines a fixed set of agent roles that mirror a real software organization:

| Role | Responsibility |
|------|---------------|
| `ProductManager` | Writes PRDs, user stories, competitive analysis |
| `Architect` | System design, file structure, API specs |
| `ProjectManager` | Task decomposition, sprint planning |
| `Engineer` | Code generation, unit test execution, iterative debugging |
| `QAEngineer` | Test planning, bug reports, quality validation |

Each role is implemented as a Python class that inherits from a base `Role` abstraction. Roles have a defined set of **Actions** they can perform, inputs they watch for, and outputs they produce.

### SOP-Driven Workflows

SOPs are encoded directly into prompt sequences. Each role receives structured system instructions defining:
- What document types it consumes as input
- Exactly what it must produce as output (format, schema, completeness criteria)
- How to validate intermediate results

This is the key architectural differentiator from chat-based frameworks like ChatDev. Agents do not free-form converse; they execute structured procedures. The workflow phases are:

```
Requirement → PRD → System Design → Task List → Code → Tests → Documentation
```

Each phase produces a typed, persistent document artifact that downstream roles consume.

---

## Agent Communication Model

MetaGPT uses a **shared message pool** with a publish-subscribe model rather than direct peer-to-peer messaging:

- Agents **publish** structured outputs (documents, code files, specs) to a centralized pool
- Each agent **subscribes** only to message types relevant to its role (e.g., `Engineer` subscribes to `Task` messages, not `PRD` messages)
- This decouples producers from consumers — a role only acts when its watched message type arrives

Coordination is document-centric. The shared pool holds persistent artifacts (PRDs, design docs, task lists, code files), not raw chat logs. This means any agent can re-read upstream outputs at any point, providing a form of natural grounding.

The pub-sub design means adding a new role is mostly additive: define what it subscribes to, what action it runs, and what it publishes. The existing pipeline does not need modification.

---

## Tool & Capability System

### Action Classes

Capabilities in MetaGPT are encapsulated in **Action** classes. Each action is a discrete, reusable unit of work:

```python
class WriteCode(Action):
    async def run(self, context: str) -> str:
        prompt = CODE_PROMPT_TEMPLATE.format(context=context)
        return await self._aask(prompt)
```

Built-in actions include:

| Action | Purpose |
|--------|---------|
| `WritePRD` | Generate product requirements document |
| `WriteDesign` | Produce system architecture artifacts |
| `WriteCode` | Implement a specific task/file |
| `WriteCodeReview` | Peer-review generated code |
| `RunCode` | Execute code and capture stdout/stderr |
| `WriteTest` | Generate unit tests for a module |
| `FixBug` | Iteratively debug failing code |
| `SearchAndSummarize` | Web search + summarization (via SerpAPI) |

### Custom Action Extension

Adding a custom action requires subclassing `Action` and implementing `run()`. The role then lists that action in its `_actions` list. The framework handles prompt construction, LLM invocation, and output routing.

### Data Interpreter

Added in v0.8.0, `DataInterpreter` is a specialized sub-agent capable of planning and executing multi-step data analysis tasks — including writing and running Python code in a sandboxed environment with iterative self-correction.

---

## Memory & Knowledge Management

MetaGPT uses several memory layers:

- **Role memory**: Each agent maintains a short-term buffer of messages it has processed in the current session, giving it local context for its next action.
- **Shared document store**: Produced artifacts (PRDs, design docs, code files) are persisted to a project workspace directory. All roles can read them at any time, functioning as a long-term shared memory.
- **RAG integration** (v0.8.0+): Optional retrieval-augmented generation support allows agents to query external knowledge bases or codebases for additional context before acting.

Context management is explicit: each role's action prompts are constructed by assembling relevant prior artifacts from the shared store, not by passing the entire chat history. This keeps prompts focused and reduces context bloat on large projects.

---

## GitHub & Community

- **Stars**: ~66,400 — exceptionally high; among top 5 most-starred agent frameworks
- **Forks**: ~7,700+
- **Academic citations**: The MetaGPT paper has hundreds of citations; it is frequently used as a baseline in multi-agent systems research
- **Activity**: Maintenance has slowed since early 2024. v0.8.x patch releases continue but no major architectural changes are expected near-term
- **Community**: Active Discord and WeChat communities; primarily English and Chinese documentation
- **Related work**: The same research group produced `AgentStore`, `Data Interpreter`, and contributes to the `OpenBenchmark` ecosystem

---

## Strengths

- **Structured output quality**: SOP-driven prompting produces consistently formatted artifacts — PRDs, designs, and code that fit together — reducing the hallucination drift common in free-form agent conversations.
- **Full pipeline coverage**: End-to-end from requirement to runnable code with minimal user intervention. Few frameworks go this far out of the box.
- **Role specialization**: Each agent has a narrow, well-defined job, which makes prompt engineering tractable and outputs predictable.
- **Document-centric coordination**: Persistent artifacts mean the system is inspectable and recoverable — humans can read intermediate outputs and intervene.
- **Strong research baseline**: Well-cited paper; the architecture is well-described and reproducible.

---

## Weaknesses

- **Rigid role structure**: The fixed company-role hierarchy does not adapt well to tasks outside software development. Adding novel roles requires understanding the internals.
- **High token cost**: Generating PRDs, design docs, task lists, code, and tests for even a simple app consumes a large number of tokens across many sequential LLM calls.
- **Sequential bottleneck**: Phases are largely sequential — the Engineer cannot start until the Architect finishes. There is limited parallelism within the pipeline.
- **Python version constraint**: 3.9–3.11 only; incompatible with Python 3.12+ as of the last major release.
- **Maintenance slowdown**: No major releases since March 2024; some dependencies and integrations are showing age.
- **Deterministic structure limits flexibility**: The SOP approach excels for predictable software tasks but handles ambiguous, exploratory, or research-oriented tasks poorly.

---

## Comparison with acteon-swarm

| Dimension | MetaGPT | acteon-swarm |
|-----------|---------|--------------|
| **Orchestration model** | Fixed roles, sequential SOP pipeline | Dynamic swarm: agents spawned on-demand per task |
| **Workflow definition** | Encoded in role prompts and action classes | Defined in `swarm.toml`; tasks decomposed at runtime |
| **Communication** | Shared document pool (pub-sub, async) | Direct task delegation between orchestrator and agents |
| **Parallelism** | Limited — phase dependencies create sequential bottlenecks | High — independent subtasks run concurrently |
| **Adaptability** | Low — role structure is fixed | High — agent types and counts scale to task shape |
| **Output artifacts** | Rich, structured documents (PRDs, design, code) | Research findings, summaries, targeted artifacts |
| **Best fit** | End-to-end software project generation | Research aggregation, multi-source investigation |
| **Token efficiency** | Low — full pipeline generates many documents | Higher — agents scoped to specific subtasks |
| **Human inspectability** | High — every phase produces a readable document | Moderate — depends on how findings are surfaced |

**Key tradeoff**: MetaGPT's SOPs guarantee structured, phase-aligned outputs at the cost of flexibility and sequential execution. acteon-swarm's dynamic orchestration handles open-ended, parallel research tasks well but does not enforce the document discipline that makes MetaGPT outputs directly usable as engineering artifacts.

For software project generation specifically, MetaGPT's structured approach produces higher-quality intermediate artifacts. For research tasks requiring broad parallel information gathering — like this very document — a dynamic swarm model is more efficient.

---

## References

- MetaGPT GitHub: `FoundationAgents/MetaGPT` (formerly `geekan/MetaGPT`)
- MetaGPT paper: "MetaGPT: Meta Programming for Multi-Agent Collaborative Framework" (Hong et al., 2023)
- MetaGPT docs: `docs.deepwisdom.ai`
- Data Interpreter paper: "Data Interpreter: An LLM Agent For Data Science" (2024)
- v0.8.0 release notes: Data Interpreter integration, RAG support
