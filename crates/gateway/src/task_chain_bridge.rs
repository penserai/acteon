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
//! On a **terminal** projection the chain's step results are folded into
//! the linked task: one `Task.artifacts` entry per step (and per parallel
//! sub-step), plus a summary `Task.history` message — so a federated A2A
//! caller gets the chain's output through the standard protocol instead of
//! seeing only an opaque `Completed`. Step bodies over the 256KB artifact
//! part cap are summarized in place; the full bodies stay queryable through
//! the chain API. This data projection is best-effort: a per-artifact
//! failure is logged and never blocks the task's terminal transition.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::time::sleep;
use tracing::{error, warn};

use acteon_core::{
    Artifact, ChainState, ChainStatus, MAX_ARTIFACTS_LEN, MAX_PART_DATA_BYTES, StepResult,
    TaskArtifactUpdateEvent, TaskMessage, TaskPart, TaskRole, TaskState,
};
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
    // Terminal projection: fold the chain's step results into the task as
    // artifacts + a summary history message *before* settling it, so an A2A
    // caller gets the results through the standard protocol. Best-effort —
    // enrichment failures are logged and never block the terminal transition
    // below (the full bodies remain queryable through the chain API). The
    // task is still `Working` here, so the gate above ensures this runs at
    // most once per task; the projection is also idempotent (stable artifact
    // ids replaced in place, summary message deduped) so a best-effort retry
    // after a failed transition converges without duplicates.
    if target.is_terminal() {
        project_step_results_to_task(engine, &scope, task_id, chain_state).await;
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

/// Max chars of a chain/step *label* (artifact name/description, summary
/// text) folded into a task — chain step names are operator-defined and
/// otherwise unbounded, so bound them to keep a task row from inflating.
const MAX_LABEL_CHARS: usize = 256;

/// Max chars of a step error string folded into an artifact part, kept well
/// under the 256KB part cap so a verbose error can't get the artifact dropped.
const MAX_ERROR_CHARS: usize = 8192;

/// Max chars of a name folded into an id, leaving headroom for the fixed
/// prefix under the id-length cap (120).
const MAX_ID_FRAGMENT_LEN: usize = 100;

/// Bound a human-readable label to `max` chars (UTF-8 safe), appending an
/// ellipsis when truncated.
fn truncate_label(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max).collect();
        t.push('…');
        t
    }
}

/// Fold an arbitrary string into the `[A-Za-z0-9._-]` charset A2A ids
/// require, bounded so a prefixed id stays under the cap. Non-conforming
/// chars become `_`; an empty result collapses to `_`. Used only for the
/// summary message id (keyed on the chain id); artifact ids use indices.
fn sanitize_id_fragment(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .take(MAX_ID_FRAGMENT_LEN)
        .collect();
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// The single data [`TaskPart`] for a step result: the JSON body when
/// present and within the 256KB part cap, a small truncation marker when
/// the body is too large, or a success/error summary when there is no body.
/// All embedded free text is length-bounded so the part itself never
/// exceeds the cap (which would drop the artifact).
fn step_result_part(result: &StepResult) -> TaskPart {
    match &result.response_body {
        Some(body) => {
            let bytes = serde_json::to_vec(body).map_or(usize::MAX, |v| v.len());
            if bytes <= MAX_PART_DATA_BYTES {
                TaskPart::data(body.clone())
            } else {
                TaskPart::data(serde_json::json!({
                    "truncated": true,
                    "originalBytes": bytes,
                    "step": truncate_label(&result.step_name, MAX_LABEL_CHARS),
                    "note": "step result exceeds the 256KB artifact part cap; query the chain API for the full body",
                }))
            }
        }
        None => TaskPart::data(serde_json::json!({
            "success": result.success,
            "error": result.error.as_deref().map(|e| truncate_label(e, MAX_ERROR_CHARS)),
        })),
    }
}

/// Build one [`Artifact`] from a step result. `artifact_id` must already
/// be a valid, stable, collision-free id; the (length-bounded) step name
/// rides the human-readable `name` field.
fn step_artifact(artifact_id: String, result: &StepResult) -> Artifact {
    let name = truncate_label(&result.step_name, MAX_LABEL_CHARS);
    let mut artifact = Artifact::new(artifact_id, vec![step_result_part(result)]);
    artifact.description = Some(format!(
        "Result of chain step '{name}' ({})",
        if result.success {
            "succeeded"
        } else {
            "failed"
        },
    ));
    artifact.name = Some(name);
    artifact
}

/// Build a one-shot, complete artifact event that is **idempotent** under
/// re-projection. `last_chunk` is left `false` so the artifact stream is
/// never closed: a re-applied event (`append = false`) is a clean replace
/// rather than an `ArtifactStreamClosed` error. These ids are owned solely
/// by this projection — nothing else streams to them — and the task's
/// terminal state is the real completion signal.
fn idempotent_artifact_event(task_id: &str, artifact: Artifact) -> TaskArtifactUpdateEvent {
    TaskArtifactUpdateEvent {
        task_id: task_id.to_owned(),
        context_id: None,
        artifact,
        append: false,
        last_chunk: false,
        chunk_index: None,
        total_chunks: None,
        metadata: HashMap::new(),
    }
}

/// Stable, dedup-friendly id for a chain's summary history message, so a
/// re-invoked best-effort projection appends it at most once.
fn chain_summary_message_id(chain_id: &str) -> String {
    format!("chain-summary-{}", sanitize_id_fragment(chain_id))
}

fn chain_status_word(status: &ChainStatus) -> &'static str {
    match status {
        ChainStatus::Completed => "completed",
        ChainStatus::Failed => "failed",
        ChainStatus::TimedOut => "timed out",
        ChainStatus::Cancelled => "canceled",
        ChainStatus::Running | ChainStatus::WaitingSubChain | ChainStatus::WaitingParallel => {
            "settled"
        }
    }
}

/// Summary history message: how the chain settled, how many step results it
/// produced, how many succeeded, and how many were actually recorded as
/// artifacts (`applied` may be < `total` when results exceed the artifact
/// cap or an individual apply fails) — plus where to find full bodies.
fn build_chain_summary_message(chain: &ChainState, applied: usize, total: usize) -> TaskMessage {
    let succeeded = chain
        .step_results
        .iter()
        .flatten()
        .filter(|r| r.success)
        .count()
        + chain
            .parallel_sub_results
            .values()
            .filter(|r| r.success)
            .count();
    let text = format!(
        "Backing chain '{}' {}: {total} step result(s), {succeeded} succeeded; recorded {applied} \
         as task artifact(s). Each artifact carries a step's output (bodies over 256KB are \
         summarized); query the chain API for full bodies.",
        truncate_label(&chain.chain_name, MAX_LABEL_CHARS),
        chain_status_word(&chain.status),
    );
    TaskMessage::text(
        chain_summary_message_id(&chain.chain_id),
        TaskRole::Agent,
        text,
    )
}

/// Project a terminal chain's step results onto the linked task as
/// artifacts plus a summary history message.
///
/// Best-effort by design: each artifact (and the summary) is a separate
/// CAS mutation on the task row, applied independently; a failure — CAS
/// contention or an oversized part — is logged and skipped rather than
/// propagated, so it never blocks the caller's terminal transition.
/// Sequential steps project in chain order; parallel sub-steps follow,
/// sorted by name for determinism, each keyed by a collision-free index.
/// The artifact set is capped at [`MAX_ARTIFACTS_LEN`].
///
/// Cost note: this is up to `min(step_count, MAX_ARTIFACTS_LEN)` sequential
/// CAS round-trips on the one task row. A client polling `tasks/get` while a
/// chain-backed task is still `Working` may observe artifacts appear
/// incrementally before the task settles; the terminal state is the
/// authoritative "complete" signal.
async fn project_step_results_to_task(
    engine: &TaskEngine,
    scope: &TaskScope,
    task_id: &str,
    chain: &ChainState,
) {
    let mut artifacts: Vec<Artifact> = Vec::new();
    for (i, slot) in chain.step_results.iter().enumerate() {
        if let Some(result) = slot {
            artifacts.push(step_artifact(format!("step-{i}"), result));
        }
    }
    // Parallel sub-results are a name-keyed map; sort for a deterministic
    // order and key each artifact by its position so distinct names can
    // never collide onto one id (the human name rides `Artifact.name`).
    let mut sub: Vec<(&String, &StepResult)> = chain.parallel_sub_results.iter().collect();
    sub.sort_by(|a, b| a.0.cmp(b.0));
    for (i, (_name, result)) in sub.iter().enumerate() {
        artifacts.push(step_artifact(format!("substep-{i}"), result));
    }

    let total = artifacts.len();
    if total > MAX_ARTIFACTS_LEN {
        warn!(
            chain_id = %chain.chain_id,
            task_id,
            total,
            cap = MAX_ARTIFACTS_LEN,
            "a2a: chain produced more step results than the task artifact cap; projecting a prefix"
        );
        artifacts.truncate(MAX_ARTIFACTS_LEN);
    }

    let mut applied = 0usize;
    for artifact in artifacts {
        match engine
            .apply_artifact_update(scope, idempotent_artifact_event(task_id, artifact))
            .await
        {
            Ok(_) => applied += 1,
            Err(e) => warn!(
                chain_id = %chain.chain_id,
                task_id,
                error = %e,
                "a2a: skipped projecting a chain step result artifact"
            ),
        }
    }

    let summary = build_chain_summary_message(chain, applied, total);
    if let Err(e) = engine.append_history(scope, task_id, summary).await {
        warn!(
            chain_id = %chain.chain_id,
            task_id,
            error = %e,
            "a2a: failed to append chain summary to task history"
        );
    }
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
            caller: None,
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

    // ---- step-result projection (artifacts + summary history) -----------

    fn step_result(name: &str, success: bool, body: Option<serde_json::Value>) -> StepResult {
        StepResult::new(
            name,
            success,
            body,
            (!success).then(|| format!("{name} failed")),
            Utc::now(),
        )
    }

    /// Create a Working task `t1` linked to a Completed chain `c1` whose
    /// step_results / parallel_sub_results are set by `prep`, project, and
    /// return the post-projection task.
    async fn project_completed_chain(
        prep: impl FnOnce(&mut ChainState),
    ) -> (Arc<dyn StateStore>, TaskEngine, acteon_core::Task) {
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
        let mut chain = sample_chain("c1", ChainStatus::Completed);
        chain.task_id = Some("t1".into());
        prep(&mut chain);
        project_chain_to_linked_task(&engine, &chain).await.unwrap();
        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        (state, engine, task)
    }

    #[tokio::test]
    async fn terminal_projection_folds_step_results_into_artifacts() {
        let (_state, _engine, task) = project_completed_chain(|chain| {
            chain.total_steps = 2;
            chain.current_step = 2;
            chain.step_results = vec![
                Some(step_result(
                    "validate",
                    true,
                    Some(serde_json::json!({"ok": true})),
                )),
                Some(step_result(
                    "charge",
                    true,
                    Some(serde_json::json!({"txn": "abc"})),
                )),
            ];
        })
        .await;

        assert_eq!(task.status.state, TaskState::Completed);
        // One artifact per step, keyed by stable index, in chain order.
        assert_eq!(task.artifacts.len(), 2);
        assert_eq!(task.artifacts[0].artifact_id, "step-0");
        assert_eq!(task.artifacts[0].name.as_deref(), Some("validate"));
        assert_eq!(
            task.artifacts[0].parts[0].data,
            Some(serde_json::json!({"ok": true})),
        );
        assert_eq!(task.artifacts[1].artifact_id, "step-1");
        assert_eq!(
            task.artifacts[1].parts[0].data,
            Some(serde_json::json!({"txn": "abc"})),
        );
        // A summary history message names the chain and reports counts.
        let summary = task
            .history
            .iter()
            .find(|m| m.parts.iter().any(|p| p.text.is_some()))
            .expect("summary history message");
        let text = summary.parts[0].text.as_deref().unwrap();
        assert!(text.contains("test-chain"), "{text}");
        assert!(text.contains("2 step result"), "{text}");
        assert!(text.contains("recorded 2"), "{text}");
    }

    #[tokio::test]
    async fn parallel_sub_results_project_with_collision_free_ids() {
        let (_state, _engine, task) = project_completed_chain(|chain| {
            // Two names that would COLLIDE under naive sanitization
            // ("a/b" and "a_b" both → "a_b"); index-keyed ids keep them
            // distinct.
            chain.parallel_sub_results.insert(
                "a/b".into(),
                step_result("a/b", true, Some(serde_json::json!(1))),
            );
            chain.parallel_sub_results.insert(
                "a_b".into(),
                step_result("a_b", true, Some(serde_json::json!(2))),
            );
        })
        .await;

        // sample_chain's lone sequential slot is None; the two sub-results
        // project under distinct index-keyed ids (sorted by name: "a/b" < "a_b").
        let ids: Vec<&str> = task
            .artifacts
            .iter()
            .map(|a| a.artifact_id.as_str())
            .collect();
        assert_eq!(ids, vec!["substep-0", "substep-1"], "ids must not collide");
        let names: Vec<Option<&str>> = task.artifacts.iter().map(|a| a.name.as_deref()).collect();
        assert_eq!(
            names,
            vec![Some("a/b"), Some("a_b")],
            "both names preserved"
        );
    }

    #[tokio::test]
    async fn oversized_step_body_is_truncated_not_rejected() {
        let big = serde_json::json!({ "blob": "x".repeat(MAX_PART_DATA_BYTES + 1) });
        let (_state, _engine, task) = project_completed_chain(move |chain| {
            chain.step_results = vec![Some(step_result("huge", true, Some(big)))];
        })
        .await;

        // The task still settled, and the oversized body was replaced by a
        // small truncation marker rather than rejected by the part cap.
        assert_eq!(task.status.state, TaskState::Completed);
        assert_eq!(task.artifacts.len(), 1);
        let data = task.artifacts[0].parts[0].data.as_ref().unwrap();
        assert_eq!(data.get("truncated"), Some(&serde_json::json!(true)));
        assert!(data.get("originalBytes").is_some());
    }

    #[tokio::test]
    async fn failed_step_without_body_records_error() {
        let (_state, _engine, task) = project_completed_chain(|chain| {
            chain.step_results = vec![Some(step_result("charge", false, None))];
        })
        .await;

        let data = task.artifacts[0].parts[0].data.as_ref().unwrap();
        assert_eq!(data.get("success"), Some(&serde_json::json!(false)));
        assert_eq!(data.get("error"), Some(&serde_json::json!("charge failed")));
    }

    #[tokio::test]
    async fn unbounded_step_name_is_length_bounded_in_artifact() {
        let huge_name = "n".repeat(MAX_LABEL_CHARS * 4);
        let (_state, _engine, task) = project_completed_chain(move |chain| {
            chain.step_results = vec![Some(step_result(
                &huge_name,
                true,
                Some(serde_json::json!(1)),
            ))];
        })
        .await;

        // The artifact still applied, and the name is bounded (not the raw
        // 4×-cap string) so it can't inflate the task row.
        assert_eq!(task.artifacts.len(), 1);
        let name = task.artifacts[0].name.as_deref().unwrap();
        assert!(
            name.chars().count() <= MAX_LABEL_CHARS + 1,
            "len {}",
            name.chars().count()
        );
    }

    #[tokio::test]
    async fn in_flight_projection_attaches_no_artifacts() {
        // A Running → Working projection must not project step results;
        // folding is terminal-only.
        let state = store();
        let engine = engine(&state);
        engine
            .create_task(Task::new("t1", "agents", "demo"))
            .await
            .unwrap();
        let mut chain = sample_chain("c1", ChainStatus::Running);
        chain.task_id = Some("t1".into());
        chain.step_results = vec![Some(step_result(
            "validate",
            true,
            Some(serde_json::json!(1)),
        ))];
        project_chain_to_linked_task(&engine, &chain).await.unwrap();

        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.status.state, TaskState::Working);
        assert!(task.artifacts.is_empty());
        assert!(task.history.is_empty());
    }

    #[tokio::test]
    async fn reprojection_is_idempotent_and_replaces_in_place() {
        // A direct re-invocation (e.g. a best-effort retry after a
        // transition that failed mid-way) must not duplicate or error, and
        // must leave the artifact present with up-to-date content — proving
        // the non-closing event is a clean idempotent replace.
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
        let mut chain = sample_chain("c1", ChainStatus::Completed);
        chain.task_id = Some("t1".into());
        chain.step_results = vec![Some(step_result(
            "validate",
            true,
            Some(serde_json::json!(1)),
        ))];

        project_step_results_to_task(&engine, &scope(), "t1", &chain).await;
        // Re-run with an updated body — the replace must take effect.
        chain.step_results = vec![Some(step_result(
            "validate",
            true,
            Some(serde_json::json!(2)),
        ))];
        project_step_results_to_task(&engine, &scope(), "t1", &chain).await;

        let task = engine.get_task(&scope(), "t1").await.unwrap().unwrap();
        assert_eq!(task.artifacts.len(), 1, "artifact not duplicated");
        assert_eq!(task.artifacts[0].artifact_id, "step-0");
        assert_eq!(
            task.artifacts[0].parts[0].data,
            Some(serde_json::json!(2)),
            "re-projection replaced the artifact in place",
        );
        assert_eq!(task.history.len(), 1, "summary message deduped");
    }
}
