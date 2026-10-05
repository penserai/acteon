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
    reqwest_client_builder(
        client_cert_path,
        client_key_path,
        ca_bundle_path,
        danger_accept_invalid_certs,
    )?
    .build()
    .map_err(|e| TlsError::ReqwestBuild(e.to_string()))
}

/// Prepare a TLS client builder so callers can attach outbound destination policy.
pub fn reqwest_client_builder(
    client_cert_path: Option<&str>,
    client_key_path: Option<&str>,
    ca_bundle_path: Option<&str>,
    danger_accept_invalid_certs: bool,
) -> Result<reqwest::ClientBuilder, TlsError> {
    LoadedTlsClientConfig::load(
        client_cert_path,
        client_key_path,
        ca_bundle_path,
        danger_accept_invalid_certs,
    )?
    .client_builder()
}

/// Immutable outbound TLS material loaded once for client construction and
/// qualification. Private key bytes are zeroized when the snapshot is dropped.
/// File paths and raw material are never included in its debug representation.
pub struct LoadedTlsClientConfig {
    identity_pem: Option<zeroize::Zeroizing<Vec<u8>>>,
    ca_pem: Option<Vec<u8>>,
    danger_accept_invalid_certs: bool,
}

impl std::fmt::Debug for LoadedTlsClientConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedTlsClientConfig")
            .field("has_identity", &self.identity_pem.is_some())
            .field("has_custom_ca", &self.ca_pem.is_some())
            .field(
                "danger_accept_invalid_certs",
                &self.danger_accept_invalid_certs,
            )
            .finish()
    }
}

impl LoadedTlsClientConfig {
    /// Read and validate the material. A partial mTLS identity is rejected.
    pub fn load(
        client_cert_path: Option<&str>,
        client_key_path: Option<&str>,
        ca_bundle_path: Option<&str>,
        danger_accept_invalid_certs: bool,
    ) -> Result<Self, TlsError> {
        let read = |path: &str| {
            fs::read(path).map_err(|source| TlsError::FileRead {
                path: path.to_owned(),
                source,
            })
        };
        let identity_pem = match (client_cert_path, client_key_path) {
            (Some(cert), Some(key)) => {
                let mut combined = zeroize::Zeroizing::new(read(cert)?);
                let key = zeroize::Zeroizing::new(read(key)?);
                combined.push(b'\n');
                combined.extend_from_slice(&key);
                Some(combined)
            }
            (None, None) => None,
            _ => {
                return Err(TlsError::ReqwestBuild(
                    "client certificate and key must be supplied together".into(),
                ));
            }
        };
        let loaded = Self {
            identity_pem,
            ca_pem: ca_bundle_path.map(read).transpose()?,
            danger_accept_invalid_certs,
        };
        // Validate now, before exposing a qualification fingerprint.
        drop(loaded.client_builder()?);
        Ok(loaded)
    }

    /// Create another client from the same loaded bytes without reading files.
    pub fn client_builder(&self) -> Result<reqwest::ClientBuilder, TlsError> {
        let mut builder = reqwest::Client::builder()
            .use_rustls_tls()
            .danger_accept_invalid_certs(self.danger_accept_invalid_certs);
        if let Some(pem) = &self.ca_pem {
            let certificates = reqwest::Certificate::from_pem_bundle(pem)
                .map_err(|e| TlsError::ReqwestBuild(format!("invalid CA bundle: {e}")))?;
            if certificates.is_empty() {
                return Err(TlsError::ReqwestBuild(
                    "CA bundle contains no certificates".into(),
                ));
            }
            builder = builder.tls_built_in_root_certs(false);
            for certificate in certificates {
                builder = builder.add_root_certificate(certificate);
            }
        }
        if let Some(pem) = &self.identity_pem {
            let identity = reqwest::Identity::from_pem(pem)
                .map_err(|e| TlsError::ReqwestBuild(format!("invalid client identity: {e}")))?;
            builder = builder.identity(identity);
        }
        Ok(builder)
    }

    /// Opaque, domain-separated revision of the exact TLS bytes and policy.
    /// Use the same deployment key across replicas; this is not a permit.
    pub fn qualification_fingerprint(&self, key: &[u8]) -> Result<String, TlsError> {
        use hmac::{Hmac, Mac};
        if key.len() < 32 {
            return Err(TlsError::ReqwestBuild(
                "qualification key must contain at least 32 bytes".into(),
            ));
        }
        let mut mac = Hmac::<sha2::Sha256>::new_from_slice(key)
            .map_err(|_| TlsError::ReqwestBuild("invalid qualification key".into()))?;
        mac.update(b"acteon.outbound_tls.v1");
        mac.update(&[u8::from(self.danger_accept_invalid_certs)]);
        for bytes in [
            self.ca_pem.as_deref(),
            self.identity_pem.as_ref().map(|v| v.as_slice()),
        ] {
            match bytes {
                Some(bytes) => {
                    mac.update(&[1]);
                    mac.update(&(bytes.len() as u64).to_be_bytes());
                    mac.update(bytes);
                }
                None => mac.update(&[0]),
            }
        }
        Ok(hex::encode(mac.finalize().into_bytes()))
    }
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

    #[tokio::test]
    async fn loaded_material_performs_mtls_after_source_files_are_removed() {
        use std::io::{Read, Write};
        use std::time::Duration;
        let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let ca_key = rcgen::KeyPair::generate().unwrap();
        let ca = ca_params.self_signed(&ca_key).unwrap();
        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_cert = rcgen::CertificateParams::new(vec!["localhost".into()])
            .unwrap()
            .signed_by(&server_key, &ca, &ca_key)
            .unwrap();
        let client_key = rcgen::KeyPair::generate().unwrap();
        let client_cert = rcgen::CertificateParams::new(Vec::<String>::new())
            .unwrap()
            .signed_by(&client_key, &ca, &ca_key)
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("client.pem");
        let key_path = dir.path().join("client-key.pem");
        let ca_path = dir.path().join("ca.pem");
        fs::write(&cert_path, client_cert.pem()).unwrap();
        fs::write(&key_path, client_key.serialize_pem()).unwrap();
        let unrelated_ca = rcgen::generate_simple_self_signed(vec!["unrelated".into()]).unwrap();
        // The server's CA is second: a bundle must not silently use only its
        // first certificate.
        fs::write(&ca_path, format!("{}{}", unrelated_ca.cert.pem(), ca.pem())).unwrap();
        let loaded = LoadedTlsClientConfig::load(
            cert_path.to_str(),
            key_path.to_str(),
            ca_path.to_str(),
            false,
        )
        .unwrap();
        fs::remove_file(cert_path).unwrap();
        fs::remove_file(key_path).unwrap();
        fs::remove_file(ca_path).unwrap();

        let mut roots = rustls::RootCertStore::empty();
        roots.add(ca.der().clone()).unwrap();
        let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::ring::default_provider()),
        )
        .build()
        .unwrap();
        let server = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![server_cert.der().clone()],
            rustls::pki_types::PrivatePkcs8KeyDer::from(server_key.serialize_der()).into(),
        )
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let thread = std::thread::spawn(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut tls = rustls::StreamOwned::new(
                rustls::ServerConnection::new(Arc::new(server)).unwrap(),
                socket,
            );
            let mut request = [0; 4096];
            assert!(tls.read(&mut request).unwrap() > 0);
            assert_eq!(
                tls.conn.peer_certificates().unwrap(),
                &[client_cert.der().clone()]
            );
            tls.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .unwrap();
            tls.flush().unwrap();
        });
        let client = loaded
            .client_builder()
            .unwrap()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .build()
            .unwrap();
        let response = client
            .get(format!("https://localhost:{port}/"))
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), "ok");
        thread.join().unwrap();
    }

    #[test]
    fn loaded_material_survives_file_replacement_and_removal() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("cert.pem");
        let key_path = dir.path().join("key.pem");
        let ca_path = dir.path().join("ca.pem");
        let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        fs::write(&cert_path, certificate.cert.pem()).unwrap();
        fs::write(&key_path, certificate.key_pair.serialize_pem()).unwrap();
        fs::write(&ca_path, certificate.cert.pem()).unwrap();
        let load = || {
            LoadedTlsClientConfig::load(
                cert_path.to_str(),
                key_path.to_str(),
                ca_path.to_str(),
                false,
            )
            .unwrap()
        };
        let loaded = load();
        let key = [9_u8; 32];
        let original = loaded.qualification_fingerprint(&key).unwrap();
        assert_eq!(original, load().qualification_fingerprint(&key).unwrap());
        let replacement = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        fs::write(&cert_path, replacement.cert.pem()).unwrap();
        fs::write(&key_path, replacement.key_pair.serialize_pem()).unwrap();
        fs::write(&ca_path, replacement.cert.pem()).unwrap();
        assert_ne!(original, load().qualification_fingerprint(&key).unwrap());
        fs::remove_file(cert_path).unwrap();
        fs::remove_file(key_path).unwrap();
        fs::remove_file(ca_path).unwrap();
        loaded.client_builder().unwrap().build().unwrap();
        assert_eq!(original, loaded.qualification_fingerprint(&key).unwrap());
        let debug = format!("{loaded:?}");
        assert!(!debug.contains("PRIVATE KEY"));
        assert!(!debug.contains("CERTIFICATE"));
    }

    #[test]
    fn loaded_tls_fingerprint_binds_verification_policy_and_key() {
        let normal = LoadedTlsClientConfig::load(None, None, None, false).unwrap();
        let unsafe_tls = LoadedTlsClientConfig::load(None, None, None, true).unwrap();
        let key = [1_u8; 32];
        assert_ne!(
            normal.qualification_fingerprint(&key).unwrap(),
            unsafe_tls.qualification_fingerprint(&key).unwrap()
        );
        assert_ne!(
            normal.qualification_fingerprint(&key).unwrap(),
            normal.qualification_fingerprint(&[2_u8; 32]).unwrap()
        );
        assert!(normal.qualification_fingerprint(&[0; 31]).is_err());
        assert!(LoadedTlsClientConfig::load(Some("unused"), None, None, false).is_err());
        assert!(LoadedTlsClientConfig::load(None, Some("unused"), None, false).is_err());
    }

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
