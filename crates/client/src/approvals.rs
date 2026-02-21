use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Response from approving or rejecting an action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalActionResponse {
    /// The approval ID.
    pub id: String,
    /// The resulting status ("approved" or "rejected").
    pub status: String,
    /// The outcome of the original action (only present when approved).
    pub outcome: Option<serde_json::Value>,
}

/// Public-facing approval status (no payload exposed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalStatusResponse {
    /// The approval token.
    pub token: String,
    /// Current status: "pending", "approved", or "rejected".
    pub status: String,
    /// Rule that triggered the approval.
    pub rule: String,
    /// When the approval was created.
    pub created_at: String,
    /// When the approval expires.
    pub expires_at: String,
    /// When a decision was made (if any).
    pub decided_at: Option<String>,
    /// Optional message from the rule.
    pub message: Option<String>,
}

/// Response from listing pending approvals.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalListResponse {
    /// List of pending approvals.
    pub approvals: Vec<ApprovalStatusResponse>,
    /// Total number of approvals returned.
    pub count: usize,
}

impl ActeonClient {
    /// Approve a pending action by namespace, tenant, ID, and HMAC signature.
    ///
    /// The original action is executed upon approval. This does not require
    /// authentication -- the HMAC signature in the query string serves as
    /// proof of authorization.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.approve("payments", "tenant-1", "abc-123", "hmac-sig", 1700000000).await?;
    /// println!("Status: {}, Outcome: {:?}", result.status, result.outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn approve(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
    ) -> Result<ApprovalActionResponse, Error> {
        self.approve_with_kid(namespace, tenant, id, sig, expires_at, None)
            .await
    }

    /// Approve a pending action, optionally specifying which HMAC key was used.
    ///
    /// When `kid` is `Some`, the `kid` query parameter is appended so the
    /// server can look up the correct key without trying all of them.
    pub async fn approve_with_kid(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<ApprovalActionResponse, Error> {
        let mut url = format!(
            "{}/v1/approvals/{}/{}/{}/approve?sig={}&expires_at={}",
            self.base_url, namespace, tenant, id, sig, expires_at
        );
        if let Some(k) = kid {
            write!(url, "&kid={k}").expect("writing to String cannot fail");
        }

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalActionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: "Approval not found or expired".to_string(),
            })
        } else if response.status() == reqwest::StatusCode::GONE {
            Err(Error::Http {
                status: 410,
                message: "Approval already decided".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to approve: {}", response.status()),
            })
        }
    }

    /// Reject a pending action by namespace, tenant, ID, and HMAC signature.
    ///
    /// This does not require authentication -- the HMAC signature in the
    /// query string serves as proof of authorization.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.reject("payments", "tenant-1", "abc-123", "hmac-sig", 1700000000).await?;
    /// println!("Status: {}", result.status);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reject(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
    ) -> Result<ApprovalActionResponse, Error> {
        self.reject_with_kid(namespace, tenant, id, sig, expires_at, None)
            .await
    }

    /// Reject a pending action, optionally specifying which HMAC key was used.
    ///
    /// When `kid` is `Some`, the `kid` query parameter is appended so the
    /// server can look up the correct key without trying all of them.
    pub async fn reject_with_kid(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<ApprovalActionResponse, Error> {
        let mut url = format!(
            "{}/v1/approvals/{}/{}/{}/reject?sig={}&expires_at={}",
            self.base_url, namespace, tenant, id, sig, expires_at
        );
        if let Some(k) = kid {
            write!(url, "&kid={k}").expect("writing to String cannot fail");
        }

        let response = self
            .client
            .post(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalActionResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Err(Error::Http {
                status: 404,
                message: "Approval not found or expired".to_string(),
            })
        } else if response.status() == reqwest::StatusCode::GONE {
            Err(Error::Http {
                status: 410,
                message: "Approval already decided".to_string(),
            })
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to reject: {}", response.status()),
            })
        }
    }

    /// Get the status of an approval by namespace, tenant, ID, and HMAC signature.
    ///
    /// Returns `None` if the approval is not found or has expired.
    /// Does not expose the original action payload.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// if let Some(status) = client.get_approval("payments", "tenant-1", "abc-123", "hmac-sig", 1700000000).await? {
    ///     println!("Approval status: {}", status.status);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn get_approval(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
    ) -> Result<Option<ApprovalStatusResponse>, Error> {
        self.get_approval_with_kid(namespace, tenant, id, sig, expires_at, None)
            .await
    }

    /// Get approval status, optionally specifying which HMAC key was used.
    ///
    /// When `kid` is `Some`, the `kid` query parameter is appended so the
    /// server can look up the correct key without trying all of them.
    pub async fn get_approval_with_kid(
        &self,
        namespace: &str,
        tenant: &str,
        id: &str,
        sig: &str,
        expires_at: i64,
        kid: Option<&str>,
    ) -> Result<Option<ApprovalStatusResponse>, Error> {
        let mut url = format!(
            "{}/v1/approvals/{}/{}/{}?sig={}&expires_at={}",
            self.base_url, namespace, tenant, id, sig, expires_at
        );
        if let Some(k) = kid {
            write!(url, "&kid={k}").expect("writing to String cannot fail");
        }

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalStatusResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(Some(result))
        } else if response.status() == reqwest::StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to get approval: {}", response.status()),
            })
        }
    }

    /// List pending approvals filtered by namespace and tenant.
    ///
    /// Requires authentication.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.list_approvals("payments", "tenant-1").await?;
    /// for approval in result.approvals {
    ///     println!("{}: {}", approval.token, approval.status);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_approvals(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<ApprovalListResponse, Error> {
        let url = format!("{}/v1/approvals", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .query(&[("namespace", namespace), ("tenant", tenant)])
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ApprovalListResponse>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list approvals: {}", response.status()),
            })
        }
    }
}
