use serde::{Deserialize, Serialize};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// Query parameters for listing events.
#[derive(Debug, Default, Clone, Serialize)]
pub struct EventQuery {
    /// Filter by namespace (required).
    pub namespace: String,
    /// Filter by tenant (required).
    pub tenant: String,
    /// Filter by state (e.g., "open", "closed").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Maximum number of results to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Current state of an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventState {
    /// The event fingerprint.
    pub fingerprint: String,
    /// Current state of the event.
    pub state: String,
    /// The action type that created this event.
    pub action_type: Option<String>,
    /// When the state was last updated.
    pub updated_at: Option<String>,
}

/// Response from listing events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventListResponse {
    /// List of events.
    pub events: Vec<EventState>,
    /// Total number of events returned.
    pub count: usize,
}

/// Response from transitioning an event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionResponse {
    /// The event fingerprint.
    pub fingerprint: String,
    /// The previous state.
    pub previous_state: String,
    /// The new state.
    pub new_state: String,
    /// Whether the transition triggered a notification.
    pub notify: bool,
}

impl ActeonClient {
    /// List events filtered by namespace, tenant, and optionally status.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, EventQuery};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let query = EventQuery {
    ///     namespace: "notifications".to_string(),
    ///     tenant: "tenant-1".to_string(),
    ///     status: Some("open".to_string()),
    ///     limit: Some(50),
    /// };
    /// let events = client.list_events(&query).await?;
    /// for event in events.events {
    ///     println!("{}: {}", event.fingerprint, event.state);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_events(&self, query: &EventQuery) -> Result<EventListResponse, Error> {
        let url = format!("{}/v1/events", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(query)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<EventListResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list events: {}", response.status()),
            })
        }
    }

    /// Get the current state of an event by fingerprint.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(event) = client.get_event("fingerprint-123", "notifications", "tenant-1").await? {
    ///     println!("Event state: {}", event.state);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_event(
        &self,
        fingerprint: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<Option<EventState>, Error> {
        let url = format!("{}/v1/events/{}", self.base_url, fingerprint);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let event = response
                .json::<EventState>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(event))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get event: {}", response.status()),
            })
        }
    }

    /// Transition an event to a new state.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.transition_event(
    ///     "fingerprint-123",
    ///     "investigating",
    ///     "notifications",
    ///     "tenant-1"
    /// ).await?;
    /// println!("Transitioned from {} to {}", result.previous_state, result.new_state);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn transition_event(
        &self,
        fingerprint: &str,
        to_state: &str,
        namespace: &str,
        tenant: &str,
    ) -> Result<TransitionResponse, Error> {
        let url = format!("{}/v1/events/{}/transition", self.base_url, fingerprint);

        let body = serde_json::json!({
            "to": to_state,
            "namespace": namespace,
            "tenant": tenant,
        });

        let response = self
            .add_auth(self.client.put(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<TransitionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: format!("Event not found: {fingerprint}"),
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
