#!/usr/bin/env python3
"""
Audit Cursor MCP server env blocks without printing secret VALUES.

Reads ~/.cursor/mcp.json and prints:
- server name
- command/url (redacted summary)
- env key names (not values)

This is meant to answer: "what envs do we use across MCP servers?"
"""

from __future__ import annotations

import argparse
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Optional


def load_json(path: Path) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def summarize_command(server: dict[str, Any]) -> str:
    if "url" in server:
        return "remote(url)"
    cmd = server.get("command")
    if isinstance(cmd, str) and cmd:
        return cmd
    return "unknown"


def env_keys(server: dict[str, Any]) -> list[str]:
    env = server.get("env")
    if not isinstance(env, dict):
        return []
    keys: list[str] = []
    for k, v in env.items():
        if not isinstance(k, str):
            continue
        # Only surface keys that look like env vars.
        if not k:
            continue
        if isinstance(v, str) and v == "":
            # keep empties too (still meaningful)
            keys.append(k)
        elif v is not None:
            keys.append(k)
    keys.sort()
    return keys


@dataclass(frozen=True)
class Row:
    name: str
    kind: str
    env_keys: list[str]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--mcp-json",
        default=str(Path.home() / ".cursor" / "mcp.json"),
        help="Path to Cursor MCP config (default: ~/.cursor/mcp.json)",
    )
    ap.add_argument("--json", action="store_true", help="Emit JSON instead of text")
    args = ap.parse_args()

    path = Path(args.mcp_json)
    cfg = load_json(path)
    servers = cfg.get("mcpServers")
    if not isinstance(servers, dict):
        raise SystemExit("mcpServers missing or not an object")

    rows: list[Row] = []
    for name, server in servers.items():
        if not isinstance(name, str) or not isinstance(server, dict):
            continue
        rows.append(Row(name=name, kind=summarize_command(server), env_keys=env_keys(server)))

    rows.sort(key=lambda r: r.name)

    if args.json:
        out = {
            "schema_version": 1,
            "path": str(path),
            "servers": [
                {"name": r.name, "kind": r.kind, "env_keys": r.env_keys, "env_key_count": len(r.env_keys)}
                for r in rows
            ],
        }
        print(json.dumps(out, indent=2))
        return

    print(f"MCP env audit: {path}")
    for r in rows:
        keys = ", ".join(r.env_keys) if r.env_keys else "(none)"
        print(f"- {r.name} [{r.kind}] env: {keys}")


if __name__ == "__main__":
    main()

