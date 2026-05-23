use std::sync::Arc;

use azure_core::credentials::{Secret, TokenCredential};
use tracing::{debug, info};

use crate::config::AzureBaseConfig;
use crate::error::AzureProviderError;

/// Build an Azure credential from the given [`AzureBaseConfig`].
///
/// If `tenant_id`, `client_id`, and `client_credential` are all present,
/// uses `ClientSecretCredential` for service-principal authentication.
/// Otherwise falls back to `AzureCliCredential` which uses the Azure CLI
/// login context (suitable for development and CI/CD environments).
///
/// For production environments without service principal credentials,
/// use `ManagedIdentityCredential` by providing tenant/client/credential
/// config or by configuring the Azure environment externally.
///
/// # Errors
///
/// Returns [`AzureProviderError::CredentialError`] if credential construction fails.
#[allow(clippy::unused_async)]
pub async fn build_azure_credential(
    config: &AzureBaseConfig,
) -> Result<Arc<dyn TokenCredential>, AzureProviderError> {
    if let (Some(tenant_id), Some(client_id), Some(client_cred)) = (
        &config.tenant_id,
        &config.client_id,
        &config.client_credential,
    ) {
        info!("using service-principal credentials for Azure");
        debug!(tenant_id = %tenant_id, "building ClientSecretCredential");

        let credential = azure_identity::ClientSecretCredential::new(
            tenant_id,
            client_id.clone(),
            Secret::new(client_cred.clone()),
            None,
        )
        .map_err(|e| AzureProviderError::CredentialError(e.to_string()))?;

        Ok(credential)
    } else {
        info!("using AzureCliCredential for Azure (dev/CI fallback)");
        let credential = azure_identity::AzureCliCredential::new(None)
            .map_err(|e| AzureProviderError::CredentialError(e.to_string()))?;

        Ok(credential)
    }
}
