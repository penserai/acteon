# Swarm Example: Stock Market Analysis

An agent swarm that analyzes Q4 2025 tech earnings (Apple, Microsoft, Google, Amazon) and their impact on related market sectors.

## Run Results

| Metric | Value |
|--------|-------|
| Status | Completed |
| Agents spawned | 21 |
| Agents completed | 21 / 21 |
| Duration | ~46 minutes |
| Model | Sonnet |
| TesseraiDB entities | 94 (2 runs, 31 agents, 30 episodic memories, 29 findings) |

## Output (9 files, 176KB)

```
output/analysis/
  README.md                  114 lines   Executive summary
  correlations.md            297 lines   Cross-company correlation analysis
  companies/
    apple.md                 149 lines   Apple Q4 FY2025 earnings
    microsoft.md             171 lines   Microsoft Q4 2025 earnings
    google.md                184 lines   Alphabet Q4 2025 earnings
    amazon.md                187 lines   Amazon Q4 2025 earnings
  sectors/
    semiconductors.md        181 lines   Semiconductor sector impact
    cloud.md                 200 lines   Cloud provider sector impact
    ad-tech.md               177 lines   Ad tech sector impact
```

## Task Decomposition (auto-generated, 11 tasks)

1. **Setup directories** (executor)
2. **Research Apple earnings** (researcher)
3. **Research Microsoft earnings** (researcher)
4. **Research Google earnings** (researcher)
5. **Research Amazon earnings** (researcher)
6. **Write per-company analyses** (coder) — 4 subtasks
7. **Research semiconductor sector** (researcher)
8. **Research cloud sector** (researcher)
9. **Research ad tech sector** (researcher)
10. **Write sector reports + correlations** (coder) — 4 subtasks
11. **Write executive summary** (coder) — compiled README.md

## How to Reproduce

```bash
# Start Acteon and TesseraiDB (see news-harvesting example)

mkdir /tmp/stocks-demo && cp examples/swarm-stock-analysis/swarm.toml /tmp/stocks-demo/
cd /tmp/stocks-demo
acteon-swarm run \
  --prompt "Analyze how Q4 2025 earnings from Apple, Microsoft, Google, and Amazon affected related market sectors..." \
  --auto-approve
```

## Note

This example is for educational/analytical purposes only. The swarm produces factual analysis of publicly available market data — it does not provide investment advice.
