//! Trusted workflow provenance propagation, separate from effect authorization.
use acteon_core::{ResourceKind, ResourceRef, WorkerTask, WorkflowExecution, WorkflowStatus};
use acteon_governance::context::{AcceptedEffect, VerifiedExecutionContext};
use sha2::{Digest, Sha256};

use crate::{Gateway, GatewayError};

/// Trusted admission adapters use this semantic digest before capturing a root.
/// Search attributes are observation metadata, not workflow input/authority.
pub fn workflow_context_digest(
    namespace: &str,
    tenant: &str,
    workflow: &str,
    queue: &str,
    input: &serde_json::Value,
) -> Result<String, GatewayError> {
    fn canonical(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<_, _> =
                    map.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(canonical).collect())
            }
            _ => value.clone(),
        }
    }
    let encoded = serde_json::to_vec(&canonical(&serde_json::json!({
        "format":"acteon-workflow-root:v1", "namespace":namespace, "tenant":tenant,
        "workflow":workflow, "queue":queue, "input":input,
    })))
    .map_err(|_| GatewayError::TaskQueue("invalid workflow context input".into()))?;
    Ok(hex::encode(Sha256::digest(encoded)))
}

pub fn workflow_context_effect(
    namespace: &str,
    tenant: &str,
    workflow: &str,
    queue: &str,
) -> Result<AcceptedEffect, GatewayError> {
    let resource = |kind, id: &str| {
        ResourceRef::new(kind, namespace, tenant, id)
            .map_err(|_| GatewayError::TaskQueue("invalid workflow context resource".into()))
    };
    Ok(AcceptedEffect {
        operation: "workflow.start".into(),
        resources: vec![
            resource(ResourceKind::Workflow, workflow)?,
            resource(ResourceKind::Queue, queue)?,
        ],
    })
}

impl Gateway {
    pub(crate) async fn verify_workflow_context(
        &self,
        exec: &WorkflowExecution,
    ) -> Result<Option<VerifiedExecutionContext>, GatewayError> {
        let Some(reference) = &exec.execution_context else {
            if self.workflow_context_store.is_some() {
                return Err(GatewayError::TaskQueue(
                    "workflow lacks required execution context; parked for migration".into(),
                ));
            }
            return Ok(None);
        };
        let store = self.workflow_context_store.as_ref().ok_or_else(|| {
            GatewayError::TaskQueue("workflow context verifier is not configured".into())
        })?;
        if reference.namespace() != exec.namespace
            || reference.tenant() != exec.tenant
            || reference.execution_id().to_string() != exec.execution_id
            || reference.request_digest()
                != workflow_context_digest(
                    &exec.namespace,
                    &exec.tenant,
                    &exec.workflow,
                    &exec.queue,
                    &exec.input,
                )?
        {
            return Err(GatewayError::TaskQueue(
                "workflow context ownership/input mismatch".into(),
            ));
        }
        let verified = store
            .recover_reference(reference, self.clock.now().timestamp_millis())
            .await
            .map_err(|e| {
                GatewayError::TaskQueue(format!("workflow context verification failed: {e}"))
            })?;
        if !verified.within_accepted_ceiling(&workflow_context_effect(
            &exec.namespace,
            &exec.tenant,
            &exec.workflow,
            &exec.queue,
        )?) {
            return Err(GatewayError::TaskQueue(
                "workflow is outside its accepted context ceiling".into(),
            ));
        }
        Ok(Some(verified))
    }

    pub(crate) async fn verify_worker_workflow_context(
        &self,
        task: &WorkerTask,
    ) -> Result<(), GatewayError> {
        let Some(execution_id) = &task.workflow_execution_id else {
            if task.execution_context.is_some() {
                return Err(GatewayError::TaskQueue(
                    "context-bearing standalone worker task is unsupported".into(),
                ));
            }
            return Ok(());
        };
        let exec = self
            .get_workflow_execution(&task.namespace, &task.tenant, execution_id)
            .await?;
        let Some(exec) = exec else {
            if task.execution_context.is_none() && self.workflow_context_store.is_none() {
                return Ok(());
            }
            return Err(GatewayError::TaskQueue(
                "workflow for context-bearing continuation is missing".into(),
            ));
        };
        // Preserve legacy behavior only when neither side has context and this
        // gateway has not enabled the provenance profile.
        if exec.execution_context.is_none()
            && task.execution_context.is_none()
            && self.workflow_context_store.is_none()
        {
            return Ok(());
        }
        if task.execution_context != exec.execution_context
            || exec.status != WorkflowStatus::Running
            || exec.current_task_id.as_deref() != Some(task.task_id.as_str())
            || task.queue != exec.queue
            || task.action_type != acteon_core::WORKFLOW_TASK_ACTION_TYPE
            || task.payload
                != serde_json::json!({"execution_id":exec.execution_id,"workflow":exec.workflow})
        {
            return Err(GatewayError::TaskQueue(
                "workflow continuation context/identity mismatch".into(),
            ));
        }
        self.verify_workflow_context(&exec).await?;
        Ok(())
    }
}
