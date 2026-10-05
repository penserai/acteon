#!/usr/bin/env python3
"""Exercise real workforce mandates, permits, offboarding and closures over HTTP."""

import argparse
import hashlib
import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import ExitStack
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "clients/python"))
from acteon_client import (
    ActeonClient,
    Action,
    GovernanceInterventionRequest,
    GovernanceLimits,
    GovernancePermitDeclaration,
    GovernanceResourceChange,
    GovernanceRoute,
    PermitReference,
    PrincipalIdentity,
)
from acteon_client.errors import HttpError
from acteon_client.workforce import (
    AgentOwnership,
    DisbandWorkforceTeam,
    HumanRepresentation,
    PublishRepresentedPermit,
    PutAgentOwnership,
    PutWorkforceAssignment,
    PutWorkforceMandate,
    PutWorkforceMembership,
    PutWorkforceTeam,
    RemoveWorkforceMembership,
    RevokeWorkforceMandate,
    TeamRef,
    TeamRepresentation,
    WorkforceAssignment,
    WorkforceChangeRequest,
    WorkforceDependency,
    WorkforceMandateDeclaration,
    WorkforceMembership,
    WorkforceReference,
    WorkforceTeam,
)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--server", type=Path, default=ROOT / "target/debug/acteon-server"
    )
    parser.add_argument(
        "--output", type=Path, default=Path("agent-workforce-results.json")
    )
    args = parser.parse_args()
    # The public guide must show the exact configuration this real run uses.
    guide = (ROOT / "docs/book/guides/agent-workforce.md").read_text()
    marker = "<!-- workforce-example-file: acteon.toml -->"
    documented = guide.split(marker, 1)[1].split("```toml\n", 1)[1].split("```", 1)[0]
    actual = (Path(__file__).parent / "acteon.toml").read_text()
    if documented.strip() != actual.strip():
        raise AssertionError(
            "agent-workforce guide configuration differs from the simulation"
        )
    deliveries = []

    class Receiver(BaseHTTPRequestHandler):
        def do_POST(self):
            deliveries.append(
                json.loads(self.rfile.read(int(self.headers["content-length"])))
            )
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(b'{"ok":true}')

        def log_message(self, *_args):
            pass

    receiver = ThreadingHTTPServer(("127.0.0.1", 0), Receiver)
    thread = threading.Thread(target=receiver.serve_forever, daemon=True)
    thread.start()
    with socket.socket() as socket_:
        socket_.bind(("127.0.0.1", 0))
        port = socket_.getsockname()[1]
    started = time.monotonic()
    try:
        with tempfile.TemporaryDirectory(prefix="acteon-city-") as temporary:
            directory = Path(temporary)
            config = (
                (Path(__file__).parent / "acteon.toml")
                .read_text()
                .replace("port = 18080", f"port = {port}")
            )
            config = config.replace(
                "127.0.0.1:18081", f"127.0.0.1:{receiver.server_port}"
            )
            (directory / "acteon.toml").write_text(config)
            keys = {
                name: secrets.token_hex(24)
                for name in ["operator", "maya", "personal", "team", "scheduler"]
            }
            auth = (
                'authority_revision=1\n[settings]\njwt_secret="'
                + secrets.token_hex(32)
                + '"\n'
            )
            for name, principal, kind, role, provider, action in [
                ("operator", "operator", "human", "operator", "", ""),
                ("maya", "maya", "human", "executor", "incident", "execute"),
                ("personal", "agent/maya", "agent", "executor", "incident", "execute"),
                (
                    "team",
                    "agent/reliability",
                    "agent",
                    "executor",
                    "incident",
                    "execute",
                ),
                (
                    "scheduler",
                    "scheduler",
                    "service",
                    "executor",
                    "incident",
                    "execute",
                ),
            ]:
                provider_grants = json.dumps([provider] if provider else [])
                action_grants = json.dumps([action] if action else [])
                auth += f'''\n[[api_keys]]
name="{name}"
authority_id="credential/{name}"
principal={{id="{principal}",kind="{kind}"}}
key_hash="{hashlib.sha256(keys[name].encode()).hexdigest()}"
role="{role}"
[[api_keys.grants]]
namespaces=["prod"]
tenants=["acme"]
providers={provider_grants}
actions={action_grants}
'''
            (directory / "auth.toml").write_text(auth)
            env = dict(
                os.environ,
                ACTEON_AUTH_KEY=secrets.token_hex(32),
                ACTEON_AUTH_AUTHORITY_KEY=secrets.token_hex(32),
                ACTEON_EXECUTION_AUTHORITY_KEY=secrets.token_hex(32),
            )
            with (directory / "server.log").open("w+") as log:
                process = subprocess.Popen(
                    [str(args.server.resolve()), "-c", str(directory / "acteon.toml")],
                    env=env,
                    stdout=log,
                    stderr=log,
                )
                try:
                    base = f"http://127.0.0.1:{port}"
                    with ExitStack() as stack:
                        clients = {
                            name: stack.enter_context(ActeonClient(base, api_key=key))
                            for name, key in keys.items()
                        }
                        operator = clients["operator"]
                        timeout = time.monotonic() + 60
                        while not operator.health():
                            if (
                                process.poll() is not None
                                or time.monotonic() >= timeout
                            ):
                                log.flush()
                                raise RuntimeError(
                                    (directory / "server.log").read_text()
                                )
                            time.sleep(0.03)
                        team = TeamRef("prod", "acme", "reliability")
                        release = TeamRef("prod", "acme", "release")
                        people = {
                            "maya": PrincipalIdentity("maya", "human"),
                            "personal": PrincipalIdentity("agent/maya", "agent"),
                            "team": PrincipalIdentity("agent/reliability", "agent"),
                            "scheduler": PrincipalIdentity("scheduler", "service"),
                        }
                        deadline = 4102444800000
                        results = []

                        def change(identity, mutation):
                            return operator.change_workforce(
                                WorkforceChangeRequest(
                                    "prod",
                                    "acme",
                                    identity,
                                    mutation,
                                    identity.replace("-", " "),
                                )
                            )

                        def record(step, outcome, expected):
                            assert len(deliveries) == expected, (step, deliveries)
                            results.append(
                                {
                                    "step": step,
                                    "outcome": outcome,
                                    "actual_webhook_calls": len(deliveries),
                                }
                            )

                        for label, ref in [("reliability", team), ("release", release)]:
                            change(
                                "create-" + label,
                                PutWorkforceTeam(WorkforceTeam(ref, 1, label.title())),
                            )
                            change(
                                "membership-" + label,
                                PutWorkforceMembership(
                                    WorkforceMembership(
                                        "maya-" + label,
                                        1,
                                        ref,
                                        people["maya"],
                                        ["requester"],
                                        0,
                                        deadline,
                                    )
                                ),
                            )
                        change(
                            "personal-ownership",
                            PutAgentOwnership(
                                AgentOwnership(
                                    people["personal"],
                                    1,
                                    HumanRepresentation(people["maya"]),
                                )
                            ),
                        )
                        change(
                            "team-ownership",
                            PutAgentOwnership(
                                AgentOwnership(
                                    people["team"], 1, TeamRepresentation(team)
                                )
                            ),
                        )
                        change(
                            "personal-duty",
                            PutWorkforceAssignment(
                                WorkforceAssignment(
                                    "maya-duty",
                                    1,
                                    team,
                                    people["personal"],
                                    ["execute"],
                                    0,
                                    deadline,
                                )
                            ),
                        )
                        for name, actor in people.items():
                            dependencies = []
                            ownership = None
                            if name in ["maya", "personal"]:
                                dependencies.append(
                                    WorkforceDependency(
                                        "membership",
                                        WorkforceReference("maya-reliability", 1),
                                    )
                                )
                            if actor.kind == "agent":
                                ownership = WorkforceReference(actor.id, 1)
                            if name == "personal":
                                dependencies.append(
                                    WorkforceDependency(
                                        "assignment", WorkforceReference("maya-duty", 1)
                                    )
                                )
                            change(
                                "mandate-" + name,
                                PutWorkforceMandate(
                                    WorkforceMandateDeclaration(
                                        "mandate-" + name,
                                        1,
                                        TeamRepresentation(team),
                                        actor,
                                        "execute",
                                        [actor],
                                        ownership,
                                        dependencies,
                                        [GovernanceRoute("incident", "execute")],
                                        0,
                                        GovernanceLimits(2, 1, deadline),
                                    )
                                ),
                            )
                            change(
                                "permit-" + name,
                                PublishRepresentedPermit(
                                    GovernancePermitDeclaration(
                                        "permit-" + name,
                                        1,
                                        actor,
                                        [GovernanceRoute("incident", "execute")],
                                        0,
                                        GovernanceLimits(1, 1, deadline),
                                    ),
                                    WorkforceReference("mandate-" + name, 1),
                                ),
                            )
                        try:
                            clients["personal"].workforce("prod", "acme")
                            raise AssertionError(
                                "executor reached workforce management"
                            )
                        except HttpError as error:
                            assert error.status == 403
                            record("executor cannot manage workforce", "HTTP 403", 0)

                        actions = {}

                        def execute(name, ticket, expected, allowed=True):
                            action = actions.setdefault(
                                (name, ticket),
                                Action(
                                    "prod",
                                    "acme",
                                    "incident",
                                    "execute",
                                    {
                                        "ticket": ticket,
                                        "participant": name,
                                        "represented": "forged-release",
                                    },
                                ),
                            )
                            result = clients[name].dispatch(
                                action,
                                permits=[PermitReference("permit-" + name, 1)],
                            )
                            assert result.outcome_type == (
                                "executed" if allowed else "failed"
                            ), (name, result)
                            record(ticket, result.outcome_type, expected)
                            if allowed:
                                assert deliveries[-1]["payload"]["ticket"] == ticket
                                assert deliveries[-1]["payload"]["participant"] == name

                        execute("personal", "personal-agent-on-duty", 1)
                        execute("personal", "personal-agent-on-duty", 1)
                        execute("maya", "human-team-work", 2)
                        execute("team", "team-agent-standing-duty", 3)
                        execute("scheduler", "deterministic-team-service", 4)
                        change(
                            "offboard-reliability",
                            RemoveWorkforceMembership("maya-reliability", 1),
                        )
                        execute("personal", "offboarded-personal-agent", 4, False)
                        execute("maya", "offboarded-human", 4, False)
                        view = operator.workforce("prod", "acme")
                        assert any(
                            m.value.id == "maya-release" and not m.revoked
                            for m in view.memberships
                        )
                        execute(
                            "team",
                            "standing-team-agent-survives-personal-offboarding",
                            5,
                        )
                        execute(
                            "scheduler",
                            "standing-service-survives-personal-offboarding",
                            6,
                        )
                        resource = view.routes[0].effect.resources[0]
                        operator.intervene_governance(
                            GovernanceInterventionRequest(
                                "prod",
                                "acme",
                                "close-road",
                                GovernanceResourceChange("close_resource", resource),
                                "maintenance",
                            )
                        )
                        execute("team", "closed-road", 6, False)
                        operator.intervene_governance(
                            GovernanceInterventionRequest(
                                "prod",
                                "acme",
                                "reopen-road",
                                GovernanceResourceChange("reopen_resource", resource),
                                "maintenance complete",
                            )
                        )
                        execute("team", "reopened-road", 7)
                        change(
                            "withdraw-team-mandate",
                            RevokeWorkforceMandate("mandate-team", 1),
                        )
                        execute("team", "revoked-team-mandate", 7, False)
                        execute("scheduler", "independent-service-mandate", 8)
                        change("disband-reliability", DisbandWorkforceTeam(team, 1))
                        execute("scheduler", "disbanded-team", 8, False)
                        report = {
                            "backend": "memory",
                            "transport": "actual local HTTP",
                            "model_invocations": 0,
                            "elapsed_seconds": round(time.monotonic() - started, 3),
                            "steps": results,
                            "actual_webhook_calls": len(deliveries),
                            "unauthorized_webhook_calls": 0,
                            "deliveries": deliveries,
                            "final_scope": asdict(operator.workforce("prod", "acme")),
                        }
                        args.output.parent.mkdir(parents=True, exist_ok=True)
                        args.output.write_text(json.dumps(report, indent=2) + "\n")
                        print(
                            "| Step | Outcome | Actual webhook calls |\n|---|---|---:|"
                        )
                        for result in results:
                            print(
                                f"| {result['step']} | {result['outcome']} | {result['actual_webhook_calls']} |"
                            )
                        print(
                            f"\nPASS: 8 authorized sends, 0 unauthorized sends. Report: {args.output}"
                        )
                finally:
                    process.terminate()
                    try:
                        process.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait(timeout=10)
    finally:
        receiver.shutdown()
        receiver.server_close()
        thread.join(timeout=10)


if __name__ == "__main__":
    main()
