# Swarm Example: Stock Market Analysis

An agent swarm that analyzes Q4 2025 tech earnings (Apple, Microsoft, Google, Amazon) and their impact on related market sectors.

## Run Results

- **Status**: 14/15 agents completed (1 timeout on final README assembly)
- **Agents spawned**: 15 (8 tasks)
- **Duration**: ~25 minutes
- **Model**: Sonnet

## Output Files

```
analysis/
  companies/
    apple.md       (128 lines) — Apple Q4 FY2025 earnings analysis
    google.md      (156 lines) — Alphabet Q4 2025 earnings analysis
    microsoft.md   (151 lines) — Microsoft Q4 FY2025 earnings analysis
```

## How to Reproduce

```bash
# Start Acteon and TesseraiDB (see news-harvesting example)

cd /tmp/stocks-demo
cp examples/swarm-stock-analysis/swarm.toml .
acteon-swarm run \
  --prompt "Analyze how Q4 2025 earnings reports from Apple, Microsoft, Google, and Amazon affected related market sectors..." \
  --auto-approve
```

## Task Decomposition

The swarm decomposed into 8 tasks:

1. **Research Apple earnings** (researcher)
2. **Research Microsoft earnings** (researcher)
3. **Research Google earnings** (researcher)
4. **Research Amazon earnings** (researcher)
5. **Write per-company analyses** (coder) — created `analysis/companies/*.md`
6. **Research sector correlations** (researcher)
7. **Research sector impacts** (researcher)
8. **Compile cross-company analysis** (researcher)

## Note

This example is for educational/analytical purposes only. The swarm produces factual analysis of publicly available market data — it does not provide investment advice.
