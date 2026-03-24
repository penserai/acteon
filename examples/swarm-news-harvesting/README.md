# Swarm Example: News Harvesting

An agent swarm that researches recent news about EU AI regulation, analyzes key developments, and produces a structured briefing document.

## Run Results

- **Status**: Completed
- **Agents spawned**: 10 (5 tasks, 10 subtasks)
- **Duration**: ~24 minutes
- **Model**: Sonnet

## Output Files

| File | Description |
|------|-------------|
| `briefing.md` | Final briefing document (22KB, 94 lines) with 5 key developments, sources, and outlook |
| `research-enforcement.md` | Raw research on AI Act enforcement actions |
| `research-policy.md` | Raw research on policy debates and legislative changes |
| `research-reactions.md` | Raw research on industry reactions and geopolitical tensions |
| `development-outline.md` | Structured outline used by the coder agent to compile the briefing |
| `review-notes.md` | Reviewer agent's assessment of the final document |

## How to Reproduce

```bash
# Start Acteon (in-memory, no deps)
cargo run -p acteon-server --release -- --port 8090 -c examples/swarm-news-harvesting/../../crates/swarm/swarm-demo.toml &

# Start TesseraiDB (in-memory)
DATABASE_PATH=:memory: PORT=8091 DISABLE_AUTH=true /path/to/tesseraidb &

# Run the swarm
cd /tmp/news-demo
cp examples/swarm-news-harvesting/swarm.toml .
acteon-swarm run \
  --prompt "Research the latest news about AI regulation in the European Union..." \
  --auto-approve
```

## Task Decomposition

The swarm automatically decomposed the objective into:

1. **Research enforcement** (researcher) — searched for EU AI Office enforcement actions
2. **Research policy** (researcher) — searched for legislative changes and the Digital Omnibus
3. **Research reactions** (researcher) — searched for industry and geopolitical reactions
4. **Compile briefing** (coder) — wrote the structured briefing from research findings
5. **Review** (reviewer) — checked accuracy and completeness
