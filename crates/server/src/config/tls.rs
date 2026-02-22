use serde::Deserialize;

/// Top-level TLS configuration.
///
/// Controls both inbound HTTPS termination and outbound mTLS for backend
/// connections and provider HTTP calls.
///
/// # Example
///
/// ```toml
/// [tls]
/// enabled = true
///
/// [tls.server]
/// cert_path = "/etc/acteon/tls/server.crt"
/// key_path = "/etc/acteon/tls/server.key"
///
/// [tls.client]
/// ca_bundle_path = "/etc/acteon/tls/ca-bundle.crt"
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct TlsConfig {
    /// Whether TLS is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Server-side TLS for HTTPS termination.
    #[serde(default)]
    pub server: ServerTlsConfig,

    /// Client-side TLS for outbound connections.
    #[serde(default)]
    pub client: ClientTlsConfig,
}

/// Server-side TLS configuration for HTTPS termination.
#[derive(Debug, Default, Deserialize)]
pub struct ServerTlsConfig {
    /// Path to the server certificate PEM file.
    #[serde(default)]
    pub cert_path: Option<String>,

    /// Path to the server private key PEM file.
    #[serde(default)]
    pub key_path: Option<String>,

    /// Path to the CA certificate for client cert verification (enables inbound mTLS).
    #[serde(default)]
    pub client_ca_path: Option<String>,

    /// Minimum TLS version: `"1.2"` (default) or `"1.3"`.
    #[serde(default = "default_min_version")]
    pub min_version: String,
}

/// Client-side TLS configuration for outbound mTLS.
#[derive(Debug, Default, Deserialize)]
pub struct ClientTlsConfig {
    /// Path to the client certificate PEM file.
    #[serde(default)]
    pub cert_path: Option<String>,

    /// Path to the client private key PEM file.
    #[serde(default)]
    pub key_path: Option<String>,

    /// Path to a custom CA bundle PEM file. If omitted, Mozilla roots are used.
    #[serde(default)]
    pub ca_bundle_path: Option<String>,

    /// Accept invalid certificates (dev/test only).
    #[serde(default)]
    pub danger_accept_invalid_certs: bool,
}

fn default_min_version() -> String {
    "1.2".to_owned()
}
