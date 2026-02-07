use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::ir::rule::RuleAction;

/// The verdict produced by the rule engine after evaluating all rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RuleVerdict {
    /// Allow the action to proceed.
    ///
    /// Contains the name of the matched rule, if any. `None` when no rule
    /// matched and the engine fell through to the default allow.
    Allow(Option<String>),
    /// Deny the action with a reason.
    Deny(String),
    /// Deduplicate with an optional TTL.
    Deduplicate {
        /// Time-to-live in seconds.
        ttl_seconds: Option<u64>,
    },
    /// Suppress the action with a reason.
    Suppress(String),
    /// Reroute to a different provider.
    Reroute {
        /// Name of the rule that triggered the reroute.
        rule: String,
        /// The target provider.
        target_provider: String,
    },
    /// Throttle the action.
    Throttle {
        /// Name of the rule that triggered throttling.
        rule: String,
        /// Maximum count in the window.
        max_count: u64,
        /// Window size in seconds.
        window_seconds: u64,
    },
    /// Modify the action.
    Modify {
        /// Name of the rule that triggered the modification.
        rule: String,
        /// The JSON changes to apply.
        changes: serde_json::Value,
    },
    /// Process through a state machine.
    StateMachine {
        /// Name of the rule that triggered the state machine.
        rule: String,
        /// Name of the state machine to use.
        state_machine: String,
        /// Fields to use for computing the fingerprint.
        fingerprint_fields: Vec<String>,
    },
    /// Group events for batched notification.
    Group {
        /// Name of the rule that triggered grouping.
        rule: String,
        /// Fields to group events by.
        group_by: Vec<String>,
        /// Seconds to wait before sending first notification.
        group_wait_seconds: u64,
        /// Minimum seconds between notifications for same group.
        group_interval_seconds: u64,
        /// Maximum events in a single group.
        max_group_size: usize,
        /// Optional template name for group notification.
        template: Option<String>,
    },
    /// Request human approval before executing the action.
    RequestApproval {
        /// Name of the rule that triggered the approval request.
        rule: String,
        /// Provider to use for sending the approval notification.
        notify_provider: String,
        /// Timeout in seconds before the approval request expires.
        timeout_seconds: u64,
        /// Optional message to include in the approval notification.
        message: Option<String>,
    },
    /// Execute action as the first step of a named task chain.
    Chain {
        /// Name of the rule that triggered the chain.
        rule: String,
        /// Name of the chain configuration to use.
        chain: String,
    },
}

impl RuleVerdict {
    /// Extract the rule name from the verdict, if any.
    ///
    /// Returns `None` for `Allow` and `Deduplicate` (which don't carry a rule name).
    pub fn rule_name(&self) -> Option<&str> {
        match self {
            Self::Allow(name) => name.as_deref(),
            Self::Deduplicate { .. } => None,
            Self::Deny(name) | Self::Suppress(name) => Some(name),
            Self::Reroute { rule, .. }
            | Self::Throttle { rule, .. }
            | Self::Modify { rule, .. }
            | Self::StateMachine { rule, .. }
            | Self::Group { rule, .. }
            | Self::RequestApproval { rule, .. }
            | Self::Chain { rule, .. } => Some(rule),
        }
    }
}

/// Convert a `RuleAction` into a `RuleVerdict` with the rule name attached.
pub(crate) fn action_to_verdict(rule_name: &str, action: &RuleAction) -> RuleVerdict {
    match action {
        RuleAction::Allow => RuleVerdict::Allow(Some(rule_name.to_owned())),
        RuleAction::Deny => RuleVerdict::Deny(rule_name.to_owned()),
        RuleAction::Deduplicate { ttl_seconds } => RuleVerdict::Deduplicate {
            ttl_seconds: *ttl_seconds,
        },
        RuleAction::Suppress => RuleVerdict::Suppress(rule_name.to_owned()),
        RuleAction::Reroute { target_provider } => RuleVerdict::Reroute {
            rule: rule_name.to_owned(),
            target_provider: target_provider.clone(),
        },
        RuleAction::Throttle {
            max_count,
            window_seconds,
        } => RuleVerdict::Throttle {
            rule: rule_name.to_owned(),
            max_count: *max_count,
            window_seconds: *window_seconds,
        },
        RuleAction::Modify { changes } => RuleVerdict::Modify {
            rule: rule_name.to_owned(),
            changes: changes.clone(),
        },
        RuleAction::Custom { name, params: _ } => {
            // Custom actions fall through as Allow for now, with a debug log.
            debug!(custom_action = %name, "custom action not handled, allowing");
            RuleVerdict::Allow(Some(rule_name.to_owned()))
        }
        RuleAction::StateMachine {
            state_machine,
            fingerprint_fields,
        } => RuleVerdict::StateMachine {
            rule: rule_name.to_owned(),
            state_machine: state_machine.clone(),
            fingerprint_fields: fingerprint_fields.clone(),
        },
        RuleAction::Group {
            group_by,
            group_wait_seconds,
            group_interval_seconds,
            max_group_size,
            template,
        } => RuleVerdict::Group {
            rule: rule_name.to_owned(),
            group_by: group_by.clone(),
            group_wait_seconds: *group_wait_seconds,
            group_interval_seconds: *group_interval_seconds,
            max_group_size: *max_group_size,
            template: template.clone(),
        },
        RuleAction::RequestApproval {
            notify_provider,
            timeout_seconds,
            message,
        } => RuleVerdict::RequestApproval {
            rule: rule_name.to_owned(),
            notify_provider: notify_provider.clone(),
            timeout_seconds: *timeout_seconds,
            message: message.clone(),
        },
        RuleAction::Chain { chain } => RuleVerdict::Chain {
            rule: rule_name.to_owned(),
            chain: chain.clone(),
        },
    }
}
