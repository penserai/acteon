# Azure Providers

Acteon ships with native Azure provider integrations for **Blob Storage** and **Event Hubs**. Both are first-class citizens -- they implement the same `Provider` trait, participate in circuit breaking, health checks, and per-provider metrics, and require no external plugins.

## Compile-Time Feature Selection

Azure providers are **not compiled by default**. Each provider maps to a feature flag on `acteon-server`, so you only compile the Azure SDKs you need:

```bash
# Only Blob Storage
cargo build -p acteon-server --features "azure-blob"

# Only Event Hubs
cargo build -p acteon-server --features "azure-eventhubs"

# Both Azure providers
cargo build -p acteon-server --features azure-all
```

| Server Feature | Provider Type | Azure Service |
|----------------|---------------|---------------|
| `azure-blob` | `azure-blob` | Azure Blob Storage |
| `azure-eventhubs` | `azure-eventhubs` | Azure Event Hubs |
| `azure-all` | All of the above | All of the above |

The `acteon-azure` crate uses the same pattern internally -- each provider is behind a matching feature flag (`blob`, `eventhubs`), and provider registration in `main.rs` is guarded by `#[cfg(feature = "azure-*")]`.

## Overview

| Provider | Azure Service | Action Types | Use Case |
|----------|--------------|--------------|----------|
| `azure-blob` | Blob Storage | `upload_blob`, `download_blob`, `delete_blob` | Store and retrieve blobs (logs, artifacts, images, archives) |
| `azure-eventhubs` | Event Hubs | `send_event`, `send_batch` | Publish events and telemetry for stream processing |

All Azure providers:

- Share a common authentication layer with Azure AD service principal or Azure CLI fallback
- Support endpoint URL overrides for `Azurite` and other Azure-compatible services
- Report per-provider health metrics (success rate, latency percentiles, error tracking)
- Map Azure SDK errors to the standard `ProviderError` enum for circuit breaker integration

## TOML Configuration

### Azure Blob Storage

```toml
[[providers]]
name = "archive"
type = "azure-blob"
location = "eastus"
account_name = "mystorageaccount"
container_name = "data-lake"
# prefix = "acteon/artifacts/"                   # Optional key prefix
# endpoint_url = "http://127.0.0.1:10000"        # Azurite
# tenant_id = "00000000-0000-0000-0000-000000000000"
# client_id = "00000000-0000-0000-0000-000000000001"
# client_credential = "my-app-credential"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"azure-blob"` |
| `location` | Yes | Azure region (e.g. `"eastus"`) |
| `account_name` | Yes | Azure Storage account name |
| `container_name` | No | Default container name. Can be overridden per-action in the payload |
| `prefix` | No | Blob name prefix prepended to all blob names |
| `endpoint_url` | No | Endpoint URL override (for `Azurite`) |
| `tenant_id` | No | Azure AD tenant ID for service principal auth |
| `client_id` | No | Azure AD application (client) ID |
| `client_credential` | No | Azure AD client credential |

### Azure Event Hubs

```toml
[[providers]]
name = "telemetry-hub"
type = "azure-eventhubs"
location = "eastus"
namespace = "mynamespace.servicebus.windows.net"
event_hub_name = "telemetry"
# endpoint_url = "http://localhost:5672"           # Local emulator
# tenant_id = "00000000-0000-0000-0000-000000000000"
# client_id = "00000000-0000-0000-0000-000000000001"
# client_credential = "my-app-credential"
```

| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Unique provider name used in action dispatch |
| `type` | Yes | Must be `"azure-eventhubs"` |
| `location` | Yes | Azure region (e.g. `"eastus"`) |
| `namespace` | Yes | Event Hubs fully-qualified namespace (e.g. `"mynamespace.servicebus.windows.net"`) |
| `event_hub_name` | Yes | Default Event Hub name. Can be overridden per-action in the payload |
| `endpoint_url` | No | Endpoint URL override |
| `tenant_id` | No | Azure AD tenant ID for service principal auth |
| `client_id` | No | Azure AD application (client) ID |
| `client_credential` | No | Azure AD client credential |

## Payload Formats

### Blob Storage: `upload_blob`

```json
{
  "container": "data-lake",
  "blob_name": "reports/2026/02/daily.json",
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
| `blob_name` | Yes | string | Blob name (path within the container) |
| `container` | No | string | Override the default container |
| `body` | No | string | Blob body as UTF-8 text. Mutually exclusive with `body_base64` |
| `body_base64` | No | string | Blob body as base64-encoded bytes. Mutually exclusive with `body` |
| `content_type` | No | string | MIME type for the blob |
| `metadata` | No | object | Key-value metadata pairs attached to the blob |

**Response:**

```json
{
  "container": "data-lake",
  "blob_name": "reports/2026/02/daily.json",
  "status": "uploaded"
}
```

### Blob Storage: `download_blob`

```json
{
  "container": "data-lake",
  "blob_name": "reports/2026/02/daily.json"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `blob_name` | Yes | string | Blob name to download |
| `container` | No | string | Override the default container |

**Response** (UTF-8 content):

```json
{
  "container": "data-lake",
  "blob_name": "reports/2026/02/daily.json",
  "content_length": 1234,
  "body": "{\"readings\": [72.5, 68.1]}",
  "status": "downloaded"
}
```

**Response** (binary content):

```json
{
  "container": "data-lake",
  "blob_name": "images/logo.png",
  "content_length": 5678,
  "body_base64": "iVBORw0KGgo...",
  "status": "downloaded"
}
```

### Blob Storage: `delete_blob`

```json
{
  "container": "data-lake",
  "blob_name": "old/data.csv"
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `blob_name` | Yes | string | Blob name to delete |
| `container` | No | string | Override the default container |

**Response:**

```json
{
  "container": "data-lake",
  "blob_name": "old/data.csv",
  "status": "deleted"
}
```

### Event Hubs: `send_event`

```json
{
  "body": {"device_id": "sensor-001", "temperature": 72.5},
  "event_hub_name": "telemetry",
  "partition_id": "0",
  "properties": {
    "source": "iot-gateway"
  }
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `body` | Yes | any | Event body (JSON object or string) |
| `event_hub_name` | No | string | Override the default Event Hub name |
| `partition_id` | No | string | Partition ID for routing |
| `properties` | No | object | Application properties (key-value string pairs) |

**Response:**

```json
{
  "event_hub_name": "telemetry",
  "event_count": 1,
  "status": "sent"
}
```

### Event Hubs: `send_batch`

```json
{
  "event_hub_name": "telemetry",
  "events": [
    {
      "body": {"device_id": "sensor-001", "temperature": 72.5},
      "properties": {"region": "us-west"}
    },
    {
      "body": {"device_id": "sensor-002", "temperature": 68.1},
      "partition_id": "1"
    }
  ]
}
```

| Field | Required | Type | Description |
|-------|----------|------|-------------|
| `event_hub_name` | No | string | Override the default Event Hub name |
| `events` | Yes | array | List of events to send |
| `events[].body` | Yes | any | Event body (JSON object or string) |
| `events[].partition_id` | No | string | Partition ID for routing this event |
| `events[].properties` | No | object | Application properties for this event |

**Response:**

```json
{
  "event_hub_name": "telemetry",
  "event_count": 2,
  "status": "sent"
}
```

## Authentication

Azure providers share a common authentication layer in `acteon-azure`. Credentials are resolved in this order:

1. **Service principal** -- if `tenant_id`, `client_id`, and `client_credential` are all configured, uses `ClientSecretCredential` for Azure AD authentication
2. **Azure CLI** (fallback) -- if service principal fields are not fully configured, falls back to `AzureCliCredential` which uses the Azure CLI login context (suitable for development and CI/CD environments)

For production deployments, use one of these approaches:

- **Service principal credentials** in TOML config or environment variables -- best for non-Azure hosted workloads
- **Managed identity** -- for workloads running on Azure VMs, AKS, App Service, or Azure Functions; configure externally via the Azure environment and omit service principal fields from TOML config

### Service Principal Setup

```bash
# Create a service principal for Acteon
az ad sp create-for-rbac \
  --name "acteon-blob-writer" \
  --role "Storage Blob Data Contributor" \
  --scopes "/subscriptions/<sub-id>/resourceGroups/<rg>/providers/Microsoft.Storage/storageAccounts/<account>"

# The output provides the tenant_id, client_id (appId), and client_credential (credential value)
```

## Health Check Behavior

| Provider | Health Check Method | Notes |
|----------|-------------------|-------|
| `azure-blob` | `ListContainers` | Verifies API connectivity and credentials |
| `azure-eventhubs` | `GetEventHubProperties` | Verifies namespace connectivity and permissions |

All health checks are lightweight read-only operations that do not modify any resources.

## Error Handling

Azure SDK errors are classified into the standard `ProviderError` enum:

| Azure Error Pattern | ProviderError Variant | Retryable | Circuit Breaker |
|---------------------|----------------------|-----------|-----------------|
| Connection / DNS / network failure | `Connection` | Yes | Counts toward trip |
| HTTP 429 / throttling / rate exceeded | `RateLimited` | Yes | Counts toward trip |
| Timeout | `Timeout` | Yes | Counts toward trip |
| Credential errors | `Configuration` | No | Does not count |
| Invalid payload | `Serialization` | No | Does not count |
| Other service errors | `ExecutionFailed` | No | Counts toward trip |

## Example: Dispatching via the API

```bash
# Upload a blob
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "storage",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "upload_blob",
    "payload": {
      "blob_name": "reports/2026/02/daily.json",
      "body": "{\"total\": 42}",
      "content_type": "application/json"
    }
  }'

# Download a blob
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "storage",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "download_blob",
    "payload": {
      "blob_name": "reports/2026/02/daily.json"
    }
  }'

# Delete a blob
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "storage",
    "tenant": "acme-corp",
    "provider": "archive",
    "action_type": "delete_blob",
    "payload": {
      "blob_name": "old/data.csv"
    }
  }'

# Send an event to Event Hubs
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "events",
    "tenant": "acme-corp",
    "provider": "telemetry-hub",
    "action_type": "send_event",
    "payload": {
      "body": {"device_id": "sensor-001", "temperature": 72.5}
    }
  }'

# Send a batch of events
curl -X POST http://localhost:8080/v1/dispatch \
  -H "Content-Type: application/json" \
  -d '{
    "namespace": "events",
    "tenant": "acme-corp",
    "provider": "telemetry-hub",
    "action_type": "send_batch",
    "payload": {
      "events": [
        {"body": {"device_id": "sensor-001", "temperature": 72.5}},
        {"body": {"device_id": "sensor-002", "temperature": 68.1}}
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

    // Upload a blob
    client.dispatch_action(
        "storage", "acme-corp", "archive", "upload_blob",
        serde_json::json!({
            "blob_name": "data/report.json",
            "body": "{\"total\": 42}",
            "content_type": "application/json"
        }),
    ).await?;

    // Download a blob
    client.dispatch_action(
        "storage", "acme-corp", "archive", "download_blob",
        serde_json::json!({
            "blob_name": "data/report.json"
        }),
    ).await?;

    // Delete a blob
    client.dispatch_action(
        "storage", "acme-corp", "archive", "delete_blob",
        serde_json::json!({
            "blob_name": "old/data.csv"
        }),
    ).await?;

    // Send an event to Event Hubs
    client.dispatch_action(
        "events", "acme-corp", "telemetry-hub", "send_event",
        serde_json::json!({
            "body": {"device_id": "sensor-001", "temperature": 72.5}
        }),
    ).await?;

    // Send a batch of events
    client.dispatch_action(
        "events", "acme-corp", "telemetry-hub", "send_batch",
        serde_json::json!({
            "events": [
                {"body": {"device_id": "sensor-001", "temperature": 72.5}},
                {"body": {"device_id": "sensor-002", "temperature": 68.1}}
            ]
        }),
    ).await?;

    Ok(())
}
```

## Local Development with Azurite

Azure Blob Storage supports endpoint URL overrides, making it easy to develop and test locally with [Azurite](https://learn.microsoft.com/en-us/azure/storage/common/storage-use-azurite), the official Azure Storage emulator:

```bash
# Start Azurite via Docker
docker run --rm -d --name azurite -p 10000:10000 -p 10001:10001 -p 10002:10002 \
  mcr.microsoft.com/azure-storage/azurite

# Or install and run locally via npm
npm install -g azurite
azurite --silent --location /tmp/azurite
```

Point the Blob provider at the Azurite endpoint:

```toml
[[providers]]
name = "archive"
type = "azure-blob"
location = "local"
account_name = "devstoreaccount1"
container_name = "test-container"
endpoint_url = "http://127.0.0.1:10000"
```

> **Note:** `Azurite` uses the well-known development storage account `devstoreaccount1`. There is no official local emulator for Azure Event Hubs; for Event Hubs integration testing, use an Azure subscription with a dedicated test namespace.
