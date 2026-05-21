//! A2A protocol client helpers.
//!
//! Wraps the REST surface of the A2A endpoints exposed under
//! `/a2a/{namespace}/{tenant}/…`. The JSON-RPC transport is not
//! exposed here — the REST binding (A2A spec §11) is one-to-one with
//! the JSON-RPC methods and is easier to call directly, so callers
//! that don't need the JSON-RPC envelope can skip it entirely.
//!
//! All methods set the `A2A-Version: 1.0` header so a version-pinned
//! caller is honoured by the server. Acteon's API-key /
//! `Authorization: Bearer` header is added through the shared
//! `add_auth` helper used by every other client module.
//!
//! Re-exports the core `Task`, `TaskMessage`, `Artifact`, etc., from
//! `acteon_core` so callers don't need a direct dependency on it.

use acteon_core::{Task, TaskMessage, TaskPushNotificationConfig};

use crate::dispatch::ErrorResponse;
use crate::{ActeonClient, Error};

/// `A2A-Version` header carried on every request. Mirrors
/// `acteon_server::api::a2a::A2A_PROTOCOL_VERSION`.
pub const A2A_PROTOCOL_VERSION: &str = "1.0";

const A2A_VERSION_HEADER: &str = "A2A-Version";

/// Re-exported core types so callers can construct messages /
/// configs without depending on `acteon_core` directly.
pub use acteon_core::{
    AgentCard, Artifact, PauseKind, PushAuthentication, TaskArtifactUpdateEvent, TaskPart,
    TaskRole, TaskState, TaskStatus,
};

/// Map a non-success HTTP response to an `Error` by best-effort JSON
/// decode of the error body. Mirrors `crate::bus::map_error`.
async fn map_error(resp: reqwest::Response) -> Error {
    let status = resp.status().as_u16();
    let err = resp.json::<ErrorResponse>().await.ok();
    Error::Http {
        status,
        message: err.map_or_else(|| "a2a API error".to_string(), |e| e.message),
    }
}

impl ActeonClient {
    // -------------------------------------------------------------
    // Task lifecycle
    // -------------------------------------------------------------

    /// `POST /a2a/{namespace}/{tenant}/v1/message:send` — start a new
    /// A2A Task or continue an existing one.
    ///
    /// Set `message.task_id` to the id of an existing task to thread
    /// the message into its history; leave it `None` to mint a fresh
    /// Task.
    pub async fn a2a_send_message(
        &self,
        namespace: &str,
        tenant: &str,
        message: &TaskMessage,
    ) -> Result<Task, Error> {
        let url = format!("{}/a2a/{namespace}/{tenant}/v1/message:send", self.base_url);
        let body = MessageSendBody { message };
        let resp = self
            .add_auth(self.client.post(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<Task>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// `GET /a2a/{namespace}/{tenant}/v1/tasks/{id}` — read a Task
    /// by id. Returns `Err(Error::Server { status: 404, .. })` when
    /// the task does not exist for the caller.
    pub async fn a2a_get_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Task, Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/v1/tasks/{task_id}",
            self.base_url
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<Task>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// `POST /a2a/{namespace}/{tenant}/v1/tasks/{id}:cancel` —
    /// cancel a non-terminal Task.
    ///
    /// The `:cancel` verb suffix is part of the URL (spec §11) — the
    /// server splits it off in-handler. Returns the updated Task on
    /// success; surfaces `TaskNotCancelable` (HTTP 409) when the task
    /// is already terminal.
    pub async fn a2a_cancel_task(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Task, Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/v1/tasks/{task_id}:cancel",
            self.base_url
        );
        let resp = self
            .add_auth(self.client.post(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<Task>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    // -------------------------------------------------------------
    // Push-notification configs
    // -------------------------------------------------------------

    /// `POST /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs`
    /// — register (or upsert, if `id` is set) a push-notification
    /// webhook for a Task. Returns the saved row with timestamps
    /// stamped.
    pub async fn a2a_set_push_config(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        body: &PushConfigInput,
    ) -> Result<TaskPushNotificationConfig, Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/v1/tasks/{task_id}/pushNotificationConfigs",
            self.base_url
        );
        let resp = self
            .add_auth(self.client.post(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .json(body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<TaskPushNotificationConfig>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// `GET /a2a/{ns}/{tenant}/v1/tasks/{id}/pushNotificationConfigs`
    /// — list every config registered for the task. Empty Vec on
    /// existing-but-unconfigured tasks.
    pub async fn a2a_list_push_configs(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
    ) -> Result<Vec<TaskPushNotificationConfig>, Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/v1/tasks/{task_id}/pushNotificationConfigs",
            self.base_url
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<Vec<TaskPushNotificationConfig>>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// `GET …/pushNotificationConfigs/{cfgId}` — read one config.
    pub async fn a2a_get_push_config(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        config_id: &str,
    ) -> Result<TaskPushNotificationConfig, Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/v1/tasks/{task_id}/pushNotificationConfigs/{config_id}",
            self.base_url
        );
        let resp = self
            .add_auth(self.client.get(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<TaskPushNotificationConfig>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// `DELETE …/pushNotificationConfigs/{cfgId}` — remove one
    /// config. `Ok(())` on success; `Err` with HTTP 404 when the
    /// config doesn't exist (the server never silently no-ops).
    pub async fn a2a_delete_push_config(
        &self,
        namespace: &str,
        tenant: &str,
        task_id: &str,
        config_id: &str,
    ) -> Result<(), Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/v1/tasks/{task_id}/pushNotificationConfigs/{config_id}",
            self.base_url
        );
        let resp = self
            .add_auth(self.client.delete(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(map_error(resp).await)
        }
    }

    // -------------------------------------------------------------
    // Discovery
    // -------------------------------------------------------------

    /// `GET /a2a/{namespace}/{tenant}/.well-known/agent.json` —
    /// unauthenticated discovery endpoint. Returns the tenant's
    /// `AgentCard` (single-card verbatim or aggregated across agents).
    ///
    /// Per A2A spec this endpoint is unauthenticated, so the request
    /// is sent **without** Acteon's API-key header. A 404 surfaces
    /// as `Err(Error::Server { status: 404, .. })`.
    pub async fn a2a_discover_agent(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<AgentCard, Error> {
        let url = format!(
            "{}/a2a/{namespace}/{tenant}/.well-known/agent.json",
            self.base_url
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if resp.status().is_success() {
            resp.json::<AgentCard>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))
        } else {
            Err(map_error(resp).await)
        }
    }

    /// JSON-RPC `agent/getAuthenticatedExtendedCard` — authenticated
    /// extended discovery card. Issued through the JSON-RPC envelope
    /// against `POST /a2a/{ns}/{tenant}` rather than a dedicated
    /// REST route (A2A spec defines no REST counterpart for this
    /// method). The returned card has
    /// `capabilities.extendedAgentCard = true`.
    pub async fn a2a_get_authenticated_extended_card(
        &self,
        namespace: &str,
        tenant: &str,
    ) -> Result<AgentCard, Error> {
        let url = format!("{}/a2a/{namespace}/{tenant}", self.base_url);
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "agent/getAuthenticatedExtendedCard",
        });
        let resp = self
            .add_auth(self.client.post(&url))
            .header(A2A_VERSION_HEADER, A2A_PROTOCOL_VERSION)
            .json(&envelope)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(map_error(resp).await);
        }
        let env: JsonRpcReply<AgentCard> = resp
            .json()
            .await
            .map_err(|e| Error::Deserialization(e.to_string()))?;
        match (env.result, env.error) {
            (Some(card), None) => Ok(card),
            (_, Some(err)) => Err(Error::Api {
                code: err.code.to_string(),
                message: err.message,
                retryable: false,
            }),
            _ => Err(Error::Deserialization(
                "JSON-RPC reply had neither result nor error".to_string(),
            )),
        }
    }
}

// ---------------------------------------------------------------------
// Wire shapes
// ---------------------------------------------------------------------

/// Body of `POST /v1/message:send`. The server's request shape is
/// `{ "message": <TaskMessage> }`; mirrored here as a borrowed wrapper
/// so we don't have to clone the message just to serialize it.
#[derive(serde::Serialize)]
struct MessageSendBody<'a> {
    message: &'a TaskMessage,
}

/// Body of `POST .../v1/tasks/{id}/pushNotificationConfigs`. Matches
/// the JSON shape the server's `SetPushConfigInput` deserializes.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushConfigInput {
    /// Optional pre-allocated config id. Omit to mint a fresh one
    /// server-side.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Destination URL (must be `http://` or `https://`).
    pub url: String,
    /// Optional bearer token sent in `Authorization: Bearer …`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// Optional richer authentication metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<PushAuthentication>,
}

/// Minimal JSON-RPC 2.0 response envelope used by
/// `a2a_get_authenticated_extended_card`. `T` is `Option`-wrapped on
/// purpose so `serde` doesn't require `T: Default` for the default
/// `None` branch.
#[derive(Debug, serde::Deserialize)]
struct JsonRpcReply<T> {
    result: Option<T>,
    #[serde(default)]
    error: Option<JsonRpcError>,
}

#[derive(Debug, serde::Deserialize)]
struct JsonRpcError {
    code: i64,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_send_body_serializes_camelcase_message() {
        // The server's wire format for message is camelCase
        // (`messageId`, `taskId`, etc.). Verify the wrapper doesn't
        // double-wrap or lose the field shape.
        let msg = TaskMessage::text("msg-1".to_string(), TaskRole::User, "hi");
        let body = MessageSendBody { message: &msg };
        let json = serde_json::to_value(&body).unwrap();
        assert!(json.get("message").is_some());
        let inner = &json["message"];
        assert_eq!(inner["messageId"], "msg-1");
        // Role enum serializes as snake_case "user" / "agent".
        assert_eq!(inner["role"], "user");
    }

    #[test]
    fn push_config_input_skips_optional_none_fields() {
        let input = PushConfigInput {
            id: None,
            url: "https://x.example.com/hook".to_string(),
            token: None,
            authentication: None,
        };
        let json = serde_json::to_value(&input).unwrap();
        assert_eq!(json["url"], "https://x.example.com/hook");
        assert!(json.get("id").is_none());
        assert!(json.get("token").is_none());
        assert!(json.get("authentication").is_none());
    }
}
