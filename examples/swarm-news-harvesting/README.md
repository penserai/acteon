# Swarm Example: News Harvesting

An agent swarm that researches recent EU AI regulation news and produces a structured briefing document.

## Run Results

| Metric | Value |
|--------|-------|
| Status | Completed |
| Agents spawned | 6 |
| Agents completed | 6 / 6 |
| Duration | ~28 minutes |
| Model | Sonnet |
| TesseraiDB entities | 31 (2 runs, 10 agents, 9 episodic memories, 8 findings) |

## Output

- `output/briefing.md` — 125 lines, 16KB. Comprehensive briefing with 5 key developments, dated source URLs, executive summary, and outlook.

## Task Decomposition (auto-generated)

1. **Research EU AI Act enforcement** (researcher) — AI Office investigations, compliance deadlines
2. **Research policy and legislative changes** (researcher) — Digital Omnibus, high-risk AI delays
3. **Research industry and geopolitical context** (researcher) — US-EU tensions, industry reactions
4. **Compile briefing document** (coder) — wrote `briefing.md` from research findings
5. **Review** (reviewer) — checked accuracy and completeness

## How to Reproduce

```bash
# Start Acteon (in-memory)
cargo run -p acteon-server --release -- --port 8090 -c crates/swarm/swarm-demo.toml &

# Start TesseraiDB (in-memory)
DATABASE_PATH=:memory: PORT=8091 DISABLE_AUTH=true /path/to/tesseraidb &

# Run
mkdir /tmp/news-demo && cp examples/swarm-news-harvesting/swarm.toml /tmp/news-demo/
cd /tmp/news-demo
acteon-swarm run \
  --prompt "Research the latest news about AI regulation in the European Union..." \
  --auto-approve
```
