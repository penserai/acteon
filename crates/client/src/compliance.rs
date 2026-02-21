use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Current compliance configuration status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceStatus {
    /// The active compliance mode (`"none"`, `"soc2"`, or `"hipaa"`).
    pub mode: String,
    /// Whether audit writes block the dispatch pipeline.
    pub sync_audit_writes: bool,
    /// Whether audit records are immutable (deletes rejected).
    pub immutable_audit: bool,
    /// Whether `SHA-256` hash chaining is enabled for audit records.
    pub hash_chain: bool,
}

/// Result of verifying the integrity of an audit hash chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashChainVerification {
    /// Whether the hash chain is intact (no broken links).
    pub valid: bool,
    /// Total number of records verified.
    pub records_checked: u64,
    /// ID of the first record where the chain broke, if any.
    #[serde(default)]
    pub first_broken_at: Option<String>,
    /// ID of the first record in the verified range.
    #[serde(default)]
    pub first_record_id: Option<String>,
    /// ID of the last record in the verified range.
    #[serde(default)]
    pub last_record_id: Option<String>,
}

/// Request body for hash chain verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyHashChainRequest {
    /// Namespace to verify.
    pub namespace: String,
    /// Tenant to verify.
    pub tenant: String,
    /// Optional start of the time range (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Optional end of the time range (ISO 8601).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

impl ActeonClient {
    /// Get the current compliance configuration status.
    pub async fn get_compliance_status(&self) -> Result<ComplianceStatus, Error> {
        let url = format!("{}/v1/compliance/status", self.base_url);
        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<ComplianceStatus>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to get compliance status".into(),
            })
        }
    }

    /// Verify the integrity of the audit hash chain for a namespace/tenant pair.
    pub async fn verify_audit_chain(
        &self,
        req: &VerifyHashChainRequest,
    ) -> Result<HashChainVerification, Error> {
        let url = format!("{}/v1/audit/verify", self.base_url);
        let response = self
            .add_auth(self.client.post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            response
                .json::<HashChainVerification>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: "Failed to verify audit chain".into(),
            })
        }
    }
}
