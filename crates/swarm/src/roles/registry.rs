use std::collections::{HashMap, HashSet};

use crate::config::AgentRoleConfig;
use crate::types::agent::AgentRole;

use super::builtins::builtin_roles;

/// Registry of available agent roles (built-in + custom).
#[derive(Debug, Clone)]
pub struct RoleRegistry {
    roles: HashMap<String, AgentRole>,
}

impl RoleRegistry {
    /// Create a registry with only the built-in roles.
    pub fn with_builtins() -> Self {
        let mut roles = HashMap::new();
        for role in builtin_roles() {
            roles.insert(role.name.clone(), role);
        }
        Self { roles }
    }

    /// Create a registry with built-in roles + custom overrides from config.
    pub fn with_config(custom_roles: &[AgentRoleConfig]) -> Self {
        let mut registry = Self::with_builtins();

        for cfg in custom_roles {
            let role = AgentRole {
                name: cfg.name.clone(),
                description: cfg.description.clone(),
                system_prompt_template: cfg.system_prompt_template.clone(),
                allowed_tools: cfg.allowed_tools.clone(),
                can_delegate_to: cfg.can_delegate_to.clone(),
                max_concurrent_instances: cfg.max_concurrent,
            };
            // Custom roles override built-ins with the same name.
            registry.roles.insert(role.name.clone(), role);
        }

        registry
    }

    /// Get a role by name.
    pub fn get(&self, name: &str) -> Option<&AgentRole> {
        self.roles.get(name)
    }

    /// Returns all registered role names.
    pub fn names(&self) -> HashSet<String> {
        self.roles.keys().cloned().collect()
    }

    /// Returns all registered roles.
    pub fn all(&self) -> impl Iterator<Item = &AgentRole> {
        self.roles.values()
    }

    /// Check if a role exists.
    pub fn contains(&self, name: &str) -> bool {
        self.roles.contains_key(name)
    }
}

impl Default for RoleRegistry {
    fn default() -> Self {
        Self::with_builtins()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builtins_registered() {
        let registry = RoleRegistry::with_builtins();
        assert!(registry.contains("coder"));
        assert!(registry.contains("planner"));
        assert!(registry.contains("researcher"));
        assert!(registry.contains("reviewer"));
        assert!(registry.contains("executor"));
        assert_eq!(registry.names().len(), 5);
    }

    #[test]
    fn test_custom_role_override() {
        let custom = vec![AgentRoleConfig {
            name: "coder".into(),
            description: "Custom coder".into(),
            system_prompt_template: "Custom template".into(),
            allowed_tools: vec!["Read".into()],
            can_delegate_to: vec![],
            max_concurrent: 1,
        }];
        let registry = RoleRegistry::with_config(&custom);
        let coder = registry.get("coder").unwrap();
        assert_eq!(coder.description, "Custom coder");
        assert_eq!(coder.allowed_tools, vec!["Read"]);
    }

    #[test]
    fn test_custom_role_addition() {
        let custom = vec![AgentRoleConfig {
            name: "db-admin".into(),
            description: "Database admin".into(),
            system_prompt_template: "You manage databases".into(),
            allowed_tools: vec!["Bash".into(), "Read".into()],
            can_delegate_to: vec![],
            max_concurrent: 1,
        }];
        let registry = RoleRegistry::with_config(&custom);
        assert!(registry.contains("db-admin"));
        assert_eq!(registry.names().len(), 6); // 5 built-in + 1 custom
    }
}
