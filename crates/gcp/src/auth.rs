use google_cloud_auth::credentials::{self, Credentials};
use tracing::info;

use crate::error::GcpProviderError;

/// Build GCP credentials from a service account JSON key file or inline JSON.
///
/// If `credentials_json` is `Some`, it is used directly.
/// If `credentials_path` is `Some`, the file is read and used.
/// Otherwise, returns `None` for ADC fallback.
///
/// # Errors
///
/// Returns [`GcpProviderError::CredentialError`] if the key is invalid.
pub async fn build_gcp_credentials(
    credentials_path: Option<&str>,
    credentials_json: Option<&str>,
) -> Result<Option<Credentials>, GcpProviderError> {
    let content = if let Some(json) = credentials_json {
        info!("loading GCP credentials from inline JSON");
        json.to_owned()
    } else if let Some(path) = credentials_path {
        info!("loading GCP credentials from service account file");
        tokio::fs::read_to_string(path).await.map_err(|e| {
            GcpProviderError::CredentialError(format!(
                "failed to read credentials file '{path}': {e}"
            ))
        })?
    } else {
        info!("using Application Default Credentials (ADC) for GCP");
        return Ok(None);
    };

    let key_value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| GcpProviderError::CredentialError(format!("invalid credentials JSON: {e}")))?;

    let creds = credentials::service_account::Builder::new(key_value)
        .build()
        .map_err(|e| {
            GcpProviderError::CredentialError(format!(
                "failed to build service account credentials: {e}"
            ))
        })?;

    Ok(Some(creds))
}
