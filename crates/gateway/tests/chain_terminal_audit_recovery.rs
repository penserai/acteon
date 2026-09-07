//! Fault-injection coverage for terminal chain audit recovery.

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use acteon_audit::{AuditError, AuditPage, AuditQuery, AuditRecord, AuditStore};
use acteon_core::{
    Action, ActionOutcome,
    chain::{ChainConfig, ChainStepConfig, TimerStepConfig},
};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_rules::ir::{
    expr::Expr,
    rule::{Rule, RuleAction},
};
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use serde_json::json;

const NAMESPACE: &str = "ns";
const TENANT: &str = "tenant";

/// A store that can reject writes while preserving the durable records it has
/// already accepted. It models an audit backend returning after the terminal
/// chain state has committed.
struct ToggleAuditStore {
    unavailable: AtomicBool,
    records: Mutex<HashMap<String, AuditRecord>>,
}

impl ToggleAuditStore {
    fn new() -> Self {
        Self {
            unavailable: AtomicBool::new(false),
            records: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl AuditStore for ToggleAuditStore {
    async fn record(&self, entry: AuditRecord) -> Result<(), AuditError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(AuditError::Storage("injected audit outage".to_owned()));
        }
        self.records.lock().unwrap().insert(entry.id.clone(), entry);
        Ok(())
    }

    async fn get_by_action_id(&self, action_id: &str) -> Result<Option<AuditRecord>, AuditError> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .values()
            .find(|record| record.action_id == action_id)
            .cloned())
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<AuditRecord>, AuditError> {
        Ok(self.records.lock().unwrap().get(id).cloned())
    }

    async fn query(&self, _: &AuditQuery) -> Result<AuditPage, AuditError> {
        Ok(AuditPage {
            records: Vec::new(),
            total: Some(0),
            limit: 0,
            offset: 0,
            next_cursor: None,
        })
    }

    async fn cleanup_expired(&self) -> Result<u64, AuditError> {
        Ok(0)
    }
}

fn gateway(audit: Arc<dyn AuditStore>) -> Gateway {
    let chain = ChainConfig::new("flow").with_step(ChainStepConfig::new_timer(
        "wait",
        TimerStepConfig {
            duration_seconds: Some(60),
            until: None,
        },
    ));
    GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .audit(audit)
        .chain(chain)
        .rules(vec![Rule::new(
            "start",
            Expr::Bool(true),
            RuleAction::Chain {
                chain: "flow".to_owned(),
            },
        )])
        .build()
        .unwrap()
}

#[tokio::test]
async fn terminal_audit_replays_after_an_outage_with_one_stable_receipt() {
    let audit = Arc::new(ToggleAuditStore::new());
    let gateway = gateway(audit.clone());
    let outcome = gateway
        .dispatch(
            Action::new(NAMESPACE, TENANT, "source", "start", json!({})),
            None,
        )
        .await
        .unwrap();
    let ActionOutcome::ChainStarted { chain_id, .. } = outcome else {
        panic!("chain did not start");
    };

    audit.unavailable.store(true, Ordering::SeqCst);
    gateway
        .cancel_chain(
            NAMESPACE,
            TENANT,
            &chain_id,
            Some("operator request".into()),
            None,
        )
        .await
        .unwrap();
    let audit_id = format!("chain-terminal-{chain_id}");
    assert!(audit.get_by_id(&audit_id).await.unwrap().is_none());

    audit.unavailable.store(false, Ordering::SeqCst);
    assert_eq!(gateway.reconcile_chain_terminal_audits().await.unwrap(), 1);
    let record = audit
        .get_by_id(&audit_id)
        .await
        .unwrap()
        .expect("recovery writes the stable terminal receipt");
    assert_eq!(record.outcome, "chain_cancelled");
    assert_eq!(record.chain_id.as_deref(), Some(chain_id.as_str()));
    assert_eq!(gateway.reconcile_chain_terminal_audits().await.unwrap(), 0);
}
