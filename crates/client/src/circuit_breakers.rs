use acteon_core::{CircuitBreakerActionResponse, ListCircuitBreakersResponse};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

impl ActeonClient {
    /// List all circuit breakers and their current status.
    ///
    /// Requires admin permissions.
    pub async fn list_circuit_breakers(&self) -> Result<ListCircuitBreakersResponse, Error> {
        let url = format!("{}/admin/circuit-breakers", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ListCircuitBreakersResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list circuit breakers: {}", response.status()),
            })
        }
    }

    /// Trip (force open) a circuit breaker for a specific provider.
    ///
    /// Requires admin permissions.
    pub async fn trip_circuit_breaker(
        &self,
        provider: &str,
    ) -> Result<CircuitBreakerActionResponse, Error> {
        let url = format!("{}/admin/circuit-breakers/{}/trip", self.base_url, provider);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<CircuitBreakerActionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }

    /// Reset (force close) a circuit breaker for a specific provider.
    ///
    /// Requires admin permissions.
    pub async fn reset_circuit_breaker(
        &self,
        provider: &str,
    ) -> Result<CircuitBreakerActionResponse, Error> {
        let url = format!(
            "{}/admin/circuit-breakers/{}/reset",
            self.base_url, provider
        );

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<CircuitBreakerActionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            let error = response
                .json::<ErrorResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Err(Error::Api {
                code: error.code,
                message: error.message,
                retryable: error.retryable,
            })
        }
    }
}
