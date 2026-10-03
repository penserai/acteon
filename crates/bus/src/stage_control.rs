//! Operator commands are committed with their immutable audit and fencing change.
use super::{
    CAS_RETRIES, DateTime, Deserialize, ManagedStageState, Serialize, StreamCheckpointError,
    StreamStageError, StreamStageOperator, Utc, Uuid,
};
use std::collections::BTreeSet;

// Never evict IDs: an old retry must not undo a newer command.
const MAX_CONTROLS: usize = 1000;
const MAX_CONTROL_BYTES: usize = 16 * 1024 * 1024;
// Reserve one command and the worst-case escaped JSON audit for a resume.
const RESUME_RESERVE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamStageCommand {
    Halt,
    Resume,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStageControlRequest {
    pub request_id: Uuid,
    pub command: StreamStageCommand,
    pub expected_control_revision: u64,
    pub reason: String,
    /// Explicitly grant a new normal-input attempt budget; retain its source anchor.
    #[serde(default)]
    pub reset_retry_budget: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStageControlAudit {
    pub request: StreamStageControlRequest,
    pub actor: String,
    pub reason: String,
    pub applied_at: DateTime<Utc>,
    pub control_revision: u64,
    pub checkpoint_generation: u64,
    pub previous_operator_halted: bool,
    pub previous_retry_attempts: Option<u32>,
    pub previous_retry_terminal: bool,
    pub lease_fenced: bool,
}

impl ManagedStageState {
    pub(super) fn operator_halted(&self) -> bool {
        self.controls
            .last()
            .is_some_and(|a| a.request.command == StreamStageCommand::Halt)
    }
    pub(super) fn control_revision(&self) -> u64 {
        self.controls.len() as u64
    }
    pub(super) fn validate_controls(&self) -> Result<(), String> {
        if self.controls.len() > MAX_CONTROLS
            || serde_json::to_vec(&self.controls)
                .map_err(|e| e.to_string())?
                .len()
                > MAX_CONTROL_BYTES
        {
            return Err("control audit retention capacity exceeded".into());
        }
        let mut ids = BTreeSet::new();
        for (i, a) in self.controls.iter().enumerate() {
            if a.request.request_id.is_nil()
                || !ids.insert(a.request.request_id)
                || a.actor.trim().is_empty()
                || a.actor.len() > 256
                || a.reason.trim().is_empty()
                || a.reason.len() > 4096
                || a.reason != a.request.reason
                || a.request.expected_control_revision != i as u64
                || a.control_revision != i as u64 + 1
                || (a.request.reset_retry_budget && a.request.command != StreamStageCommand::Resume)
                || a.previous_operator_halted
                    != (i > 0 && self.controls[i - 1].request.command == StreamStageCommand::Halt)
            {
                return Err("invalid stage control audit".into());
            }
        }
        if self.operator_halted() && self.lease.is_some() {
            return Err("operator halted stage cannot hold a lease".into());
        }
        if self.retry.as_ref().is_some_and(|r| r.reset_budget)
            && !self.controls.iter().any(|a| a.request.reset_retry_budget)
        {
            return Err("retry budget reset requires an audit".into());
        }
        Ok(())
    }
}

impl StreamStageOperator {
    pub fn control_audits(&self) -> &[StreamStageControlAudit] {
        &self
            .coordinator
            .snapshot
            .processing
            .as_ref()
            .unwrap()
            .controls
    }
    /// Request IDs bind actor and command. Check idempotency before revision so a
    /// lost response can be recovered even after another operator command.
    pub async fn control(
        &mut self,
        actor: &str,
        request: &StreamStageControlRequest,
    ) -> Result<StreamStageControlAudit, StreamStageError> {
        if actor.trim().is_empty()
            || actor.len() > 256
            || request.request_id.is_nil()
            || request.reason.trim().is_empty()
            || request.reason.len() > 4096
            || (request.reset_retry_budget && request.command != StreamStageCommand::Resume)
        {
            return Err(StreamStageError::InvalidConfig(
                "invalid control identity, reason or retry reset".into(),
            ));
        }
        for _ in 0..CAS_RETRIES {
            self.coordinator.reload().await?;
            let mut next = self.coordinator.snapshot.clone();
            let m = next
                .processing
                .as_mut()
                .ok_or(StreamStageError::MissingManagedState)?;
            if let Some(a) = m
                .controls
                .iter()
                .find(|a| a.request.request_id == request.request_id)
            {
                return if a.actor == actor && &a.request == request {
                    Ok(a.clone())
                } else {
                    Err(StreamStageError::ControlConflict)
                };
            }
            if m.control_revision() != request.expected_control_revision {
                return Err(StreamStageError::ControlConflict);
            }
            let audit = StreamStageControlAudit {
                request: request.clone(),
                actor: actor.into(),
                reason: request.reason.clone(),
                applied_at: Utc::now(),
                control_revision: m.control_revision() + 1,
                checkpoint_generation: next.generation,
                previous_operator_halted: m.operator_halted(),
                previous_retry_attempts: m.retry.as_ref().map(|r| r.attempts),
                previous_retry_terminal: m.retry.as_ref().is_some_and(|r| r.terminal),
                lease_fenced: m.lease.is_some(),
            };
            m.controls.push(audit.clone());
            let control_bytes = serde_json::to_vec(&m.controls)
                .map_err(|e| StreamStageError::InvalidConfig(e.to_string()))?
                .len();
            if request.command == StreamStageCommand::Halt
                && (m.controls.len() >= MAX_CONTROLS
                    || control_bytes > MAX_CONTROL_BYTES - RESUME_RESERVE_BYTES)
            {
                return Err(StreamStageError::ControlCapacity);
            }
            if m.controls.len() > MAX_CONTROLS || control_bytes > MAX_CONTROL_BYTES {
                return Err(StreamStageError::ControlCapacity);
            }
            m.lease = None;
            m.revision = m
                .revision
                .checked_add(1)
                .ok_or(StreamStageError::ControlConflict)?;
            if request.reset_retry_budget
                && let Some(r) = &mut m.retry
            {
                r.reset_budget = true;
                r.terminal = false;
                r.next_attempt_at = None;
            }
            match self.coordinator.persist(next).await {
                Err(StreamCheckpointError::Conflict { .. }) => continue,
                result => result?,
            }
            return Ok(audit);
        }
        Err(StreamStageError::ControlConflict)
    }
}
