# Ads measurement pipeline simulation

This scenario models a live CTV campaign from ad decision through measurement,
billing reconciliation, and governed alert delivery. The executable source is
[`crates/simulation/examples/ads_measurement_pipeline.rs`](../../crates/simulation/examples/ads_measurement_pipeline.rs).

Run it from the repository root:

```bash
cargo run -p acteon-simulation --example ads_measurement_pipeline
```

The simulation is deterministic and needs no external services. See the
[complete guide](../../docs/book/guides/ads-measurement-pipeline.md) for the
production architecture, data contract, delivery guarantees, operational
playbook, and the limits of the local workload.
