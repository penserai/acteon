"""Typed workforce declarations; relationships and wire labels do not grant authority."""

from collections.abc import Callable
from dataclasses import asdict, dataclass, field
from typing import TYPE_CHECKING, Any, Generic, Literal, TypeVar

from .governance import (
    GovernanceChangeReceipt,
    GovernanceEffect,
    GovernanceLimits,
    GovernancePermitDeclaration,
    GovernanceRoute,
    GovernanceRouteView,
)
from .models import PrincipalIdentity
from .platform_catalog import PlatformOperation


@dataclass
class TeamRef:
    domain: str
    tenant: str
    id: str


@dataclass
class HumanRepresentation:
    principal: PrincipalIdentity
    kind: Literal["human"] = field(default="human", init=False)


@dataclass
class TeamRepresentation:
    team: TeamRef
    kind: Literal["team"] = field(default="team", init=False)


RepresentedParty = HumanRepresentation | TeamRepresentation


def _party(data: dict[str, Any]) -> RepresentedParty:
    if data["kind"] == "human":
        return HumanRepresentation(PrincipalIdentity(**data["principal"]))
    if data["kind"] == "team":
        return TeamRepresentation(TeamRef(**data["team"]))
    raise ValueError("Unknown represented party")


@dataclass
class WorkforceReference:
    id: str
    accepted_revision: int


@dataclass
class WorkforceDependency:
    kind: Literal["membership", "assignment"]
    reference: WorkforceReference


TeamRole = Literal["requester", "approver", "workforce_manager", "mandate_issuer"]


@dataclass
class WorkforceTeam:
    team: TeamRef
    revision: int
    name: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceTeam":
        return cls(TeamRef(**data["team"]), data["revision"], data["name"])


@dataclass
class WorkforceMembership:
    id: str
    revision: int
    team: TeamRef
    human: PrincipalIdentity
    roles: list[TeamRole]
    valid_from_ms: int
    deadline_ms: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceMembership":
        return cls(
            data["id"],
            data["revision"],
            TeamRef(**data["team"]),
            PrincipalIdentity(**data["human"]),
            data["roles"],
            data["valid_from_ms"],
            data["deadline_ms"],
        )


@dataclass
class AgentOwnership:
    agent: PrincipalIdentity
    revision: int
    owner: RepresentedParty

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "AgentOwnership":
        return cls(PrincipalIdentity(**data["agent"]), data["revision"], _party(data["owner"]))


@dataclass
class WorkforceAssignment:
    id: str
    revision: int
    team: TeamRef
    agent: PrincipalIdentity
    job_classes: list[str]
    valid_from_ms: int
    deadline_ms: int

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceAssignment":
        return cls(
            data["id"],
            data["revision"],
            TeamRef(**data["team"]),
            PrincipalIdentity(**data["agent"]),
            data["job_classes"],
            data["valid_from_ms"],
            data["deadline_ms"],
        )


def _mandate_fields(data: dict[str, Any]) -> dict[str, Any]:
    fields = dict(data)
    fields["represented"] = _party(data["represented"])
    fields["actor"] = PrincipalIdentity(**data["actor"])
    fields["eligible_initiators"] = [PrincipalIdentity(**p) for p in data["eligible_initiators"]]
    fields["ownership"] = WorkforceReference(**data["ownership"]) if data.get("ownership") else None
    fields["dependencies"] = [
        WorkforceDependency(d["kind"], WorkforceReference(**d["reference"]))
        for d in data["dependencies"]
    ]
    fields["limits"] = GovernanceLimits(**data["limits"])
    return fields


@dataclass
class WorkforceMandateDeclaration:
    id: str
    revision: int
    represented: RepresentedParty
    actor: PrincipalIdentity
    job_class: str
    eligible_initiators: list[PrincipalIdentity]
    ownership: WorkforceReference | None
    dependencies: list[WorkforceDependency]
    routes: list[GovernanceRoute]
    valid_from_ms: int
    limits: GovernanceLimits

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceMandateDeclaration":
        fields = _mandate_fields(data)
        fields["routes"] = [GovernanceRoute(**r) for r in data["routes"]]
        return cls(**fields)


@dataclass
class WorkforceMandateView:
    id: str
    revision: int
    represented: RepresentedParty
    actor: PrincipalIdentity
    job_class: str
    eligible_initiators: list[PrincipalIdentity]
    ownership: WorkforceReference | None
    dependencies: list[WorkforceDependency]
    effects: list[GovernanceEffect]
    valid_from_ms: int
    limits: GovernanceLimits

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceMandateView":
        fields = _mandate_fields(data)
        fields["effects"] = [GovernanceEffect.from_dict(e) for e in data["effects"]]
        return cls(**fields)


@dataclass
class PutWorkforceTeam:
    team: WorkforceTeam
    kind: Literal["put_team"] = field(default="put_team", init=False)


@dataclass
class DisbandWorkforceTeam:
    team: TeamRef
    expected_revision: int
    kind: Literal["disband_team"] = field(default="disband_team", init=False)


@dataclass
class PutWorkforceMembership:
    membership: WorkforceMembership
    kind: Literal["put_membership"] = field(default="put_membership", init=False)


@dataclass
class RemoveWorkforceMembership:
    id: str
    expected_revision: int
    kind: Literal["remove_membership"] = field(default="remove_membership", init=False)


@dataclass
class PutAgentOwnership:
    ownership: AgentOwnership
    kind: Literal["put_ownership"] = field(default="put_ownership", init=False)


@dataclass
class PutWorkforceAssignment:
    assignment: WorkforceAssignment
    kind: Literal["put_assignment"] = field(default="put_assignment", init=False)


@dataclass
class RemoveWorkforceAssignment:
    id: str
    expected_revision: int
    kind: Literal["remove_assignment"] = field(default="remove_assignment", init=False)


@dataclass
class PutWorkforceMandate:
    mandate: WorkforceMandateDeclaration
    kind: Literal["put_mandate"] = field(default="put_mandate", init=False)


@dataclass
class RevokeWorkforceMandate:
    id: str
    expected_revision: int
    kind: Literal["revoke_mandate"] = field(default="revoke_mandate", init=False)


@dataclass
class PublishRepresentedPermit:
    permit: GovernancePermitDeclaration
    mandate: WorkforceReference
    kind: Literal["publish_represented_permit"] = field(
        default="publish_represented_permit", init=False
    )


WorkforceChange = (
    PutWorkforceTeam
    | DisbandWorkforceTeam
    | PutWorkforceMembership
    | RemoveWorkforceMembership
    | PutAgentOwnership
    | PutWorkforceAssignment
    | RemoveWorkforceAssignment
    | PutWorkforceMandate
    | RevokeWorkforceMandate
    | PublishRepresentedPermit
)


def _change(data: dict[str, Any]) -> WorkforceChange:
    match data["kind"]:
        case "put_team":
            return PutWorkforceTeam(WorkforceTeam.from_dict(data["team"]))
        case "disband_team":
            return DisbandWorkforceTeam(TeamRef(**data["team"]), data["expected_revision"])
        case "put_membership":
            return PutWorkforceMembership(WorkforceMembership.from_dict(data["membership"]))
        case "remove_membership":
            return RemoveWorkforceMembership(data["id"], data["expected_revision"])
        case "put_ownership":
            return PutAgentOwnership(AgentOwnership.from_dict(data["ownership"]))
        case "put_assignment":
            return PutWorkforceAssignment(WorkforceAssignment.from_dict(data["assignment"]))
        case "remove_assignment":
            return RemoveWorkforceAssignment(data["id"], data["expected_revision"])
        case "put_mandate":
            return PutWorkforceMandate(WorkforceMandateDeclaration.from_dict(data["mandate"]))
        case "revoke_mandate":
            return RevokeWorkforceMandate(data["id"], data["expected_revision"])
        case "publish_represented_permit":
            p = data["permit"]
            return PublishRepresentedPermit(
                GovernancePermitDeclaration(
                    p["id"],
                    p["revision"],
                    PrincipalIdentity(**p["subject"]),
                    [GovernanceRoute(**r) for r in p["routes"]],
                    p["valid_from_ms"],
                    GovernanceLimits(**p["limits"]),
                ),
                WorkforceReference(**data["mandate"]),
            )
        case _:
            raise ValueError("Unknown workforce change")


@dataclass
class WorkforceChangeRequest:
    namespace: str
    tenant: str
    change_id: str
    change: WorkforceChange
    reason: str

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceChangeRequest":
        return cls(
            data["namespace"],
            data["tenant"],
            data["change_id"],
            _change(data["change"]),
            data["reason"],
        )


T = TypeVar("T")


@dataclass
class WorkforceEntry(Generic[T]):
    value: T
    revoked: bool


def _entries(
    data: list[dict[str, Any]], parse: Callable[[dict[str, Any]], T]
) -> list[WorkforceEntry[T]]:
    return [WorkforceEntry(parse(r["value"]), r["revoked"]) for r in data]


@dataclass
class WorkforcePermitBindingView:
    permit_id: str
    permit_revision: int
    mandate: WorkforceReference


@dataclass
class WorkforceManagementBounds:
    teams: list[TeamRef]
    principals: list[PrincipalIdentity]
    job_classes: list[str]
    can_manage_roster: bool
    can_issue_mandates: bool
    can_issue_permits: bool
    valid_from_ms: int
    limits: GovernanceLimits


@dataclass
class WorkforceScopeView:
    namespace: str
    tenant: str
    incarnation: str
    generation: int
    management: WorkforceManagementBounds
    routes: list[GovernanceRouteView]
    teams: list[WorkforceEntry[WorkforceTeam]]
    memberships: list[WorkforceEntry[WorkforceMembership]]
    ownership: list[WorkforceEntry[AgentOwnership]]
    assignments: list[WorkforceEntry[WorkforceAssignment]]
    mandates: list[WorkforceEntry[WorkforceMandateView]]
    permit_bindings: list[WorkforcePermitBindingView]

    @classmethod
    def from_dict(cls, data: dict[str, Any]) -> "WorkforceScopeView":
        b = data["management"]
        bounds = WorkforceManagementBounds(
            [TeamRef(**t) for t in b["teams"]],
            [PrincipalIdentity(**p) for p in b["principals"]],
            b["job_classes"],
            b["can_manage_roster"],
            b["can_issue_mandates"],
            b["can_issue_permits"],
            b["valid_from_ms"],
            GovernanceLimits(**b["limits"]),
        )
        return cls(
            data["namespace"],
            data["tenant"],
            data["incarnation"],
            data["generation"],
            bounds,
            [
                GovernanceRouteView(
                    GovernanceRoute(**r["route"]),
                    GovernanceEffect.from_dict(r["effect"]),
                    r["closed"],
                )
                for r in data["routes"]
            ],
            _entries(data["teams"], WorkforceTeam.from_dict),
            _entries(data["memberships"], WorkforceMembership.from_dict),
            _entries(data["ownership"], AgentOwnership.from_dict),
            _entries(data["assignments"], WorkforceAssignment.from_dict),
            _entries(data["mandates"], WorkforceMandateView.from_dict),
            [
                WorkforcePermitBindingView(
                    b["permit_id"], b["permit_revision"], WorkforceReference(**b["mandate"])
                )
                for b in data["permit_bindings"]
            ],
        )


class _WorkforceMixin:
    if TYPE_CHECKING:

        def platform_request(
            self,
            operation: PlatformOperation,
            *,
            path: dict[str, str] | None = None,
            query: dict[str, Any] | None = None,
            body: Any = None,
        ) -> Any: ...

    def workforce(self, namespace: str, tenant: str) -> WorkforceScopeView:
        return WorkforceScopeView.from_dict(
            self.platform_request(
                PlatformOperation.WORKFORCE_INSPECT,
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    def change_workforce(self, request: WorkforceChangeRequest) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            self.platform_request(PlatformOperation.WORKFORCE_CHANGE, body=asdict(request))
        )


class _AsyncWorkforceMixin:
    if TYPE_CHECKING:

        async def platform_request(
            self,
            operation: PlatformOperation,
            *,
            path: dict[str, str] | None = None,
            query: dict[str, Any] | None = None,
            body: Any = None,
        ) -> Any: ...

    async def workforce(self, namespace: str, tenant: str) -> WorkforceScopeView:
        return WorkforceScopeView.from_dict(
            await self.platform_request(
                PlatformOperation.WORKFORCE_INSPECT,
                query={"namespace": namespace, "tenant": tenant},
            )
        )

    async def change_workforce(self, request: WorkforceChangeRequest) -> GovernanceChangeReceipt:
        return GovernanceChangeReceipt.from_dict(
            await self.platform_request(PlatformOperation.WORKFORCE_CHANGE, body=asdict(request))
        )
