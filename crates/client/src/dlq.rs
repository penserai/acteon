use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Dead letter queue statistics.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DlqStatsResponse {
    /// Whether the DLQ is enabled.
    pub enabled: bool,
    /// Number of entries currently in the DLQ.
    pub count: usize,
}

/// A single entry in the dead letter queue.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DlqEntry {
    /// The original action ID.
    pub action_id: String,
    /// Namespace of the failed action.
    pub namespace: String,
    /// Tenant of the failed action.
    pub tenant: String,
    /// Target provider.
    pub provider: String,
    /// Action type discriminator.
    pub action_type: String,
    /// Error message describing the failure.
    pub error: String,
    /// Number of delivery attempts made.
    pub attempts: u32,
    /// Unix timestamp (seconds) when the entry was added.
    pub timestamp: u64,
}

/// Response from draining the dead letter queue.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DlqDrainResponse {
    /// The drained entries.
    pub entries: Vec<DlqEntry>,
    /// Number of entries drained.
    pub count: usize,
}

impl ActeonClient {
    /// Get dead letter queue statistics.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let stats = client.dlq_stats().await?;
    /// println!("DLQ enabled: {}, count: {}", stats.enabled, stats.count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dlq_stats(&self) -> Result<DlqStatsResponse, Error> {
        let url = format!("{}/v1/dlq/stats", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<DlqStatsResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get DLQ stats: {}", response.status()),
            })
        }
    }

    /// Drain all entries from the dead letter queue.
    ///
    /// Returns the drained entries along with a count.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.dlq_drain().await?;
    /// println!("Drained {} entries", result.count);
    /// for entry in &result.entries {
    ///     println!("  {}: {} ({})", entry.action_id, entry.error, entry.attempts);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dlq_drain(&self) -> Result<DlqDrainResponse, Error> {
        let url = format!("{}/v1/dlq/drain", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<DlqDrainResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to drain DLQ: {}", response.status()),
            })
        }
    }
}
