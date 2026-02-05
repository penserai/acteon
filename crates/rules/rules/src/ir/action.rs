//! Convenience re-exports and helpers for [`RuleAction`].

pub use super::rule::RuleAction;

impl RuleAction {
    /// Returns `true` if this action allows the operation to proceed.
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow)
    }

    /// Returns `true` if this action denies the operation.
    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny)
    }

    /// Returns `true` if this action suppresses the operation.
    pub fn is_suppress(&self) -> bool {
        matches!(self, Self::Suppress)
    }

    /// Returns `true` if this action reroutes to another provider.
    pub fn is_reroute(&self) -> bool {
        matches!(self, Self::Reroute { .. })
    }

    /// Returns `true` if this action throttles the operation.
    pub fn is_throttle(&self) -> bool {
        matches!(self, Self::Throttle { .. })
    }

    /// Returns `true` if this action modifies the operation.
    pub fn is_modify(&self) -> bool {
        matches!(self, Self::Modify { .. })
    }

    /// Returns `true` if this action deduplicates.
    pub fn is_deduplicate(&self) -> bool {
        matches!(self, Self::Deduplicate { .. })
    }

    /// Returns `true` if this action processes through a state machine.
    pub fn is_state_machine(&self) -> bool {
        matches!(self, Self::StateMachine { .. })
    }

    /// Returns `true` if this action groups events.
    pub fn is_group(&self) -> bool {
        matches!(self, Self::Group { .. })
    }

    /// Returns `true` if this action requests human approval.
    pub fn is_request_approval(&self) -> bool {
        matches!(self, Self::RequestApproval { .. })
    }

    /// Returns a human-readable label for the action kind.
    pub fn kind_label(&self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::Deduplicate { .. } => "deduplicate",
            Self::Suppress => "suppress",
            Self::Reroute { .. } => "reroute",
            Self::Throttle { .. } => "throttle",
            Self::Modify { .. } => "modify",
            Self::Custom { .. } => "custom",
            Self::StateMachine { .. } => "state_machine",
            Self::Group { .. } => "group",
            Self::RequestApproval { .. } => "request_approval",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_predicates() {
        assert!(RuleAction::Allow.is_allow());
        assert!(!RuleAction::Allow.is_deny());

        assert!(RuleAction::Deny.is_deny());
        assert!(RuleAction::Suppress.is_suppress());

        let reroute = RuleAction::Reroute {
            target_provider: "sms".into(),
        };
        assert!(reroute.is_reroute());

        let throttle = RuleAction::Throttle {
            max_count: 10,
            window_seconds: 60,
        };
        assert!(throttle.is_throttle());

        let modify = RuleAction::Modify {
            changes: serde_json::json!({}),
        };
        assert!(modify.is_modify());

        let dedup = RuleAction::Deduplicate {
            ttl_seconds: Some(300),
        };
        assert!(dedup.is_deduplicate());
    }

    #[test]
    fn kind_labels() {
        assert_eq!(RuleAction::Allow.kind_label(), "allow");
        assert_eq!(RuleAction::Deny.kind_label(), "deny");
        assert_eq!(RuleAction::Suppress.kind_label(), "suppress");
        assert_eq!(
            RuleAction::Reroute {
                target_provider: "x".into()
            }
            .kind_label(),
            "reroute"
        );
        assert_eq!(
            RuleAction::Throttle {
                max_count: 1,
                window_seconds: 1
            }
            .kind_label(),
            "throttle"
        );
        assert_eq!(
            RuleAction::Modify {
                changes: serde_json::json!(null)
            }
            .kind_label(),
            "modify"
        );
        assert_eq!(
            RuleAction::Deduplicate { ttl_seconds: None }.kind_label(),
            "deduplicate"
        );
        assert_eq!(
            RuleAction::Custom {
                name: "x".into(),
                params: serde_json::json!(null)
            }
            .kind_label(),
            "custom"
        );
        assert_eq!(
            RuleAction::StateMachine {
                state_machine: "alert".into(),
                fingerprint_fields: vec![]
            }
            .kind_label(),
            "state_machine"
        );
        assert_eq!(
            RuleAction::Group {
                group_by: vec![],
                group_wait_seconds: 30,
                group_interval_seconds: 60,
                max_group_size: 100,
                template: None
            }
            .kind_label(),
            "group"
        );
    }

    #[test]
    fn state_machine_predicates() {
        let sm = RuleAction::StateMachine {
            state_machine: "alert".into(),
            fingerprint_fields: vec!["action_type".into()],
        };
        assert!(sm.is_state_machine());
        assert!(!sm.is_group());
    }

    #[test]
    fn group_predicates() {
        let group = RuleAction::Group {
            group_by: vec!["cluster".into()],
            group_wait_seconds: 30,
            group_interval_seconds: 60,
            max_group_size: 100,
            template: Some("alert_template".into()),
        };
        assert!(group.is_group());
        assert!(!group.is_state_machine());
    }
}
