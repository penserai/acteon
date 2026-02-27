use google_cloud_auth::credentials::{self, Credentials};
use tracing::info;

use crate::error::GcpProviderError;

/// Build GCP credentials from a service account JSON key file.
///
/// If `credentials_path` is `Some`, reads the file and constructs credentials
/// from the service account key. If `None`, returns `None` so that callers can
/// fall back to Application Default Credentials (ADC) via the SDK default.
///
/// # Errors
///
/// Returns [`GcpProviderError::CredentialError`] if the file cannot be read or
/// contains invalid JSON.
pub async fn build_gcp_credentials(
    credentials_path: Option<&str>,
) -> Result<Option<Credentials>, GcpProviderError> {
    if let Some(path) = credentials_path {
        info!("loading GCP credentials from service account file");
        let content = tokio::fs::read_to_string(path).await.map_err(|e| {
            GcpProviderError::CredentialError(format!(
                "failed to read credentials file '{path}': {e}"
            ))
        })?;
        let key_value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            GcpProviderError::CredentialError(format!("invalid credentials JSON: {e}"))
        })?;
        let creds = credentials::service_account::Builder::new(key_value)
            .build()
            .map_err(|e| {
                GcpProviderError::CredentialError(format!(
                    "failed to build service account credentials: {e}"
                ))
            })?;
        Ok(Some(creds))
    } else {
        info!("using Application Default Credentials (ADC) for GCP");
        Ok(None)
    }
}
