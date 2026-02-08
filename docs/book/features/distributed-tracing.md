# Distributed Tracing

OpenTelemetry distributed tracing gives you end-to-end visibility into every action flowing through Acteon. Each dispatch request generates a trace that spans the full pipeline -- HTTP ingress, rule evaluation, state operations, provider execution, and audit recording -- and exports it via OTLP to your existing observability stack (Jaeger, Grafana Tempo, Zipkin, etc.).

## How It Works

1. A client sends a dispatch request, optionally including a W3C `traceparent` header
2. Acteon extracts the trace context and links the server-side spans to the caller's trace
3. As the action flows through the gateway pipeline, each stage creates a child span with relevant attributes
4. Spans are batched and exported via OTLP (gRPC or HTTP) to your collector
5. On shutdown, pending spans are flushed to avoid data loss

When tracing is disabled (the default), zero OpenTelemetry overhead is added -- only the standard `tracing` `fmt` subscriber runs.

## Configuration

Add a `[telemetry]` section to your `acteon.toml`:

```toml title="acteon.toml"
[telemetry]
enabled = true
endpoint = "http://localhost:4317"   # OTLP collector endpoint
service_name = "acteon"              # Service name in traces
sample_ratio = 1.0                   # 1.0 = trace every request
protocol = "grpc"                    # "grpc" or "http"
timeout_seconds = 10                 # Exporter timeout

# Optional resource attributes (added to every span)
[telemetry.resource_attributes]
"deployment.environment" = "production"
"service.instance.id" = "acteon-us-east-1a"
```

### Configuration Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | bool | `false` | Enable OpenTelemetry tracing |
| `endpoint` | string | `"http://localhost:4317"` | OTLP exporter endpoint |
| `service_name` | string | `"acteon"` | Service name reported in traces |
| `sample_ratio` | f64 | `1.0` | Sampling ratio (0.0 to 1.0). `1.0` traces every request, `0.1` traces 10% |
| `protocol` | string | `"grpc"` | OTLP transport: `"grpc"` (port 4317) or `"http"` (port 4318) |
| `timeout_seconds` | u64 | `10` | Exporter timeout in seconds |
| `resource_attributes` | map | `{}` | Additional key-value pairs attached to all spans |

The following resource attributes are always set automatically:

| Attribute | Value |
|-----------|-------|
| `service.name` | Value of `service_name` config |
| `service.version` | Acteon binary version (from `Cargo.toml`) |

## Span Hierarchy

Every dispatch request produces a trace with nested spans that mirror the gateway pipeline:

```
HTTP POST /v1/dispatch
  └─ gateway.dispatch
       ├─ action.id = "661f9511-..."
       ├─ action.namespace = "alerts"
       ├─ action.tenant = "acme"
       ├─ action.provider = "email"
       ├─ action.action_type = "send_email"
       ├─ dry_run = false
       │
       ├─ gateway.llm_guardrail          (if LLM guardrail enabled)
       │
       ├─ gateway.execute_action          (if verdict = Allow/Execute)
       │    └─ provider = "email"
       │
       ├─ gateway.handle_dedup            (if verdict = Deduplicate)
       │
       ├─ gateway.handle_reroute          (if verdict = Reroute)
       │    └─ target_provider = "webhook"
       │
       ├─ gateway.handle_state_machine    (if verdict = StateMachine)
       │
       ├─ gateway.handle_request_approval (if verdict = RequestApproval)
       │    ├─ rule = "require-approval"
       │    └─ notify_provider = "slack"
       │
       ├─ gateway.handle_group            (if verdict = Group)
       │
       ├─ gateway.handle_chain            (if verdict = Chain)
       │    └─ chain_name = "search-summarize-email"
       │
       └─ gateway.advance_chain           (async chain step execution)
            ├─ namespace = "alerts"
            ├─ tenant = "acme"
            └─ chain_id = "ch-abc123"
```

## Span Reference

### `gateway.dispatch`

The root gateway span for every action dispatch. Contains the full action metadata.

| Attribute | Type | Description |
|-----------|------|-------------|
| `action.id` | string | Unique action identifier |
| `action.namespace` | string | Action namespace |
| `action.tenant` | string | Tenant identifier |
| `action.provider` | string | Target provider name |
| `action.action_type` | string | Action type (e.g., `send_email`) |
| `dry_run` | bool | Whether this is a dry-run dispatch |

### `gateway.execute_action`

Created when an action is executed by a provider (verdict: Allow).

| Attribute | Type | Description |
|-----------|------|-------------|
| `provider` | string | Provider that executed the action |

### `gateway.handle_reroute`

Created when an action is rerouted to a different provider.

| Attribute | Type | Description |
|-----------|------|-------------|
| `target_provider` | string | Provider the action was rerouted to |

### `gateway.handle_request_approval`

Created when an action requires human approval.

| Attribute | Type | Description |
|-----------|------|-------------|
| `rule` | string | Rule that triggered the approval requirement |
| `notify_provider` | string | Provider used to send the approval notification |

### `gateway.handle_chain`

Created when a chain is triggered.

| Attribute | Type | Description |
|-----------|------|-------------|
| `chain_name` | string | Name of the chain configuration |

### `gateway.advance_chain`

Created when a chain step is advanced (may be async, in the background).

| Attribute | Type | Description |
|-----------|------|-------------|
| `namespace` | string | Chain namespace |
| `tenant` | string | Chain tenant |
| `chain_id` | string | Chain instance identifier |

### Other Spans

| Span | Description |
|------|-------------|
| `gateway.llm_guardrail` | LLM-based content evaluation |
| `gateway.handle_dedup` | Deduplication check against state store |
| `gateway.handle_state_machine` | State machine event lifecycle tracking |
| `gateway.handle_group` | Event grouping for batched notifications |

## W3C Trace Context Propagation

Acteon supports [W3C Trace Context](https://www.w3.org/TR/trace-context/) propagation via the `traceparent` and `tracestate` HTTP headers. When a client includes these headers, the server-side spans are linked to the caller's trace, providing cross-service visibility.

```bash
# Dispatch with trace context from an upstream service
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -H "traceparent: 00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01" \
  -d '{"namespace": "alerts", "tenant": "acme", "provider": "email", ...}'
```

When the `traceparent` header is absent, Acteon starts a new root trace. This is the typical case for direct API calls.

## Quick Start

### Jaeger

Jaeger's all-in-one image includes an OTLP collector on port 4317:

```bash
# Start Jaeger
docker run -d --name jaeger \
  -p 16686:16686 \
  -p 4317:4317 \
  jaegertracing/all-in-one:latest

# Configure Acteon
cat >> acteon.toml <<EOF
[telemetry]
enabled = true
endpoint = "http://localhost:4317"
EOF

# Start Acteon
cargo run -p acteon-server

# Open Jaeger UI
open http://localhost:16686
```

Search for service `acteon` in the Jaeger UI to see traces.

### Grafana Tempo

Tempo accepts OTLP on port 4317. Use Grafana to query traces:

```bash
# Start Tempo + Grafana (docker-compose.yml)
docker compose up -d

# Configure Acteon
cat >> acteon.toml <<EOF
[telemetry]
enabled = true
endpoint = "http://localhost:4317"
service_name = "acteon"
EOF
```

Add Tempo as a data source in Grafana, then use the Explore tab to search for traces by service name or span attributes.

### Zipkin

Zipkin requires the OTLP HTTP protocol (not gRPC):

```bash
# Start Zipkin
docker run -d --name zipkin -p 9411:9411 openzipkin/zipkin:latest

# Use an OTel Collector to bridge OTLP -> Zipkin format,
# or configure Acteon to use the HTTP protocol with a collector:
cat >> acteon.toml <<EOF
[telemetry]
enabled = true
endpoint = "http://localhost:4318"
protocol = "http"
EOF
```

### OpenTelemetry Collector

For production deployments, use the [OpenTelemetry Collector](https://opentelemetry.io/docs/collector/) as an intermediary. This decouples Acteon from your backend and supports batching, retry, and multi-destination export:

```yaml title="otel-collector-config.yaml"
receivers:
  otlp:
    protocols:
      grpc:
        endpoint: "0.0.0.0:4317"
      http:
        endpoint: "0.0.0.0:4318"

exporters:
  otlp/tempo:
    endpoint: "tempo:4317"
    tls:
      insecure: true
  prometheus:
    endpoint: "0.0.0.0:8889"

service:
  pipelines:
    traces:
      receivers: [otlp]
      exporters: [otlp/tempo]
```

```toml title="acteon.toml"
[telemetry]
enabled = true
endpoint = "http://otel-collector:4317"
service_name = "acteon-prod"

[telemetry.resource_attributes]
"deployment.environment" = "production"
"service.instance.id" = "acteon-01"
```

## Sampling Strategies

### Development

Trace every request for full visibility:

```toml
[telemetry]
enabled = true
sample_ratio = 1.0
```

### Staging

Sample a percentage to reduce volume while still catching issues:

```toml
[telemetry]
enabled = true
sample_ratio = 0.25  # 25% of requests
```

### Production

Use a low sample ratio or the OpenTelemetry Collector's tail-sampling processor to capture only slow or errored traces:

```toml
[telemetry]
enabled = true
sample_ratio = 0.01  # 1% of requests
```

For more sophisticated strategies (tail sampling, error-only sampling), configure the sampling in your OTel Collector rather than in Acteon:

```yaml title="otel-collector-config.yaml (tail sampling)"
processors:
  tail_sampling:
    policies:
      - name: errors-policy
        type: status_code
        status_code: {status_codes: [ERROR]}
      - name: slow-traces
        type: latency
        latency: {threshold_ms: 1000}
      - name: probabilistic
        type: probabilistic
        probabilistic: {sampling_percentage: 5}
```

### Disable Tracing

When disabled, no OpenTelemetry libraries are initialized and no overhead is added:

```toml
[telemetry]
enabled = false  # or simply omit the [telemetry] section
```

## Environment Variables

The tracing subscriber respects the standard `RUST_LOG` environment variable for controlling log and span verbosity:

```bash
# Show debug-level spans (more detail in traces)
RUST_LOG=debug cargo run -p acteon-server

# Only show warnings and errors
RUST_LOG=warn cargo run -p acteon-server
```

## Graceful Shutdown

On server shutdown (SIGINT/SIGTERM), Acteon flushes all pending spans to the collector before exiting. This ensures that in-flight traces are not lost during deployments or restarts.

The flush happens after the HTTP server stops accepting connections and after pending audit tasks complete, but before the process exits.

## Troubleshooting

### No traces appearing

1. Verify tracing is enabled:

    ```toml
    [telemetry]
    enabled = true
    ```

2. Check that the endpoint is reachable from Acteon:

    ```bash
    # Test gRPC connectivity
    grpcurl -plaintext localhost:4317 list

    # Test HTTP connectivity
    curl -v http://localhost:4318/v1/traces
    ```

3. Check the Acteon logs for the startup message:

    ```
    INFO OpenTelemetry tracing enabled endpoint=http://localhost:4317 protocol=grpc sample_ratio=1
    ```

    If this message is missing, tracing is not enabled.

4. Verify the `sample_ratio` is not `0.0`.

### Traces are incomplete

- Ensure the collector is not dropping spans due to resource limits. Check collector logs for `dropping span` warnings.
- Increase `timeout_seconds` if spans are timing out before export.
- Verify that the collector's OTLP receiver protocol matches the `protocol` setting in Acteon (gRPC on port 4317, HTTP on port 4318).

### High memory usage

- Reduce `sample_ratio` to decrease the volume of exported spans.
- The batch span processor buffers spans in memory before export. If the collector is slow or unreachable, this buffer grows. Ensure the collector endpoint is healthy.

### Traces not linked across services

- Ensure your upstream service sends the `traceparent` header using W3C Trace Context format.
- Verify that both services export to the same collector/backend.
- The `traceparent` header format is: `00-{trace-id}-{parent-span-id}-{flags}`.

## See Also

- [Event Streaming](event-streaming.md) -- real-time SSE event stream
- [Audit Trail](audit-trail.md) -- persistent searchable record of every action
- [Circuit Breaker](circuit-breaker.md) -- provider health tracking
