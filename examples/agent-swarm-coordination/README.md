# Agent coordination examples

Start with the tested [policy demo](policy-demo/acteon.toml) and the
[public guide](https://penserai.github.io/acteon/guides/agent-swarm-coordination/).
It runs real authentication, rules, approvals, quotas and chains with local log
providers. From the repository root:

```bash
cargo build --locked -p acteon-server
python3 scripts/ci/agent_guide.py --server target/debug/acteon-server
```

The remaining files in this directory are older host-specific hook and swarm
integration sketches. Their configuration and outcome parsing are not the tested
entry point; adapt and verify them for your host before use. No Claude engine or
external notification is invoked by the policy demo.
