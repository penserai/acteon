use acteon_core::{Action, ActionOutcome, Attachment};
use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

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
    /// Action was processed successfully.
    Success(ActionOutcome),
    /// Action processing failed.
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
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new("ns", "tenant", "email", "send", serde_json::json!({}));
    ///
    /// let outcome = client.dispatch(&action).await?;
    /// println!("Outcome: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch(&self, action: &Action) -> Result<ActionOutcome, Error> {
        self.dispatch_inner(action, false).await
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
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new("ns", "tenant", "email", "send", serde_json::json!({}));
    ///
    /// let outcome = client.dispatch_dry_run(&action).await?;
    /// println!("Would result in: {:?}", outcome);
    /// # Ok(())
    /// # }
    /// ```
    pub async fn dispatch_dry_run(&self, action: &Action) -> Result<ActionOutcome, Error> {
        self.dispatch_inner(action, true).await
    }

    /// Dispatch a single action with file attachments.
    ///
    /// This is a convenience wrapper that clones the action, sets the given
    /// attachments, and dispatches it. For repeated use, prefer constructing
    /// the action with [`Action::with_attachments`] directly.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, Attachment};
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new("ns", "tenant", "email", "send", serde_json::json!({}));
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
        action: &Action,
        attachments: Vec<Attachment>,
    ) -> Result<ActionOutcome, Error> {
        let mut action = action.clone();
        action.attachments = attachments;
        self.dispatch_inner(&action, false).await
    }

    async fn dispatch_inner(&self, action: &Action, dry_run: bool) -> Result<ActionOutcome, Error> {
        let mut url = format!("{}/v1/dispatch", self.base_url);
        if dry_run {
            url.push_str("?dry_run=true");
        }

        let response = self
            .add_auth(self.client.post(&url))
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

    /// Dispatch multiple actions in a single request.
    ///
    /// Returns a result for each action, preserving order.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let actions = vec![
    ///     Action::new("ns", "tenant", "email", "send", serde_json::json!({})),
    ///     Action::new("ns", "tenant", "sms", "send", serde_json::json!({})),
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
    pub async fn dispatch_batch(&self, actions: &[Action]) -> Result<Vec<BatchResult>, Error> {
        self.dispatch_batch_inner(actions, false).await
    }

    /// Dispatch multiple actions in dry-run mode.
    ///
    /// Evaluates rules for each action and returns the verdicts without
    /// executing any actions.
    pub async fn dispatch_batch_dry_run(
        &self,
        actions: &[Action],
    ) -> Result<Vec<BatchResult>, Error> {
        self.dispatch_batch_inner(actions, true).await
    }

    async fn dispatch_batch_inner(
        &self,
        actions: &[Action],
        dry_run: bool,
    ) -> Result<Vec<BatchResult>, Error> {
        let mut url = format!("{}/v1/dispatch/batch", self.base_url);
        if dry_run {
            url.push_str("?dry_run=true");
        }

        let response = self
            .add_auth(self.client.post(&url))
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
