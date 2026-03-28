# Swarm Example: Stock Market Analysis (Parallel Execution)

An agent swarm that analyzes Q4 2025 tech earnings and their impact on related market sectors. Runs with **parallel task execution** — independent research tasks start simultaneously.

## Swarm Digital Twin Graph

![Swarm Graph](output/swarm-graph.png)

**Legend**: Blue = SwarmRun, Purple = SwarmTask, Green = AgentSession, Orange = EpisodicMemory, Red = SemanticMemory

## Run Results

| Metric | Value |
|--------|-------|
| Status | 10/12 agents completed |
| Tasks | 7 |
| Agents spawned | 12 |
| Duration | ~20 minutes |
| Model | Sonnet |
| TesseraiDB entities | 57 |
| RDF triples | 201KB |
| Graph | 39 nodes, 58 edges |

## Output

```
output/analysis/
  companies/
    apple.md                 112 lines   Apple Q4 FY2025 earnings
    microsoft.md             107 lines   Microsoft Q4 2025 earnings
    google.md                133 lines   Alphabet Q4 2025 earnings
    amazon.md                124 lines   Amazon Q4 2025 earnings
```

## Knowledge Graph Artifacts

| File | Description |
|------|-------------|
| `output/swarm-graph.png` | Visual graph (39 nodes, 58 edges) |
| `output/swarm-graph.mmd` | Mermaid source |
| `output/knowledge-graph.ttl` | Full RDF triples (201KB) |

## Note

This example is for educational/analytical purposes only. The swarm produces factual analysis of publicly available market data — it does not provide investment advice.
