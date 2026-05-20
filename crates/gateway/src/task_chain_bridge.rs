//! A2A Task ↔ Acteon Chain bridge (Phase 2).
//!
//! An A2A Task can be **backed** by an Acteon Chain execution. When
//! the two are linked (via [`link_task_to_chain`]), the chain's
//! status projects onto the task's state: chain progress → `Working`,
//! terminal chain status → matching terminal task state. The
//! projection runs whenever the chain engine's `advance_chain` /
//! `cancel_chain` finishes, so a step that drives the chain to a
//! terminal status — or an explicit cancel — settles the task
//! immediately.
//!
//! Cancel propagates both ways: a chain cancel projects onto the
//! task via the hook here; an A2A `tasks/cancel` on a linked task
//! calls `Gateway::cancel_chain` (in the A2A handler), which then
//! projects back via the same hook.
//!
//! V1 projects **state only**. Step results are not folded into
//! `Task.history` / `Task.artifacts` — that data projection is a
//! deferred follow-up; for now the chain's results stay queryable
//! through the chain API.

use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::error;

use acteon_core::{ChainState, ChainStatus, TaskMessage, TaskRole, TaskState};
use acteon_state::{CasResult, KeyKind, StateError, StateKey, StateStore};

use crate::task_engine::{MAX_CAS_RETRY_ATTEMPTS, TaskEngine, TaskEngineError, TaskScope};

/// Project a chain's status onto the matching A2A [`TaskState`].
///
/// In-flight chain states ([`ChainStatus::Running`],
/// [`ChainStatus::WaitingSubChain`], [`ChainStatus::WaitingParallel`])
/// all map to [`TaskState::Working`] — the task is making progress;
/// the specific internal step is a chain concern. Terminal chain
/// states map to their A2A counterparts ([`ChainStatus::TimedOut`] is
/// reported as [`TaskState::Failed`], since A2A has no separate
/// timeout state).
#[must_use]
pub fn project_chain_status_to_task_state(status: &ChainStatus) -> TaskState {
    match status {
        ChainStatus::Running | ChainStatus::WaitingSubChain | ChainStatus::WaitingParallel => {
            TaskState::Working
        }
        ChainStatus::Completed => TaskState::Completed,
        ChainStatus::Failed | ChainStatus::TimedOut => TaskState::Failed,
        ChainStatus::Cancelled => TaskState::Canceled,
    }
}

/// Errors specific to the two-write Task↔Chain link.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error(transparent)]
    Engine(#[from] TaskEngineError),
    #[error(transparent)]
    State(#[from] StateError),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
    #[error("chain '{0}' not found")]
    ChainNotFound(String),
    #[error("CAS contention exhausted for chain '{0}'")]
    CasExhausted(String),
}

/// Link an existing Task to an existing Chain so chain status
/// projects onto the task. Writes `Task.chain_id` and
/// `ChainState.task_id` together (separate state keys, ordered: task
/// first, chain second). If the chain-side write fails the task-side
/// `chain_id` is rolled back best-effort so a failed link does not
/// leave a one-sided pointer.
pub async fn link_task_to_chain(
    state: &Arc<dyn StateStore>,
    engine: &TaskEngine,
    scope: &TaskScope,
    task_id: &str,
    chain_id: &str,
) -> Result<(), BridgeError> {
    engine
        .link_to_chain(scope, task_id, Some(chain_id.to_string()))
        .await?;
    if let Err(e) = set_chain_task_id(state, scope, chain_id, Some(task_id.to_string())).await {
        // Roll back Task.chain_id so a failed link doesn't leave a
        // dangling one-sided pointer (the bridge's chain→task
        // projection keys off `ChainState.task_id`, so a dangling
        // `Task.chain_id` is invisible to the auto-hook). Retry the
        // rollback a few times with backoff for transient failures;
        // if it permanently fails we log loudly so an operator can
        // unlink manually, and the stale-task reaper eventually
        // settles the task at its TTL as the ultimate backstop.
        let mut rollback_err: Option<TaskEngineError> = None;
        for attempt in 0u64..3 {
            match engine.link_to_chain(scope, task_id, None).await {
                Ok(_) => {
                    rollback_err = None;
                    break;
                }
                Err(rb) => {
                    rollback_err = Some(rb);
                    if attempt < 2 {
                        sleep(Duration::from_millis(50 * (attempt + 1))).await;
                    }
                }
            }
        }
        if let Some(rb) = rollback_err {
            error!(
                task_id = %task_id,
                chain_id = %chain_id,
                rollback_error = %rb,
                link_error = %e,
                "bridge: link_task_to_chain rollback failed — Task.chain_id is dangling; \
                 operator may want to manually unlink. The stale-task reaper will settle \
                 the task at its TTL as a backstop.",
            );
        }
        return Err(e);
    }
    Ok(())
}

/// CAS-set `ChainState.task_id` (None to unlink), retrying on contention.
async fn set_chain_task_id(
    state: &Arc<dyn StateStore>,
    scope: &TaskScope,
    chain_id: &str,
    task_id: Option<String>,
) -> Result<(), BridgeError> {
    let key = StateKey::new(
        scope.namespace.clone(),
        scope.tenant.clone(),
        KeyKind::Chain,
        chain_id,
    );
    for _ in 0..MAX_CAS_RETRY_ATTEMPTS {
        let Some((raw, version)) = state.get_versioned(&key).await? else {
            return Err(BridgeError::ChainNotFound(chain_id.to_string()));
        };
        let mut chain: ChainState = serde_json::from_str(&raw)?;
        chain.task_id.clone_from(&task_id);
        let payload = serde_json::to_string(&chain)?;
        match state
            .compare_and_swap(&key, version, &payload, None)
            .await?
        {
            CasResult::Ok => return Ok(()),
            // Lost the CAS race — re-read and try again.
            CasResult::Conflict { .. } => {}
        }
    }
    Err(BridgeError::CasExhausted(chain_id.to_string()))
}

/// Project the chain's current status onto its linked task, if any.
///
/// Best-effort and idempotent: a no-op when the chain has no linked
/// task, when the task is already at the projected state, or when the
/// task has already settled in a terminal state (the chain may catch
/// up after a task was independently canceled / failed). A chain
/// status whose projection is an illegal task transition surfaces as
/// [`TaskEngineError::Validation`] from `transition_task` — the
/// caller (the chain engine hook) treats it as best-effort.
pub async fn project_chain_to_linked_task(
    engine: &TaskEngine,
    chain_state: &ChainState,
) -> Result<(), TaskEngineError> {
    let Some(task_id) = chain_state.task_id.as_deref() else {
        return Ok(());
    };
    let scope = TaskScope::new(&chain_state.namespace, &chain_state.tenant);
    let Some(task) = engine.get_task(&scope, task_id).await? else {
        return Ok(());
    };
    let target = project_chain_status_to_task_state(&chain_state.status);
    if task.status.state == target || task.status.state.is_terminal() {
        return Ok(());
    }
    // Attach a brief synthetic agent message for terminal projections
    // so an A2A client sees *why* the task ended — "chain timed out",
    // the operator's cancel reason, and so on — instead of an opaque
    // Failed. In-flight projections (→ Working) carry no message;
    // Working is the steady state and a message every advance would
    // spam history.
    let message = target
        .is_terminal()
        .then(|| build_projection_message(target, chain_state));
    engine
        .transition_task(&scope, task_id, target, message)
        .await?;
    Ok(())
}

/// Synthesize a brief agent message describing the chain transition
/// that drove the projection. Surfaces the chain's cancel reason when
/// present and distinguishes a timeout from a generic failure so an
/// A2A client gets context the chain API would otherwise have to be
/// queried for.
fn build_projection_message(target: TaskState, chain: &ChainState) -> TaskMessage {
    let text = match target {
        TaskState::Canceled => match chain.cancel_reason.as_deref() {
            Some(reason) => format!("Backing chain '{}' canceled: {reason}", chain.chain_id),
            None => format!("Backing chain '{}' canceled", chain.chain_id),
        },
        TaskState::Failed if matches!(chain.status, ChainStatus::TimedOut) => {
            format!("Backing chain '{}' timed out", chain.chain_id)
        }
        TaskState::Failed => format!("Backing chain '{}' failed", chain.chain_id),
        TaskState::Completed => format!("Backing chain '{}' completed", chain.chain_id),
        // Unreachable in practice — caller gates on `is_terminal` —
        // but keep the match defensible.
        _ => format!(
            "Backing chain '{}' status: {:?}",
            chain.chain_id, chain.status
        ),
    };
    TaskMessage::text(uuid::Uuid::now_v7().to_string(), TaskRole::Agent, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use acteon_core::{Action, Task};
    use acteon_state_memory::MemoryStateStore;
    use chrono::Utc;
    use std::collections::HashMap;

    fn store() -> Arc<dyn StateStore> {
        Arc::new(MemoryStateStore::new())
    }

    fn engine(state: &Arc<dyn StateStore>) -> TaskEngine {
        TaskEngine::new(state.clone())
    }

    fn scope() -> TaskScope {
        TaskScope::new("agents", "demo")
    }

    fn sample_chain(chain_id: &str, status: ChainStatus) -> ChainState {
        let now = Utc::now();
        ChainState {
            chain_id: chain_id.into(),
            chain_name: "test-chain".into(),
            origin_action: Action::new(
                "agents",
                "demo",
                "provider",
                "action_type",
                serde_json::json!({}),
            ),
            current_step: 0,
            total_steps: 1,
            status,
            step_results: vec![None],
            started_at: now,
            updated_at: now,
            expires_at: None,
            namespace: "agents".into(),
            tenant: "demo".into(),
            cancel_reason: None,
            cancelled_by: None,
            execution_path: vec!["step1".into()],
            parent_chain_id: None,
            parent_step_index: None,
            child_chain_ids: Vec::new(),
            task_id: None,
            parallel_state: None,
            parallel_sub_results: HashMap::new(),
            step_attempts: vec![0],
            step_history: vec![Vec::new()],
        }
    }

    async fn put_chain(state: &Arc<dyn StateStore>, chain: &ChainState) {
        let key = StateKey::new(
            chain.namespace.clone(),
            chain.tenant.clone(),
            KeyKind::Chain,
            chain.chain_id.clone(),
        );
        let raw = serde_json::to_string(chain).unwrap();
        state.set(&key, &raw, None).await.unwrap();
    }

    async fn load_chain(state: &Arc<dyn StateStore>, chain_id: &str) -> ChainState {
        let key = StateKey::new("agents", "demo", KeyKind::Chain, chain_id);
        let raw = state.get(&key).await.unwrap().expect("chain row");
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn projection_in_flight_chain_states_map_to_working() {
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::Running),
            TaskState::Working
        );
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::WaitingSubChain),
            TaskState::Working
        );
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::WaitingParallel),
            TaskState::Working
        );
    }

    #[test]
    fn projection_terminal_mapping() {
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::Completed),
            TaskState::Completed,
        );
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::Failed),
            TaskState::Failed,
        );
        // Timeout has no A2A counterpart; surfaces as Failed.
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::TimedOut),
            TaskState::Failed,
        );
        assert_eq!(
            project_chain_status_to_task_state(&ChainStatus::Cancelled),
            TaskState::Canceled,
        );
    }

    #[tokio::test]
    async fn link_writes_both_sides() {
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        let chain = sample_chain("c1", ChainStatus::Running);
        put_chain(&state, &chain).await;

        link_task_to_chain(&state, &engine, &scope(), "t1", "c1")
            .await
            .unwrap();

        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.chain_id.as_deref(), Some("c1"));
        let chain = load_chain(&state, "c1").await;
        assert_eq!(chain.task_id.as_deref(), Some("t1"));
    }

    #[tokio::test]
    async fn link_rolls_back_task_when_chain_missing() {
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        // No chain row inserted — the chain-side write fails.
        let err = link_task_to_chain(&state, &engine, &scope(), "t1", "ghost")
            .await
            .unwrap_err();
        assert!(matches!(err, BridgeError::ChainNotFound(_)));
        // Task.chain_id was rolled back to None.
        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert!(task.chain_id.is_none());
    }

    #[tokio::test]
    async fn project_unlinked_chain_is_a_noop() {
        let state = store();
        let engine = engine(&state);
        let mut chain = sample_chain("c1", ChainStatus::Completed);
        chain.task_id = None;
        project_chain_to_linked_task(&engine, &chain).await.unwrap();
        // No task created; nothing observable to assert beyond a clean
        // Ok return.
    }

    #[tokio::test]
    async fn project_transitions_working_to_completed() {
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        // Move the task to Working (the only state from which the
        // engine allows the Working → Completed transition).
        engine
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let mut chain = sample_chain("c1", ChainStatus::Completed);
        chain.task_id = Some("t1".into());

        project_chain_to_linked_task(&engine, &chain).await.unwrap();

        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::Completed);
    }

    #[tokio::test]
    async fn project_is_noop_when_already_at_target() {
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        engine
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let mut chain = sample_chain("c1", ChainStatus::Running);
        chain.task_id = Some("t1".into());

        // Working == projection of Running — should not call transition.
        project_chain_to_linked_task(&engine, &chain).await.unwrap();
        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::Working);
    }

    #[tokio::test]
    async fn project_skips_already_terminal_task() {
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        // A2A allows Submitted → Canceled directly.
        engine
            .transition_task(&scope(), "t1", TaskState::Canceled, None)
            .await
            .unwrap();
        let mut chain = sample_chain("c1", ChainStatus::Failed);
        chain.task_id = Some("t1".into());

        // Task is already terminal — projection must NOT try to push
        // it to Failed (illegal transition).
        project_chain_to_linked_task(&engine, &chain).await.unwrap();
        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::Canceled);
    }

    /// Set up a Working task linked to a chain with the given status
    /// and an optional cancel reason, then project. Returns the
    /// post-projection task.
    async fn project_with_status_and_reason(
        status: ChainStatus,
        cancel_reason: Option<&str>,
    ) -> acteon_core::Task {
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        engine
            .transition_task(&scope(), "t1", TaskState::Working, None)
            .await
            .unwrap();
        let mut chain = sample_chain("c1", status);
        chain.task_id = Some("t1".into());
        chain.cancel_reason = cancel_reason.map(str::to_string);
        project_chain_to_linked_task(&engine, &chain).await.unwrap();
        engine.get_task(&scope(), "t1").await.unwrap().unwrap()
    }

    /// Extract the synthetic-message text the projection attaches to
    /// the task's status — the user-facing "why" of a terminal
    /// transition.
    fn projection_message_text(task: &acteon_core::Task) -> String {
        let msg = task
            .status
            .message
            .as_ref()
            .expect("terminal projection attaches a status message");
        msg.parts
            .first()
            .and_then(|p| p.text.clone())
            .expect("synthetic message carries a text part")
    }

    #[tokio::test]
    async fn project_failed_attaches_synthetic_message_naming_chain() {
        let task = project_with_status_and_reason(ChainStatus::Failed, None).await;
        assert_eq!(task.status.state, TaskState::Failed);
        let text = projection_message_text(&task);
        assert!(text.contains("c1"), "should name the chain; got {text:?}");
        assert!(text.contains("failed"), "should say 'failed'; got {text:?}");
    }

    #[tokio::test]
    async fn project_timed_out_is_distinguishable_from_failed() {
        let task = project_with_status_and_reason(ChainStatus::TimedOut, None).await;
        assert_eq!(task.status.state, TaskState::Failed);
        let text = projection_message_text(&task);
        // Timeout has no separate A2A state, but the message must let
        // a client tell timeout from generic failure.
        assert!(
            text.contains("timed out"),
            "timeout should be distinguishable; got {text:?}",
        );
    }

    #[tokio::test]
    async fn project_canceled_includes_cancel_reason_when_present() {
        let task =
            project_with_status_and_reason(ChainStatus::Cancelled, Some("operator requested"))
                .await;
        assert_eq!(task.status.state, TaskState::Canceled);
        let text = projection_message_text(&task);
        assert!(
            text.contains("operator requested"),
            "should surface cancel_reason; got {text:?}",
        );
    }

    #[tokio::test]
    async fn project_working_attaches_no_message() {
        // In-flight projections (→ Working) carry no message — Working
        // is the steady state.
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        let mut chain = sample_chain("c1", ChainStatus::Running);
        chain.task_id = Some("t1".into());
        project_chain_to_linked_task(&engine, &chain).await.unwrap();
        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::Working);
        assert!(task.status.message.is_none());
    }
}
