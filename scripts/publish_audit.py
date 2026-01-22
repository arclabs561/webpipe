#!/usr/bin/env python3
"""
Guardrail: publishing posture for this repo.

Policy (current):
- Publishable: package name in {"webpipe", "webpipe-core"}.
- Internal-only: everything else must have publish = false.
"""

from __future__ import annotations

import pathlib
import sys
import tomllib


def load_toml(path: pathlib.Path) -> dict:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def package_name_and_publish_flag(cargo_toml: pathlib.Path) -> tuple[str | None, object]:
    d = load_toml(cargo_toml)
    pkg = d.get("package", {})
    return pkg.get("name"), pkg.get("publish", None)


def main() -> int:
    repo_root = pathlib.Path(__file__).resolve().parents[1]
    ws = load_toml(repo_root / "Cargo.toml")
    members = ws.get("workspace", {}).get("members", [])
    if not members:
        print("publish_audit: no workspace.members found", file=sys.stderr)
        return 2

    errors: list[str] = []

    for m in members:
        cargo_toml = (repo_root / m / "Cargo.toml").resolve()
        name, publish = package_name_and_publish_flag(cargo_toml)
        if name is None:
            errors.append(f"{cargo_toml}: missing [package].name")
            continue

        if name in {"webpipe", "webpipe-core"}:
            # publishable: allow publish unset or true.
            if publish is False:
                errors.append(f"{cargo_toml}: {name} should be publishable (publish=false found)")
        else:
            # internal-only: must be explicitly publish=false.
            if publish is not False:
                errors.append(
                    f"{cargo_toml}: {name} must be internal-only (set publish = false)"
                )

    if errors:
        print("publish_audit: FAILED", file=sys.stderr)
        for e in errors:
            print(f"- {e}", file=sys.stderr)
        return 1

    print("publish_audit: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

