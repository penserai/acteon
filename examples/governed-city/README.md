# Governed city: real permits and closures

Run from the repository root:

```sh
cargo build -p acteon-server --no-default-features
python3 -m pip install -e clients/python
python3 examples/governed-city/run.py --output /tmp/governed-city-results.json
```

The actual server uses memory and ephemeral local ports. The runner invokes the
native typed Python SDK and counts real webhook POSTs. It asserts executor
management denial, operator permit issuance, retained execution replay, closure,
reopening, permit revocation and credential offboarding. All processes and
credentials are temporary; no existing Redis or PostgreSQL data is touched.

Success means two authorized sends and zero unauthorized sends. Business data
is synthetic; no model or autonomous peer selection is invoked. The JSON report
contains each checked step and the final management view. See the public
[guide](https://penserai.github.io/acteon/guides/governed-city/).
