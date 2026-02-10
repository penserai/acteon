//! Demonstration of Recurring Actions in the simulation framework.
//!
//! This example shows how cron-scheduled recurring actions are created, stored,
//! validated, and managed. Each recurring action holds a cron expression and an
//! action template that is dispatched on every tick. The background processor
//! polls the pending-recurring index for due actions, claims them atomically,
//! and emits dispatch events.
//!
//! Scenarios demonstrated:
//!   1. Daily digest email (cron: `0 9 * * *`)
//!   2. Weekly report with timezone (cron: `0 8 * * 1`, tz: US/Eastern)
//!   3. Hourly health check (cron: `0 * * * *`)
//!   4. Business-hours-only notification (cron: `0 9-17 * * 1-5`)
//!   5. Monthly billing reminder with end_date (cron: `0 10 1 * *`)
//!   6. Recurring with max_executions limit
//!   7. Pause and resume lifecycle
//!   8. Multi-tenant concurrent recurring actions
//!
//! Run with: `cargo run -p acteon-simulation --example recurring_actions_simulation`

use std::collections::HashMap;
use std::sync::Arc;

use chrono::{Datelike, Duration, Timelike, Utc};

use acteon_core::{
    RecurringAction, RecurringActionTemplate, next_occurrence, validate_cron_expr,
    validate_min_interval, validate_timezone,
};
use acteon_state::{KeyKind, StateKey, StateStore};
use acteon_state_memory::MemoryStateStore;

// =============================================================================
// Helper: create a RecurringAction with sensible defaults
// =============================================================================

fn make_recurring(
    id: &str,
    namespace: &str,
    tenant: &str,
    cron_expr: &str,
    timezone: &str,
    provider: &str,
    action_type: &str,
    payload: serde_json::Value,
) -> RecurringAction {
    let now = Utc::now();
    let cron = validate_cron_expr(cron_expr).expect("valid cron");
    let tz = validate_timezone(timezone).expect("valid timezone");
    let next = next_occurrence(&cron, tz, &now);

    RecurringAction {
        id: id.to_string(),
        namespace: namespace.to_string(),
        tenant: tenant.to_string(),
        cron_expr: cron_expr.to_string(),
        timezone: timezone.to_string(),
        enabled: true,
        action_template: RecurringActionTemplate {
            provider: provider.to_string(),
            action_type: action_type.to_string(),
            payload,
            metadata: HashMap::new(),
            dedup_key: None,
        },
        created_at: now,
        updated_at: now,
        last_executed_at: None,
        next_execution_at: next,
        ends_at: None,
        execution_count: 0,
        description: None,
        labels: HashMap::new(),
    }
}

/// Store a recurring action in the state store and index it for the background
/// processor.
async fn store_recurring(
    state: &Arc<dyn StateStore>,
    action: &RecurringAction,
) -> Result<(), Box<dyn std::error::Error>> {
    let key = StateKey::new(
        action.namespace.as_str(),
        action.tenant.as_str(),
        KeyKind::RecurringAction,
        action.id.as_str(),
    );
    let json = serde_json::to_string(action)?;
    state.set(&key, &json, None).await?;

    // Index for the background processor if there's a next execution time.
    if let Some(next) = action.next_execution_at {
        let pending_key = StateKey::new(
            action.namespace.as_str(),
            action.tenant.as_str(),
            KeyKind::PendingRecurring,
            action.id.as_str(),
        );
        state
            .index_timeout(&pending_key, next.timestamp_millis())
            .await?;
    }

    Ok(())
}

/// Load a recurring action from the state store.
async fn load_recurring(
    state: &Arc<dyn StateStore>,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<Option<RecurringAction>, Box<dyn std::error::Error>> {
    let key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, id);
    match state.get(&key).await? {
        Some(data) => Ok(Some(serde_json::from_str(&data)?)),
        None => Ok(None),
    }
}

/// Remove a recurring action from both the data store and pending index.
async fn delete_recurring(
    state: &Arc<dyn StateStore>,
    namespace: &str,
    tenant: &str,
    id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let key = StateKey::new(namespace, tenant, KeyKind::RecurringAction, id);
    state.delete(&key).await?;
    let pending_key = StateKey::new(namespace, tenant, KeyKind::PendingRecurring, id);
    state.delete(&pending_key).await?;
    state.remove_timeout_index(&pending_key).await?;
    Ok(())
}

// =============================================================================
// Main
// =============================================================================

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║          RECURRING ACTIONS SIMULATION DEMO                   ║");
    println!("╚══════════════════════════════════════════════════════════════╝\n");

    // =========================================================================
    // SCENARIO 1: Daily Digest Email
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 1: DAILY DIGEST EMAIL");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A daily digest email fires at 09:00 UTC every day.");
    println!("  Cron: 0 9 * * *\n");

    let state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());

    let daily_digest = make_recurring(
        "rec-daily-001",
        "notifications",
        "acme-corp",
        "0 9 * * *",
        "UTC",
        "email",
        "send_digest",
        serde_json::json!({
            "to": "team@acme.com",
            "subject": "Daily Activity Digest",
            "template": "daily_digest_v2",
        }),
    );

    store_recurring(&state, &daily_digest).await?;

    println!("  [create]   ID: {}", daily_digest.id);
    println!("  [create]   Cron: {}", daily_digest.cron_expr);
    println!(
        "  [create]   Provider: {}",
        daily_digest.action_template.provider
    );
    println!(
        "  [create]   Action type: {}",
        daily_digest.action_template.action_type
    );
    println!(
        "  [create]   Next execution: {}",
        daily_digest
            .next_execution_at
            .map_or_else(|| "none".to_string(), |t| t.to_rfc3339())
    );

    // Verify stored correctly
    let loaded = load_recurring(&state, "notifications", "acme-corp", "rec-daily-001")
        .await?
        .expect("should be stored");
    assert_eq!(loaded.id, "rec-daily-001");
    assert_eq!(loaded.cron_expr, "0 9 * * *");
    assert!(loaded.enabled);
    println!("  [verify]   Stored and loaded successfully");

    // Demonstrate next occurrences
    let cron = validate_cron_expr("0 9 * * *")?;
    let tz = validate_timezone("UTC")?;
    let now = Utc::now();
    let first = next_occurrence(&cron, tz, &now).expect("has next");
    let second = next_occurrence(&cron, tz, &first).expect("has next");
    let third = next_occurrence(&cron, tz, &second).expect("has next");
    println!("  [schedule] Next 3 occurrences:");
    println!("             1st: {}", first.format("%Y-%m-%d %H:%M UTC"));
    println!("             2nd: {}", second.format("%Y-%m-%d %H:%M UTC"));
    println!("             3rd: {}", third.format("%Y-%m-%d %H:%M UTC"));
    assert_eq!(first.hour(), 9);
    assert_eq!(second.hour(), 9);
    assert_eq!((second - first).num_hours(), 24);
    println!("  [verify]   All at 09:00, 24h apart\n");

    // =========================================================================
    // SCENARIO 2: Weekly Report with Timezone
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 2: WEEKLY REPORT WITH TIMEZONE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A weekly report fires every Monday at 08:00 US/Eastern.");
    println!("  The cron expression is evaluated in the specified timezone,");
    println!("  so the UTC time shifts with daylight saving time.\n");

    let weekly_report = make_recurring(
        "rec-weekly-001",
        "analytics",
        "acme-corp",
        "0 8 * * 1",
        "US/Eastern",
        "report-engine",
        "generate_weekly_report",
        serde_json::json!({
            "report_type": "weekly_summary",
            "format": "pdf",
            "recipients": ["cto@acme.com", "vp-eng@acme.com"],
        }),
    );

    store_recurring(&state, &weekly_report).await?;

    println!("  [create]   ID: {}", weekly_report.id);
    println!(
        "  [create]   Cron: {} (timezone: {})",
        weekly_report.cron_expr, weekly_report.timezone
    );
    println!(
        "  [create]   Next execution: {}",
        weekly_report
            .next_execution_at
            .map_or_else(|| "none".to_string(), |t| t.to_rfc3339())
    );

    // Demonstrate timezone-aware scheduling
    let cron = validate_cron_expr("0 8 * * 1")?;
    let eastern = validate_timezone("US/Eastern")?;
    let first = next_occurrence(&cron, eastern, &now).expect("has next");
    let second = next_occurrence(&cron, eastern, &first).expect("has next");
    println!("  [schedule] Next 2 Monday 08:00 ET occurrences (in UTC):");
    println!(
        "             1st: {}",
        first.format("%Y-%m-%d %H:%M UTC (weekday: %A)")
    );
    println!(
        "             2nd: {}",
        second.format("%Y-%m-%d %H:%M UTC (weekday: %A)")
    );
    assert_eq!(
        first.weekday().num_days_from_monday(),
        0,
        "should be Monday"
    );
    assert_eq!(
        (second - first).num_days(),
        7,
        "should be exactly 7 days apart"
    );
    println!("  [verify]   Both on Monday, 7 days apart");

    // Validate interval -- weekly is well above the 60-second minimum
    let interval_result = validate_min_interval(&cron, eastern, 60);
    assert!(interval_result.is_ok());
    println!("  [verify]   Interval validation passed (weekly >> 60s minimum)\n");

    // =========================================================================
    // SCENARIO 3: Hourly Health Check
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 3: HOURLY HEALTH CHECK");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  An hourly health check pings a webhook every hour on the hour.");
    println!("  Cron: 0 * * * *\n");

    let health_check = make_recurring(
        "rec-hourly-001",
        "monitoring",
        "acme-corp",
        "0 * * * *",
        "UTC",
        "webhook",
        "health_check",
        serde_json::json!({
            "url": "https://status.acme.com/api/ping",
            "method": "POST",
            "headers": {"X-Source": "acteon-recurring"},
        }),
    );

    store_recurring(&state, &health_check).await?;

    println!("  [create]   ID: {}", health_check.id);
    println!("  [create]   Cron: {}", health_check.cron_expr);
    println!(
        "  [create]   Next execution: {}",
        health_check
            .next_execution_at
            .map_or_else(|| "none".to_string(), |t| t.to_rfc3339())
    );

    // Show consecutive occurrences
    let cron = validate_cron_expr("0 * * * *")?;
    let tz = validate_timezone("UTC")?;
    let first = next_occurrence(&cron, tz, &now).expect("has next");
    let second = next_occurrence(&cron, tz, &first).expect("has next");
    let third = next_occurrence(&cron, tz, &second).expect("has next");
    let fourth = next_occurrence(&cron, tz, &third).expect("has next");
    println!("  [schedule] Next 4 occurrences:");
    println!("             {}", first.format("%H:%M UTC"));
    println!("             {}", second.format("%H:%M UTC"));
    println!("             {}", third.format("%H:%M UTC"));
    println!("             {}", fourth.format("%H:%M UTC"));
    assert_eq!((second - first).num_minutes(), 60);
    assert_eq!((third - second).num_minutes(), 60);
    println!("  [verify]   All exactly 60 minutes apart");

    // Validate interval -- hourly passes the default 60-second minimum
    let interval_result = validate_min_interval(&cron, tz, 60);
    assert!(interval_result.is_ok());
    println!("  [verify]   Interval validation passed (3600s >> 60s minimum)\n");

    // =========================================================================
    // SCENARIO 4: Business-Hours-Only Notification
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 4: BUSINESS-HOURS-ONLY NOTIFICATION");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Notifications fire hourly only during business hours (9-17)");
    println!("  on weekdays (Mon-Fri). No weekend or off-hours noise.");
    println!("  Cron: 0 9-17 * * 1-5\n");

    let biz_hours = make_recurring(
        "rec-bizhours-001",
        "notifications",
        "acme-corp",
        "0 9-17 * * 1-5",
        "America/New_York",
        "slack",
        "standup_reminder",
        serde_json::json!({
            "channel": "#engineering",
            "message": "Time for standup! Join the huddle.",
        }),
    );

    store_recurring(&state, &biz_hours).await?;

    println!("  [create]   ID: {}", biz_hours.id);
    println!(
        "  [create]   Cron: {} (timezone: {})",
        biz_hours.cron_expr, biz_hours.timezone
    );

    // Show how the cron skips weekends
    let cron = validate_cron_expr("0 9-17 * * 1-5")?;
    let ny_tz = validate_timezone("America/New_York")?;
    println!("  [schedule] Next 5 occurrences (in local time):");
    let mut cursor = now;
    for i in 1..=5 {
        let next = next_occurrence(&cron, ny_tz, &cursor).expect("has next");
        let local = next.with_timezone(&ny_tz);
        println!(
            "             {i}. {} ({})",
            local.format("%Y-%m-%d %H:%M %Z"),
            local.format("%A"),
        );
        // Verify it's a weekday (Mon=0 .. Fri=4)
        let weekday = local.weekday().num_days_from_monday();
        assert!(weekday < 5, "should be a weekday, got {weekday}");
        // Verify it's within business hours (9-17)
        let hour = local.hour();
        assert!((9..=17).contains(&hour), "should be 9-17, got {hour}");
        cursor = next;
    }
    println!("  [verify]   All on weekdays, all within 9:00-17:00\n");

    // =========================================================================
    // SCENARIO 5: Monthly Billing Reminder with End Date
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 5: MONTHLY BILLING REMINDER WITH END DATE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A billing reminder fires on the 1st of every month at 10:00.");
    println!("  It has an end_date set 6 months from now, after which the");
    println!("  background processor will auto-disable it.\n");

    let ends_at = now + Duration::days(180);
    let mut monthly_billing = make_recurring(
        "rec-monthly-001",
        "billing",
        "acme-corp",
        "0 10 1 * *",
        "UTC",
        "email",
        "billing_reminder",
        serde_json::json!({
            "to": "billing@acme.com",
            "subject": "Monthly invoice ready",
            "invoice_url": "https://billing.acme.com/invoices/latest",
        }),
    );
    monthly_billing.ends_at = Some(ends_at);
    monthly_billing.description =
        Some("Monthly billing reminder -- auto-expires in 6 months".into());

    store_recurring(&state, &monthly_billing).await?;

    println!("  [create]   ID: {}", monthly_billing.id);
    println!("  [create]   Cron: {}", monthly_billing.cron_expr);
    println!(
        "  [create]   End date: {}",
        ends_at.format("%Y-%m-%d %H:%M UTC")
    );
    println!(
        "  [create]   Description: {}",
        monthly_billing.description.as_deref().unwrap_or("none")
    );

    // Show monthly occurrences
    let cron = validate_cron_expr("0 10 1 * *")?;
    let tz = validate_timezone("UTC")?;
    println!("  [schedule] Next 4 monthly occurrences:");
    let mut cursor = now;
    for i in 1..=4 {
        let next = next_occurrence(&cron, tz, &cursor).expect("has next");
        println!(
            "             {i}. {}",
            next.format("%Y-%m-%d %H:%M UTC (%B)")
        );
        assert_eq!(next.day(), 1, "should be the 1st of the month");
        assert_eq!(next.hour(), 10);
        cursor = next;
    }

    // Demonstrate that the background processor would skip after end_date
    let loaded = load_recurring(&state, "billing", "acme-corp", "rec-monthly-001")
        .await?
        .expect("stored");
    assert!(loaded.ends_at.is_some());
    let will_expire = loaded.ends_at.unwrap() <= now + Duration::days(181);
    assert!(will_expire, "should expire within 181 days");
    println!("  [verify]   End date set -- background processor will auto-disable after expiry\n");

    // =========================================================================
    // SCENARIO 6: Max Executions Limit
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 6: RECURRING WITH MAX EXECUTIONS LIMIT");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A recurring action tracks execution_count. Applications can");
    println!("  enforce a max-executions limit by disabling the action once");
    println!("  the count reaches the threshold.\n");

    let max_executions: u64 = 5;
    let mut limited_action = make_recurring(
        "rec-limited-001",
        "campaigns",
        "acme-corp",
        "0 12 * * *",
        "UTC",
        "email",
        "campaign_email",
        serde_json::json!({
            "campaign_id": "spring-sale-2026",
            "template": "daily_promo",
        }),
    );
    limited_action
        .labels
        .insert("max_executions".into(), max_executions.to_string());

    store_recurring(&state, &limited_action).await?;

    println!("  [create]   ID: {}", limited_action.id);
    println!("  [create]   Max executions: {max_executions}");
    println!(
        "  [create]   Current count: {}",
        limited_action.execution_count
    );

    // Simulate executions incrementing the count
    println!("\n  Simulating {max_executions} executions...");
    for i in 1..=max_executions {
        limited_action.execution_count = i;
        limited_action.last_executed_at = Some(Utc::now());
        limited_action.updated_at = Utc::now();

        if i >= max_executions {
            limited_action.enabled = false;
            limited_action.next_execution_at = None;
            println!("  [exec {i}]   Reached max -- disabling recurring action");
        } else {
            println!("  [exec {i}]   Execution count: {i}/{max_executions}");
        }
    }

    store_recurring(&state, &limited_action).await?;

    let loaded = load_recurring(&state, "campaigns", "acme-corp", "rec-limited-001")
        .await?
        .expect("stored");
    assert!(!loaded.enabled, "should be disabled after max executions");
    assert_eq!(loaded.execution_count, max_executions);
    assert!(loaded.next_execution_at.is_none());
    println!("  [verify]   Disabled after {max_executions} executions");
    println!("  [verify]   execution_count = {}", loaded.execution_count);
    println!("  [verify]   enabled = {}", loaded.enabled);
    println!("  [verify]   next_execution_at = none\n");

    // =========================================================================
    // SCENARIO 7: Pause and Resume Lifecycle
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 7: PAUSE AND RESUME LIFECYCLE");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  A recurring action can be paused (disabled) and later resumed.");
    println!("  Pausing removes it from the pending index. Resuming recalculates");
    println!("  the next execution time and re-indexes it.\n");

    let lifecycle_state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());

    let mut lifecycle_action = make_recurring(
        "rec-lifecycle-001",
        "alerts",
        "acme-corp",
        "0 * * * *",
        "UTC",
        "slack",
        "hourly_status",
        serde_json::json!({
            "channel": "#ops",
            "message": "Hourly system status check",
        }),
    );

    store_recurring(&lifecycle_state, &lifecycle_action).await?;

    println!("  [create]   ID: {}", lifecycle_action.id);
    println!("  [create]   Enabled: {}", lifecycle_action.enabled);
    println!(
        "  [create]   Next: {}",
        lifecycle_action
            .next_execution_at
            .map_or_else(|| "none".to_string(), |t| t.format("%H:%M UTC").to_string())
    );

    // Simulate a few executions
    for i in 1..=3 {
        lifecycle_action.execution_count = i;
        lifecycle_action.last_executed_at = Some(Utc::now());
        println!("  [exec {i}]   Executed (count: {i})");
    }

    // PAUSE
    println!("\n  [pause]    Pausing recurring action...");
    lifecycle_action.enabled = false;
    lifecycle_action.next_execution_at = None;
    lifecycle_action.updated_at = Utc::now();
    store_recurring(&lifecycle_state, &lifecycle_action).await?;

    // Remove from pending index
    let pending_key = StateKey::new(
        "alerts",
        "acme-corp",
        KeyKind::PendingRecurring,
        "rec-lifecycle-001",
    );
    lifecycle_state.remove_timeout_index(&pending_key).await?;

    let loaded = load_recurring(&lifecycle_state, "alerts", "acme-corp", "rec-lifecycle-001")
        .await?
        .expect("stored");
    assert!(!loaded.enabled);
    assert!(loaded.next_execution_at.is_none());
    println!("  [pause]    Enabled: {}", loaded.enabled);
    println!("  [pause]    Next execution: none");
    println!("  [pause]    Background processor will skip this action");

    // RESUME
    println!("\n  [resume]   Resuming recurring action...");
    lifecycle_action.enabled = true;
    let cron = validate_cron_expr(&lifecycle_action.cron_expr)?;
    let tz = validate_timezone(&lifecycle_action.timezone)?;
    let resume_now = Utc::now();
    lifecycle_action.next_execution_at = next_occurrence(&cron, tz, &resume_now);
    lifecycle_action.updated_at = Utc::now();
    store_recurring(&lifecycle_state, &lifecycle_action).await?;

    let loaded = load_recurring(&lifecycle_state, "alerts", "acme-corp", "rec-lifecycle-001")
        .await?
        .expect("stored");
    assert!(loaded.enabled);
    assert!(loaded.next_execution_at.is_some());
    println!("  [resume]   Enabled: {}", loaded.enabled);
    println!(
        "  [resume]   Next execution: {}",
        loaded
            .next_execution_at
            .map_or_else(|| "none".to_string(), |t| t.format("%H:%M UTC").to_string())
    );
    println!(
        "  [resume]   Execution count preserved: {}",
        loaded.execution_count
    );
    assert_eq!(
        loaded.execution_count, 3,
        "count should be preserved across pause/resume"
    );
    println!("  [verify]   Lifecycle: create -> execute x3 -> pause -> resume\n");

    // =========================================================================
    // SCENARIO 8: Multi-Tenant Concurrent Recurring Actions
    // =========================================================================
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━");
    println!("  SCENARIO 8: MULTI-TENANT CONCURRENT RECURRING ACTIONS");
    println!("━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━\n");

    println!("  Multiple tenants each have their own recurring actions with");
    println!("  different schedules. Actions are fully isolated per tenant.");
    println!("  Deleting one tenant's action does not affect others.\n");

    let multi_state: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());

    // Tenant Alpha: daily digest
    let alpha_daily = make_recurring(
        "rec-alpha-001",
        "notifications",
        "tenant-alpha",
        "0 9 * * *",
        "UTC",
        "email",
        "daily_digest",
        serde_json::json!({"to": "team@alpha.io"}),
    );

    // Tenant Alpha: weekly report
    let alpha_weekly = make_recurring(
        "rec-alpha-002",
        "analytics",
        "tenant-alpha",
        "0 8 * * 1",
        "US/Pacific",
        "report-engine",
        "weekly_report",
        serde_json::json!({"format": "csv"}),
    );

    // Tenant Beta: hourly health check
    let beta_hourly = make_recurring(
        "rec-beta-001",
        "monitoring",
        "tenant-beta",
        "0 * * * *",
        "Europe/London",
        "webhook",
        "health_check",
        serde_json::json!({"url": "https://beta.io/health"}),
    );

    // Tenant Gamma: every 5 minutes
    let gamma_frequent = make_recurring(
        "rec-gamma-001",
        "monitoring",
        "tenant-gamma",
        "*/5 * * * *",
        "Asia/Tokyo",
        "webhook",
        "metrics_poll",
        serde_json::json!({"endpoint": "/metrics"}),
    );

    // Store all
    for action in [&alpha_daily, &alpha_weekly, &beta_hourly, &gamma_frequent] {
        store_recurring(&multi_state, action).await?;
        println!(
            "  [create]   {} / {} -- cron: {} (tz: {})",
            action.tenant, action.id, action.cron_expr, action.timezone,
        );
    }

    println!();

    // Verify isolation: load each tenant's actions
    let a1 = load_recurring(
        &multi_state,
        "notifications",
        "tenant-alpha",
        "rec-alpha-001",
    )
    .await?;
    let a2 = load_recurring(&multi_state, "analytics", "tenant-alpha", "rec-alpha-002").await?;
    let b1 = load_recurring(&multi_state, "monitoring", "tenant-beta", "rec-beta-001").await?;
    let g1 = load_recurring(&multi_state, "monitoring", "tenant-gamma", "rec-gamma-001").await?;

    assert!(a1.is_some(), "alpha daily should exist");
    assert!(a2.is_some(), "alpha weekly should exist");
    assert!(b1.is_some(), "beta hourly should exist");
    assert!(g1.is_some(), "gamma frequent should exist");
    println!("  [verify]   All 4 recurring actions stored independently");

    // Verify cross-tenant isolation: beta can't see alpha's actions
    let cross = load_recurring(
        &multi_state,
        "notifications",
        "tenant-beta",
        "rec-alpha-001",
    )
    .await?;
    assert!(cross.is_none(), "beta should not see alpha's actions");
    println!("  [verify]   Tenant isolation confirmed (beta cannot see alpha's actions)");

    // Delete one tenant's action -- others remain
    println!("\n  [delete]   Deleting tenant-beta's hourly health check...");
    delete_recurring(&multi_state, "monitoring", "tenant-beta", "rec-beta-001").await?;

    let deleted = load_recurring(&multi_state, "monitoring", "tenant-beta", "rec-beta-001").await?;
    assert!(deleted.is_none(), "beta action should be deleted");
    println!("  [verify]   tenant-beta action deleted");

    // Verify others are unaffected
    let a1_still = load_recurring(
        &multi_state,
        "notifications",
        "tenant-alpha",
        "rec-alpha-001",
    )
    .await?;
    let g1_still =
        load_recurring(&multi_state, "monitoring", "tenant-gamma", "rec-gamma-001").await?;
    assert!(a1_still.is_some(), "alpha should be unaffected");
    assert!(g1_still.is_some(), "gamma should be unaffected");
    println!("  [verify]   Other tenants' actions unaffected by deletion");

    // Show concurrent schedule summary
    println!("\n  Schedule summary (all tenants):");
    for action in [&alpha_daily, &alpha_weekly, &gamma_frequent] {
        println!(
            "    {} / {} : next = {}",
            action.tenant,
            action.id,
            action
                .next_execution_at
                .map_or_else(|| "none".to_string(), |t| t.to_rfc3339()),
        );
    }
    println!("    tenant-beta / rec-beta-001 : DELETED\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("╔══════════════════════════════════════════════════════════════╗");
    println!("║            RECURRING ACTIONS DEMO COMPLETE                   ║");
    println!("╠══════════════════════════════════════════════════════════════╣");
    println!("║                                                              ║");
    println!("║  Scenarios demonstrated:                                     ║");
    println!("║                                                              ║");
    println!("║  1. Daily Digest Email                                       ║");
    println!("║     - Cron: 0 9 * * * (every day at 09:00 UTC)              ║");
    println!("║     - Verified 24-hour gaps between occurrences              ║");
    println!("║                                                              ║");
    println!("║  2. Weekly Report with Timezone                              ║");
    println!("║     - Cron: 0 8 * * 1 (Monday 08:00 US/Eastern)             ║");
    println!("║     - Timezone-aware scheduling with DST handling            ║");
    println!("║                                                              ║");
    println!("║  3. Hourly Health Check                                      ║");
    println!("║     - Cron: 0 * * * * (every hour on the hour)              ║");
    println!("║     - 60-minute intervals, passes min-interval validation    ║");
    println!("║                                                              ║");
    println!("║  4. Business-Hours-Only Notification                         ║");
    println!("║     - Cron: 0 9-17 * * 1-5 (weekdays 9am-5pm)              ║");
    println!("║     - Skips weekends and off-hours automatically             ║");
    println!("║                                                              ║");
    println!("║  5. Monthly Billing Reminder                                 ║");
    println!("║     - Cron: 0 10 1 * * (1st of month at 10:00)             ║");
    println!("║     - Auto-expires via end_date after 6 months               ║");
    println!("║                                                              ║");
    println!("║  6. Max Executions Limit                                     ║");
    println!("║     - Tracks execution_count, disables at threshold          ║");
    println!("║     - Label-based max_executions enforcement                 ║");
    println!("║                                                              ║");
    println!("║  7. Pause and Resume Lifecycle                               ║");
    println!("║     - Pause removes from pending index                       ║");
    println!("║     - Resume recalculates next_execution_at                  ║");
    println!("║     - Execution count preserved across lifecycle             ║");
    println!("║                                                              ║");
    println!("║  8. Multi-Tenant Concurrent Actions                          ║");
    println!("║     - 4 tenants with independent schedules/timezones         ║");
    println!("║     - Full tenant isolation verified                         ║");
    println!("║     - Delete one tenant's action, others unaffected          ║");
    println!("║                                                              ║");
    println!("╚══════════════════════════════════════════════════════════════╝");

    Ok(())
}
