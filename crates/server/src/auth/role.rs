use std::fmt;

use serde::{Deserialize, Serialize};

/// Roles that control which HTTP endpoints a principal can access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Admin,
    Operator,
    /// Execute granted actions without administering execution policy.
    Executor,
    Viewer,
}

impl Role {
    /// Parse a role from a string.
    pub fn from_str_loose(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "admin" => Some(Self::Admin),
            "operator" => Some(Self::Operator),
            "executor" => Some(Self::Executor),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    /// Check whether this role has a given permission.
    pub fn has_permission(self, perm: Permission) -> bool {
        match perm {
            Permission::Dispatch => matches!(self, Self::Admin | Self::Operator | Self::Executor),
            Permission::OperationsManage
            | Permission::RulesManage
            | Permission::CircuitBreakerManage
            | Permission::PluginsManage
            | Permission::SilencesManage
            | Permission::TimeIntervalsManage
            | Permission::TemplatesManage => matches!(self, Self::Admin | Self::Operator),
            Permission::AuditRead
            | Permission::RulesRead
            | Permission::RulesTest
            | Permission::StreamSubscribe
            | Permission::Session => true,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Admin => write!(f, "admin"),
            Self::Operator => write!(f, "operator"),
            Self::Executor => write!(f, "executor"),
            Self::Viewer => write!(f, "viewer"),
        }
    }
}

/// Permissions that map to endpoint groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum Permission {
    Dispatch,
    /// Administer registries, policy, schedules, and recovery. Never implied by dispatch.
    OperationsManage,
    /// Manage the authenticated caller's own login session.
    Session,
    AuditRead,
    RulesManage,
    RulesRead,
    RulesTest,
    CircuitBreakerManage,
    PluginsManage,
    StreamSubscribe,
    /// Create, update, or expire silences. Held by admin and operator.
    SilencesManage,
    /// Create, update, or delete time intervals. Held by admin and operator.
    TimeIntervalsManage,
    /// Create, update, delete, or reload payload templates and profiles.
    /// Held by admin and operator. Reads (get/list/render preview) are open
    /// to all roles but remain tenant-scoped.
    TemplatesManage,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executor_serializes_and_executes_without_control_plane_permissions() {
        assert_eq!(Role::from_str_loose("EXECUTOR"), Some(Role::Executor));
        assert_eq!(
            serde_json::to_string(&Role::Executor).unwrap(),
            "\"executor\""
        );
        assert_eq!(Role::Executor.to_string(), "executor");
        assert!(Role::Executor.has_permission(Permission::Dispatch));
        for permission in [
            Permission::OperationsManage,
            Permission::RulesManage,
            Permission::CircuitBreakerManage,
            Permission::PluginsManage,
            Permission::SilencesManage,
            Permission::TimeIntervalsManage,
            Permission::TemplatesManage,
        ] {
            assert!(!Role::Executor.has_permission(permission));
            assert!(!Role::Viewer.has_permission(permission));
            assert!(Role::Operator.has_permission(permission));
            assert!(Role::Admin.has_permission(permission));
        }
    }
}
