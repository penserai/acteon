use crate::{ActeonClient, Error};

impl ActeonClient {
    /// List per-provider health status, execution metrics, and latency percentiles.
    pub async fn list_provider_health(
        &self,
    ) -> Result<acteon_core::ListProviderHealthResponse, Error> {
        let url = format!("{}/v1/providers/health", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<acteon_core::ListProviderHealthResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list provider health: {}", response.status()),
            })
        }
    }
}
