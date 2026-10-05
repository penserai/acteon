"""Typed governance management. Public values never establish caller authority."""

from dataclasses import asdict, dataclass, field
from typing import TYPE_CHECKING, Any, Literal

from .models import PrincipalIdentity
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

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "GovernanceManagementBounds":
        return cls(
            [PrincipalIdentity(**s) for s in data["subjects"]],
            data["can_issue_permits"],
            data["can_intervene"],
            data["valid_from_ms"],
            GovernanceLimits(**data["limits"]),
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
