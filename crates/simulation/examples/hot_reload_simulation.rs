//! End-to-end demonstration of TOML hot reload for quotas + templates.
//!
//! Exercises the full pipeline:
//!   1. Write a `quotas.toml` and `templates.toml` to a temp dir.
//!   2. Call the loader directly → verify gateway + state store updates.
//!   3. Dispatch real actions through the gateway and assert that quota
//!      enforcement reflects the loaded TOML.
//!   4. Edit the TOML (raise the limit), reload, dispatch again and
//!      confirm the new limit applies.
//!   5. Remove an entry from the TOML, reload, confirm the previously-
//!      enforced policy is gone (dispatches that used to block now pass).
//!   6. Spawn the `file_watcher`, rewrite the TOML on disk, sleep past
//!      the debounce, and confirm an automatic reload happened — no
//!      explicit reload call needed.
//!
//! Run with: `cargo run -p acteon-simulation --example hot_reload_simulation`

use std::sync::Arc;
use std::time::Duration;

use acteon_core::{Action, ActionOutcome};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_provider::{DynProvider, ProviderError};
use acteon_state::StateStore;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use async_trait::async_trait;
use tokio::sync::RwLock;
use tracing::info;

// =============================================================================
// Mock provider: always succeeds, used so dispatch reaches the quota check.
// =============================================================================

struct MockProvider {
    name: &'static str,
}

impl MockProvider {
    const fn new(name: &'static str) -> Self {
        Self { name }
    }
}

#[async_trait]
impl DynProvider for MockProvider {
    fn name(&self) -> &str {
        self.name
    }

    async fn execute(
        &self,
        _action: &Action,
    ) -> Result<acteon_core::ProviderResponse, ProviderError> {
        Ok(acteon_core::ProviderResponse::success(
            serde_json::json!({ "provider": self.name, "ok": true }),
        ))
    }

    async fn health_check(&self) -> Result<(), ProviderError> {
        Ok(())
    }
}

fn make_action(idx: usize) -> Action {
    Action::new(
        "notifications",
        "acme",
        "email",
        "send_email",
        serde_json::json!({ "to": format!("user{idx}@example.com") }),
    )
}

async fn build_gateway() -> (Arc<RwLock<Gateway>>, Arc<dyn StateStore>) {
    let store: Arc<dyn StateStore> = Arc::new(MemoryStateStore::new());
    let gw = GatewayBuilder::new()
        .state(Arc::clone(&store))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .provider(Arc::new(MockProvider::new("email")))
        .build()
        .expect("gateway should build");
    (Arc::new(RwLock::new(gw)), store)
}

async fn count_executed(gw: &Arc<RwLock<Gateway>>, dispatches: usize) -> (usize, usize) {
    let mut executed = 0usize;
    let mut blocked = 0usize;
    for i in 0..dispatches {
        let outcome = gw
            .read()
            .await
            .dispatch(make_action(i), None)
            .await
            .unwrap();
        match outcome {
            ActionOutcome::Executed(_) => executed += 1,
            ActionOutcome::QuotaExceeded { .. } => blocked += 1,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    (executed, blocked)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter("info,hot_reload_simulation=info")
        .init();

    // Unique temp dir for this simulation run.
    let tmp = std::env::temp_dir().join(format!(
        "acteon-hot-reload-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&tmp)?;
    let quotas_path = tmp.join("quotas.toml");
    let templates_path = tmp.join("templates.toml");

    info!(dir = %tmp.display(), "set up temp manifest directory");

    // -----------------------------------------------------------------
    // Step 1: write initial manifests
    // -----------------------------------------------------------------
    info!("step 1: write initial quotas.toml (cap = 3) + templates.toml");
    std::fs::write(
        &quotas_path,
        r#"
            [[quotas]]
            namespace = "notifications"
            tenant = "acme"
            max_actions = 3
            window = "hourly"
            overage_behavior = "block"
            description = "initial cap"
        "#,
    )?;
    std::fs::write(
        &templates_path,
        r#"
            [[templates]]
            namespace = "notifications"
            tenant = "acme"
            name = "greeting"
            content = "Hello {{ user.name }}"

            [[profiles]]
            namespace = "notifications"
            tenant = "acme"
            name = "welcome"
            fields = { subject = "Hi", body = { "$ref" = "greeting" } }
        "#,
    )?;

    let (gateway, state) = build_gateway().await;

    // -----------------------------------------------------------------
    // Step 2: initial load + assert gateway picked things up
    // -----------------------------------------------------------------
    info!("step 2: load both manifests");
    let q_report =
        acteon_server::quotas_loader::reload_from_file(&quotas_path, &gateway, &state).await?;
    let t_report =
        acteon_server::templates_loader::reload_from_file(&templates_path, &gateway, &state)
            .await?;
    info!(
        upserted = q_report.upserted,
        deleted = q_report.deleted,
        skipped = q_report.skipped,
        "quotas reconciled"
    );
    info!(
        templates_upserted = t_report.templates_upserted,
        profiles_upserted = t_report.profiles_upserted,
        "templates reconciled"
    );
    assert_eq!(q_report.upserted, 1);
    assert_eq!(t_report.templates_upserted, 1);
    assert_eq!(t_report.profiles_upserted, 1);

    // Template ended up in the gateway's in-memory map.
    {
        let gw = gateway.read().await;
        assert!(
            gw.template_exists("notifications", "acme", "greeting"),
            "greeting template should be loaded"
        );
    }

    // -----------------------------------------------------------------
    // Step 3: dispatch up to the cap
    // -----------------------------------------------------------------
    info!("step 3: dispatch 5 actions — expect 3 executed, 2 blocked");
    let (ok, blocked) = count_executed(&gateway, 5).await;
    assert_eq!((ok, blocked), (3, 2), "initial cap of 3 must be enforced");
    info!(executed = ok, blocked, "✓ initial cap enforced");

    // -----------------------------------------------------------------
    // Step 4: edit TOML (raise cap to 10), call reload, dispatch again
    // -----------------------------------------------------------------
    info!("step 4: edit quotas.toml (cap → 10) and reload");
    std::fs::write(
        &quotas_path,
        r#"
            [[quotas]]
            namespace = "notifications"
            tenant = "acme"
            max_actions = 10
            window = "hourly"
            overage_behavior = "block"
            description = "raised cap"
        "#,
    )?;
    let report =
        acteon_server::quotas_loader::reload_from_file(&quotas_path, &gateway, &state).await?;
    info!(
        upserted = report.upserted,
        deleted = report.deleted,
        "✓ reconcile is idempotent on same ID, no deletes"
    );
    assert_eq!(report.upserted, 1);
    assert_eq!(report.deleted, 0, "deterministic ID means upsert, not new");

    // The counter from step 3 already used 5 slots (3 executed + 2
    // blocked rolled back). New cap is 10, so we should get 7 more
    // executed before hitting the limit. The remaining 3 should
    // block.
    let (ok2, blocked2) = count_executed(&gateway, 10).await;
    assert_eq!(
        (ok2, blocked2),
        (7, 3),
        "raised cap should let 7 more through"
    );
    info!(
        executed = ok2,
        blocked = blocked2,
        "✓ raised cap takes effect"
    );

    // -----------------------------------------------------------------
    // Step 5: remove the entry, reload, confirm deletion
    // -----------------------------------------------------------------
    info!("step 5: empty quotas.toml + reload — entry should be diff-deleted");
    std::fs::write(&quotas_path, "# no quotas here\n")?;
    let report =
        acteon_server::quotas_loader::reload_from_file(&quotas_path, &gateway, &state).await?;
    assert_eq!(report.upserted, 0);
    assert_eq!(
        report.deleted, 1,
        "removed entry must be pruned because it carried _source=toml"
    );
    info!(
        upserted = report.upserted,
        deleted = report.deleted,
        "✓ stale entry pruned, API-managed records untouched"
    );

    // No more quota → all dispatches pass.
    let (ok3, blocked3) = count_executed(&gateway, 5).await;
    assert_eq!((ok3, blocked3), (5, 0), "no policy → no enforcement");
    info!(
        executed = ok3,
        blocked = blocked3,
        "✓ all dispatches pass with empty manifest"
    );

    // -----------------------------------------------------------------
    // Step 6: file watcher round trip — use a *fresh* (ns, tenant)
    // scope so we don't have to reason about persistent counters
    // from previous steps. The watcher path is what's under test,
    // not the counter arithmetic.
    // -----------------------------------------------------------------
    info!("step 6: spawn file watcher, edit TOML on disk, wait past debounce");

    // Start with no entries in the file so we can observe the
    // watcher *adding* the policy.
    std::fs::write(&quotas_path, "# empty manifest\n")?;

    let gw_for_watch = Arc::clone(&gateway);
    let store_for_watch = Arc::clone(&state);
    let path_for_watch = quotas_path.clone();
    let _watcher_handle = acteon_server::file_watcher::spawn_watcher(
        quotas_path.clone(),
        acteon_server::file_watcher::WatchMode::SingleFile,
        Duration::from_millis(200),
        None,
        move || {
            let gw = Arc::clone(&gw_for_watch);
            let store = Arc::clone(&store_for_watch);
            let path = path_for_watch.clone();
            async move {
                match acteon_server::quotas_loader::reload_from_file(&path, &gw, &store).await {
                    Ok(report) => info!(
                        upserted = report.upserted,
                        deleted = report.deleted,
                        "watcher: reload complete"
                    ),
                    Err(e) => tracing::error!(error = %e, "watcher reload failed"),
                }
            }
        },
    );

    // Let the watcher establish itself before we edit.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Edit the file to add a cap-of-2 policy scoped to a *fresh*
    // (namespace, tenant) pair so its counter bucket starts at 0.
    std::fs::write(
        &quotas_path,
        r#"
            [[quotas]]
            namespace = "watcher-test"
            tenant = "fresh"
            max_actions = 2
            window = "hourly"
            overage_behavior = "block"
            description = "watcher reload added me"
        "#,
    )?;

    // Wait past the 200ms debounce + a buffer for the reconcile to
    // complete. macOS FSEvents and Linux inotify both deliver within
    // a few hundred ms.
    tokio::time::sleep(Duration::from_millis(1500)).await;

    // The watcher should have fired and the new policy should now be
    // in the gateway's in-memory bucket. Dispatch against the fresh
    // scope and assert cap-of-2 enforcement.
    info!("step 6: dispatch 5 actions to watcher-test/fresh — expect cap-of-2");
    let mut watch_ok = 0usize;
    let mut watch_blocked = 0usize;
    for i in 0..5 {
        let action = Action::new(
            "watcher-test",
            "fresh",
            "email",
            "send_email",
            serde_json::json!({ "to": format!("user{i}@example.com") }),
        );
        let outcome = gateway.read().await.dispatch(action, None).await?;
        match outcome {
            ActionOutcome::Executed(_) => watch_ok += 1,
            ActionOutcome::QuotaExceeded { .. } => watch_blocked += 1,
            other => panic!("unexpected outcome: {other:?}"),
        }
    }
    assert_eq!(
        (watch_ok, watch_blocked),
        (2, 3),
        "watcher reload should have applied the new cap of 2"
    );
    info!(
        executed = watch_ok,
        blocked = watch_blocked,
        "✓ file watcher → automatic reload → new cap enforced"
    );

    // -----------------------------------------------------------------
    // Clean up
    // -----------------------------------------------------------------
    let _ = std::fs::remove_dir_all(&tmp);
    info!("✓ hot-reload simulation passed end-to-end");
    Ok(())
}
