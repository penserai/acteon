# Swarm Example: Stock Market Analysis

An agent swarm that analyzes Q4 2025 tech earnings and their impact on related market sectors.

## Swarm Digital Twin Graph

![Swarm Graph](output/swarm-graph.png)

**Legend**: Blue = SwarmRun, Purple = SwarmTask, Green = AgentSession, Orange = EpisodicMemory, Red = SemanticMemory

## Run Results

| Metric | Value |
|--------|-------|
| Status | Completed |
| Agents spawned | 20 |
| Agents completed | 13 / 20 |
| Duration | ~34 minutes |
| Model | Sonnet |
| TesseraiDB entities | 90 twins |
| RDF triples | 356KB |
| Relationships | 91 edges |

## Output

```
output/analysis/
  companies/
    apple.md        Apple Q4 FY2025 earnings analysis
    microsoft.md    Microsoft Q4 2025 earnings analysis
    google.md       Alphabet Q4 2025 earnings analysis
    amazon.md       Amazon Q4 2025 earnings analysis
  sectors/
    semiconductors.md   Semiconductor sector impact report
```

## Knowledge Graph Artifacts

| File | Description |
|------|-------------|
| `output/swarm-graph.png` | Visual graph of all agent interactions |
| `output/swarm-graph.mmd` | Mermaid source for the graph |
| `output/knowledge-graph.ttl` | Full RDF triples in Turtle format (356KB) |

## Twin Types in Graph

| Type | Count | Description |
|------|-------|-------------|
| SwarmRun | 1 | Run metadata — objective, plan, roles |
| SwarmTask | 8 | Task decomposition with dependencies |
| AgentSession | 20 | Agent lifecycle — role, subtask, timestamps |
| EpisodicMemory | 20 | Per-action records — full agent output |
| SemanticMemory | 12 | Key findings — earnings data, sector analysis |

## Note

This example is for educational/analytical purposes only. The swarm produces factual analysis of publicly available market data — it does not provide investment advice.
