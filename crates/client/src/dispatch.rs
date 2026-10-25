use acteon_core::{ActionOutcome, ActionRequest, Attachment};
use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Explicit reference to an operator-issued permit; this is not authority proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermitReference {
    pub id: String,
    pub accepted_revision: u64,
}

/// Error response from the API.
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Error code.
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Whether the request can be retried.
    #[serde(default)]
    pub retryable: bool,
}

/// Result from a batch dispatch operation.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BatchResult {
    /// `ActionRequest` was processed successfully.
    Success(ActionOutcome),
    /// `ActionRequest` processing failed.
    Error {
        /// Error details.
        error: ErrorResponse,
    },
}

impl BatchResult {
    /// Returns `true` if this is a success result.
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Success(_))
    }

    /// Returns `true` if this is an error result.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }

    /// Returns the outcome if this is a success result.
    pub fn outcome(&self) -> Option<&ActionOutcome> {
        match self {
            Self::Success(outcome) => Some(outcome),
            Self::Error { .. } => None,
        }
    }

    /// Returns the error if this is an error result.
    pub fn error(&self) -> Option<&ErrorResponse> {
        match self {
            Self::Success(_) => None,
            Self::Error { error } => Some(error),
        }
    }
}

impl ActeonClient {
    /// Dispatch a single action.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::ActionRequest;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = ActionRequest::new("email", "send", serde_json::json!({}));
    ///
    /// let outcome = client.dispatch(&action).await?;
    /// println!("Outcome: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch(&self, action: &ActionRequest) -> Result<ActionOutcome, Error> {
        self.dispatch_inner(action, false, None).await
    }

    /// Sign an action with an Ed25519 key, then dispatch it.
    ///
    /// Computes the action's canonical bytes, signs them, sets the
    /// `signature` and `signer_id` fields on a cloned action, and
    /// dispatches the signed copy. Requires the `signing` feature.
    ///
    /// The dispatched action does NOT carry a `kid` — use
    /// [`dispatch_signed_with_kid`](Self::dispatch_signed_with_kid)
    /// to stamp a key identifier when participating in a key
    /// rotation window.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::ActionRequest;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = ActionRequest::new("email", "send", serde_json::json!({}));
    /// let key = acteon_crypto::signing::parse_signing_key("hex-or-b64-key", "my-signer")
    ///     .expect("valid key");
    /// let outcome = client.dispatch_signed(&action, &key).await?;
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "signing")]
    pub async fn dispatch_signed(
        &self,
        action: &ActionRequest,
        key: &acteon_crypto::signing::ActionSigningKey,
    ) -> Result<ActionOutcome, Error> {
        let mut signed = action.clone();
        let scope = self.identity().await?.scope;
        let canonical = signed.clone().into_action(&scope).canonical_bytes();
        signed.signature = Some(key.sign(&canonical));
        signed.signer_id = Some(key.signer_id().to_owned());
        self.dispatch_inner(&signed, false, None).await
    }

    /// Sign an action and stamp it with a `kid` for the key rotation
    /// window, then dispatch it.
    ///
    /// During a rotation, the same `signer_id` may have multiple
    /// active keys on the server (old + new). When the client sends
    /// `kid`, the server's [`Keyring::verify_with_kid`] looks up the
    /// exact key and fails fast on a stale or never-issued kid.
    /// Without `kid`, the server falls back to a try-all-keys check.
    ///
    /// `kid` is excluded from the canonical bytes used for signing,
    /// so a signature produced with one kid stays valid if the
    /// server later removes that key (until the operator retires it).
    ///
    /// [`Keyring::verify_with_kid`]: acteon_crypto::signing::Keyring::verify_with_kid
    #[cfg(feature = "signing")]
    pub async fn dispatch_signed_with_kid(
        &self,
        action: &ActionRequest,
        key: &acteon_crypto::signing::ActionSigningKey,
        kid: impl Into<String>,
    ) -> Result<ActionOutcome, Error> {
        let mut signed = action.clone();
        let scope = self.identity().await?.scope;
        let canonical = signed.clone().into_action(&scope).canonical_bytes();
        signed.signature = Some(key.sign(&canonical));
        signed.signer_id = Some(key.signer_id().to_owned());
        signed.kid = Some(kid.into());
        self.dispatch_inner(&signed, false, None).await
    }

    /// Dispatch a single action in dry-run mode.
    ///
    /// Evaluates rules and returns the verdict without executing the action,
    /// recording state, or emitting audit records.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::ActionRequest;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = ActionRequest::new("email", "send", serde_json::json!({}));
    ///
    /// let outcome = client.dispatch_dry_run(&action).await?;
    /// println!("Would result in: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_dry_run(&self, action: &ActionRequest) -> Result<ActionOutcome, Error> {
        self.dispatch_inner(action, true, None).await
    }

    /// Dispatch a single action with file attachments.
    ///
    /// This is a convenience wrapper that clones the action, sets the given
    /// attachments, and dispatches it. For repeated use, prefer constructing
    /// the action with [`ActionRequest::with_attachments`] directly.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, Attachment};
    /// use acteon_core::ActionRequest;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = ActionRequest::new("email", "send", serde_json::json!({}));
    ///
    /// let attachments = vec![
    ///     Attachment {
    ///         id: "att-1".into(),
    ///         name: "Hello".into(),
    ///         filename: "hello.txt".into(),
    ///         content_type: "text/plain".into(),
    ///         data_base64: "SGVsbG8=".into(),
    ///     },
    /// ];
    ///
    /// let outcome = client.dispatch_with_attachments(&action, attachments).await?;
    /// println!("Outcome: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_with_attachments(
        &self,
        action: &ActionRequest,
        attachments: Vec<Attachment>,
    ) -> Result<ActionOutcome, Error> {
        let mut action = action.clone();
        action.attachments = attachments;
        self.dispatch_inner(&action, false, None).await
    }

    /// Dispatch with explicit permit references and the configured credentials.
    pub async fn dispatch_with_permits(
        &self,
        action: &ActionRequest,
        permits: &[PermitReference],
    ) -> Result<ActionOutcome, Error> {
        self.dispatch_inner(action, false, Some(permits)).await
    }

    /// Dispatch a batch using explicit permit references for every action.
    pub async fn dispatch_batch_with_permits(
        &self,
        actions: &[ActionRequest],
        permits: &[PermitReference],
    ) -> Result<Vec<BatchResult>, Error> {
        self.dispatch_batch_inner(actions, false, Some(permits))
            .await
    }

    async fn dispatch_inner(
        &self,
        action: &ActionRequest,
        dry_run: bool,
        permits: Option<&[PermitReference]>,
    ) -> Result<ActionOutcome, Error> {
        let mut url = format!("{}/v1/dispatch", self.base_url);
        if dry_run {
            url.push_str("?dry_run=true");
        }

        let mut request = self.add_auth(self.client.post(&url));
        if let Some(permits) = permits {
            request = request.header(
                "x-acteon-execution-permits",
                serde_json::to_string(permits).map_err(|e| Error::Configuration(e.to_string()))?,
            );
        }
        let response = request
            .json(action)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let outcome = response
                .json::<ActionOutcome>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(outcome)
        } else {
            Err(dispatch_error(response).await?)
        }
    }

    /// Dispatch multiple actions in a single request.
    ///
    /// Returns a result for each action, preserving order.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::ActionRequest;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let actions = vec![
    ///     ActionRequest::new("email", "send", serde_json::json!({})),
    ///     ActionRequest::new("sms", "send", serde_json::json!({})),
    /// ];
    ///
    /// let results = client.dispatch_batch(&actions).await?;
    /// for result in results {
    ///     match result {
    ///         acteon_client::BatchResult::Success(outcome) => println!("Success: {:?}", outcome),
    ///         acteon_client::BatchResult::Error { error } => println!("Error: {}", error.message),
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_batch(
        &self,
        actions: &[ActionRequest],
    ) -> Result<Vec<BatchResult>, Error> {
        self.dispatch_batch_inner(actions, false, None).await
    }

    /// Dispatch multiple actions in dry-run mode.
    ///
    /// Evaluates rules for each action and returns the verdicts without
    /// executing any actions.
    pub async fn dispatch_batch_dry_run(
        &self,
        actions: &[ActionRequest],
    ) -> Result<Vec<BatchResult>, Error> {
        self.dispatch_batch_inner(actions, true, None).await
    }

    async fn dispatch_batch_inner(
        &self,
        actions: &[ActionRequest],
        dry_run: bool,
        permits: Option<&[PermitReference]>,
    ) -> Result<Vec<BatchResult>, Error> {
        let mut url = format!("{}/v1/dispatch/batch", self.base_url);
        if dry_run {
            url.push_str("?dry_run=true");
        }

        let mut request = self.add_auth(self.client.post(&url));
        if let Some(permits) = permits {
            request = request.header(
                "x-acteon-execution-permits",
                serde_json::to_string(permits).map_err(|e| Error::Configuration(e.to_string()))?,
            );
        }
        let response = request
            .json(actions)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let results = response
                .json::<Vec<BatchResult>>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(results)
        } else {
            Err(dispatch_error(response).await?)
        }
    }
}

async fn dispatch_error(response: reqwest::Response) -> Result<Error, Error> {
    let status = response.status().as_u16();
    let body = response
        .text()
        .await
        .map_err(|e| Error::Connection(e.to_string()))?;
    if let Ok(error) = serde_json::from_str::<ErrorResponse>(&body) {
        Ok(Error::Api {
            code: error.code,
            message: error.message,
            retryable: error.retryable,
        })
    } else {
        Ok(Error::Http {
            status,
            message: body,
        })
    }
}

#[cfg(all(test, feature = "signing"))]
mod context_tests {
    use super::*;
    use acteon_core::TenantContext;
    use axum::{
        Json, Router,
        http::HeaderMap,
        routing::{get, post},
    };
    use serde_json::{Value, json};
    use std::future::IntoFuture;

    #[tokio::test]
    async fn signing_binds_authenticated_context_without_sending_scope() {
        let key = acteon_crypto::signing::ActionSigningKey::from_bytes([17; 32], "pipeline");
        let verifier = key.verifying_key();
        let app = Router::new()
            .route("/v1/auth/identity", get(|headers: HeaderMap| async move {
                assert_eq!(headers["authorization"], "Bearer service-key");
                Json(json!({"credential_id":"service-key","role":"executor","auth_method":"api_key",
                    "scope":{"namespace":"payments","tenant":"bank-a"},"principal":null}))
            }))
            .route("/v1/dispatch", post(move |headers: HeaderMap, Json(body): Json<Value>| {
                let verifier = verifier.clone();
                async move {
                    assert_eq!(headers["authorization"], "Bearer service-key");
                    assert!(body.get("namespace").is_none());
                    assert!(body.get("tenant").is_none());
                    assert_eq!(body["payload"]["tenant"], "business-value");
                    let request: ActionRequest = serde_json::from_value(body).unwrap();
                    let action = request.into_action(&TenantContext::new("payments", "bank-a"));
                    let signature = action.signature.as_ref().unwrap();
                    verifier.verify(signature, &action.canonical_bytes()).unwrap();
                    let mut foreign = action.clone();
                    foreign.tenant = "bank-b".into();
                    assert!(verifier.verify(signature, &foreign.canonical_bytes()).is_err());
                    Json(json!("Deduplicated"))
                }
            }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = ActeonClient::builder(format!("http://{}", listener.local_addr().unwrap()))
            .api_key("service-key")
            .build()
            .unwrap();
        let server = tokio::spawn(axum::serve(listener, app).into_future());
        let action = ActionRequest::new("email", "send", json!({"tenant":"business-value"}));
        client.dispatch_signed(&action, &key).await.unwrap();
        client
            .dispatch_signed_with_kid(&action, &key, "rotation-2")
            .await
            .unwrap();
        server.abort();
    }
}
