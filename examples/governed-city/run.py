#!/usr/bin/env python3
"""Run real authenticated governance and webhook calls; emit checked results."""

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
from dataclasses import asdict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "clients/python"))
from acteon_client import (  # noqa: E402
    ActeonClient,
    Action,
    GovernanceCredentialRevocation,
    GovernanceInterventionRequest,
    GovernanceLimits,
    GovernancePermitDeclaration,
    GovernancePermitRevocation,
    GovernanceResourceChange,
    GovernanceRoute,
    PermitReference,
    PrincipalIdentity,
    PublishGovernancePermitRequest,
)
from acteon_client.errors import HttpError  # noqa: E402


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--server", type=Path, default=ROOT / "target/debug/acteon-server"
    )
    parser.add_argument(
        "--output", type=Path, default=Path("governed-city-results.json")
    )
    args = parser.parse_args()
    # The public guide must show the exact configuration this real run uses.
    guide = (ROOT / "docs/book/guides/governed-city.md").read_text()
    marker = "<!-- governance-example-file: acteon.toml -->"
    documented = guide.split(marker, 1)[1].split("```toml\n", 1)[1].split("```", 1)[0]
    actual = (Path(__file__).parent / "acteon.toml").read_text()
    if documented.strip() != actual.strip():
        raise AssertionError("governed-city guide configuration differs from the simulation")
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
            keys = {"operator": secrets.token_hex(24), "maya": secrets.token_hex(24)}
            auth = (
                'authority_revision=1\n[settings]\njwt_secret="'
                + secrets.token_hex(32)
                + '"\n'
            )
            for name, principal, kind, role, provider, action in [
                ("operator", "operator", "human", "operator", "", ""),
                ("maya", "agent/maya", "agent", "executor", "incident", "execute"),
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
                    with (
                        ActeonClient(base, api_key=keys["operator"]) as operator,
                        ActeonClient(base, api_key=keys["maya"]) as maya,
                    ):
                        deadline = time.monotonic() + 60
                        while not operator.health():
                            if (
                                process.poll() is not None
                                or time.monotonic() >= deadline
                            ):
                                log.flush()
                                raise RuntimeError(
                                    (directory / "server.log").read_text()
                                )
                            time.sleep(0.03)
                        view = operator.governance("prod", "acme")
                        resources = view.routes[0].effect.resources
                        references = [PermitReference("maya-response", 1)]
                        results = []

                        def record(step, outcome, expected_deliveries):
                            assert len(deliveries) == expected_deliveries, (
                                step,
                                deliveries,
                            )
                            results.append(
                                {
                                    "step": step,
                                    "outcome": outcome,
                                    "actual_webhook_calls": len(deliveries),
                                }
                            )

                        try:
                            maya.governance("prod", "acme")
                            raise AssertionError("executor reached management")
                        except HttpError as error:
                            assert error.status == 403
                            record("executor management denied", "HTTP 403", 0)
                        issued = operator.publish_governance_permit(
                            PublishGovernancePermitRequest(
                                "prod",
                                "acme",
                                "issue-response",
                                0,
                                GovernancePermitDeclaration(
                                    "maya-response",
                                    1,
                                    PrincipalIdentity("agent/maya", "agent"),
                                    [GovernanceRoute("incident", "execute")],
                                    0,
                                    GovernanceLimits(5, 1, 4102444800000),
                                ),
                                "incident response",
                            )
                        )
                        record(
                            "operator issues permit",
                            f"generation {issued.generation}",
                            0,
                        )
                        action = Action(
                            "prod", "acme", "incident", "execute", {"ticket": 42}
                        )
                        outcome = maya.dispatch(action, permits=references)
                        assert outcome.outcome_type == "executed"
                        record("permitted action", outcome.outcome_type, 1)
                        replay = maya.dispatch(action, permits=references)
                        assert replay.outcome_type == "executed"
                        record("same action replay", replay.outcome_type, 1)
                        closed = operator.intervene_governance(
                            GovernanceInterventionRequest(
                                "prod",
                                "acme",
                                "close-endpoint",
                                GovernanceResourceChange(
                                    "close_resource", resources[0]
                                ),
                                "maintenance",
                            )
                        )
                        record(
                            "operator closes resource",
                            f"generation {closed.generation}",
                            1,
                        )
                        blocked = maya.dispatch(
                            Action(
                                "prod", "acme", "incident", "execute", {"ticket": 43}
                            ),
                            permits=references,
                        )
                        assert blocked.outcome_type == "failed"
                        record("closed resource", blocked.outcome_type, 1)
                        operator.intervene_governance(
                            GovernanceInterventionRequest(
                                "prod",
                                "acme",
                                "reopen-endpoint",
                                GovernanceResourceChange(
                                    "reopen_resource", resources[0]
                                ),
                                "maintenance complete",
                            )
                        )
                        resumed = maya.dispatch(
                            Action(
                                "prod", "acme", "incident", "execute", {"ticket": 44}
                            ),
                            permits=references,
                        )
                        assert resumed.outcome_type == "executed"
                        record("reopened resource", resumed.outcome_type, 2)
                        operator.intervene_governance(
                            GovernanceInterventionRequest(
                                "prod",
                                "acme",
                                "revoke-permit",
                                GovernancePermitRevocation("maya-response", 1),
                                "incident resolved",
                            )
                        )
                        revoked = maya.dispatch(
                            Action(
                                "prod", "acme", "incident", "execute", {"ticket": 45}
                            ),
                            permits=references,
                        )
                        assert revoked.outcome_type == "failed"
                        record("revoked permit", revoked.outcome_type, 2)
                        operator.intervene_governance(
                            GovernanceInterventionRequest(
                                "prod",
                                "acme",
                                "offboard-maya",
                                GovernanceCredentialRevocation("credential/maya", 1),
                                "offboarding",
                            )
                        )
                        try:
                            maya.dispatch(
                                Action(
                                    "prod",
                                    "acme",
                                    "incident",
                                    "execute",
                                    {"ticket": 46},
                                ),
                                permits=references,
                            )
                            raise AssertionError("revoked credential executed")
                        except HttpError as error:
                            assert error.status == 403
                            record("offboarded credential", "HTTP 403", 2)
                        report = {
                            "backend": "memory",
                            "transport": "actual local HTTP",
                            "model_invocations": 0,
                            "elapsed_seconds": round(time.monotonic() - started, 3),
                            "steps": results,
                            "actual_webhook_calls": len(deliveries),
                            "unauthorized_webhook_calls": 0,
                            "final_scope": asdict(operator.governance("prod", "acme")),
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
                            f"\nPASS: 2 authorized sends, 0 unauthorized sends. Report: {args.output}"
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
