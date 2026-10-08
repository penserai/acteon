#!/usr/bin/env python3
"""Require a reviewed permission assignment for every registered operation.

No automatic classification: new routes must update the checked inventory.
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts/sdk"))
from platform_catalog import registered_operations  # noqa: E402


def main():
    rows = json.loads(
        (ROOT / "crates/server/src/auth/route_permissions.json").read_text()
    )
    actual = [
        {k: op[k] for k in ("name", "method", "path")}
        for op in registered_operations()
    ]
    expected = [{k: op[k] for k in ("name", "method", "path")} for op in rows]
    assert actual == expected, "Registered routes and permission inventory differ"
    assert len({(r["method"], r["path"]) for r in rows}) == len(rows)
    valid = {None, "Dispatch", "Session", "OperationsManage", "AuditRead", "StreamSubscribe", "RulesTest"}
    assert all(row["permission"] in valid for row in rows)
    # Public must match the public router, not merely an inventory label.
    source = (ROOT / "crates/server/src/api/mod.rs").read_text()
    public_source = source.split("let public = Router::new()", 1)[1].split("let protected =", 1)[0]
    import re
    public_handlers = set(re.findall(r"\b(?:get|post|put|delete|patch)\((\w+)::(\w+)\)", public_source))
    public_names = {module + "_" + handler for module, handler in public_handlers}
    assert {r["name"] for r in rows if r["permission"] is None} == public_names
    print(f"Permission inventory verified: {len(rows)} operations; {len(public_names)} public contracts")


if __name__ == "__main__":
    main()
