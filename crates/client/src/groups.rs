use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// Summary of an event group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupSummary {
    /// Unique identifier for the group.
    pub group_id: String,
    /// The group key used for matching events.
    pub group_key: String,
    /// Number of events in the group.
    pub event_count: usize,
    /// Current state of the group.
    pub state: String,
    /// When the group will be notified.
    pub notify_at: Option<String>,
    /// When the group was created.
    pub created_at: String,
}

/// Response from listing groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupListResponse {
    /// List of groups.
    pub groups: Vec<GroupSummary>,
    /// Total number of groups.
    pub total: usize,
}

/// Detailed information about a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupDetail {
    /// Group summary.
    pub group: GroupSummary,
    /// Event fingerprints in this group.
    pub events: Vec<String>,
    /// Labels used to group events.
    pub labels: std::collections::HashMap<String, String>,
}

/// Response from flushing a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlushGroupResponse {
    /// The group ID that was flushed.
    pub group_id: String,
    /// Number of events that were flushed.
    pub event_count: usize,
    /// Whether notification was sent.
    pub notified: bool,
}

impl ActeonClient {
    /// List all active event groups.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let groups = client.list_groups().await?;
    /// println!("Active groups: {}", groups.total);
    /// for group in groups.groups {
    ///     println!("{}: {} events", group.group_id, group.event_count);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_groups(&self) -> Result<GroupListResponse, Error> {
        let url = format!("{}/v1/groups", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<GroupListResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list groups: {}", response.status()),
            })
        }
    }

    /// Get details of a specific group.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(group) = client.get_group("group-key-123").await? {
    ///     println!("Group {} has {} events", group.group.group_id, group.group.event_count);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_group(&self, group_key: &str) -> Result<Option<GroupDetail>, Error> {
        let url = format!("{}/v1/groups/{}", self.base_url, group_key);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<GroupDetail>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get group: {}", response.status()),
            })
        }
    }

    /// Force flush a group, triggering immediate notification.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.flush_group("group-key-123").await?;
    /// println!("Flushed group {} with {} events", result.group_id, result.event_count);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn flush_group(&self, group_key: &str) -> Result<FlushGroupResponse, Error> {
        let url = format!("{}/v1/groups/{}", self.base_url, group_key);

        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<FlushGroupResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Group not found: {group_key}"),
            })
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
