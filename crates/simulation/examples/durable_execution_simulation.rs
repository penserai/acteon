//! Simulation of durable executions: timers, signals, worker queues, and
//! checkpoint-based workflows.
//!
//! Demonstrates:
//! 1. A chain that pauses on a durable timer, then resumes
//! 2. A chain that waits for an external signal (human approval)
//! 3. A chain step executed by an external worker via a task queue
//! 4. A workflow execution driven by a simulated worker: checkpointed
//!    steps, a durable sleep, a signal wait, and completion — with the
//!    full event history printed at the end
//!
//! Run with: `cargo run -p acteon-simulation --example durable_execution_simulation`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use acteon_core::chain::{
    ChainConfig, ChainStepConfig, SignalStepConfig, TimerStepConfig, WorkerStepConfig,
};
use acteon_core::{Action, ActionOutcome, ChainStatus, WorkflowDirective, WorkflowStatus};
use acteon_gateway::{Gateway, GatewayBuilder};
use acteon_rules::Rule;
use acteon_rules_yaml::YamlFrontend;
use acteon_simulation::prelude::*;
use acteon_state_memory::{MemoryDistributedLock, MemoryStateStore};
use tracing::info;

const NS: &str = "orders";
const TENANT: &str = "tenant-1";

const CHAIN_RULE: &str = r#"
rules:
  - name: order-pipeline
    priority: 1
    condition:
      field: action.action_type
      eq: "order_created"
    action:
      type: chain
      chain: order-chain
"#;

fn parse_rules(yaml: &str) -> Vec<Rule> {
    let frontend = YamlFrontend;
    acteon_rules::RuleFrontend::parse(&frontend, yaml).expect("failed to parse rules")
}

fn build_gateway(chain: ChainConfig) -> Gateway {
    let recorder = RecordingProvider::new("email");
    GatewayBuilder::new()
        .state(Arc::new(MemoryStateStore::new()))
        .lock(Arc::new(MemoryDistributedLock::new()))
        .rules(parse_rules(CHAIN_RULE))
        .provider(Arc::new(recorder))
        .chain(chain)
        .build()
        .expect("gateway should build")
}

async fn start_order_chain(gateway: &Gateway) -> String {
    let action = Action::new(
        NS,
        TENANT,
        "email",
        "order_created",
        serde_json::json!({"order_id": "ord-42", "email": "user@example.com"}),
    );
    match gateway.dispatch(action, None).await.unwrap() {
        ActionOutcome::ChainStarted { chain_id, .. } => chain_id,
        other => panic!("expected ChainStarted, got {other:?}"),
    }
}

async fn drive(gateway: &Gateway, chain_id: &str) {
    gateway.advance_chain(NS, TENANT, chain_id).await.unwrap();
}

async fn status(gateway: &Gateway, chain_id: &str) -> ChainStatus {
    gateway
        .get_chain_status(NS, TENANT, chain_id)
        .await
        .unwrap()
        .unwrap()
        .status
}

#[tokio::main]
#[allow(clippy::too_many_lines)]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    info!("==================================================================");
    info!("           ACTEON DURABLE EXECUTION SIMULATION");
    info!("==================================================================\n");

    // =========================================================================
    // DEMO 1: Durable timer step
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 1: DURABLE TIMER");
    info!("------------------------------------------------------------------\n");

    let chain = ChainConfig::new("order-chain")
        .with_step(ChainStepConfig::new_timer(
            "cooling-off",
            TimerStepConfig {
                duration_seconds: Some(1),
                until: None,
            },
        ))
        .with_step(ChainStepConfig::new(
            "confirm",
            "email",
            "send_email",
            serde_json::json!({"to": "{{origin.payload.email}}"}),
        ));
    let gateway = build_gateway(chain);
    let chain_id = start_order_chain(&gateway).await;

    drive(&gateway, &chain_id).await;
    info!(
        "after arming timer: status = {:?}",
        status(&gateway, &chain_id).await
    );
    tokio::time::sleep(Duration::from_millis(1200)).await;
    drive(&gateway, &chain_id).await; // timer fires
    drive(&gateway, &chain_id).await; // confirm step executes
    info!(
        "after timer fired:  status = {:?}\n",
        status(&gateway, &chain_id).await
    );

    // =========================================================================
    // DEMO 2: Wait-for-signal step (human approval)
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 2: WAIT FOR SIGNAL");
    info!("------------------------------------------------------------------\n");

    let chain = ChainConfig::new("order-chain")
        .with_step(ChainStepConfig::new_wait_for_signal(
            "wait-approval",
            SignalStepConfig {
                signal_name: "approved".into(),
                timeout_seconds: Some(3600),
                on_timeout: None,
            },
        ))
        .with_step(ChainStepConfig::new(
            "ship",
            "email",
            "send_email",
            serde_json::json!({"approver": "{{prev.body.approver}}"}),
        ));
    let gateway = build_gateway(chain);
    let chain_id = start_order_chain(&gateway).await;

    drive(&gateway, &chain_id).await;
    info!("waiting: status = {:?}", status(&gateway, &chain_id).await);

    gateway
        .signal_chain(
            NS,
            TENANT,
            &chain_id,
            "approved",
            serde_json::json!({"approver": "renzo"}),
        )
        .await?;
    drive(&gateway, &chain_id).await; // consume signal
    drive(&gateway, &chain_id).await; // ship step
    info!(
        "after signal: status = {:?}\n",
        status(&gateway, &chain_id).await
    );

    // =========================================================================
    // DEMO 3: Worker-queue chain step
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 3: EXTERNAL WORKER STEP");
    info!("------------------------------------------------------------------\n");

    let chain = ChainConfig::new("order-chain").with_step(ChainStepConfig::new_worker(
        "fulfill",
        WorkerStepConfig {
            queue: "fulfillment".into(),
            action_type: Some("pack_and_ship".into()),
            timeout_seconds: None,
            max_attempts: Some(3),
        },
        serde_json::json!({"order_id": "{{origin.payload.order_id}}"}),
    ));
    let gateway = build_gateway(chain);
    let chain_id = start_order_chain(&gateway).await;

    drive(&gateway, &chain_id).await;
    info!(
        "task enqueued: status = {:?}",
        status(&gateway, &chain_id).await
    );

    // Simulate an external worker: poll, execute, complete.
    let tasks = gateway
        .poll_worker_tasks(NS, TENANT, "fulfillment", 1, Some(60), Some("sim-worker"))
        .await?;
    let task = &tasks[0];
    info!(
        "worker leased task {} (action_type={}, payload={})",
        task.task_id, task.action_type, task.payload
    );
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::json!({"tracking": "TRACK-123"}),
        )
        .await?;
    info!(
        "worker completed: status = {:?}\n",
        status(&gateway, &chain_id).await
    );

    // =========================================================================
    // DEMO 4: Checkpoint-based workflow
    // =========================================================================
    info!("------------------------------------------------------------------");
    info!("  DEMO 4: WORKFLOW AS CODE (simulated worker)");
    info!("------------------------------------------------------------------\n");

    let gateway = build_gateway(
        ChainConfig::new("order-chain").with_step(ChainStepConfig::new(
            "noop",
            "email",
            "send_email",
            serde_json::json!({}),
        )),
    );
    let exec = gateway
        .start_workflow(
            NS,
            TENANT,
            "order-flow",
            "wf-queue",
            serde_json::json!({"order_id": "ord-42"}),
            HashMap::new(),
        )
        .await?;
    let exec_id = exec.execution_id.clone();
    info!("workflow started: {exec_id}");

    // Continuation 1: the "worker" records a step, then sleeps durably.
    let task = &gateway
        .poll_worker_tasks(NS, TENANT, "wf-queue", 1, Some(60), None)
        .await?[0];
    gateway
        .record_workflow_checkpoint(
            NS,
            TENANT,
            &exec_id,
            "step:charge#1",
            serde_json::json!({"charge_id": "ch_1"}),
        )
        .await?;
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::to_value(WorkflowDirective::Sleep {
                checkpoint: "sleep#1".into(),
                seconds: 1,
            })?,
        )
        .await?;
    info!("workflow sleeping (durable timer)...");

    tokio::time::sleep(Duration::from_millis(1200)).await;
    gateway.process_due_workflow_timers().await?;

    // Continuation 2: the worker fetches replayed checkpoints from the
    // execution record (continuation payloads are slim); await a signal.
    let task = &gateway
        .poll_worker_tasks(NS, TENANT, "wf-queue", 1, Some(60), None)
        .await?[0];
    let snapshot = gateway
        .get_workflow_execution(NS, TENANT, &exec_id)
        .await?
        .unwrap();
    info!(
        "continuation snapshot checkpoints: {}",
        snapshot.checkpoints.len()
    );
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::to_value(WorkflowDirective::AwaitSignal {
                checkpoint: "signal:approved#1".into(),
                name: "approved".into(),
                timeout_seconds: None,
            })?,
        )
        .await?;
    gateway
        .signal_workflow(NS, TENANT, &exec_id, "approved", serde_json::json!(true))
        .await?;

    // Continuation 3: complete.
    let task = &gateway
        .poll_worker_tasks(NS, TENANT, "wf-queue", 1, Some(60), None)
        .await?[0];
    gateway
        .complete_worker_task(
            NS,
            TENANT,
            &task.task_id,
            task.lease_token.as_deref().unwrap(),
            serde_json::to_value(WorkflowDirective::Complete {
                result: serde_json::json!({"status": "fulfilled"}),
            })?,
        )
        .await?;

    let exec = gateway
        .get_workflow_execution(NS, TENANT, &exec_id)
        .await?
        .unwrap();
    assert_eq!(exec.status, WorkflowStatus::Completed);
    info!("workflow completed with result: {:?}", exec.result);

    info!("\nfull event history:");
    let history = gateway.get_execution_history(NS, TENANT, &exec_id).await?;
    for event in &history.events {
        info!(
            "  #{} {}",
            event.event_id,
            serde_json::to_string(&event.event)?
        );
    }

    info!("\n==================================================================");
    info!("           SIMULATION COMPLETE");
    info!("==================================================================");
    Ok(())
}
