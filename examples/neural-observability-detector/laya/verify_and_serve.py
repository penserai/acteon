#!/usr/bin/env python3
"""Verify the governed Laya runtime and checkpoint before serving traffic."""

from __future__ import annotations

import hashlib
import importlib.metadata
import json
import os
from pathlib import Path

from huggingface_hub import snapshot_download


LOCK_PATH = Path("/opt/acteon/model.lock.json")
RUNTIME_DISTRIBUTIONS = {
    "laya": "laya",
    "torch": "torch",
    "transformers": "transformers",
    "huggingface_hub": "huggingface-hub",
    "safetensors": "safetensors",
    "numpy": "numpy",
}
ROOT_FIELDS = {
    "schema_version",
    "runtime",
    "checkpoint",
    "artifacts",
    "question_sets",
}


def fail(message: str) -> None:
    raise SystemExit(f"model governance rejected startup: {message}")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as artifact:
        for chunk in iter(lambda: artifact.read(1024 * 1024), b""):
            digest.update(chunk)
    return f"sha256:{digest.hexdigest()}"


def main() -> None:
    lock = json.loads(LOCK_PATH.read_text(encoding="utf-8"))
    if set(lock) != ROOT_FIELDS or lock["schema_version"] != 1:
        fail("unsupported model.lock.json schema")

    runtime = lock["runtime"]
    if set(runtime) != set(RUNTIME_DISTRIBUTIONS):
        fail("runtime package set differs from the lock")
    for name, distribution in RUNTIME_DISTRIBUTIONS.items():
        actual = importlib.metadata.version(distribution)
        if actual != runtime[name]:
            fail(f"{name} version {actual} differs from locked {runtime[name]}")

    checkpoint = lock["checkpoint"]
    repository = os.environ.get("LAYA_REPOSITORY", "")
    model = os.environ.get("LAYA_MODELS", "")
    revision = os.environ.get("LAYA_REVISION", "")
    if repository != checkpoint["repository"]:
        fail(f"repository {repository!r} differs from the lock")
    if model != checkpoint["name"]:
        fail(f"checkpoint {model!r} differs from the lock")
    if revision != checkpoint["revision"]:
        fail(f"revision {revision!r} differs from the lock")

    snapshot = Path(
        snapshot_download(
            repo_id=repository,
            revision=revision,
            allow_patterns=[f"{model}/**"],
        )
    )
    model_root = snapshot / model
    actual_files = {
        str(path.relative_to(model_root)): path
        for path in model_root.rglob("*")
        if path.is_file()
    }
    artifacts = lock["artifacts"]
    if set(actual_files) != set(artifacts):
        fail(
            "checkpoint files differ from the lock: "
            f"expected {sorted(artifacts)}, got {sorted(actual_files)}"
        )
    for relative_path, expected in artifacts.items():
        actual = sha256(actual_files[relative_path])
        if actual != expected:
            fail(f"{relative_path} digest {actual} differs from locked {expected}")

    print(
        f"model governance verified {repository}/{model}@{revision} "
        f"with {len(artifacts)} artifacts",
        flush=True,
    )
    os.execvp("laya-serve", ["laya-serve"])


if __name__ == "__main__":
    main()
