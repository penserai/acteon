//! Adversarial swarm simulation — demonstrates the adversarial review pattern
//! where a primary swarm builds a project and an adversarial reviewer challenges
//! the output, triggering a recovery phase.
//!
//! This does NOT spawn real LLM agents. Instead it simulates the full flow using
//! Acteon's dispatch and audit infrastructure with recording providers.
//!
//! Run with: cargo run -p acteon-simulation --example adversarial_swarm_simulation

use acteon_core::Action;
use acteon_simulation::prelude::*;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║         ADVERSARIAL SWARM SIMULATION                        ║");
    info!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SETUP: Create providers for primary swarm and adversarial reviewer
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 0: SETUP");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let harness = SimulationHarness::start(
        SimulationConfig::builder()
            .nodes(1)
            .add_recording_provider("primary-swarm")
            .add_recording_provider("adversarial-reviewer")
            .build(),
    )
    .await?;

    info!("Started simulation cluster with 1 node");
    info!("Registered 'primary-swarm' recording provider");
    info!("Registered 'adversarial-reviewer' recording provider\n");

    // =========================================================================
    // PHASE 1: Primary swarm builds the project
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 1: PRIMARY SWARM — BUILD");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let build_actions = vec![
        (
            "write_file",
            serde_json::json!({
                "path": "src/main.rs",
                "description": "Scaffold application entry point with Axum router",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "src/db.rs",
                "description": "Add database connection pool and query helpers",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "src/handlers.rs",
                "description": "Implement REST handlers for user CRUD",
            }),
        ),
        (
            "execute_command",
            serde_json::json!({
                "command": "cargo build",
                "description": "Compile the project to verify it builds",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "src/models.rs",
                "description": "Define User and Session domain models",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "tests/integration.rs",
                "description": "Add integration tests for user endpoints",
            }),
        ),
        (
            "execute_command",
            serde_json::json!({
                "command": "cargo test",
                "description": "Run test suite to verify correctness",
            }),
        ),
    ];

    for (action_type, payload) in &build_actions {
        let action = Action::new(
            "swarm",
            "project-alpha",
            "primary-swarm",
            action_type,
            payload.clone(),
        );

        info!(
            "  [build] Dispatching {}: {}",
            action_type,
            payload
                .get("path")
                .or_else(|| payload.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
        );
        let outcome = harness.dispatch(&action).await?;
        info!("           Outcome: {outcome:?}");
    }

    let build_count = harness.provider("primary-swarm").unwrap().call_count();
    info!("\n  Primary swarm executed {build_count} actions in build phase\n");

    // =========================================================================
    // PHASE 2: Adversarial reviewer challenges the build
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 2: ADVERSARIAL REVIEWER — CHALLENGE");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let challenges = vec![
        (
            "review_finding",
            serde_json::json!({
                "file": "src/handlers.rs",
                "severity": "high",
                "finding": "Missing error handling in create_user — unwrap on db insert will panic on constraint violation",
                "category": "error-handling",
            }),
        ),
        (
            "review_finding",
            serde_json::json!({
                "file": "src/db.rs",
                "severity": "critical",
                "finding": "N+1 query in list_users_with_sessions — fetches sessions in a loop instead of a JOIN",
                "category": "performance",
            }),
        ),
        (
            "review_finding",
            serde_json::json!({
                "file": "src/handlers.rs",
                "severity": "high",
                "finding": "No input validation on user email field — accepts arbitrary strings",
                "category": "input-validation",
            }),
        ),
        (
            "review_finding",
            serde_json::json!({
                "file": "src/main.rs",
                "severity": "medium",
                "finding": "No rate limiting on public endpoints — vulnerable to abuse",
                "category": "security",
            }),
        ),
        (
            "review_finding",
            serde_json::json!({
                "file": "tests/integration.rs",
                "severity": "medium",
                "finding": "Tests do not cover error paths — only happy-path scenarios validated",
                "category": "test-coverage",
            }),
        ),
    ];

    for (action_type, payload) in &challenges {
        let action = Action::new(
            "swarm",
            "project-alpha",
            "adversarial-reviewer",
            action_type,
            payload.clone(),
        );

        info!(
            "  [challenge] {} ({}): {}",
            payload["file"].as_str().unwrap_or("?"),
            payload["severity"].as_str().unwrap_or("?"),
            payload["finding"].as_str().unwrap_or("?"),
        );
        let outcome = harness.dispatch(&action).await?;
        info!("              Outcome: {outcome:?}");
    }

    let review_count = harness
        .provider("adversarial-reviewer")
        .unwrap()
        .call_count();
    info!("\n  Adversarial reviewer raised {review_count} findings\n");

    // =========================================================================
    // PHASE 3: Primary swarm fixes the challenges
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 3: PRIMARY SWARM — RECOVERY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let fixes = vec![
        (
            "write_file",
            serde_json::json!({
                "path": "src/handlers.rs",
                "description": "Add proper error handling with Result types and meaningful error responses",
                "fixes": "error-handling",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "src/db.rs",
                "description": "Replace N+1 loop with a single JOIN query for list_users_with_sessions",
                "fixes": "performance",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "src/handlers.rs",
                "description": "Add email validation using the validator crate on user input",
                "fixes": "input-validation",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "src/main.rs",
                "description": "Add tower-governor rate limiting middleware to public routes",
                "fixes": "security",
            }),
        ),
        (
            "write_file",
            serde_json::json!({
                "path": "tests/integration.rs",
                "description": "Add error-path tests for invalid input, DB failures, and rate limiting",
                "fixes": "test-coverage",
            }),
        ),
        (
            "execute_command",
            serde_json::json!({
                "command": "cargo test",
                "description": "Re-run test suite after fixes to confirm all pass",
            }),
        ),
    ];

    for (action_type, payload) in &fixes {
        let action = Action::new(
            "swarm",
            "project-alpha",
            "primary-swarm",
            action_type,
            payload.clone(),
        );

        info!(
            "  [fix] {}: {}",
            payload
                .get("path")
                .or_else(|| payload.get("command"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown"),
            payload["description"].as_str().unwrap_or("?"),
        );
        let outcome = harness.dispatch(&action).await?;
        info!("         Outcome: {outcome:?}");
    }

    let total_primary = harness.provider("primary-swarm").unwrap().call_count();
    info!(
        "\n  Primary swarm executed {} total actions ({build_count} build + {} fixes)\n",
        total_primary,
        total_primary - build_count
    );

    // =========================================================================
    // PHASE 4: Audit trail — full action timeline
    // =========================================================================
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  PHASE 4: AUDIT TRAIL");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    let primary_calls = harness.provider("primary-swarm").unwrap().calls();
    let reviewer_calls = harness.provider("adversarial-reviewer").unwrap().calls();

    info!(
        "  Primary swarm action log ({} entries):",
        primary_calls.len()
    );
    for (i, call) in primary_calls.iter().enumerate() {
        info!(
            "    [{:>2}] {} | {} | {:?}",
            i + 1,
            call.action.action_type,
            call.timestamp.format("%H:%M:%S%.3f"),
            call.response.as_ref().map(|r| &r.status),
        );
    }

    info!(
        "\n  Adversarial reviewer action log ({} entries):",
        reviewer_calls.len()
    );
    for (i, call) in reviewer_calls.iter().enumerate() {
        info!(
            "    [{:>2}] {} | {} | {:?}",
            i + 1,
            call.action.action_type,
            call.timestamp.format("%H:%M:%S%.3f"),
            call.response.as_ref().map(|r| &r.status),
        );
    }

    // =========================================================================
    // PHASE 5: Summary
    // =========================================================================
    info!("\n━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    info!("  ADVERSARIAL ROUND SUMMARY");
    info!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    info!("  Build phase:     {build_count} actions dispatched");
    info!("  Challenge phase: {review_count} findings raised");
    info!(
        "  Recovery phase:  {} fix actions dispatched",
        total_primary - build_count
    );
    info!("  Total actions:   {}", total_primary + review_count);

    let all_ok = primary_calls
        .iter()
        .chain(reviewer_calls.iter())
        .all(|c| c.response.is_ok());
    if all_ok {
        info!("  Result:          ALL ACTIONS EXECUTED SUCCESSFULLY");
    } else {
        info!("  Result:          SOME ACTIONS FAILED");
    }

    harness.teardown().await?;
    info!("\n  Simulation cluster shut down\n");

    info!("╔══════════════════════════════════════════════════════════════╗");
    info!("║         ADVERSARIAL SWARM SIMULATION COMPLETE               ║");
    info!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
