#!/usr/bin/env python3
"""Exercise the agent guide's checked-in configuration against the real server."""

import re
import argparse
import os
import secrets
import json
import socket
import subprocess
import tempfile
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import ProxyHandler, Request, build_opener

ROOT = Path(__file__).resolve().parents[2]
CONFIG = "examples/agent-swarm-coordination/policy-demo/acteon.toml"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--server", type=Path, required=True)
    args = parser.parse_args()
    guide = (ROOT / "docs/book/guides/agent-swarm-coordination.md").read_text()
    blocks = re.findall(
        r"<!-- agent-guide-file: (.*?) -->\n```(?:toml|yaml)\n(.*?)```", guide, re.S
    )
    assert {name for name, _ in blocks} == {
        "acteon.toml",
        "auth.toml",
        "quotas.toml",
        "rules/policy.yaml",
    }
    for name, content in blocks:
        assert (
            content
            == (
                ROOT / "examples/agent-swarm-coordination/policy-demo" / name
            ).read_text()
        ), name

    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    base = f"http://127.0.0.1:{port}"
    opener = build_opener(ProxyHandler({}))

    def request(path, body=None, method=None):
        data = None if body is None else json.dumps(body).encode()
        req = Request(
            base + path,
            data=data,
            method=method,
            headers={
                "Authorization": "Bearer acteon-policy-demo",
                "Content-Type": "application/json",
            },
        )
        with opener.open(req, timeout=10) as response:
            return json.load(response)

    def dispatch(kind, **extra):
        action = {
            "id": str(uuid.uuid4()),
            "created_at": datetime.now(timezone.utc).isoformat(),
            "namespace": "agent-swarm",
            "tenant": "researcher",
            "provider": "research",
            "action_type": kind,
            "payload": {"query": "Summarize local evidence"},
            **extra,
        }
        return request("/v1/dispatch", action)

    with tempfile.TemporaryFile(mode="w+") as log:
        process = subprocess.Popen(
            [
                str(args.server.resolve()),
                "-c",
                CONFIG,
                "--host",
                "127.0.0.1",
                "--port",
                str(port),
            ],
            cwd=ROOT,
            stdout=log,
            stderr=subprocess.STDOUT,
            env={**os.environ, "ACTEON_AUTH_KEY": secrets.token_hex(32)},
        )
        try:
            deadline = time.monotonic() + 20
            while True:
                if process.poll() is not None:
                    raise RuntimeError("Guide server exited during startup")
                try:
                    request("/health")
                    break
                except URLError:
                    if time.monotonic() > deadline:
                        raise RuntimeError("Guide server readiness timeout") from None
                    time.sleep(0.1)
            # Even a correctly scoped runtime credential cannot alter policy.
            for path, method in [
                ("/v1/rules/reload", "POST"),
                ("/v1/quotas", "POST"),
                ("/v1/bus/agents", "POST"),
                ("/v1/chains/definitions/research", "PUT"),
            ]:
                try:
                    request(path, {}, method)
                    raise AssertionError(f"Executor reached administration: {path}")
                except HTTPError as error:
                    assert error.code == 403, (path, error.code)
            assert "Executed" in dispatch("search", dedup_key="guide-research")
            assert dispatch("search", dedup_key="guide-research") == "Deduplicated"
            assert "Suppressed" in dispatch("delete_database")
            assert "Suppressed" in dispatch("unknown_operation")
            assert "Suppressed" in dispatch("search", provider="deploy")
            assert "Suppressed" in dispatch("research_request", provider="deploy")
            assert "Suppressed" in dispatch("deploy", provider="research")
            try:
                dispatch("search", tenant="another-team")
                raise AssertionError("Cross-tenant dispatch was accepted")
            except HTTPError as error:
                assert error.code == 403
            approval = dispatch("deploy", provider="deploy")["PendingApproval"]
            # Use the exact signed capability returned by this server.
            from urllib.parse import urlsplit

            url = urlsplit(approval["approve_url"])
            decision = request(url.path + "?" + url.query, {}, "POST")
            assert decision["status"] == "approved", decision
            assert "Executed" in decision["outcome"], decision
            chain = dispatch("research_request")["ChainStarted"]
            deadline = time.monotonic() + 15
            while True:
                state = request(
                    f"/v1/chains/{chain['chain_id']}?namespace=agent-swarm&tenant=researcher"
                )
                if state["status"] == "completed":
                    break
                assert time.monotonic() < deadline, state
                time.sleep(0.2)
            quotas = request("/v1/quotas?namespace=agent-swarm&tenant=researcher")
            assert quotas["count"] == 1, quotas
            assert quotas["quotas"][0]["max_actions"] == 50, quotas
            print(
                "Agent guide passed: execution-only role, scoped auth, control-plane denial, execution, deduplication, forbidden/unknown suppression, approval, chain completion, static quota loading"
            )
        except BaseException:
            log.seek(0)
            print(log.read())
            raise
        finally:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == "__main__":
    main()
