# GCP Providers

Acteon ships with native GCP provider integrations for **Cloud Storage** and **Pub/Sub**. Both are first-class citizens -- they implement the same `Provider` trait, participate in circuit breaking, health checks, and per-provider metrics, and require no external plugins.

## Compile-Time Feature Selection

GCP providers are **not compiled by default**. Each provider maps to a feature flag on `acteon-server`, so you only compile the GCP SDKs you need:

```bash
# Only Cloud Storage
cargo build -p acteon-server --features "gcp-storage"

# Only Pub/Sub
cargo build -p acteon-server --features "gcp-pubsub"

# Both GCP providers
cargo build -p acteon-server --features gcp-all
```

| Server Feature | Provider Type | GCP Service |
|----------------|---------------|-------------|
| `gcp-storage` | `gcp-storage` | Cloud Storage |
| `gcp-pubsub` | `gcp-pubsub` | Pub/Sub |
| `gcp-all` | All of the above | All of the above |

The `acteon-gcp` crate uses the same pattern internally -- each provider is behind a matching feature flag (`storage`, `pubsub`), and provider registration in `main.rs` is guarded by `#[cfg(feature = "gcp-*")]`.

## Overview

| Provider | GCP Service | Action Types | Use Case |
|----------|------------|--------------|----------|
| `gcp-storage` | Cloud Storage | `upload_object`, `download_object`, `delete_object` | Store and retrieve objects (logs, artifacts, images, archives) |
| `gcp-pubsub` | Pub/Sub | `publish`, `publish_batch` | Publish messages for asynchronous event processing |

All GCP providers:

- Share a common authentication layer with service account JSON keys or Application Default Credentials (ADC) fallback
- Support endpoint URL overrides for local emulators (Pub/Sub emulator, fake-gcs-server)
- Report per-provider health metrics (success rate, latency percentiles, error tracking)
- Map GCP SDK errors to the standard `ProviderError` enum for circuit breaker integration

## TOML Configuration

### GCP Cloud Storage

```toml
[[providers]]
name = "archive"
type = "gcp-storage"
location = "us-central1"
project_id = "my-gcp-project"
bucket = "data-lake"
# prefix = "acteon/artifacts/"                        # Optional key prefix
# endpoint_url = "http://127.0.0.1:4443"              # fake-gcs-server
# credentials_json = "/path/to/service-account.json"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"gcp-storage"` |
| `location` | Yes | GCP region (e.g. `"us-central1"`) |
| `project_id` | Yes | GCP project ID |
| `bucket` | No | Default bucket name. Can be overridden per-action in the payload |
| `prefix` | No | Object name prefix prepended to all object names |
| `endpoint_url` | No | Endpoint URL override (for `fake-gcs-server` or other emulators) |
| `credentials_json` | No | Path to a service account JSON key file |

### GCP Pub/Sub

```toml
[[providers]]
name = "telemetry-topic"
type = "gcp-pubsub"
location = "us-central1"
project_id = "my-gcp-project"
topic = "telemetry"
# endpoint_url = "http://localhost:8085"               # Pub/Sub emulator
# credentials_json = "/path/to/service-account.json"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"gcp-pubsub"` |
| `location` | Yes | GCP region (e.g. `"us-central1"`) |
| `project_id` | Yes | GCP project ID |
| `topic` | Yes | Default Pub/Sub topic name. Can be overridden per-action in the payload |
| `endpoint_url` | No | Endpoint URL override (for the Pub/Sub emulator) |
| `credentials_json` | No | Path to a service account JSON key file |

## Payload Formats

### Cloud Storage: `upload_object`

```json
{
  "bucket": "data-lake",
  "object_name": "reports/2026/02/daily.json",
  "body": "{\"readings\": [72.5, 68.1]}",
  "content_type": "application/json",
  "metadata": {
    "source": "acteon",
    "pipeline_version": "2.0"
  }
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `object_name` | Yes | string | Object name (path within the bucket) |
| `bucket` | No | string | Override the default bucket |
| `body` | No | string | Object body as UTF-8 text. Mutually exclusive with `body_base64` |
| `body_base64` | No | string | Object body as base64-encoded bytes. Mutually exclusive with `body` |
| `content_type` | No | string | MIME type for the object |
| `metadata` | No | object | Key-value metadata pairs attached to the object |

**Response:**

```json
{
  "bucket": "data-lake",
  "object_name": "reports/2026/02/daily.json",
  "status": "uploaded"
}
```

### Cloud Storage: `download_object`

```json
{
  "bucket": "data-lake",
  "object_name": "reports/2026/02/daily.json"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `object_name` | Yes | string | Object name to download |
| `bucket` | No | string | Override the default bucket |

**Response** (UTF-8 content):

```json
{
  "bucket": "data-lake",
  "object_name": "reports/2026/02/daily.json",
  "content_length": 1234,
  "body": "{\"readings\": [72.5, 68.1]}",
  "status": "downloaded"
}
```

**Response** (binary content):

```json
{
  "bucket": "data-lake",
  "object_name": "images/logo.png",
  "content_length": 5678,
  "body_base64": "iVBORw0KGgo...",
  "status": "downloaded"
}
```

### Cloud Storage: `delete_object`

```json
{
  "bucket": "data-lake",
  "object_name": "old/data.csv"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `object_name` | Yes | string | Object name to delete |
| `bucket` | No | string | Override the default bucket |

**Response:**

```json
{
  "bucket": "data-lake",
  "object_name": "old/data.csv",
  "status": "deleted"
}
```

### Pub/Sub: `publish`

```json
{
  "data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}",
  "topic": "telemetry",
  "ordering_key": "sensor-001",
  "attributes": {
    "source": "iot-gateway"
  }
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `data` | Yes | string | Message data (typically a JSON string) |
| `topic` | No | string | Override the default topic name |
| `ordering_key` | No | string | Ordering key for ordered delivery within the same key |
| `attributes` | No | object | Message attributes (key-value string pairs) |

**Response:**

```json
{
  "topic": "telemetry",
  "message_count": 1,
  "status": "published"
}
```

### Pub/Sub: `publish_batch`

```json
{
  "topic": "telemetry",
  "messages": [
    {
      "data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}",
      "attributes": {"region": "us-west1"}
    },
    {
      "data": "{\"device_id\": \"sensor-002\", \"temperature\": 68.1}",
      "ordering_key": "sensor-002"
    }
  ]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `topic` | No | string | Override the default topic name |
| `messages` | Yes | array | List of messages to publish |
| `messages[].data` | Yes | string | Message data (typically a JSON string) |
| `messages[].ordering_key` | No | string | Ordering key for this message |
| `messages[].attributes` | No | object | Message attributes for this message |

**Response:**

```json
{
  "topic": "telemetry",
  "message_count": 2,
  "status": "published"
}
```

## Authentication

GCP providers share a common authentication layer in `acteon-gcp`. Credentials are resolved in this order:

1. **Service account JSON key** -- if `credentials_json` is configured, loads the service account key file and uses it for authentication
2. **Application Default Credentials** (fallback) -- if `credentials_json` is not configured, falls back to ADC which checks, in order: `GOOGLE_APPLICATION_CREDENTIALS` environment variable, gcloud CLI credentials, Compute Engine / GKE metadata server

For production deployments, use one of these approaches:

- **Service account JSON key** in TOML config or `GOOGLE_APPLICATION_CREDENTIALS` environment variable -- best for non-GCP hosted workloads
- **Workload Identity** -- for workloads running on GKE; configure via Kubernetes service account annotation and omit `credentials_json` from TOML config
- **Attached service account** -- for workloads running on Compute Engine, Cloud Run, or Cloud Functions; configure externally via the GCP console and omit `credentials_json` from TOML config

### Service Account Setup

```bash
# Create a service account for Acteon
gcloud iam service-accounts create acteon-storage-writer \
  --display-name="Acteon Storage Writer"

# Grant Cloud Storage permissions
gcloud projects add-iam-policy-binding my-gcp-project \
  --member="serviceAccount:acteon-storage-writer@my-gcp-project.iam.gserviceaccount.com" \
  --role="roles/storage.objectAdmin"

# Grant Pub/Sub permissions
gcloud projects add-iam-policy-binding my-gcp-project \
  --member="serviceAccount:acteon-storage-writer@my-gcp-project.iam.gserviceaccount.com" \
  --role="roles/pubsub.publisher"

# Create and download the key file
gcloud iam service-accounts keys create /path/to/service-account.json \
  --iam-account=acteon-storage-writer@my-gcp-project.iam.gserviceaccount.com
```

## Health Check Behavior

| Provider | Health Check Method | Notes |
|----------|-------------------|-------|
| `gcp-storage` | `ListBuckets` | Verifies API connectivity and credentials |
| `gcp-pubsub` | `GetTopic` | Verifies topic existence and permissions |

All health checks are lightweight read-only operations that do not modify any resources.

## Error Handling

GCP SDK errors are classified into the standard `ProviderError` enum:

| GCP Error Pattern | ProviderError Variant | Retryable | Circuit Breaker |
|-------------------|----------------------|-----------|-----------------|
| Connection / DNS / network failure | `Connection` | Yes | Counts toward trip |
| HTTP 429 / quota exceeded / rate limited | `RateLimited` | Yes | Counts toward trip |
| Timeout | `Timeout` | Yes | Counts toward trip |
| Credential / permission errors | `Configuration` | No | Does not count |
| Invalid payload | `Serialization` | No | Does not count |
| Other service errors | `ExecutionFailed` | No | Counts toward trip |

## Example: Dispatching via the API

```bash
# Upload an object
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "storage",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "upload_object",
    "payload": {
      "object_name": "reports/2026/02/daily.json",
      "body": "{\"total\": 42}",
      "content_type": "application/json"
    }
  }'

# Download an object
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "storage",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "download_object",
    "payload": {
      "object_name": "reports/2026/02/daily.json"
    }
  }'

# Delete an object
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "storage",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "delete_object",
    "payload": {
      "object_name": "old/data.csv"
    }
  }'

# Publish a message to Pub/Sub
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "events",
    "tenant": "acme-corp",
    "provider": "telemetry-topic",
    "action_type": "publish",
    "payload": {
      "data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}"
    }
  }'

# Publish a batch of messages
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "events",
    "tenant": "acme-corp",
    "provider": "telemetry-topic",
    "action_type": "publish_batch",
    "payload": {
      "messages": [
        {"data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}"},
        {"data": "{\"device_id\": \"sensor-002\", \"temperature\": 68.1}"}
      ]
    }
  }'
```

## Example: Rust Client

```rust
use acteon_client::ActeonClient;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = ActeonClient::new("http://localhost:8080", "your-api-token")?;

    // Upload an object
    client.dispatch_action(
        "storage", "acme-corp", "archive", "upload_object",
        serde_json::json!({
            "object_name": "data/report.json",
            "body": "{\"total\": 42}",
            "content_type": "application/json"
        }),
    ).await?;

    // Download an object
    client.dispatch_action(
        "storage", "acme-corp", "archive", "download_object",
        serde_json::json!({
            "object_name": "data/report.json"
        }),
    ).await?;

    // Delete an object
    client.dispatch_action(
        "storage", "acme-corp", "archive", "delete_object",
        serde_json::json!({
            "object_name": "old/data.csv"
        }),
    ).await?;

    // Publish a message to Pub/Sub
    client.dispatch_action(
        "events", "acme-corp", "telemetry-topic", "publish",
        serde_json::json!({
            "data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}"
        }),
    ).await?;

    // Publish a batch of messages
    client.dispatch_action(
        "events", "acme-corp", "telemetry-topic", "publish_batch",
        serde_json::json!({
            "messages": [
                {"data": "{\"device_id\": \"sensor-001\", \"temperature\": 72.5}"},
                {"data": "{\"device_id\": \"sensor-002\", \"temperature\": 68.1}"}
            ]
        }),
    ).await?;

    Ok(())
}
```

## Local Development with Emulators

### Pub/Sub Emulator

GCP provides an official [Pub/Sub emulator](https://cloud.google.com/pubsub/docs/emulator) for local development and testing:

```bash
# Install and start via gcloud CLI
gcloud components install pubsub-emulator
gcloud beta emulators pubsub start --project=test-project

# Or start via Docker
docker run --rm -d --name pubsub-emulator -p 8085:8085 \
  gcr.io/google.com/cloudsdktool/google-cloud-cli:emulators \
  gcloud beta emulators pubsub start --host-port=0.0.0.0:8085 --project=test-project
```

Point the Pub/Sub provider at the emulator endpoint:

```toml
[[providers]]
name = "telemetry-topic"
type = "gcp-pubsub"
location = "local"
project_id = "test-project"
topic = "telemetry"
endpoint_url = "http://localhost:8085"
```

> **Note:** When using the Pub/Sub emulator, authentication is bypassed automatically. You must create topics and subscriptions manually via the emulator API before publishing.

### Cloud Storage Emulator (fake-gcs-server)

For Cloud Storage, use [fake-gcs-server](https://github.com/fsouza/fake-gcs-server), a community-maintained emulator:

```bash
# Start via Docker
docker run --rm -d --name fake-gcs -p 4443:4443 \
  fsouza/fake-gcs-server -scheme http -port 4443

# Create a bucket
curl -X POST http://localhost:4443/storage/v1/b \
  -H "Content-Type: application/json" \
  -d '{"name": "test-bucket"}'
```

Point the Cloud Storage provider at the emulator endpoint:

```toml
[[providers]]
name = "archive"
type = "gcp-storage"
location = "local"
project_id = "test-project"
bucket = "test-bucket"
endpoint_url = "http://127.0.0.1:4443"
```

> **Note:** `fake-gcs-server` does not enforce IAM permissions. For full integration testing with authentication, use a GCP project with a dedicated test bucket.
