# Swarm Example: Stock Market Analysis (Parallel Execution)

An agent swarm that analyzes Q4 2025 tech earnings and their impact on related market sectors. Runs with **parallel task execution** — 8 independent research tasks started simultaneously.

## Swarm Digital Twin Graph

![Swarm Graph](output/swarm-graph.png)

**Legend**: Blue = SwarmRun, Purple = SwarmTask, Green = AgentSession, Orange = EpisodicMemory, Red = SemanticMemory

## Run Results

| Metric | Value |
|--------|-------|
| Status | Completed |
| Tasks | 17 |
| Agents spawned | 26 |
| Agents completed | 26 / 26 |
| Duration | ~32 minutes |
| Model | Sonnet (agents) + Haiku (refiner) |
| Refinements | 4 (dynamic reprioritization) |
| TesseraiDB entities | 140 |
| RDF triples | 334KB |
| Relationships | 139 edges |
| Max concurrent | 8 agents running simultaneously |

## Parallel Execution Highlights

- **8 tasks started at the same timestamp** (17:02:26) — all independent research tasks ran concurrently
- **Refiner active**: dynamically reprioritized tasks 4 times during execution
- **Memory reuse**: prior findings injected into agent prompts (count=5 across later tasks)
- **Dependency-driven scheduling**: coder tasks started as soon as their research dependencies completed

## Output (9 files, 164KB)

```
output/analysis/
  README.md                  110 lines   Executive summary
  correlations.md            346 lines   Cross-company correlation analysis
  companies/
    apple.md                 104 lines   Apple Q4 FY2025 earnings
    microsoft.md             119 lines   Microsoft Q4 2025 earnings
    google.md                112 lines   Alphabet Q4 2025 earnings
    amazon.md                129 lines   Amazon Q4 2025 earnings
  sectors/
    semiconductors.md        172 lines   Semiconductor sector impact
    cloud.md                 240 lines   Cloud provider sector impact
    ad-tech.md               274 lines   Ad tech sector impact
```

## Knowledge Graph Artifacts

| File | Description |
|------|-------------|
| `output/swarm-graph.png` | Visual graph (96 nodes, 139 edges) |
| `output/swarm-graph.mmd` | Mermaid source |
| `output/knowledge-graph.ttl` | Full RDF triples (334KB) |

## Twin Types in Graph

| Type | Count | Description |
|------|-------|-------------|
| SwarmRun | 1 | Run metadata |
| SwarmTask | 17 | Task decomposition with dependencies |
| AgentSession | 26 | Agent lifecycle per subtask |
| EpisodicMemory | 26 | Per-action records |
| SemanticMemory | 26 | Key findings from research |

## Note

This example is for educational/analytical purposes only. The swarm produces factual analysis of publicly available market data — it does not provide investment advice.
