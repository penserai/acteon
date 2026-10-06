//! Durable reference to independently verified authority provenance.
//! Deserializing this metadata never creates a verified context or permission.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{PrincipalIdentity, ResourceKind, ResourceRef};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(try_from = "ReferenceWire")]
pub struct ExecutionContextReference {
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    context_id: Uuid,
    #[cfg_attr(feature = "openapi", schema(value_type = String, format = "uuid"))]
    execution_id: Uuid,
    namespace: String,
    tenant: String,
    principal: PrincipalIdentity,
    request_digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceWire {
    context_id: Uuid,
    execution_id: Uuid,
    namespace: String,
    tenant: String,
    principal: PrincipalIdentity,
    request_digest: String,
}
#[derive(Debug, thiserror::Error)]
#[error("invalid execution context reference")]
pub struct ExecutionContextReferenceError;

impl TryFrom<ReferenceWire> for ExecutionContextReference {
    type Error = ExecutionContextReferenceError;
    fn try_from(wire: ReferenceWire) -> Result<Self, Self::Error> {
        Self::new(
            wire.context_id,
            wire.execution_id,
            wire.namespace,
            wire.tenant,
            wire.principal,
            wire.request_digest,
        )
    }
}
impl ExecutionContextReference {
    pub fn new(
        context_id: Uuid,
        execution_id: Uuid,
        namespace: String,
        tenant: String,
        principal: PrincipalIdentity,
        request_digest: String,
    ) -> Result<Self, ExecutionContextReferenceError> {
        if context_id.is_nil()
            || execution_id.is_nil()
            || request_digest.len() != 64
            || !request_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || ResourceRef::new(
                ResourceKind::Action,
                &namespace,
                &tenant,
                execution_id.to_string(),
            )
            .is_err()
        {
            return Err(ExecutionContextReferenceError);
        }
        Ok(Self {
            context_id,
            execution_id,
            namespace,
            tenant,
            principal,
            request_digest,
        })
    }
    #[must_use]
    pub const fn context_id(&self) -> Uuid {
        self.context_id
    }
    #[must_use]
    pub const fn execution_id(&self) -> Uuid {
        self.execution_id
    }
    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
    #[must_use]
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
    #[must_use]
    pub fn principal(&self) -> &PrincipalIdentity {
        &self.principal
    }
    #[must_use]
    pub fn request_digest(&self) -> &str {
        &self.request_digest
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn serialized_reference_validates_identity_without_becoming_authority() {
        let reference = ExecutionContextReference::new(
            Uuid::new_v4(),
            Uuid::new_v4(),
            "city".into(),
            "tenant".into(),
            PrincipalIdentity::new("actor", crate::PrincipalKind::Agent).unwrap(),
            "a".repeat(64),
        )
        .unwrap();
        let wire = serde_json::to_value(&reference).unwrap();
        assert_eq!(
            serde_json::from_value::<ExecutionContextReference>(wire.clone()).unwrap(),
            reference
        );
        for (field, value) in [
            ("context_id", serde_json::json!(Uuid::nil())),
            ("request_digest", serde_json::json!("untrusted")),
            ("tenant", serde_json::json!("*")),
            ("permit", serde_json::json!("admin")),
        ] {
            let mut changed = wire.clone();
            changed[field] = value;
            assert!(serde_json::from_value::<ExecutionContextReference>(changed).is_err());
        }
        let task = crate::WorkerTask::new("city", "tenant", "q", "a", serde_json::json!({}));
        assert!(
            serde_json::to_value(task)
                .unwrap()
                .get("execution_context")
                .is_none()
        );
        let workflow =
            crate::WorkflowExecution::new("city", "tenant", "wf", "q", serde_json::json!({}));
        assert!(
            serde_json::to_value(workflow)
                .unwrap()
                .get("execution_context")
                .is_none()
        );
    }
    #[test]
    fn queues_have_exact_resource_identity_separate_from_topics() {
        let queue = ResourceRef::new(ResourceKind::Queue, "city", "tenant", "diagnostic").unwrap();
        assert_eq!(queue.canonical().parse::<ResourceRef>().unwrap(), queue);
        assert_ne!(
            queue,
            ResourceRef::new(ResourceKind::Topic, "city", "tenant", "diagnostic").unwrap()
        );
    }
}
