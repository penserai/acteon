use serde::Serialize;

/// A simple `MessageCard` for Teams incoming webhooks.
///
/// This follows the [Office 365 MessageCard](https://learn.microsoft.com/en-us/outlook/actionable-messages/message-card-reference)
/// format which is the simplest way to send formatted messages.
#[derive(Debug, Clone, Serialize)]
pub struct TeamsMessageCard {
    /// Card type — always `"MessageCard"`.
    #[serde(rename = "@type")]
    pub card_type: String,

    /// Card context — always the Office 365 connector schema.
    #[serde(rename = "@context")]
    pub context: String,

    /// Card title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,

    /// Card body text (supports basic markdown).
    pub text: String,

    /// Summary text (displayed in notifications).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// Theme color as a hex string (e.g., `"FF0000"` for red).
    #[serde(rename = "themeColor", skip_serializing_if = "Option::is_none")]
    pub theme_color: Option<String>,
}

impl TeamsMessageCard {
    /// Create a new message card with the given body text.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            card_type: "MessageCard".to_owned(),
            context: "https://schema.org/extensions".to_owned(),
            title: None,
            text: text.into(),
            summary: None,
            theme_color: None,
        }
    }

    /// Set the card title.
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Set the summary text.
    #[must_use]
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Set the theme color.
    #[must_use]
    pub fn with_theme_color(mut self, color: impl Into<String>) -> Self {
        self.theme_color = Some(color.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_card_serializes_correctly() {
        let card = TeamsMessageCard::new("Hello from Acteon!")
            .with_title("Alert")
            .with_theme_color("FF0000");

        let json = serde_json::to_value(&card).unwrap();
        assert_eq!(json["@type"], "MessageCard");
        assert_eq!(json["@context"], "https://schema.org/extensions");
        assert_eq!(json["title"], "Alert");
        assert_eq!(json["text"], "Hello from Acteon!");
        assert_eq!(json["themeColor"], "FF0000");
        assert!(json.get("summary").is_none());
    }

    #[test]
    fn message_card_minimal() {
        let card = TeamsMessageCard::new("Just text");
        let json = serde_json::to_value(&card).unwrap();
        assert_eq!(json["text"], "Just text");
        assert!(json.get("title").is_none());
        assert!(json.get("themeColor").is_none());
    }
}
