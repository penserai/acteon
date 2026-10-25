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
    /// Approve an action in the authenticated context (operator or admin).
    pub async fn approve(&self, id: &str) -> Result<ApprovalActionResponse, Error> {
        self.approval_request(crate::PlatformOperation::ApprovalsApprove, id)
            .await
    }

    /// Reject an action in the authenticated context (operator or admin).
    pub async fn reject(&self, id: &str) -> Result<ApprovalActionResponse, Error> {
        self.approval_request(crate::PlatformOperation::ApprovalsReject, id)
            .await
    }

    /// Read an approval in the authenticated context.
    pub async fn get_approval(&self, id: &str) -> Result<Option<ApprovalStatusResponse>, Error> {
        match self
            .approval_request(crate::PlatformOperation::ApprovalsGetApproval, id)
            .await
        {
            Ok(value) => Ok(Some(value)),
            Err(Error::Http { status: 404, .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// List pending approvals in the authenticated context.
    pub async fn list_approvals(&self) -> Result<ApprovalListResponse, Error> {
        serde_json::from_value(
            self.platform_request(
                crate::PlatformOperation::ApprovalsListApprovals,
                &[],
                &[],
                None,
            )
            .await?,
        )
        .map_err(|error| Error::Deserialization(error.to_string()))
    }

    async fn approval_request<T: serde::de::DeserializeOwned>(
        &self,
        operation: crate::PlatformOperation,
        id: &str,
    ) -> Result<T, Error> {
        serde_json::from_value(
            self.platform_request(operation, &[("id", id)], &[], None)
                .await?,
        )
        .map_err(|error| Error::Deserialization(error.to_string()))
    }
}
