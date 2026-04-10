//! Silences CRUD — thin HTTP wrappers for the `/v1/silences` endpoints.
//!
//! Silences are tenant-scoped time-bounded label-pattern mutes that
//! suppress dispatched actions during the active window. See the
//! feature page at `docs/book/features/silences.md` for the full model.

pub use acteon_core::{MatchOp, Silence, SilenceMatcher};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Request body for creating a silence.
#[derive(Debug, Clone, Serialize)]
pub struct CreateSilenceRequest {
    pub namespace: String,
    pub tenant: String,
    pub matchers: Vec<SilenceMatcher>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub starts_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<u64>,
    pub comment: String,
}

/// Request body for updating a silence.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UpdateSilenceRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ends_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Response shape from silence endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SilenceResponse {
    pub id: String,
    pub namespace: String,
    pub tenant: String,
    pub matchers: Vec<SilenceMatcher>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub created_by: String,
    pub comment: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub active: bool,
}

/// Paginated list of silences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSilencesResponse {
    pub silences: Vec<SilenceResponse>,
    pub count: usize,
}

/// Query parameters for listing silences.
#[derive(Debug, Clone, Default)]
pub struct ListSilencesQuery {
    pub namespace: Option<String>,
    pub tenant: Option<String>,
    pub include_expired: bool,
}

impl ActeonClient {
    /// Create a new silence.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, CreateSilenceRequest, MatchOp, SilenceMatcher};
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let req = CreateSilenceRequest {
    ///     namespace: "prod".into(),
    ///     tenant: "acme".into(),
    ///     matchers: vec![
    ///         SilenceMatcher::new("severity", "warning", MatchOp::Equal).unwrap(),
    ///     ],
    ///     starts_at: None,
    ///     ends_at: None,
    ///     duration_seconds: Some(3600),
    ///     comment: "deploy window".into(),
    /// };
    /// let silence = client.create_silence(&req).await?;
    /// println!("created silence {}", silence.id);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn create_silence(
        &self,
        req: &CreateSilenceRequest,
    ) -> Result<SilenceResponse, Error> {
        let url = format!("{}/v1/silences", self.base_url);
        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<SilenceResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to create silence: {}", response.status()),
            })
        }
    }

    /// List silences, optionally filtered by namespace, tenant, or expiry.
    pub async fn list_silences(
        &self,
        query: &ListSilencesQuery,
    ) -> Result<ListSilencesResponse, Error> {
        let url = format!("{}/v1/silences", self.base_url);
        let mut req = self.add_auth(self.client.get(&url));
        if let Some(ref ns) = query.namespace {
            req = req.query(&[("namespace", ns)]);
        }
        if let Some(ref t) = query.tenant {
            req = req.query(&[("tenant", t)]);
        }
        if query.include_expired {
            req = req.query(&[("include_expired", "true")]);
        }

        let response = req
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ListSilencesResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to list silences: {}", response.status()),
            })
        }
    }

    /// Fetch a single silence by ID.
    pub async fn get_silence(&self, id: &str) -> Result<Option<SilenceResponse>, Error> {
        let url = format!("{}/v1/silences/{id}", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<SilenceResponse>()
                .await
                .map(Some)
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to get silence: {}", response.status()),
            })
        }
    }

    /// Extend the end time or edit the comment on an existing silence.
    pub async fn update_silence(
        &self,
        id: &str,
        req: &UpdateSilenceRequest,
    ) -> Result<SilenceResponse, Error> {
        let url = format!("{}/v1/silences/{id}", self.base_url);
        let response = self
            .add_auth(self.client.put(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<SilenceResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to update silence: {}", response.status()),
            })
        }
    }

    /// Expire a silence immediately by ID.
    pub async fn delete_silence(&self, id: &str) -> Result<(), Error> {
        let url = format!("{}/v1/silences/{id}", self.base_url);
        let response = self
            .add_auth(self.client.delete(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("failed to delete silence: {}", response.status()),
            })
        }
    }
}
