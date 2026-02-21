use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Configuration for a WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPluginConfig {
    /// Maximum memory in bytes the plugin can use.
    #[serde(default)]
    pub memory_limit_bytes: Option<u64>,
    /// Maximum execution time in milliseconds.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// List of host functions the plugin is allowed to call.
    #[serde(default)]
    pub allowed_host_functions: Option<Vec<String>>,
}

/// A registered WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmPlugin {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Optional human-readable description.
    #[serde(default)]
    pub description: Option<String>,
    /// Plugin status (e.g., "active", "disabled").
    pub status: String,
    /// Whether the plugin is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Plugin resource configuration.
    #[serde(default)]
    pub config: Option<WasmPluginConfig>,
    /// When the plugin was registered.
    pub created_at: String,
    /// When the plugin was last updated.
    pub updated_at: String,
    /// Number of times the plugin has been invoked.
    #[serde(default)]
    pub invocation_count: u64,
}

/// Request to register a new WASM plugin.
#[derive(Debug, Clone, Serialize)]
pub struct RegisterPluginRequest {
    /// Plugin name (unique identifier).
    pub name: String,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Base64-encoded WASM module bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_bytes: Option<String>,
    /// Path to the WASM file (server-side).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_path: Option<String>,
    /// Plugin resource configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<WasmPluginConfig>,
}

/// Response from listing WASM plugins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPluginsResponse {
    /// List of registered plugins.
    pub plugins: Vec<WasmPlugin>,
    /// Total count.
    pub count: usize,
}

/// Request to test-invoke a WASM plugin.
#[derive(Debug, Clone, Serialize)]
pub struct PluginInvocationRequest {
    /// The function to invoke (default: "evaluate").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// JSON input to pass to the plugin.
    pub input: serde_json::Value,
}

/// Response from test-invoking a WASM plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginInvocationResponse {
    /// Whether the plugin evaluation returned true (pass) or false (fail).
    pub verdict: bool,
    /// Optional message from the plugin.
    #[serde(default)]
    pub message: Option<String>,
    /// Optional structured metadata from the plugin.
    #[serde(default)]
    pub metadata: Option<serde_json::Value>,
    /// Execution time in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<f64>,
}

impl ActeonClient {
    /// List all registered WASM plugins.
    pub async fn list_plugins(&self) -> Result<ListPluginsResponse, Error> {
        let url = format!("{}/v1/plugins", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListPluginsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to list plugins".to_string(),
            })
        }
    }

    /// Register a new WASM plugin.
    pub async fn register_plugin(&self, req: &RegisterPluginRequest) -> Result<WasmPlugin, Error> {
        let url = format!("{}/v1/plugins", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<WasmPlugin>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to register plugin".to_string(),
            })
        }
    }

    /// Get details of a registered WASM plugin by name.
    pub async fn get_plugin(&self, name: &str) -> Result<Option<WasmPlugin>, Error> {
        let url = format!("{}/v1/plugins/{name}", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<WasmPlugin>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get plugin: {name}"),
            })
        }
    }

    /// Unregister (delete) a WASM plugin by name.
    pub async fn delete_plugin(&self, name: &str) -> Result<(), Error> {
        let url = format!("{}/v1/plugins/{name}", self.base_url);

        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            Ok(())
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Plugin not found: {name}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to delete plugin".to_string(),
            })
        }
    }

    /// Test-invoke a WASM plugin with a sample action context.
    pub async fn invoke_plugin(
        &self,
        name: &str,
        req: &PluginInvocationRequest,
    ) -> Result<PluginInvocationResponse, Error> {
        let url = format!("{}/v1/plugins/{name}/invoke", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<PluginInvocationResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Plugin not found: {name}"),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to invoke plugin: {name}"),
            })
        }
    }
}
