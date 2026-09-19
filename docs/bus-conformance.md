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

The in-memory backend runs the contract in its unit suite. The dedicated
`Kafka conformance (stable)` CI job runs the full bus unit, transport recovery,
and real-broker integration suites against a health-checked Kafka 3.7.0 service.
The job checks that `ACTEON_KAFKA_BOOTSTRAP` is nonempty before executing tests,
has a 15-minute timeout, and prints broker logs even on test failure. Local
integration runs remain opt-in through the same variable:

```sh
docker compose --profile kafka up -d kafka
ACTEON_KAFKA_BOOTSTRAP=localhost:9092 \
  cargo test --locked -p acteon-bus --test kafka_integration
```

The contract intentionally uses one partition. Multi-partition placement and
consumer-group rebalance behavior are Kafka-specific concerns exercised by the
backend's own tests rather than promises every backend must emulate.

## Consumer transport recovery

Both subscriptions and cursor scans retain their consumer when librdkafka emits
`MessageConsumption(BrokerTransportFailure)` or
`MessageConsumption(AllBrokersDown)`. These connection notifications are logged;
librdkafka owns reconnect/backoff. Other errors, including authentication,
authorization, missing-topic and fatal errors, still terminate the stream.
Consumers can remain pending during an outage; callers retain their existing
timeout/cancellation responsibility.

This follows librdkafka's [error handling guidance](https://github.com/confluentinc/librdkafka/blob/master/INTRODUCTION.md#error-handling):
connection notifications can be informational while the client recovers.
Previously, either notification destroyed the consumer before recovery; the
failed PR #337 CI run reported `BrokerTransportFailure` on earliest replay.

`cargo test --locked -p acteon-bus --test kafka_transport` runs without Docker.
It uses librdkafka's socket-based mock broker, disconnects and restores the
broker, and checks ordered delivery on the original subscription and scan.
These disconnect tests alone do not reproduce the CI notification: librdkafka
can recover internally without emitting it. A separate unit regression injects
both notifications into the shared stream handler and verifies continued delivery
and propagation of authorization/fatal errors. Disabling recovery makes that
regression fail.

Local validation: all 25 bus tests passed with a standalone Kafka 3.7.0 broker
(19 unit tests, four real-broker tests, two socket recovery tests). All four
real-broker tests also passed against Kafka 4.1.2. Both brokers used single-node
KRaft, loopback listeners, replication factor one, and disabled topic auto-creation.
The Kafka 3.7.0 archive matched Apache's published SHA-512 checksum. Clippy,
formatting, diff whitespace, and workflow YAML parsing passed.

The new CI job is configured for pushes and pull requests to `main`; its hosted
execution has not yet been observed. Repository branch-protection settings were
not changed. These local tests do not establish multi-broker failover behavior.
