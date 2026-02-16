use std::fmt;

use serde::{Deserialize, Serialize};

/// Roles that control which HTTP endpoints a principal can access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    /// Parse a role from a string.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    /// Check whether this role has a given permission.
    pub fn has_permission(self, perm: Permission) -> bool {
        match perm {
            Permission::Dispatch
            | Permission::RulesManage
            | Permission::CircuitBreakerManage
            | Permission::PluginsManage => matches!(self, Self::Admin | Self::Operator),
            Permission::AuditRead
            | Permission::RulesRead
            | Permission::RulesTest
            | Permission::StreamSubscribe => true,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::Operator => write!(f, "operator"),
            Self::Viewer => write!(f, "viewer"),
        }
    }
}

/// Permissions that map to endpoint groups.
#[derive(Debug, Clone, Copy)]
pub enum Permission {
    Dispatch,
    AuditRead,
    RulesManage,
    RulesRead,
    RulesTest,
    CircuitBreakerManage,
    PluginsManage,
    StreamSubscribe,
}
