//! Demonstration of loading rules from YAML files.
//!
//! This example shows how to:
//! - Load rules from fixture files
//! - Combine multiple rule files
//! - Reload rules dynamically
//!
//! Run with:
//!   cargo run -p acteon-simulation --example rules_from_files

use std::path::Path;
use std::sync::Arc;

use acteon_core::Action;
use acteon_gateway::GatewayBuilder;
use acteon_provider::DynProvider;
use acteon_rules::RuleFrontend;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::RecordingProvider;
use acteon_state::lock::DistributedLock;
use acteon_state::store::StateStore;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use tracing::info;

/// Load rules from a YAML file.
fn load_rules_from_file(
    path: &Path,
) -> Result<Vec<acteon_rules::Rule>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let frontend = YamlFrontend;
    Ok(frontend.parse(&content)?)
}

/// Load all YAML rules from a directory.
fn load_rules_from_directory(
    dir: &Path,
) -> Result<Vec<acteon_rules::Rule>, Box<dyn std::error::Error>> {
    let mut all_rules = Vec::new();

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path
            .extension()
            .map(|e| e == "yaml" || e == "yml")
            .unwrap_or(false)
        {
            info!("  Loading: {}", path.display());
            let rules = load_rules_from_file(&path)?;
            info!("    → {} rules loaded", rules.len());
            all_rules.extend(rules);
        }
    }

    Ok(all_rules)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║        LOADING RULES FROM CONFIGURATION FILES                ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // Get the fixtures directory path
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixtures_dir = Path::new(manifest_dir).join("fixtures/rules");

    info!("→ Loading rules from: {}\n", fixtures_dir.display());

    // =========================================================================
    // APPROACH 1: Load a single rule file
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  APPROACH 1: Load a single rule file");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let suppression_path = fixtures_dir.join("suppression.yaml");
    let suppression_rules = load_rules_from_file(&suppression_path)?;

    info!("  Loaded {} suppression rules:", suppression_rules.len());
    for rule in &suppression_rules {
        info!("    - {} (priority: {})", rule.name, rule.priority);
    }

    // =========================================================================
    // APPROACH 2: Load all rules from a directory
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  APPROACH 2: Load all rules from directory");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let all_rules = load_rules_from_directory(&fixtures_dir)?;
    info!("\n  Total rules loaded: {}", all_rules.len());

    // Group by action type
    let mut by_action: std::collections::HashMap<String, Vec<&acteon_rules::Rule>> =
        std::collections::HashMap::new();
    for rule in &all_rules {
        let action_type = format!("{:?}", rule.action);
        by_action.entry(action_type).or_default().push(rule);
    }

    info!("\n  Rules by action type:");
    for (action_type, rules) in &by_action {
        info!("    {}: {} rules", action_type, rules.len());
    }

    // =========================================================================
    // APPROACH 3: Selective loading - pick specific files
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  APPROACH 3: Selective loading - combine specific files");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let rule_files = vec!["suppression.yaml", "rerouting.yaml"];
    let mut selected_rules = Vec::new();

    for filename in &rule_files {
        let path = fixtures_dir.join(filename);
        let rules = load_rules_from_file(&path)?;
        info!("  {}: {} rules", filename, rules.len());
        selected_rules.extend(rules);
    }

    info!("  Combined: {} rules\n", selected_rules.len());

    // =========================================================================
    // DEMO: Use loaded rules in a gateway
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  DEMO: Test rules loaded from files");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    // Create providers
    let email = Arc::new(RecordingProvider::new("email"));
    let sms = Arc::new(RecordingProvider::new("sms"));
    let slack = Arc::new(RecordingProvider::new("slack"));

    // Build gateway with selected rules
    let gateway = GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()) as Arc<dyn StateStore>)
        .lock(Arc::new(MemoryDistributedLock::new()) as Arc<dyn DistributedLock>)
        .rules(selected_rules.clone())
        .provider(email.clone() as Arc<dyn DynProvider>)
        .provider(sms.clone() as Arc<dyn DynProvider>)
        .provider(slack.clone() as Arc<dyn DynProvider>)
        .build()?;

    // Test suppression rule (from suppression.yaml)
    info!("  Testing suppression rule (block-spam):");
    let spam = Action::new("ns", "t1", "email", "spam", serde_json::json!({}));
    let outcome = gateway.dispatch(spam, None).await?;
    info!("    Action type 'spam' → {:?}", outcome);
    info!(
        "    Email provider calls: {} (should be 0)\n",
        email.call_count()
    );

    // Test rerouting rule (from rerouting.yaml)
    info!("  Testing rerouting rule (reroute-urgent-to-sms):");
    let urgent = Action::new(
        "ns",
        "t1",
        "email",
        "send_notification",
        serde_json::json!({
            "priority": "urgent",
            "message": "Server down!"
        }),
    );
    let outcome = gateway.dispatch(urgent, None).await?;
    info!("    Urgent notification → {:?}", outcome);
    info!(
        "    Email calls: {}, SMS calls: {}",
        email.call_count(),
        sms.call_count()
    );

    gateway.shutdown().await;

    // =========================================================================
    // APPROACH 4: Environment-based config selection
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  APPROACH 4: Environment-based configuration");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let env = std::env::var("ACTEON_ENV").unwrap_or_else(|_| "development".to_string());
    info!("  ACTEON_ENV = {}", env);

    // In production, you might have:
    //   rules/production/strict-rules.yaml
    //   rules/development/permissive-rules.yaml
    //   rules/staging/test-rules.yaml

    let rule_files_for_env = match env.as_str() {
        "production" => vec!["suppression.yaml", "throttling.yaml"],
        "staging" => vec!["suppression.yaml", "rerouting.yaml", "throttling.yaml"],
        _ => vec!["suppression.yaml"], // minimal for development
    };

    info!("  Rule files for '{}': {:?}", env, rule_files_for_env);

    // =========================================================================
    // Note on dynamic rules
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  NOTE: Dynamic rule loading");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Rules are configured at the gateway level, not in action payloads.");
    info!("  The action payload contains data that rules EVALUATE against.\n");

    info!("  For dynamic rule management, use the HTTP API:");
    info!("    GET  /v1/rules              - List all loaded rules");
    info!("    POST /v1/rules/reload       - Reload rules from directory");
    info!("    PUT  /v1/rules/{{name}}/enabled - Enable/disable a specific rule\n");

    info!("  Example: Reload rules via curl:");
    info!("    curl -X POST http://localhost:8080/v1/rules/reload\n");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║               RULES FROM FILES DEMO COMPLETE                 ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
