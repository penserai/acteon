use serde::{Deserialize, Serialize};

/// Minimal caller identity for audit threading.
///
/// This type is shared across crates so that the gateway can record
/// who triggered each action without depending on the full auth module.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Caller {
    /// Caller identifier (username or API key name).
    pub id: String,
    /// How the caller authenticated (`"jwt"`, `"api_key"`, or `"anonymous"`).
    pub auth_method: String,
}
