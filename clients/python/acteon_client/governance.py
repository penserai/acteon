"""Typed governance management. Public values never establish caller authority."""

from dataclasses import asdict, dataclass, field
from typing import TYPE_CHECKING, Any, Literal

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

    async def intervene_governance(
        self, request: GovernanceInterventionRequest
    ) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            await self.platform_request(
                PlatformOperation.GOVERNANCE_INTERVENE, body=asdict(request)
            )
        )
