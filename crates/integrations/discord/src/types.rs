use serde::{Deserialize, Serialize};

/// Request body for a Discord webhook execution.
#[derive(Debug, Clone, Serialize)]
pub struct DiscordWebhookRequest {
    /// Message text content. At least one of `content` or `embeds` is required.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Override the webhook's default username.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,

    /// Override the webhook's default avatar URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,

    /// Whether the message should be read aloud via TTS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tts: Option<bool>,

    /// Rich embed objects. Up to 10 embeds per message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embeds: Option<Vec<DiscordEmbed>>,
}

/// A Discord embed object for rich message formatting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordEmbed {
    /// Embed title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Embed description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Embed color as a decimal integer (e.g., `16711680` for red).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<u32>,

    /// Embed fields.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fields: Option<Vec<DiscordEmbedField>>,

    /// Footer text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer: Option<DiscordEmbedFooter>,

    /// ISO 8601 timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

/// A field within a Discord embed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordEmbedField {
    /// Field name.
    pub name: String,
    /// Field value.
    pub value: String,
    /// Whether this field should be displayed inline.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inline: Option<bool>,
}

/// Footer for a Discord embed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscordEmbedFooter {
    /// Footer text.
    pub text: String,
}

/// Response from a Discord webhook execution (only returned when `?wait=true`).
#[derive(Debug, Clone, Deserialize)]
pub struct DiscordWebhookResponse {
    /// Message ID.
    pub id: Option<String>,
    /// Channel ID.
    pub channel_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webhook_request_serializes_content_only() {
        let req = DiscordWebhookRequest {
            content: Some("Hello!".into()),
            username: None,
            avatar_url: None,
            tts: None,
            embeds: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["content"], "Hello!");
        assert!(json.get("username").is_none());
        assert!(json.get("embeds").is_none());
    }

    #[test]
    fn webhook_request_serializes_with_embeds() {
        let embed = DiscordEmbed {
            title: Some("Alert".into()),
            description: Some("Something happened".into()),
            color: Some(16_711_680),
            fields: Some(vec![DiscordEmbedField {
                name: "Status".into(),
                value: "Critical".into(),
                inline: Some(true),
            }]),
            footer: Some(DiscordEmbedFooter {
                text: "Acteon".into(),
            }),
            timestamp: None,
        };

        let req = DiscordWebhookRequest {
            content: None,
            username: Some("Acteon Bot".into()),
            avatar_url: None,
            tts: None,
            embeds: Some(vec![embed]),
        };

        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("content").is_none());
        assert_eq!(json["username"], "Acteon Bot");
        assert_eq!(json["embeds"][0]["title"], "Alert");
        assert_eq!(json["embeds"][0]["color"], 16_711_680);
        assert_eq!(json["embeds"][0]["fields"][0]["name"], "Status");
    }

    #[test]
    fn webhook_response_deserializes() {
        let json = r#"{"id":"12345","channel_id":"67890"}"#;
        let resp: DiscordWebhookResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id.as_deref(), Some("12345"));
        assert_eq!(resp.channel_id.as_deref(), Some("67890"));
    }

    #[test]
    fn embed_deserializes() {
        let json = r#"{"title":"Test","description":"Desc","color":255}"#;
        let embed: DiscordEmbed = serde_json::from_str(json).unwrap();
        assert_eq!(embed.title.as_deref(), Some("Test"));
        assert_eq!(embed.color, Some(255));
    }
}
