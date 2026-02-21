use acteon_core::Action;
use serde::{Deserialize, Serialize};

use crate::{ActeonClient, Error};

/// Information about a loaded rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleInfo {
    /// Rule name.
    pub name: String,
    /// Rule priority (lower = higher priority).
    pub priority: i32,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Optional rule description.
    #[serde(default)]
    pub description: Option<String>,
}

/// Result of reloading rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadResult {
    /// Number of rules loaded.
    pub loaded: usize,
    /// Any errors that occurred during loading.
    pub errors: Vec<String>,
}

/// Options for rule evaluation playground requests.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EvaluateRulesOptions {
    /// When `true`, includes disabled rules in the trace.
    #[serde(default)]
    pub include_disabled: bool,
    /// When `true`, evaluates every rule even after a match.
    #[serde(default)]
    pub evaluate_all: bool,
    /// Optional timestamp override for time-travel debugging.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evaluate_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Optional state key overrides for testing state-dependent conditions.
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub mock_state: std::collections::HashMap<String, String>,
}

/// Details about a semantic match evaluation, used for explainability.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SemanticMatchDetail {
    /// The text that was extracted and compared.
    pub extracted_text: String,
    /// The topic the text was compared against.
    pub topic: String,
    /// The computed similarity score.
    pub similarity: f64,
    /// The threshold that was configured on the rule.
    pub threshold: f64,
}

/// Per-rule trace entry returned by the playground.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleTraceEntry {
    /// Name of the rule that was evaluated.
    pub rule_name: String,
    /// Rule priority (lower = higher priority).
    pub priority: i32,
    /// Whether the rule is enabled.
    pub enabled: bool,
    /// Human-readable display of the rule condition.
    pub condition_display: String,
    /// Evaluation result (e.g. `"matched"`, `"no_match"`, `"error"`).
    pub result: String,
    /// Time spent evaluating this rule in microseconds.
    pub evaluation_duration_us: u64,
    /// The action the rule would take on match.
    pub action: String,
    /// The source of the rule (e.g. `"yaml"`, `"cel"`).
    pub source: String,
    /// Optional description of the rule.
    pub description: Option<String>,
    /// Reason the rule was skipped, if applicable.
    pub skip_reason: Option<String>,
    /// Error message if evaluation failed.
    pub error: Option<String>,
    /// Details about semantic match evaluation, if the rule uses `SemanticMatch`.
    #[serde(default)]
    pub semantic_details: Option<SemanticMatchDetail>,
    /// The JSON merge patch this rule would apply (only for `Modify` rules in
    /// `evaluate_all` mode).
    #[serde(default)]
    pub modify_patch: Option<serde_json::Value>,
    /// Cumulative payload after applying this rule's patch (only for `Modify`
    /// rules in `evaluate_all` mode).
    #[serde(default)]
    pub modified_payload_preview: Option<serde_json::Value>,
}

/// Contextual information captured during rule evaluation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceContext {
    /// The `time.*` map that was used during evaluation.
    #[serde(default)]
    pub time: serde_json::Value,
    /// Environment keys that were actually accessed during evaluation
    /// (values omitted for security).
    #[serde(default)]
    pub environment_keys: Vec<String>,
    /// State keys that were actually accessed during evaluation.
    #[serde(default)]
    pub accessed_state_keys: Vec<String>,
    /// The effective timezone used for time-based conditions, if any.
    #[serde(default)]
    pub effective_timezone: Option<String>,
}

/// Response from the rule evaluation playground.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEvaluationTrace {
    /// Final verdict (e.g. `"allow"`, `"deny"`, `"no_match"`).
    pub verdict: String,
    /// Name of the matched rule, if any.
    pub matched_rule: Option<String>,
    /// Whether any rule produced an error during evaluation.
    #[serde(default)]
    pub has_errors: bool,
    /// Total number of rules that were evaluated.
    pub total_rules_evaluated: usize,
    /// Total number of rules that were skipped.
    pub total_rules_skipped: usize,
    /// Total evaluation time in microseconds.
    pub evaluation_duration_us: u64,
    /// Per-rule trace entries.
    pub trace: Vec<RuleTraceEntry>,
    /// The evaluation context that was used.
    pub context: TraceContext,
    /// The payload after any rule modifications, if changed.
    pub modified_payload: Option<serde_json::Value>,
}

impl ActeonClient {
    /// List all loaded rules.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let rules = client.list_rules().await?;
    /// for rule in rules {
    ///     println!("{}: priority={}, enabled={}", rule.name, rule.priority, rule.enabled);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn list_rules(&self) -> Result<Vec<RuleInfo>, Error> {
        let url = format!("{}/v1/rules", self.base_url);

        let response = self
            .add_auth(self.client.get(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let rules = response
                .json::<Vec<RuleInfo>>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(rules)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to list rules: {}", response.status()),
            })
        }
    }

    /// Reload rules from the configured directory.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let result = client.reload_rules().await?;
    /// println!("Loaded {} rules", result.loaded);
    /// if !result.errors.is_empty() {
    ///     println!("Errors: {:?}", result.errors);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn reload_rules(&self) -> Result<ReloadResult, Error> {
        let url = format!("{}/v1/rules/reload", self.base_url);

        let response = self
            .add_auth(self.client.post(&url))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let result = response
                .json::<ReloadResult>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(result)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to reload rules: {}", response.status()),
            })
        }
    }

    /// Enable or disable a specific rule.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::ActeonClient;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// client.set_rule_enabled("block-spam", false).await?;
    /// println!("Rule disabled");
    /// # Ok(())
    /// # }
    /// ```
    pub async fn set_rule_enabled(&self, rule_name: &str, enabled: bool) -> Result<(), Error> {
        let url = format!("{}/v1/rules/{}/enabled", self.base_url, rule_name);

        let response = self
            .add_auth(self.client.put(&url))
            .json(&serde_json::json!({ "enabled": enabled }))
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to set rule enabled: {}", response.status()),
            })
        }
    }

    /// Evaluate rules against a test action without dispatching.
    ///
    /// Returns a detailed trace showing how each rule would evaluate against
    /// the given action. This is useful for debugging and testing rule
    /// configurations in a playground environment.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # async fn example() -> Result<(), acteon_client::Error> {
    /// use acteon_client::{ActeonClient, EvaluateRulesOptions};
    /// use acteon_core::Action;
    ///
    /// let client = ActeonClient::new("http://localhost:8080");
    /// let action = Action::new(
    ///     "notifications",
    ///     "tenant-1",
    ///     "email",
    ///     "send_notification",
    ///     serde_json::json!({"to": "user@example.com"}),
    /// );
    ///
    /// let options = EvaluateRulesOptions {
    ///     evaluate_all: true,
    ///     ..Default::default()
    /// };
    ///
    /// let trace = client.evaluate_rules(&action, &options).await?;
    /// println!("Verdict: {}", trace.verdict);
    /// for entry in &trace.trace {
    ///     println!("  {} -> {}", entry.rule_name, entry.result);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn evaluate_rules(
        &self,
        action: &Action,
        options: &EvaluateRulesOptions,
    ) -> Result<RuleEvaluationTrace, Error> {
        #[derive(Serialize)]
        struct Body<'a> {
            namespace: &'a str,
            tenant: &'a str,
            provider: &'a str,
            action_type: &'a str,
            payload: &'a serde_json::Value,
            metadata: &'a std::collections::HashMap<String, String>,
            #[serde(flatten)]
            options: &'a EvaluateRulesOptions,
        }

        let url = format!("{}/v1/rules/evaluate", self.base_url);

        let body = Body {
            namespace: action.namespace.as_str(),
            tenant: action.tenant.as_str(),
            provider: action.provider.as_str(),
            action_type: &action.action_type,
            payload: &action.payload,
            metadata: &action.metadata.labels,
            options,
        };

        let response = self
            .add_auth(self.client.post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Connection(e.to_string()))?;

        if response.status().is_success() {
            let trace = response
                .json::<RuleEvaluationTrace>()
                .await
                .map_err(|e| Error::Deserialization(e.to_string()))?;
            Ok(trace)
        } else {
            Err(Error::Http {
                status: response.status().as_u16(),
                message: format!("Failed to evaluate rules: {}", response.status()),
            })
        }
    }
}
