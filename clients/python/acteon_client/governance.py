"""Typed governance management. Public values never establish caller authority."""

from dataclasses import asdict, dataclass, field
from typing import TYPE_CHECKING, Any, Literal, cast

from .models import ActionOutcome, PrincipalIdentity
from .platform_catalog import PlatformOperation


@dataclass
class GovernanceResource:
    kind: str
    namespace: str
    tenant: str
    id: str


@dataclass
class GovernanceLimits:
    max_units: int
    max_concurrent: int
    deadline_ms: int


@dataclass
class GovernanceRoute:
    provider: str
    action_type: str


@dataclass
class GovernanceEffect:
    operation: str
    resources: list[GovernanceResource]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GovernanceEffect":
        return cls(data["operation"], [GovernanceResource(**r) for r in data["resources"]])


@dataclass
class GovernancePermitDeclaration:
    id: str
    revision: int
    subject: PrincipalIdentity
    routes: list[GovernanceRoute]
    valid_from_ms: int
    limits: GovernanceLimits


@dataclass
class PublishGovernancePermitRequest:
    namespace: str
    tenant: str
    change_id: str
    expected_revision: int
    permit: GovernancePermitDeclaration
    reason: str


@dataclass
class FederationGrantCeilingDeclaration:
    source: PrincipalIdentity
    target: PrincipalIdentity
    agent_resource: GovernanceResource
    binding_digest: str
    skill: str
    ingress_effect: GovernanceEffect
    effects: list[GovernanceEffect]
    limits: GovernanceLimits
    max_depth: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationGrantCeilingDeclaration":
        return cls(
            PrincipalIdentity(**data["source"]),
            PrincipalIdentity(**data["target"]),
            GovernanceResource(**data["agent_resource"]),
            data["binding_digest"],
            data["skill"],
            GovernanceEffect.from_dict(data["ingress_effect"]),
            [GovernanceEffect.from_dict(effect) for effect in data["effects"]],
            GovernanceLimits(**data["limits"]),
            data["max_depth"],
        )


@dataclass
class FederationTrustDeclaration:
    id: str
    revision: int
    foreign_domain: str
    local_audience: str
    key_id: str
    verifying_key: str
    minimum_issuer_epoch: int
    max_revocation_staleness_ms: int
    max_clock_skew_ms: int
    valid_from_ms: int
    deadline_ms: int
    approved: list[FederationGrantCeilingDeclaration]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationTrustDeclaration":
        return cls(
            **{
                **data,
                "approved": [
                    FederationGrantCeilingDeclaration.from_dict(shape) for shape in data["approved"]
                ],
            }
        )


@dataclass
class PublishFederationTrustRequest:
    namespace: str
    tenant: str
    change_id: str
    expected_revision: int
    trust: FederationTrustDeclaration
    reason: str


@dataclass
class RevokeFederationTrustRequest:
    namespace: str
    tenant: str
    change_id: str
    trust_id: str
    expected_revision: int
    reason: str


@dataclass
class SignedFederationEnvelopeDeclaration:
    payload: str
    signature: str


@dataclass
class SignedFederationCapabilityCard:
    payload: str
    signature: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "SignedFederationCapabilityCard":
        if (
            not isinstance(data.get("payload"), str)
            or not data["payload"]
            or not isinstance(data.get("signature"), str)
            or len(data["signature"]) != 128
        ):
            raise ValueError("invalid signed federation capability card")
        return cls(data["payload"], data["signature"])


@dataclass
class SignedFederationInvocationDeclaration:
    payload: str
    signature: str


@dataclass
class FederationImportReceipt:
    namespace: str
    tenant: str
    import_id: str
    envelope_digest: str
    trust_id: str
    trust_revision: int
    issuer_domain: str
    source: PrincipalIdentity
    target: PrincipalIdentity
    agent_resource: GovernanceResource
    binding_digest: str
    skill: str
    root_id: str
    expires_at_ms: int
    accepted_at_ms: int
    execution_started: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationImportReceipt":
        return cls(
            **{
                **data,
                "source": PrincipalIdentity(**data["source"]),
                "target": PrincipalIdentity(**data["target"]),
                "agent_resource": GovernanceResource(**data["agent_resource"]),
            }
        )


@dataclass
class FederationExportReceipt:
    attempt_id: str
    attempt_status: Literal["in_flight", "settled", "uncertain"]
    delivery_id: str
    delivery: dict[str, Any]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationExportReceipt":
        status = data.get("attempt_status")
        delivery = data.get("delivery")
        if (
            not isinstance(data.get("attempt_id"), str)
            or not data["attempt_id"]
            or status not in {"in_flight", "settled", "uncertain"}
            or not isinstance(data.get("delivery_id"), str)
            or not data["delivery_id"]
            or not isinstance(delivery, dict)
            or delivery.get("status")
            not in {
                "registered",
                "import_uncertain",
                "imported",
                "invoke_uncertain",
                "accepted",
                "rejected",
                "abandoned",
            }
        ):
            raise ValueError("invalid federation export receipt")
        return cls(data["attempt_id"], status, data["delivery_id"], delivery)


@dataclass
class FederationCapabilityEvidence:
    card_digest: str
    key_id: str
    issuer_epoch: int
    expires_at_ms: int
    protocol_revision: str
    authentication_scheme: str
    credential_revision: str


@dataclass
class FederationDeliveryEvidence:
    delivery_id: str
    parent_execution_id: str
    source: PrincipalIdentity
    agent_id: str
    message_id: str
    trust_id: str
    trust_revision: int
    key_id: str
    issuer_epoch: int
    created_at_ms: int
    expires_at_ms: int
    expired: bool
    exact_replay_idempotent: bool
    capability: FederationCapabilityEvidence | None
    status: dict[str, Any]
    reconciliations: list["FederationDeliveryFinalityEvidence"]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationDeliveryEvidence":
        if (
            not isinstance(data.get("delivery_id"), str)
            or not data["delivery_id"]
            or not isinstance(data.get("parent_execution_id"), str)
            or not data["parent_execution_id"]
            or not isinstance(data.get("status"), dict)
        ):
            raise ValueError("invalid federation delivery evidence")
        reconciliations = data.get("reconciliations")
        if not isinstance(reconciliations, list):
            raise ValueError("invalid federation delivery reconciliation evidence")
        return cls(
            **{
                **data,
                "source": PrincipalIdentity(**data["source"]),
                "capability": (
                    FederationCapabilityEvidence(**data["capability"])
                    if isinstance(data.get("capability"), dict)
                    else None
                ),
                "reconciliations": [
                    FederationDeliveryFinalityEvidence.from_dict(item)
                    for item in reconciliations
                ],
            }
        )


@dataclass
class FederationDeliveryFinalityEvidence:
    stage: Literal["import", "invoke"]
    accepted: bool
    source_id: str
    qualification_ref: str
    verifier_revision: str
    proof_digest: str
    operator: PrincipalIdentity
    authority_incarnation: str
    authority_generation: int
    accepted_at_ms: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationDeliveryFinalityEvidence":
        if (
            data.get("stage") not in {"import", "invoke"}
            or not isinstance(data.get("accepted"), bool)
            or not isinstance(data.get("proof_digest"), str)
            or len(data["proof_digest"]) != 64
        ):
            raise ValueError("invalid federation delivery finality evidence")
        return cls(**{**data, "operator": PrincipalIdentity(**data["operator"])})


@dataclass
class FederationFinalityCorrelation:
    delivery_id: str
    stage: Literal["import", "invoke"]
    parent_execution_id: str
    source: PrincipalIdentity
    adapter_revision: str
    binding_digest: str
    envelope_digest: str
    invocation_digest: str
    import_id: str
    invocation_nonce: str
    agent_id: str
    message_id: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationFinalityCorrelation":
        if data.get("stage") not in {"import", "invoke"}:
            raise ValueError("invalid federation finality correlation")
        return cls(**{**data, "source": PrincipalIdentity(**data["source"])})


@dataclass
class FederationDeliveryEvidencePage:
    deliveries: list[FederationDeliveryEvidence]
    next_cursor: str | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationDeliveryEvidencePage":
        deliveries = data.get("deliveries")
        if not isinstance(deliveries, list):
            raise ValueError("invalid federation delivery evidence page")
        return cls(
            [FederationDeliveryEvidence.from_dict(item) for item in deliveries],
            data.get("next_cursor"),
        )


@dataclass
class AbandonFederationDeliveryRequest:
    namespace: str
    tenant: str
    disposition_id: str
    reason: str


@dataclass
class ReconcileFederationDeliveryRequest:
    namespace: str
    tenant: str
    proof_base64: str


@dataclass
class FederationTrustView:
    trust: FederationTrustDeclaration
    revoked: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "FederationTrustView":
        return cls(FederationTrustDeclaration.from_dict(data["trust"]), data["revoked"])


@dataclass
class GovernanceResourceChange:
    kind: Literal["close_resource", "reopen_resource"]
    resource: GovernanceResource


@dataclass
class GovernanceSubjectRevocation:
    subject: PrincipalIdentity
    kind: Literal["revoke_subject"] = field(default="revoke_subject", init=False)


@dataclass
class GovernancePermitRevocation:
    permit_id: str
    expected_revision: int
    kind: Literal["revoke_permit"] = field(default="revoke_permit", init=False)


@dataclass
class GovernanceCredentialRevocation:
    credential_id: str
    expected_revision: int
    kind: Literal["revoke_credential"] = field(default="revoke_credential", init=False)


GovernanceIntervention = (
    GovernanceResourceChange
    | GovernanceSubjectRevocation
    | GovernancePermitRevocation
    | GovernanceCredentialRevocation
)


@dataclass
class GovernanceInterventionRequest:
    namespace: str
    tenant: str
    change_id: str
    change: GovernanceIntervention
    reason: str


@dataclass
class GovernanceChangeReceipt:
    namespace: str
    tenant: str
    change_id: str
    actor: str
    reason: str
    generation: int
    pending: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GovernanceChangeReceipt":
        return cls(**data)


@dataclass
class GovernancePermitView:
    id: str
    revision: int
    subject: PrincipalIdentity
    effects: list[GovernanceEffect]
    valid_from_ms: int
    limits: GovernanceLimits
    revoked: bool

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GovernancePermitView":
        return cls(
            data["id"],
            data["revision"],
            PrincipalIdentity(**data["subject"]),
            [GovernanceEffect.from_dict(e) for e in data["effects"]],
            data["valid_from_ms"],
            GovernanceLimits(**data["limits"]),
            data["revoked"],
        )


@dataclass
class GovernanceRouteView:
    route: GovernanceRoute
    effect: GovernanceEffect
    closed: bool


@dataclass
class GovernanceManagementBounds:
    subjects: list[PrincipalIdentity]
    can_issue_permits: bool
    can_intervene: bool
    valid_from_ms: int
    limits: GovernanceLimits
    can_read_history: bool = False
    can_reconcile: bool = False
    can_manage_federation: bool = False

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GovernanceManagementBounds":
        return cls(
            [PrincipalIdentity(**s) for s in data["subjects"]],
            data["can_issue_permits"],
            data["can_intervene"],
            data["valid_from_ms"],
            GovernanceLimits(**data["limits"]),
            data.get("can_read_history", False),
            data.get("can_reconcile", False),
            data.get("can_manage_federation", False),
        )


@dataclass
class GovernanceScopeView:
    management: GovernanceManagementBounds
    namespace: str
    tenant: str
    incarnation: str
    generation: int
    routes: list[GovernanceRouteView]
    permits: list[GovernancePermitView]
    closed_resources: list[GovernanceResource]
    revoked_subjects: list[str]
    federation_trusts: list[FederationTrustView] = field(default_factory=list)

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GovernanceScopeView":
        return cls(
            GovernanceManagementBounds.from_dict(data["management"]),
            data["namespace"],
            data["tenant"],
            data["incarnation"],
            data["generation"],
            [
                GovernanceRouteView(
                    GovernanceRoute(**r["route"]),
                    GovernanceEffect.from_dict(r["effect"]),
                    r["closed"],
                )
                for r in data["routes"]
            ],
            [GovernancePermitView.from_dict(p) for p in data["permits"]],
            [GovernanceResource(**r) for r in data["closed_resources"]],
            data["revoked_subjects"],
            [FederationTrustView.from_dict(view) for view in data.get("federation_trusts", [])],
        )


@dataclass
class ProviderEvidenceReference:
    id: str
    digest: str


@dataclass
class ProviderHistoryStatus:
    state: Literal[
        "prepared", "in_flight", "awaiting_retry", "completed", "reconciliation_required"
    ]
    attempt_id: str | None = None
    not_before_ms: int | None = None
    outcome: ActionOutcome | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderHistoryStatus":
        return cls(
            data["state"],
            data.get("attempt_id"),
            data.get("not_before_ms"),
            ActionOutcome.from_dict(data["outcome"]) if data.get("outcome") is not None else None,
        )


@dataclass
class ProviderHistoryReceipt:
    execution_id: str
    attempts: int
    status: ProviderHistoryStatus

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderHistoryReceipt":
        return cls(
            data["execution_id"], data["attempts"], ProviderHistoryStatus.from_dict(data["status"])
        )


@dataclass
class ProviderHistoryAuthority:
    incarnation: str
    generation: int


@dataclass
class ProviderOperationMetadata:
    original_action_id: str
    max_attempts: int


@dataclass
class ProviderHistoryBinding:
    provider: str
    provider_revision: str
    failure_revision: str
    effect: GovernanceEffect


@dataclass
class ProviderReconciliationAcceptance:
    operator: PrincipalIdentity
    authority: ProviderHistoryAuthority
    accepted_at_ms: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderReconciliationAcceptance":
        return cls(
            PrincipalIdentity(**data["operator"]),
            ProviderHistoryAuthority(**data["authority"]),
            data["accepted_at_ms"],
        )


@dataclass
class ProviderHistoryReconciliation:
    prior_status: Literal["in_flight", "settled", "uncertain"]
    execution_id: str
    attempt_id: str
    original_evidence: ProviderEvidenceReference | None
    resolution: ProviderEvidenceReference
    verifier_revision: str
    proof_digest: str
    resolved_at_ms: int
    outcome: ActionOutcome
    acceptance: ProviderReconciliationAcceptance | None = None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderHistoryReconciliation":
        return cls(
            **{
                **data,
                "original_evidence": ProviderEvidenceReference(**data["original_evidence"])
                if data["original_evidence"] is not None
                else None,
                "resolution": ProviderEvidenceReference(**data["resolution"]),
                "outcome": ActionOutcome.from_dict(data["outcome"]),
                "acceptance": ProviderReconciliationAcceptance.from_dict(data["acceptance"])
                if data.get("acceptance") is not None
                else None,
            }
        )


@dataclass
class ProviderHistoryAttempt:
    attempt_id: str
    ordinal: int
    ledger_status: Literal["in_flight", "settled", "uncertain"]
    original_evidence: ProviderEvidenceReference | None
    original_outcome: ActionOutcome | None
    reconciliation: ProviderHistoryReconciliation | None

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderHistoryAttempt":
        return cls(
            **{
                **data,
                "original_evidence": ProviderEvidenceReference(**data["original_evidence"])
                if data["original_evidence"] is not None
                else None,
                "original_outcome": ActionOutcome.from_dict(data["original_outcome"])
                if data["original_outcome"] is not None
                else None,
                "reconciliation": ProviderHistoryReconciliation.from_dict(data["reconciliation"])
                if data["reconciliation"] is not None
                else None,
            }
        )


@dataclass
class ProviderExecutionHistory:
    """Retained evidence. Reading this never admits or resumes execution."""

    subject: PrincipalIdentity
    receipt: ProviderHistoryReceipt
    observed_authority: ProviderHistoryAuthority
    operation_integrity: Literal["unstarted", "sealed", "legacy"]
    metadata: ProviderOperationMetadata | None
    binding: ProviderHistoryBinding | None
    cancellation_fenced: bool
    attempts: list[ProviderHistoryAttempt]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderExecutionHistory":
        receipt = data["receipt"]
        binding = data["binding"]
        return cls(
            PrincipalIdentity(**data["subject"]),
            ProviderHistoryReceipt(
                receipt["execution_id"],
                receipt["attempts"],
                ProviderHistoryStatus.from_dict(receipt["status"]),
            ),
            ProviderHistoryAuthority(**data["observed_authority"]),
            data["operation_integrity"],
            ProviderOperationMetadata(**data["metadata"]) if data["metadata"] is not None else None,
            ProviderHistoryBinding(
                **{
                    **binding,
                    "effect": GovernanceEffect(
                        binding["effect"]["operation"],
                        [GovernanceResource(**r) for r in binding["effect"]["resources"]],
                    ),
                }
            )
            if binding is not None
            else None,
            data["cancellation_fenced"],
            [ProviderHistoryAttempt.from_dict(a) for a in data["attempts"]],
        )


@dataclass
class ProviderReconciliationContext:
    context_id: str
    execution_id: str
    namespace: str
    tenant: str
    principal: PrincipalIdentity
    request_digest: str


@dataclass
class ProviderReconciliationCorrelation:
    context: ProviderReconciliationContext
    action_id: str
    attempt_id: str
    ordinal: int
    token: str
    binding_digest: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "ProviderReconciliationCorrelation":
        context = dict(data["context"])
        context["principal"] = PrincipalIdentity(**context["principal"])
        return cls(**{**data, "context": ProviderReconciliationContext(**context)})


@dataclass
class ProviderReconciliationRequest:
    proof_base64: str


RegistryProjection = Literal["agent", "card"]


@dataclass
class GovernanceRegistryMutationRequest:
    namespace: str
    tenant: str
    agent_id: str
    change_id: str
    expected_registry_revision: int
    projection: RegistryProjection
    expected_projection_version: int | None
    value: dict[str, Any] | None
    reason: str


@dataclass
class GovernanceRegistryProjectionView:
    namespace: str
    tenant: str
    agent_id: str
    agent_resource: GovernanceResource
    projection: RegistryProjection
    registry_revision: int
    qualification_retired: bool | None
    version: int | None
    value: dict[str, Any] | None

    @classmethod
    def from_dict(
        cls,
        data: dict[str, Any],
        namespace: str,
        tenant: str,
        agent_id: str,
        projection: RegistryProjection,
    ) -> "GovernanceRegistryProjectionView":
        result = cls(**{**data, "agent_resource": GovernanceResource(**data["agent_resource"])})
        if (
            (result.namespace, result.tenant, result.agent_id, result.projection)
            != (namespace, tenant, agent_id, projection)
            or result.agent_resource != GovernanceResource("agent", namespace, tenant, agent_id)
            or type(result.registry_revision) is not int
            or result.registry_revision < 0
            or (result.registry_revision == 0) != (result.qualification_retired is None)
            or (
                result.qualification_retired is not None
                and type(result.qualification_retired) is not bool
            )
            or (
                result.version is not None
                and (type(result.version) is not int or result.version <= 0)
            )
            or (result.version is None) != (result.value is None)
            or (result.value is not None and not isinstance(result.value, dict))
        ):
            raise ValueError("registry observation identity or version mismatch")
        return result


@dataclass
class GovernanceRegistryMutationReceipt:
    namespace: str
    tenant: str
    agent_id: str
    change_id: str
    projection: RegistryProjection
    expected_registry_revision: int
    input_digest: str
    actor: str
    delivery_complete: bool
    applied: bool

    @classmethod
    def from_dict(
        cls, data: dict[str, Any], request: GovernanceRegistryMutationRequest
    ) -> "GovernanceRegistryMutationReceipt":
        result = cls(**data)
        fields = (
            "namespace",
            "tenant",
            "agent_id",
            "change_id",
            "projection",
            "expected_registry_revision",
        )
        if (
            any(getattr(result, key) != getattr(request, key) for key in fields)
            or type(result.expected_registry_revision) is not int
            or result.expected_registry_revision < 0
            or result.delivery_complete is not True
            or result.applied is not True
            or not isinstance(result.actor, str)
            or not result.actor
            or not isinstance(result.input_digest, str)
            or len(result.input_digest) != 64
            or any(c not in "0123456789abcdef" for c in result.input_digest)
        ):
            raise ValueError("unmatched or incomplete registry mutation receipt")
        return result


class _GovernanceMixin:
    if TYPE_CHECKING:

        def platform_request(
            self,
            operation: PlatformOperation,
            *,
            path: dict[str, str] | None = None,
            query: dict[str, Any] | None = None,
            body: Any = None,
        ) -> Any: ...

    def registry_projection(
        self, namespace: str, tenant: str, agent_id: str, projection: RegistryProjection
    ) -> GovernanceRegistryProjectionView:
        data = self.platform_request(
            PlatformOperation.GOVERNANCE_REGISTRY_PROJECTION,
            path={"agent_id": agent_id},
            query={"namespace": namespace, "tenant": tenant, "projection": projection},
        )
        return GovernanceRegistryProjectionView.from_dict(
            data, namespace, tenant, agent_id, projection
        )

    def mutate_registry(
        self, request: GovernanceRegistryMutationRequest
    ) -> GovernanceRegistryMutationReceipt:
        """Send once; retain this exact request/change ID for explicit recovery."""
        sent = GovernanceRegistryMutationRequest(**asdict(request))
        data = self.platform_request(
            PlatformOperation.GOVERNANCE_MUTATE_REGISTRY, body=asdict(sent)
        )
        return GovernanceRegistryMutationReceipt.from_dict(data, sent)

    def governance(self, namespace: str, tenant: str) -> GovernanceScopeView:
        return GovernanceScopeView.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_INSPECT,
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    def provider_execution_history(
        self, namespace: str, tenant: str, execution_id: str
    ) -> ProviderExecutionHistory:
        return ProviderExecutionHistory.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_PROVIDER_HISTORY,
                path={"execution_id": execution_id},
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    def provider_reconciliation_correlation(
        self, namespace: str, tenant: str, execution_id: str, ordinal: int
    ) -> ProviderReconciliationCorrelation:
        return ProviderReconciliationCorrelation.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_RECONCILIATION_CORRELATION,
                path={"execution_id": execution_id, "ordinal": str(ordinal)},
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    def accept_provider_reconciliation(
        self,
        namespace: str,
        tenant: str,
        execution_id: str,
        ordinal: int,
        request: ProviderReconciliationRequest,
    ) -> ProviderHistoryReceipt:
        return ProviderHistoryReceipt.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_ACCEPT_RECONCILIATION,
                path={"execution_id": execution_id, "ordinal": str(ordinal)},
                query={"namespace": namespace, "tenant": tenant},
                body=asdict(request),
            )
        )

    def publish_governance_permit(
        self, request: PublishGovernancePermitRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            self.platform_request(PlatformOperation.GOVERNANCE_PUBLISH_PERMIT, body=asdict(request))
        )

    def publish_federation_trust(
        self, request: PublishFederationTrustRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_PUBLISH_FEDERATION_TRUST, body=asdict(request)
            )
        )

    def revoke_federation_trust(
        self, request: RevokeFederationTrustRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_REVOKE_FEDERATION_TRUST, body=asdict(request)
            )
        )

    def federation_delivery_evidence(
        self,
        namespace: str,
        tenant: str,
        after: str | None = None,
        limit: int | None = None,
    ) -> FederationDeliveryEvidencePage:
        query: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if after is not None:
            query["after"] = after
        if limit is not None:
            query["limit"] = limit
        return FederationDeliveryEvidencePage.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_FEDERATION_DELIVERY_EVIDENCE,
                query=query,
            )
        )

    def abandon_federation_delivery(
        self,
        delivery_id: str,
        request: AbandonFederationDeliveryRequest,
    ) -> FederationDeliveryEvidence:
        return FederationDeliveryEvidence.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_ABANDON_FEDERATION_DELIVERY,
                path={"delivery_id": delivery_id},
                body=asdict(request),
            )
        )

    def federation_finality_correlation(
        self, namespace: str, tenant: str, delivery_id: str
    ) -> FederationFinalityCorrelation:
        return FederationFinalityCorrelation.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_FEDERATION_FINALITY_CORRELATION,
                path={"delivery_id": delivery_id},
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    def reconcile_federation_delivery(
        self,
        delivery_id: str,
        request: ReconcileFederationDeliveryRequest,
    ) -> FederationDeliveryEvidence:
        return FederationDeliveryEvidence.from_dict(
            self.platform_request(
                PlatformOperation.GOVERNANCE_RECONCILE_FEDERATION_DELIVERY,
                path={"delivery_id": delivery_id},
                body=asdict(request),
            )
        )

    def import_federation_envelope(
        self,
        namespace: str,
        tenant: str,
        envelope: SignedFederationEnvelopeDeclaration,
    ) -> FederationImportReceipt:
        receipt = FederationImportReceipt.from_dict(
            self.platform_request(
                PlatformOperation.FEDERATION_IMPORT_ENVELOPE,
                path={"namespace": namespace, "tenant": tenant},
                body={"envelope": asdict(envelope)},
            )
        )
        if receipt.namespace != namespace or receipt.tenant != tenant or receipt.execution_started:
            raise ValueError("federation import receipt identity or execution state mismatch")
        return receipt

    def federation_capabilities(
        self, namespace: str, tenant: str, agent: str
    ) -> SignedFederationCapabilityCard:
        return SignedFederationCapabilityCard.from_dict(
            self.platform_request(
                PlatformOperation.FEDERATION_CAPABILITIES,
                path={"namespace": namespace, "tenant": tenant, "agent": agent},
            )
        )

    def invoke_federated_agent(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        invocation: SignedFederationInvocationDeclaration,
        message: dict[str, Any],
    ) -> dict[str, Any]:
        task = cast(
            dict[str, Any],
            self.platform_request(
                PlatformOperation.FEDERATION_INVOKE_AGENT,
                path={"namespace": namespace, "tenant": tenant, "agent": agent},
                body={"invocation": asdict(invocation), "message": message},
            ),
        )
        if (
            task.get("namespace") != namespace
            or task.get("tenant") != tenant
            or not isinstance(task.get("id"), str)
            or not task["id"]
        ):
            raise ValueError("federated task receipt identity mismatch")
        return task

    def export_federated_message(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return self._federation_export_request(
            PlatformOperation.FEDERATION_EXPORT_MESSAGE,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )

    def observe_federated_export(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return self._federation_export_request(
            PlatformOperation.FEDERATION_OBSERVE_EXPORT_MESSAGE,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )

    def replay_federated_export(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return self._federation_export_request(
            PlatformOperation.FEDERATION_REPLAY_EXPORT_MESSAGE,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )

    def _federation_export_request(
        self,
        operation: PlatformOperation,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return FederationExportReceipt.from_dict(
            self.platform_request(
                operation,
                path={
                    "namespace": namespace,
                    "tenant": tenant,
                    "agent": agent,
                    "task_id": task_id,
                    "profile": profile,
                },
                body={"message": message},
            )
        )

    def intervene_governance(
        self, request: GovernanceInterventionRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            self.platform_request(PlatformOperation.GOVERNANCE_INTERVENE, body=asdict(request))
        )


class _AsyncGovernanceMixin:
    if TYPE_CHECKING:

        async def platform_request(
            self,
            operation: PlatformOperation,
            *,
            path: dict[str, str] | None = None,
            query: dict[str, Any] | None = None,
            body: Any = None,
        ) -> Any: ...

    async def registry_projection(
        self, namespace: str, tenant: str, agent_id: str, projection: RegistryProjection
    ) -> GovernanceRegistryProjectionView:
        data = await self.platform_request(
            PlatformOperation.GOVERNANCE_REGISTRY_PROJECTION,
            path={"agent_id": agent_id},
            query={"namespace": namespace, "tenant": tenant, "projection": projection},
        )
        return GovernanceRegistryProjectionView.from_dict(
            data, namespace, tenant, agent_id, projection
        )

    async def mutate_registry(
        self, request: GovernanceRegistryMutationRequest
    ) -> GovernanceRegistryMutationReceipt:
        """Send once; retain this exact request/change ID for explicit recovery."""
        sent = GovernanceRegistryMutationRequest(**asdict(request))
        data = await self.platform_request(
            PlatformOperation.GOVERNANCE_MUTATE_REGISTRY, body=asdict(sent)
        )
        return GovernanceRegistryMutationReceipt.from_dict(data, sent)

    async def governance(self, namespace: str, tenant: str) -> GovernanceScopeView:
        return GovernanceScopeView.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_INSPECT,
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    async def provider_execution_history(
        self, namespace: str, tenant: str, execution_id: str
    ) -> ProviderExecutionHistory:
        return ProviderExecutionHistory.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_PROVIDER_HISTORY,
                path={"execution_id": execution_id},
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    async def provider_reconciliation_correlation(
        self, namespace: str, tenant: str, execution_id: str, ordinal: int
    ) -> ProviderReconciliationCorrelation:
        return ProviderReconciliationCorrelation.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_RECONCILIATION_CORRELATION,
                path={"execution_id": execution_id, "ordinal": str(ordinal)},
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    async def accept_provider_reconciliation(
        self,
        namespace: str,
        tenant: str,
        execution_id: str,
        ordinal: int,
        request: ProviderReconciliationRequest,
    ) -> ProviderHistoryReceipt:
        return ProviderHistoryReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_ACCEPT_RECONCILIATION,
                path={"execution_id": execution_id, "ordinal": str(ordinal)},
                query={"namespace": namespace, "tenant": tenant},
                body=asdict(request),
            )
        )

    async def publish_governance_permit(
        self, request: PublishGovernancePermitRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_PUBLISH_PERMIT, body=asdict(request)
            )
        )

    async def publish_federation_trust(
        self, request: PublishFederationTrustRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_PUBLISH_FEDERATION_TRUST, body=asdict(request)
            )
        )

    async def revoke_federation_trust(
        self, request: RevokeFederationTrustRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_REVOKE_FEDERATION_TRUST, body=asdict(request)
            )
        )

    async def federation_delivery_evidence(
        self,
        namespace: str,
        tenant: str,
        after: str | None = None,
        limit: int | None = None,
    ) -> FederationDeliveryEvidencePage:
        query: dict[str, Any] = {"namespace": namespace, "tenant": tenant}
        if after is not None:
            query["after"] = after
        if limit is not None:
            query["limit"] = limit
        return FederationDeliveryEvidencePage.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_FEDERATION_DELIVERY_EVIDENCE,
                query=query,
            )
        )

    async def abandon_federation_delivery(
        self,
        delivery_id: str,
        request: AbandonFederationDeliveryRequest,
    ) -> FederationDeliveryEvidence:
        return FederationDeliveryEvidence.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_ABANDON_FEDERATION_DELIVERY,
                path={"delivery_id": delivery_id},
                body=asdict(request),
            )
        )

    async def federation_finality_correlation(
        self, namespace: str, tenant: str, delivery_id: str
    ) -> FederationFinalityCorrelation:
        return FederationFinalityCorrelation.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_FEDERATION_FINALITY_CORRELATION,
                path={"delivery_id": delivery_id},
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    async def reconcile_federation_delivery(
        self,
        delivery_id: str,
        request: ReconcileFederationDeliveryRequest,
    ) -> FederationDeliveryEvidence:
        return FederationDeliveryEvidence.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_RECONCILE_FEDERATION_DELIVERY,
                path={"delivery_id": delivery_id},
                body=asdict(request),
            )
        )

    async def import_federation_envelope(
        self,
        namespace: str,
        tenant: str,
        envelope: SignedFederationEnvelopeDeclaration,
    ) -> FederationImportReceipt:
        receipt = FederationImportReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.FEDERATION_IMPORT_ENVELOPE,
                path={"namespace": namespace, "tenant": tenant},
                body={"envelope": asdict(envelope)},
            )
        )
        if receipt.namespace != namespace or receipt.tenant != tenant or receipt.execution_started:
            raise ValueError("federation import receipt identity or execution state mismatch")
        return receipt

    async def federation_capabilities(
        self, namespace: str, tenant: str, agent: str
    ) -> SignedFederationCapabilityCard:
        return SignedFederationCapabilityCard.from_dict(
            await self.platform_request(
                PlatformOperation.FEDERATION_CAPABILITIES,
                path={"namespace": namespace, "tenant": tenant, "agent": agent},
            )
        )

    async def invoke_federated_agent(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        invocation: SignedFederationInvocationDeclaration,
        message: dict[str, Any],
    ) -> dict[str, Any]:
        task = cast(
            dict[str, Any],
            await self.platform_request(
                PlatformOperation.FEDERATION_INVOKE_AGENT,
                path={"namespace": namespace, "tenant": tenant, "agent": agent},
                body={"invocation": asdict(invocation), "message": message},
            ),
        )
        if (
            task.get("namespace") != namespace
            or task.get("tenant") != tenant
            or not isinstance(task.get("id"), str)
            or not task["id"]
        ):
            raise ValueError("federated task receipt identity mismatch")
        return task

    async def export_federated_message(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return await self._federation_export_request(
            PlatformOperation.FEDERATION_EXPORT_MESSAGE,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )

    async def observe_federated_export(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return await self._federation_export_request(
            PlatformOperation.FEDERATION_OBSERVE_EXPORT_MESSAGE,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )

    async def replay_federated_export(
        self,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return await self._federation_export_request(
            PlatformOperation.FEDERATION_REPLAY_EXPORT_MESSAGE,
            namespace,
            tenant,
            agent,
            task_id,
            profile,
            message,
        )

    async def _federation_export_request(
        self,
        operation: PlatformOperation,
        namespace: str,
        tenant: str,
        agent: str,
        task_id: str,
        profile: str,
        message: dict[str, Any],
    ) -> FederationExportReceipt:
        return FederationExportReceipt.from_dict(
            await self.platform_request(
                operation,
                path={
                    "namespace": namespace,
                    "tenant": tenant,
                    "agent": agent,
                    "task_id": task_id,
                    "profile": profile,
                },
                body={"message": message},
            )
        )

    async def intervene_governance(
        self, request: GovernanceInterventionRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_INTERVENE, body=asdict(request)
            )
        )
