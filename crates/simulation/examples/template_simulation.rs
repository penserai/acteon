//! Simulation of payload templates and template profiles in Acteon.
//!
//! Demonstrates 7 scenarios covering inline template fields, `$ref` template
//! references, mixed profiles, loops and conditionals, nested variable access,
//! error handling for missing profiles/templates, and multi-tenant template
//! isolation.
//!
//! Run with: `cargo run -p acteon-simulation --example template_simulation`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use acteon_core::template::{Template, TemplateProfile, TemplateProfileField};
use acteon_core::{Action, ActionOutcome};
use acteon_gateway::GatewayBuilder;
use acteon_simulation::provider::RecordingProvider;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use chrono::Utc;

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Create a `Template` with the given scope and content.
fn make_template(name: &str, namespace: &str, tenant: &str, content: &str) -> Template {
    Template {
        id: format!("tpl-{name}"),
        name: name.to_string(),
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
        content: content.to_string(),
        description: Some(format!("Template: {name}")),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        labels: HashMap::new(),
    }
}

/// Create a `TemplateProfile` with the given scope and fields.
fn make_profile(
    name: &str,
    namespace: &str,
    tenant: &str,
    fields: HashMap<String, TemplateProfileField>,
) -> TemplateProfile {
    TemplateProfile {
        id: format!("prof-{name}"),
        name: name.to_string(),
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
        fields,
        description: Some(format!("Profile: {name}")),
        created_at: Utc::now(),
        updated_at: Utc::now(),
        labels: HashMap::new(),
    }
}

/// Build a gateway with templates, profiles, and recording providers.
fn build_gateway(
    templates: Vec<Template>,
    profiles: Vec<TemplateProfile>,
    providers: Vec<Arc<RecordingProvider>>,
) -> acteon_gateway::Gateway {
    let state = Arc::new(MemoryStateStore::new());
    let lock = Arc::new(MemoryDistributedLock::new());

    let mut builder = GatewayBuilder::new().state(state).lock(lock);

    for template in templates {
        builder = builder.template(template);
    }

    for profile in profiles {
        builder = builder.template_profile(profile);
    }

    for p in providers {
        builder = builder.provider(p as Arc<dyn acteon_provider::DynProvider>);
    }

    builder.build().expect("gateway should build")
}

// ---------------------------------------------------------------------------
// Scenarios
// ---------------------------------------------------------------------------

/// Scenario 1: Inline template fields -- render simple variables.
async fn scenario_inline_fields() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 1: INLINE TEMPLATE FIELDS");
    println!("------------------------------------------------------------------\n");

    let email = Arc::new(RecordingProvider::new("email"));

    // Profile with two inline fields: subject and greeting
    let mut fields = HashMap::new();
    fields.insert(
        "subject".to_string(),
        TemplateProfileField::Inline("Welcome, {{ user_name }}!".to_string()),
    );
    fields.insert(
        "greeting".to_string(),
        TemplateProfileField::Inline(
            "Hello {{ user_name }}, your account {{ account_id }} is ready.".to_string(),
        ),
    );
    let profile = make_profile("welcome-inline", "notifications", "tenant-1", fields);

    let gateway = build_gateway(vec![], vec![profile], vec![Arc::clone(&email)]);

    // Dispatch an action with the template profile set
    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "user_name": "Alice",
            "account_id": "ACC-42",
            "to": "alice@example.com",
        }),
    )
    .with_template("welcome-inline");

    let outcome = gateway.dispatch(action, None).await?;
    println!("  Outcome: {outcome:?}");
    assert!(
        matches!(outcome, ActionOutcome::Executed(..)),
        "action should be executed"
    );

    // Verify the provider received the rendered payload
    let calls = email.calls();
    assert_eq!(calls.len(), 1);
    let payload = &calls[0].action.payload;
    println!("  Provider received payload: {payload}");

    assert_eq!(
        payload.get("subject").and_then(|v| v.as_str()),
        Some("Welcome, Alice!"),
        "subject should be rendered with user_name"
    );
    assert_eq!(
        payload.get("greeting").and_then(|v| v.as_str()),
        Some("Hello Alice, your account ACC-42 is ready."),
        "greeting should be rendered with user_name and account_id"
    );
    // Original fields should still be present
    assert_eq!(
        payload.get("to").and_then(|v| v.as_str()),
        Some("alice@example.com"),
        "original fields should be preserved"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 1 PASSED]\n");
    Ok(())
}

/// Scenario 2: `$ref` template fields -- render from stored templates.
async fn scenario_ref_fields() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 2: $ref TEMPLATE FIELDS (stored templates)");
    println!("------------------------------------------------------------------\n");

    let email = Arc::new(RecordingProvider::new("email"));

    // Create a stored template
    let body_template = make_template(
        "order-confirmation-body",
        "notifications",
        "tenant-1",
        "Dear {{ customer_name }}, your order #{{ order_id }} for {{ item }} has been confirmed. Total: ${{ total }}.",
    );

    // Profile that references the stored template via $ref
    let mut fields = HashMap::new();
    fields.insert(
        "body".to_string(),
        TemplateProfileField::Ref {
            template_ref: "order-confirmation-body".to_string(),
        },
    );
    let profile = make_profile("order-confirmation", "notifications", "tenant-1", fields);

    let gateway = build_gateway(vec![body_template], vec![profile], vec![Arc::clone(&email)]);

    let action = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({
            "customer_name": "Bob",
            "order_id": "ORD-1234",
            "item": "Rust Programming Book",
            "total": "49.99",
        }),
    )
    .with_template("order-confirmation");

    let outcome = gateway.dispatch(action, None).await?;
    println!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    let calls = email.calls();
    let payload = &calls[0].action.payload;
    println!("  Provider received payload: {payload}");

    let body = payload
        .get("body")
        .and_then(|v| v.as_str())
        .expect("body field should exist");
    assert!(body.contains("Bob"), "body should contain customer name");
    assert!(body.contains("ORD-1234"), "body should contain order id");
    assert!(
        body.contains("Rust Programming Book"),
        "body should contain item name"
    );
    assert!(
        body.contains("$49.99"),
        "body should contain formatted total"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 2 PASSED]\n");
    Ok(())
}

/// Scenario 3: Mixed inline and `$ref` fields in a single profile.
async fn scenario_mixed_inline_and_ref() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 3: MIXED INLINE + $ref FIELDS IN ONE PROFILE");
    println!("------------------------------------------------------------------\n");

    let slack = Arc::new(RecordingProvider::new("slack"));

    // Stored template for the detailed body
    let html_template = make_template(
        "incident-html",
        "monitoring",
        "acme-corp",
        "<h1>Incident: {{ title }}</h1><p>Severity: {{ severity }}</p><p>{{ details }}</p>",
    );

    // Profile mixes inline subject with $ref body
    let mut fields = HashMap::new();
    fields.insert(
        "subject".to_string(),
        TemplateProfileField::Inline("[{{ severity | upper }}] {{ title }}".to_string()),
    );
    fields.insert(
        "html_body".to_string(),
        TemplateProfileField::Ref {
            template_ref: "incident-html".to_string(),
        },
    );
    fields.insert(
        "footer".to_string(),
        TemplateProfileField::Inline("Reported by {{ reporter }} at {{ timestamp }}".to_string()),
    );
    let profile = make_profile("incident-alert", "monitoring", "acme-corp", fields);

    let gateway = build_gateway(vec![html_template], vec![profile], vec![Arc::clone(&slack)]);

    let action = Action::new(
        "monitoring",
        "acme-corp",
        "slack",
        "alert",
        serde_json::json!({
            "severity": "critical",
            "title": "Database Unreachable",
            "details": "Primary PostgreSQL node is not responding to health checks.",
            "reporter": "health-monitor",
            "timestamp": "2026-02-19T10:30:00Z",
        }),
    )
    .with_template("incident-alert");

    let outcome = gateway.dispatch(action, None).await?;
    println!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    let calls = slack.calls();
    let payload = &calls[0].action.payload;
    println!("  subject: {}", payload["subject"]);
    println!("  html_body: {}", payload["html_body"]);
    println!("  footer: {}", payload["footer"]);

    let subject = payload["subject"].as_str().unwrap();
    assert!(
        subject.contains("CRITICAL"),
        "subject should contain uppercased severity"
    );
    assert!(
        subject.contains("Database Unreachable"),
        "subject should contain title"
    );

    let html_body = payload["html_body"].as_str().unwrap();
    assert!(
        html_body.contains("<h1>Incident: Database Unreachable</h1>"),
        "html_body should contain rendered HTML"
    );
    assert!(
        html_body.contains("PostgreSQL"),
        "html_body should contain details"
    );

    let footer = payload["footer"].as_str().unwrap();
    assert!(
        footer.contains("health-monitor"),
        "footer should contain reporter"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 3 PASSED]\n");
    Ok(())
}

/// Scenario 4: Templates with loops and conditionals.
async fn scenario_loops_and_conditionals() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 4: TEMPLATES WITH LOOPS AND CONDITIONALS");
    println!("------------------------------------------------------------------\n");

    let webhook = Arc::new(RecordingProvider::new("webhook"));

    // Stored template with a for-loop rendering a list of items
    let list_template = make_template(
        "item-list",
        "orders",
        "tenant-1",
        "Order Items:\n{% for item in items %}- {{ item.name }} (x{{ item.qty }}){% if item.sale %} [SALE]{% endif %}\n{% endfor %}Total items: {{ items | length }}",
    );

    // Stored template with conditionals
    let status_template = make_template(
        "status-msg",
        "orders",
        "tenant-1",
        "{% if status == \"shipped\" %}Your order is on its way!{% elif status == \"processing\" %}We are preparing your order.{% else %}Order received.{% endif %}",
    );

    // Profile using both templates via $ref
    let mut fields = HashMap::new();
    fields.insert(
        "item_summary".to_string(),
        TemplateProfileField::Ref {
            template_ref: "item-list".to_string(),
        },
    );
    fields.insert(
        "status_message".to_string(),
        TemplateProfileField::Ref {
            template_ref: "status-msg".to_string(),
        },
    );
    // Inline conditional
    fields.insert(
        "priority_label".to_string(),
        TemplateProfileField::Inline(
            "{% if express %}EXPRESS SHIPPING{% else %}Standard Shipping{% endif %}".to_string(),
        ),
    );
    let profile = make_profile("order-summary", "orders", "tenant-1", fields);

    let gateway = build_gateway(
        vec![list_template, status_template],
        vec![profile],
        vec![Arc::clone(&webhook)],
    );

    let action = Action::new(
        "orders",
        "tenant-1",
        "webhook",
        "order_update",
        serde_json::json!({
            "status": "shipped",
            "express": true,
            "items": [
                {"name": "Widget A", "qty": 2, "sale": false},
                {"name": "Gadget B", "qty": 1, "sale": true},
                {"name": "Doohickey C", "qty": 3, "sale": false},
            ],
        }),
    )
    .with_template("order-summary");

    let outcome = gateway.dispatch(action, None).await?;
    println!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    let calls = webhook.calls();
    let payload = &calls[0].action.payload;

    let item_summary = payload["item_summary"].as_str().unwrap();
    println!("  item_summary:\n{item_summary}");
    assert!(
        item_summary.contains("Widget A (x2)"),
        "should render loop items"
    );
    assert!(
        item_summary.contains("Gadget B (x1) [SALE]"),
        "should render conditional sale tag"
    );
    assert!(
        item_summary.contains("Doohickey C (x3)"),
        "should render all items"
    );
    assert!(
        item_summary.contains("Total items: 3"),
        "should render length filter"
    );

    let status_message = payload["status_message"].as_str().unwrap();
    println!("  status_message: {status_message}");
    assert_eq!(
        status_message, "Your order is on its way!",
        "should render shipped branch"
    );

    let priority_label = payload["priority_label"].as_str().unwrap();
    println!("  priority_label: {priority_label}");
    assert_eq!(
        priority_label, "EXPRESS SHIPPING",
        "should render express conditional"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 4 PASSED]\n");
    Ok(())
}

/// Scenario 5: Error handling -- missing profile and missing template ref.
async fn scenario_error_handling() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 5: ERROR HANDLING (missing profile, missing $ref)");
    println!("------------------------------------------------------------------\n");

    let email = Arc::new(RecordingProvider::new("email"));

    // --- Part A: Missing profile ---
    println!("  Part A: Dispatch with non-existent profile name");

    let gateway_a = build_gateway(vec![], vec![], vec![Arc::clone(&email)]);

    let action_a = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"name": "test"}),
    )
    .with_template("nonexistent-profile");

    let result_a = gateway_a.dispatch(action_a, None).await;
    println!("  Result: {result_a:?}");
    assert!(result_a.is_err(), "should fail for missing profile");
    let err_msg = result_a.unwrap_err().to_string();
    assert!(
        err_msg.contains("nonexistent-profile"),
        "error should mention the missing profile name, got: {err_msg}"
    );
    email.assert_not_called();

    gateway_a.shutdown().await;

    // --- Part B: Profile references a non-existent stored template ---
    println!("\n  Part B: Profile with dangling $ref to missing template");

    let email_b = Arc::new(RecordingProvider::new("email"));

    let mut fields = HashMap::new();
    fields.insert(
        "body".to_string(),
        TemplateProfileField::Ref {
            template_ref: "ghost-template".to_string(),
        },
    );
    let profile = make_profile("bad-ref-profile", "notifications", "tenant-1", fields);

    let gateway_b = build_gateway(vec![], vec![profile], vec![Arc::clone(&email_b)]);

    let action_b = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"name": "test"}),
    )
    .with_template("bad-ref-profile");

    let result_b = gateway_b.dispatch(action_b, None).await;
    println!("  Result: {result_b:?}");
    assert!(result_b.is_err(), "should fail for missing template ref");
    let err_msg_b = result_b.unwrap_err().to_string();
    assert!(
        err_msg_b.contains("ghost-template"),
        "error should mention the missing template name, got: {err_msg_b}"
    );
    email_b.assert_not_called();

    gateway_b.shutdown().await;

    // --- Part C: Dispatch without template field works normally ---
    println!("\n  Part C: Dispatch without template field (normal path)");

    let email_c = Arc::new(RecordingProvider::new("email"));
    let gateway_c = build_gateway(vec![], vec![], vec![Arc::clone(&email_c)]);

    let action_c = Action::new(
        "notifications",
        "tenant-1",
        "email",
        "send_email",
        serde_json::json!({"to": "user@example.com"}),
    );

    let outcome_c = gateway_c.dispatch(action_c, None).await?;
    println!("  Outcome: {outcome_c:?}");
    assert!(
        matches!(outcome_c, ActionOutcome::Executed(..)),
        "should execute normally without template"
    );
    email_c.assert_called(1);

    gateway_c.shutdown().await;
    println!("\n  [Scenario 5 PASSED]\n");
    Ok(())
}

/// Scenario 6: Nested variables and complex payload structures.
async fn scenario_nested_variables() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 6: NESTED VARIABLES AND COMPLEX PAYLOADS");
    println!("------------------------------------------------------------------\n");

    let webhook = Arc::new(RecordingProvider::new("webhook"));

    // Template accessing nested objects
    let detail_template = make_template(
        "user-detail",
        "crm",
        "tenant-1",
        "User: {{ user.name }} ({{ user.email }})\nCompany: {{ user.company.name }}\nPlan: {{ user.company.plan }}",
    );

    let mut fields = HashMap::new();
    fields.insert(
        "summary".to_string(),
        TemplateProfileField::Ref {
            template_ref: "user-detail".to_string(),
        },
    );
    fields.insert(
        "title".to_string(),
        TemplateProfileField::Inline(
            "Activity for {{ user.name }} @ {{ user.company.name }}".to_string(),
        ),
    );
    let profile = make_profile("user-activity", "crm", "tenant-1", fields);

    let gateway = build_gateway(
        vec![detail_template],
        vec![profile],
        vec![Arc::clone(&webhook)],
    );

    let action = Action::new(
        "crm",
        "tenant-1",
        "webhook",
        "user_event",
        serde_json::json!({
            "user": {
                "name": "Carol",
                "email": "carol@acme.com",
                "company": {
                    "name": "Acme Inc",
                    "plan": "enterprise",
                },
            },
            "event_type": "login",
        }),
    )
    .with_template("user-activity");

    let outcome = gateway.dispatch(action, None).await?;
    println!("  Outcome: {outcome:?}");
    assert!(matches!(outcome, ActionOutcome::Executed(..)));

    let calls = webhook.calls();
    let payload = &calls[0].action.payload;

    let summary = payload["summary"].as_str().unwrap();
    println!("  summary:\n{summary}");
    assert!(summary.contains("Carol"), "should contain user name");
    assert!(
        summary.contains("carol@acme.com"),
        "should contain user email"
    );
    assert!(summary.contains("Acme Inc"), "should contain company name");
    assert!(summary.contains("enterprise"), "should contain plan");

    let title = payload["title"].as_str().unwrap();
    println!("  title: {title}");
    assert_eq!(
        title, "Activity for Carol @ Acme Inc",
        "should render nested fields in inline template"
    );

    // Original nested data should still be present
    assert_eq!(
        payload["event_type"].as_str(),
        Some("login"),
        "non-template fields should be preserved"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 6 PASSED]\n");
    Ok(())
}

/// Scenario 7: Multi-tenant template isolation.
async fn scenario_multi_tenant_isolation() -> Result<(), Box<dyn std::error::Error>> {
    println!("------------------------------------------------------------------");
    println!("  SCENARIO 7: MULTI-TENANT TEMPLATE ISOLATION");
    println!("------------------------------------------------------------------\n");

    let email = Arc::new(RecordingProvider::new("email"));

    // Tenant A has a "greeting" profile
    let mut fields_a = HashMap::new();
    fields_a.insert(
        "message".to_string(),
        TemplateProfileField::Inline("Bonjour {{ name }}! Bienvenue.".to_string()),
    );
    let profile_a = make_profile("greeting", "notifications", "tenant-a", fields_a);

    // Tenant B has a "greeting" profile with different content
    let mut fields_b = HashMap::new();
    fields_b.insert(
        "message".to_string(),
        TemplateProfileField::Inline("Hello {{ name }}! Welcome aboard.".to_string()),
    );
    let profile_b = make_profile("greeting", "notifications", "tenant-b", fields_b);

    let gateway = build_gateway(vec![], vec![profile_a, profile_b], vec![Arc::clone(&email)]);

    // Tenant A action
    let action_a = Action::new(
        "notifications",
        "tenant-a",
        "email",
        "send_email",
        serde_json::json!({"name": "Pierre"}),
    )
    .with_template("greeting");

    let outcome_a = gateway.dispatch(action_a, None).await?;
    println!("  Tenant A outcome: {outcome_a:?}");
    assert!(matches!(outcome_a, ActionOutcome::Executed(..)));

    // Tenant B action
    let action_b = Action::new(
        "notifications",
        "tenant-b",
        "email",
        "send_email",
        serde_json::json!({"name": "Sarah"}),
    )
    .with_template("greeting");

    let outcome_b = gateway.dispatch(action_b, None).await?;
    println!("  Tenant B outcome: {outcome_b:?}");
    assert!(matches!(outcome_b, ActionOutcome::Executed(..)));

    let calls = email.calls();
    assert_eq!(calls.len(), 2);

    let payload_a = &calls[0].action.payload;
    let payload_b = &calls[1].action.payload;

    let msg_a = payload_a["message"].as_str().unwrap();
    let msg_b = payload_b["message"].as_str().unwrap();
    println!("  Tenant A message: {msg_a}");
    println!("  Tenant B message: {msg_b}");

    assert_eq!(
        msg_a, "Bonjour Pierre! Bienvenue.",
        "Tenant A should get French greeting"
    );
    assert_eq!(
        msg_b, "Hello Sarah! Welcome aboard.",
        "Tenant B should get English greeting"
    );

    // Tenant C (no profile) should fail
    let action_c = Action::new(
        "notifications",
        "tenant-c",
        "email",
        "send_email",
        serde_json::json!({"name": "Unknown"}),
    )
    .with_template("greeting");

    let result_c = gateway.dispatch(action_c, None).await;
    println!("  Tenant C (no profile): {result_c:?}");
    assert!(
        result_c.is_err(),
        "tenant without profile should get an error"
    );

    gateway.shutdown().await;
    println!("\n  [Scenario 7 PASSED]\n");
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("==================================================================");
    println!("     ACTEON PAYLOAD TEMPLATE SIMULATION");
    println!("     7 scenarios covering the full template lifecycle");
    println!("==================================================================\n");

    let total_start = Instant::now();

    scenario_inline_fields().await?;
    scenario_ref_fields().await?;
    scenario_mixed_inline_and_ref().await?;
    scenario_loops_and_conditionals().await?;
    scenario_error_handling().await?;
    scenario_nested_variables().await?;
    scenario_multi_tenant_isolation().await?;

    let total_elapsed = total_start.elapsed();

    println!("==================================================================");
    println!("     ALL 7 TEMPLATE SCENARIOS PASSED");
    println!("     Total time: {total_elapsed:?}");
    println!("==================================================================");

    Ok(())
}
