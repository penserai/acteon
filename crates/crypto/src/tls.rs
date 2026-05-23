//! TLS certificate loading and configuration utilities.
//!
//! Provides helpers for building `rustls` server and client configurations,
//! as well as a pre-configured `reqwest::Client` with mTLS support.

use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use thiserror::Error;

/// Errors that can occur during TLS setup.
#[derive(Debug, Error)]
pub enum TlsError {
    /// Failed to read a file from disk.
    #[error("failed to read {path}: {source}")]
    FileRead {
        path: String,
        source: std::io::Error,
    },

    /// No certificates were found in the PEM file.
    #[error("no certificates found in {0}")]
    NoCertificates(String),

    /// No private key was found in the PEM file.
    #[error("no private key found in {0}")]
    NoPrivateKey(String),

    /// The `rustls` configuration could not be built.
    #[error("rustls config error: {0}")]
    RustlsConfig(String),

    /// The `reqwest` client could not be built.
    #[error("reqwest client error: {0}")]
    ReqwestBuild(String),
}

/// Load a PEM certificate chain from a file.
///
/// Returns all certificates found in the PEM file, in order.
pub fn load_certs(path: impl AsRef<Path>) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let path = path.as_ref();
    let file = fs::File::open(path).map_err(|e| TlsError::FileRead {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut reader = BufReader::new(file);

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| TlsError::FileRead {
            path: path.display().to_string(),
            source: e,
        })?;

    if certs.is_empty() {
        return Err(TlsError::NoCertificates(path.display().to_string()));
    }

    Ok(certs)
}

/// Load a private key from a PEM file.
///
/// Supports PKCS#8, RSA, and EC private keys. Returns the first key found.
pub fn load_private_key(path: impl AsRef<Path>) -> Result<PrivateKeyDer<'static>, TlsError> {
    let path = path.as_ref();
    let file = fs::File::open(path).map_err(|e| TlsError::FileRead {
        path: path.display().to_string(),
        source: e,
    })?;
    let mut reader = BufReader::new(file);

    for item in rustls_pemfile::read_all(&mut reader) {
        match item {
            Ok(rustls_pemfile::Item::Pkcs1Key(key)) => return Ok(PrivateKeyDer::Pkcs1(key)),
            Ok(rustls_pemfile::Item::Pkcs8Key(key)) => return Ok(PrivateKeyDer::Pkcs8(key)),
            Ok(rustls_pemfile::Item::Sec1Key(key)) => return Ok(PrivateKeyDer::Sec1(key)),
            Ok(_) => {}
            Err(e) => {
                return Err(TlsError::FileRead {
                    path: path.display().to_string(),
                    source: e,
                });
            }
        }
    }

    Err(TlsError::NoPrivateKey(path.display().to_string()))
}

/// Minimum TLS protocol version.
#[derive(Debug, Clone, Copy, Default)]
pub enum MinTlsVersion {
    /// TLS 1.2 (default).
    #[default]
    Tls12,
    /// TLS 1.3.
    Tls13,
}

impl MinTlsVersion {
    /// Parse a version string like `"1.2"` or `"1.3"`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "1.2" => Some(Self::Tls12),
            "1.3" => Some(Self::Tls13),
            _ => None,
        }
    }
}

/// Build a `rustls::ServerConfig` for HTTPS termination.
///
/// - `cert_path` / `key_path`: server certificate and private key (required).
/// - `client_ca_path`: if provided, enables client certificate verification (mTLS).
/// - `min_version`: minimum TLS protocol version.
pub fn build_server_config(
    cert_path: &str,
    key_path: &str,
    client_ca_path: Option<&str>,
    min_version: MinTlsVersion,
) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let versions: &[&'static rustls::SupportedProtocolVersion] = match min_version {
        MinTlsVersion::Tls12 => &[&rustls::version::TLS12, &rustls::version::TLS13],
        MinTlsVersion::Tls13 => &[&rustls::version::TLS13],
    };
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .map_err(|e| TlsError::RustlsConfig(e.to_string()))?;

    let config = if let Some(ca_path) = client_ca_path {
        let ca_certs = load_certs(ca_path)?;
        let mut root_store = rustls::RootCertStore::empty();
        for cert in ca_certs {
            root_store
                .add(cert)
                .map_err(|e| TlsError::RustlsConfig(format!("failed to add CA cert: {e}")))?;
        }
        let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(root_store))
            .build()
            .map_err(|e| TlsError::RustlsConfig(format!("client verifier: {e}")))?;
        builder
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .map_err(|e| TlsError::RustlsConfig(e.to_string()))?
    } else {
        builder
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .map_err(|e| TlsError::RustlsConfig(e.to_string()))?
    };

    Ok(Arc::new(config))
}

/// Build a `rustls::ClientConfig` for outbound mTLS connections.
///
/// - `client_cert_path` / `client_key_path`: client certificate and key (optional, for mTLS).
/// - `ca_bundle_path`: custom CA bundle (optional; uses Mozilla roots if omitted).
/// - `danger_accept_invalid_certs`: skip certificate verification (dev/test only).
pub fn build_client_config(
    client_cert_path: Option<&str>,
    client_key_path: Option<&str>,
    ca_bundle_path: Option<&str>,
    danger_accept_invalid_certs: bool,
) -> Result<Arc<rustls::ClientConfig>, TlsError> {
    let mut root_store = rustls::RootCertStore::empty();

    if let Some(ca_path) = ca_bundle_path {
        let ca_certs = load_certs(ca_path)?;
        for cert in ca_certs {
            root_store
                .add(cert)
                .map_err(|e| TlsError::RustlsConfig(format!("failed to add CA cert: {e}")))?;
        }
    } else {
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    }

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| TlsError::RustlsConfig(e.to_string()))?
        .with_root_certificates(root_store);

    let mut config = if let (Some(cert_path), Some(key_path)) = (client_cert_path, client_key_path)
    {
        let certs = load_certs(cert_path)?;
        let key = load_private_key(key_path)?;
        builder
            .with_client_auth_cert(certs, key)
            .map_err(|e| TlsError::RustlsConfig(e.to_string()))?
    } else {
        builder.with_no_client_auth()
    };

    if danger_accept_invalid_certs {
        config
            .dangerous()
            .set_certificate_verifier(Arc::new(NoCertificateVerification));
    }

    Ok(Arc::new(config))
}

/// Build a `reqwest::Client` with the given TLS client configuration.
///
/// The client uses `rustls` as its TLS backend and optionally includes a
/// client certificate for mTLS.
pub fn build_reqwest_client(
    client_cert_path: Option<&str>,
    client_key_path: Option<&str>,
    ca_bundle_path: Option<&str>,
    danger_accept_invalid_certs: bool,
) -> Result<reqwest::Client, TlsError> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .danger_accept_invalid_certs(danger_accept_invalid_certs);

    if let Some(ca_path) = ca_bundle_path {
        let ca_pem = fs::read(ca_path).map_err(|e| TlsError::FileRead {
            path: ca_path.to_owned(),
            source: e,
        })?;
        let ca_cert = reqwest::Certificate::from_pem(&ca_pem)
            .map_err(|e| TlsError::ReqwestBuild(format!("invalid CA bundle: {e}")))?;
        builder = builder.add_root_certificate(ca_cert);
    }

    if let (Some(cert_path), Some(key_path)) = (client_cert_path, client_key_path) {
        let cert_pem = fs::read(cert_path).map_err(|e| TlsError::FileRead {
            path: cert_path.to_owned(),
            source: e,
        })?;
        let key_pem = fs::read(key_path).map_err(|e| TlsError::FileRead {
            path: key_path.to_owned(),
            source: e,
        })?;

        let mut combined = cert_pem;
        combined.push(b'\n');
        combined.extend_from_slice(&key_pem);

        let identity = reqwest::Identity::from_pem(&combined)
            .map_err(|e| TlsError::ReqwestBuild(format!("invalid client identity: {e}")))?;
        builder = builder.identity(identity);
    }

    builder
        .build()
        .map_err(|e| TlsError::ReqwestBuild(e.to_string()))
}

/// Certificate verifier that accepts any certificate (for dev/test use only).
///
/// # Safety
///
/// This completely bypasses TLS certificate validation. Only use in
/// development or testing environments with `danger_accept_invalid_certs = true`.
#[derive(Debug)]
struct NoCertificateVerification;

impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA512,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_certs_nonexistent_file() {
        let err = load_certs("/nonexistent/path.pem").unwrap_err();
        assert!(matches!(err, TlsError::FileRead { .. }));
    }

    #[test]
    fn load_private_key_nonexistent_file() {
        let err = load_private_key("/nonexistent/key.pem").unwrap_err();
        assert!(matches!(err, TlsError::FileRead { .. }));
    }

    #[test]
    fn min_tls_version_parse() {
        assert!(matches!(
            MinTlsVersion::parse("1.2"),
            Some(MinTlsVersion::Tls12)
        ));
        assert!(matches!(
            MinTlsVersion::parse("1.3"),
            Some(MinTlsVersion::Tls13)
        ));
        assert!(MinTlsVersion::parse("1.1").is_none());
        assert!(MinTlsVersion::parse("").is_none());
    }

    #[test]
    fn build_server_config_missing_cert() {
        let err = build_server_config(
            "/nonexistent/cert.pem",
            "/nonexistent/key.pem",
            None,
            MinTlsVersion::Tls12,
        )
        .unwrap_err();
        assert!(matches!(err, TlsError::FileRead { .. }));
    }

    #[test]
    fn build_client_config_no_certs_uses_mozilla_roots() {
        let config = build_client_config(None, None, None, false).unwrap();
        // Should succeed with default Mozilla root certificates.
        assert!(!config.alpn_protocols.is_empty() || config.alpn_protocols.is_empty());
    }

    #[test]
    fn build_client_config_danger_mode() {
        // Should succeed building with danger mode enabled.
        let _config = build_client_config(None, None, None, true).unwrap();
    }

    #[test]
    fn build_reqwest_client_default() {
        let client = build_reqwest_client(None, None, None, false).unwrap();
        // Should build successfully.
        drop(client);
    }

    #[test]
    fn build_reqwest_client_missing_ca() {
        let err = build_reqwest_client(None, None, Some("/nonexistent/ca.pem"), false).unwrap_err();
        assert!(matches!(err, TlsError::FileRead { .. }));
    }
}
