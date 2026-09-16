# Bus conformance

`acteon_bus::testing::run_bus_backend_conformance_tests` defines the portable
contract for a fresh, single-partition bus topic. Backend tests provide a unique
topic and verify the behavior that callers share across transports:

- duplicate topic creation returns `TopicAlreadyExists`;
- produced receipts and earliest replay preserve topic, partition, offset,
  ordering key, headers, payload, and timestamp;
- watermarks and cursor scans use the public offset convention;
- consumer lag reports the last committed offset and remaining records; and
- a created topic can be deleted.

The in-memory backend runs the contract in its unit suite. Kafka runs it against
a disposable broker when `ACTEON_KAFKA_BOOTSTRAP` is set; the stable integration
job starts a single-listener broker and executes the same tests. Run it locally
with:

```sh
docker compose --profile kafka up -d kafka
ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \\
  cargo test --locked -p acteon-bus --test kafka_integration
```

The contract intentionally uses one partition. Multi-partition placement and
consumer-group rebalance behavior are Kafka-specific concerns exercised by the
backend's own tests rather than promises every backend must emulate.
