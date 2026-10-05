# Agent workforce simulation

Run real authenticated HTTP operations through Acteon workforce mandates and
execution permits. See [the public guide](../../docs/book/guides/agent-workforce.md)
for setup, configuration and checked results.

```bash
cargo build -p acteon-server --no-default-features
python3 -m venv /tmp/acteon-workforce-demo
/tmp/acteon-workforce-demo/bin/pip install -e clients/python
/tmp/acteon-workforce-demo/bin/python examples/agent-workforce/run.py --output /tmp/agent-workforce-results.json
```

Expected: 8 actual authorized webhook calls, zero unauthorized calls, including
one replay without an extra send. The memory-backed runner uses ephemeral ports
and temporary secrets; it starts and stops only its own processes.
