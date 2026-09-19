#!/usr/bin/env python3
"""Execute the quick-start's curl blocks against an isolated real server.

Uses only the Python standard library, bash, curl, and a prebuilt server binary.
Run trusted repository documentation only: marked blocks are executable shell.
"""

import argparse
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
from urllib.error import URLError
from urllib.request import ProxyHandler, build_opener


ROOT = Path(__file__).resolve().parents[2]
GUIDE = ROOT / "docs/book/getting-started/quickstart.md"
CONFIG = "examples/quickstart/acteon.toml"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", required=True, type=Path)
    args = parser.parse_args()
    server = args.server.resolve(strict=True)
    guide = GUIDE.read_text()
    blocks = re.findall(
        r"<!-- quickstart-check: ([a-z]+) -->\n```bash\n(.*?)\n```",
        guide,
        re.DOTALL,
    )
    names = [name for name, _ in blocks]
    require(names == ["health", "dispatch", "suppression", "metrics", "openapi", "batch"],
            "Missing, duplicate, or reordered quick-start checks")
    require(guide.count("curl --fail-with-body") == len(blocks),
            "Every documented curl request must be marked for verification")
    require(f"cargo run --locked -p acteon-server -- -c {CONFIG}" in guide,
            "Documented startup command differs from the tested configuration")
    commands = dict(blocks)
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    env = dict(os.environ, ACTEON_URL=url, NO_PROXY="127.0.0.1", no_proxy="127.0.0.1")
    opener = build_opener(ProxyHandler({}))

    with tempfile.TemporaryFile(mode="w+") as log:
        process = subprocess.Popen(
            [str(server), "-c", CONFIG, "--host", "127.0.0.1", "--port", str(port)],
            cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT,
        )
        try:
            deadline = time.monotonic() + 30
            while True:
                require(process.poll() is None, "Server exited during startup")
                try:
                    with opener.open(url + "/health", timeout=1) as response:
                        require(json.load(response)["status"] == "ok", "Unhealthy server")
                    break
                except (URLError, TimeoutError):
                    require(time.monotonic() < deadline, "Server readiness timed out")
                    time.sleep(0.1)

            def run(name):
                require(process.poll() is None, "Server exited during walkthrough")
                result = subprocess.run(
                    ["bash", "-euo", "pipefail", "-c", commands[name]],
                    cwd=ROOT, env=env, capture_output=True, text=True, timeout=15,
                )
                require(result.returncode == 0,
                        f"{name} failed: {result.stderr}\n{result.stdout}")
                return json.loads(result.stdout)

            executed = {"Executed": {"status": "success", "body": {
                "provider": "email", "logged": True}, "headers": {}}}
            shown_responses = [json.loads(block) for block in re.findall(
                r"```json\n(.*?)\n```", guide, re.DOTALL)]
            require(shown_responses == [executed, "Deduplicated",
                                        {"Suppressed": {"rule": "block-test-emails"}}],
                    "Documented JSON responses differ from the checked contract")
            require(run("health")["status"] == "ok", "Health response mismatch")
            require(run("dispatch") == executed, "First dispatch was not executed")
            require(run("dispatch") == "Deduplicated", "Repeat was not deduplicated")
            require(run("suppression") == {"Suppressed": {"rule": "block-test-emails"}},
                    "Test address was not suppressed")
            metrics = run("metrics")
            for key, expected in {"dispatched": 3, "executed": 1, "deduplicated": 1,
                                  "suppressed": 1, "failed": 0}.items():
                require(metrics[key] == expected, f"Unexpected {key}: {metrics[key]}")
            spec = run("openapi")
            require(spec["openapi"].startswith("3."), "Missing OpenAPI version")
            require("post" in spec["paths"]["/v1/dispatch"], "Missing dispatch schema")
            with opener.open(url + "/swagger-ui/", timeout=5) as response:
                require(b"swagger-ui" in response.read(), "Swagger UI not served")
            require(run("batch") == [executed, executed], "Batch outcomes mismatch")
            metrics = run("metrics")
            require(metrics["dispatched"] == 5 and metrics["executed"] == 3,
                    "Batch metrics mismatch")
            print("Quick start passed: health, execution, deduplication, suppression, "
                  "metrics, OpenAPI, Swagger UI, and batch dispatch")
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
                process.wait(timeout=5)


if __name__ == "__main__":
    main()
