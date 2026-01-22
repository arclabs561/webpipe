#!/usr/bin/env python3
"""
Install (or update) the `webpipe` MCP server entry in Cursor's `~/.cursor/mcp.json`.

Safety:
- Creates a timestamped backup next to the original file.
- Does not print secrets or file contents.
- Does not remove/disable existing servers unless explicitly requested.
"""

from __future__ import annotations

import json
import os
import shutil
import sys
import argparse
import re
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path
from typing import Any, NoReturn


def fail(msg: str) -> NoReturn:
    print(msg, file=sys.stderr)
    raise SystemExit(2)


@dataclass(frozen=True)
class Paths:
    mcp_json: Path
    workspace_root: Path
    webpipe_manifest: Path
    webpipe_bin: Path
    cache_dir: Path


def default_paths() -> Paths:
    home = Path.home()
    mcp_json = home / ".cursor" / "mcp.json"

    workspace_root = Path(os.environ.get("WEBPIPE_WORKSPACE_ROOT", Path.cwd()))
    webpipe_manifest = workspace_root / "webpipe" / "crates" / "webpipe-mcp" / "Cargo.toml"
    webpipe_bin = workspace_root / "webpipe" / "target" / "release" / "webpipe"
    cache_dir = workspace_root / "_scratch" / "webpipe-cache"

    return Paths(
        mcp_json=mcp_json,
        workspace_root=workspace_root,
        webpipe_manifest=webpipe_manifest,
        webpipe_bin=webpipe_bin,
        cache_dir=cache_dir,
    )


def load_json(path: Path) -> dict[str, Any]:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        fail(f"missing Cursor MCP config at: {path}")
    except json.JSONDecodeError as e:
        fail(f"invalid JSON in {path}: {e}")


def atomic_backup(path: Path) -> Path:
    ts = datetime.now().strftime("%Y%m%d-%H%M%S")
    backup = path.with_suffix(path.suffix + f".bak-{ts}")
    shutil.copy2(path, backup)
    return backup


def write_json(path: Path, data: dict) -> None:
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, sort_keys=False) + "\n", encoding="utf-8")
    tmp.replace(path)


def ensure_webpipe_server(cfg: dict, paths: Paths, *, launch_mode: str) -> None:
    cfg.setdefault("mcpServers", {})

    if launch_mode == "bin":
        webpipe_entry = {
            "command": str(paths.webpipe_bin),
            "args": ["mcp-stdio"],
            "env": {
                # Deterministic anchors; no API keys here.
                "CURSOR_WORKSPACE_ROOT": str(paths.workspace_root),
                "WEBPIPE_CACHE_DIR": str(paths.cache_dir),
            },
        }
    else:
        # "cargo" mode: rebuilds on launch; can fail to start if build is broken.
        webpipe_entry = {
            "command": "cargo",
            "args": [
                "run",
                "--quiet",
                "--release",
                "--manifest-path",
                str(paths.webpipe_manifest),
                "--bin",
                "webpipe",
                "--",
                "mcp-stdio",
            ],
            "env": {
                # Deterministic anchors; no API keys here.
                "CURSOR_WORKSPACE_ROOT": str(paths.workspace_root),
                "WEBPIPE_CACHE_DIR": str(paths.cache_dir),
            },
        }

    cfg["mcpServers"]["webpipe"] = webpipe_entry


def maybe_copy_env_keys(cfg: dict[str, Any], *, copy_keys: bool) -> list[str]:
    if not copy_keys:
        return []

    servers = cfg.get("mcpServers") or {}
    if not isinstance(servers, dict):
        return []

    copied: list[str] = []

    webpipe = servers.get("webpipe") or {}
    if not isinstance(webpipe, dict):
        return []
    webpipe_env = webpipe.get("env") or {}
    if not isinstance(webpipe_env, dict):
        return []

    def copy_from(server_name: str, src_key: str, dst_key: str) -> None:
        nonlocal copied
        srv = servers.get(server_name) or {}
        if not isinstance(srv, dict):
            return
        env = srv.get("env") or {}
        if not isinstance(env, dict):
            return
        v = env.get(src_key)
        if isinstance(v, str) and v:
            webpipe_env[dst_key] = v
            copied.append(dst_key)

    # Reuse existing values already in your Cursor MCP config.
    copy_from("tavily-remote-mcp", "TAVILY_API_KEY", "WEBPIPE_TAVILY_API_KEY")
    copy_from("firecrawl-mcp", "FIRECRAWL_API_KEY", "WEBPIPE_FIRECRAWL_API_KEY")

    # Write env back (mutated in place).
    webpipe["env"] = webpipe_env
    servers["webpipe"] = webpipe
    cfg["mcpServers"] = servers
    return copied


_ENV_ASSIGN_RE = re.compile(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)\s*$")


def _parse_env_file_no_print(path: Path) -> dict[str, str]:
    """
    Parse a .env-style file into key->value WITHOUT printing values.
    Best-effort only; supports simple KEY=VALUE lines and ignores comments.
    """
    try:
        raw = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        raw = path.read_text(encoding="latin-1")
    except FileNotFoundError:
        return {}

    out: dict[str, str] = {}
    for line in raw.splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        m = _ENV_ASSIGN_RE.match(line)
        if not m:
            continue
        k = m.group(1)
        v = m.group(2).strip()
        # Strip surrounding quotes for convenience.
        if (len(v) >= 2) and ((v[0] == v[-1] == '"') or (v[0] == v[-1] == "'")):
            v = v[1:-1]
        out[k] = v
    return out


def maybe_copy_keys_from_env_files(
    cfg: dict[str, Any],
    *,
    env_files: list[Path],
    overwrite: bool,
) -> list[str]:
    """
    Copy a small allowlist of keys from repo .env files into webpipe's env block.
    Never prints values.
    """
    servers = cfg.get("mcpServers") or {}
    if not isinstance(servers, dict):
        return []
    webpipe = servers.get("webpipe") or {}
    if not isinstance(webpipe, dict):
        return []
    webpipe_env = webpipe.get("env") or {}
    if not isinstance(webpipe_env, dict):
        return []

    # Source key -> destination key in webpipe env
    mapping = {
        "TAVILY_API_KEY": "WEBPIPE_TAVILY_API_KEY",
        "FIRECRAWL_API_KEY": "WEBPIPE_FIRECRAWL_API_KEY",
        "BRAVE_SEARCH_API_KEY": "WEBPIPE_BRAVE_API_KEY",
        "WEBPIPE_TAVILY_API_KEY": "WEBPIPE_TAVILY_API_KEY",
        "WEBPIPE_FIRECRAWL_API_KEY": "WEBPIPE_FIRECRAWL_API_KEY",
        "WEBPIPE_BRAVE_API_KEY": "WEBPIPE_BRAVE_API_KEY",
    }

    copied: list[str] = []
    for p in env_files:
        kv = _parse_env_file_no_print(p)
        for src, dst in mapping.items():
            v = kv.get(src)
            if not isinstance(v, str) or not v:
                continue
            if (dst in webpipe_env) and (not overwrite):
                continue
            webpipe_env[dst] = v
            copied.append(dst)

    webpipe["env"] = webpipe_env
    servers["webpipe"] = webpipe
    cfg["mcpServers"] = servers
    return sorted(set(copied))


def maybe_copy_keys_from_mcp_backups(
    cfg: dict[str, Any],
    *,
    backup_paths: list[Path],
    overwrite: bool,
) -> list[str]:
    """
    Copy web-relevant API keys from older ~/.cursor/mcp.json backups into the webpipe env.
    Never prints values.
    """
    servers = cfg.get("mcpServers") or {}
    if not isinstance(servers, dict):
        return []
    webpipe = servers.get("webpipe") or {}
    if not isinstance(webpipe, dict):
        return []
    webpipe_env = webpipe.get("env") or {}
    if not isinstance(webpipe_env, dict):
        return []

    # Source key -> destination key in webpipe env
    mapping = {
        "TAVILY_API_KEY": "WEBPIPE_TAVILY_API_KEY",
        "FIRECRAWL_API_KEY": "WEBPIPE_FIRECRAWL_API_KEY",
        "BRAVE_SEARCH_API_KEY": "WEBPIPE_BRAVE_API_KEY",
        "WEBPIPE_TAVILY_API_KEY": "WEBPIPE_TAVILY_API_KEY",
        "WEBPIPE_FIRECRAWL_API_KEY": "WEBPIPE_FIRECRAWL_API_KEY",
        "WEBPIPE_BRAVE_API_KEY": "WEBPIPE_BRAVE_API_KEY",
    }

    copied: list[str] = []

    def try_copy_from_env(env: dict[str, Any]) -> None:
        nonlocal copied
        for src, dst in mapping.items():
            v = env.get(src)
            if not isinstance(v, str) or not v:
                continue
            if (dst in webpipe_env) and (not overwrite):
                continue
            webpipe_env[dst] = v
            copied.append(dst)

    # Search backups newest-first (caller should pass in that order).
    for p in backup_paths:
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
        except Exception:
            continue
        bservers = data.get("mcpServers") or {}
        if not isinstance(bservers, dict):
            continue
        # Primary: legacy server blocks from older setups.
        for legacy_name in ("tavily-remote-mcp", "firecrawl-mcp"):
            srv = bservers.get(legacy_name) or {}
            if isinstance(srv, dict):
                env = srv.get("env") or {}
                if isinstance(env, dict):
                    try_copy_from_env(env)
        # Also: direct keys already under webpipe in some backups.
        w = bservers.get("webpipe") or {}
        if isinstance(w, dict):
            env = w.get("env") or {}
            if isinstance(env, dict):
                try_copy_from_env(env)

        # If we found everything we want, stop early.
        need = {"WEBPIPE_TAVILY_API_KEY", "WEBPIPE_FIRECRAWL_API_KEY", "WEBPIPE_BRAVE_API_KEY"}
        if need.issubset(set(webpipe_env.keys())):
            break

    webpipe["env"] = webpipe_env
    servers["webpipe"] = webpipe
    cfg["mcpServers"] = servers
    return sorted(set(copied))


def prune_servers(
    cfg: dict[str, Any],
    *,
    prune_legacy: bool,
    only_webpipe: bool,
) -> list[str]:
    servers = cfg.get("mcpServers") or {}
    if not isinstance(servers, dict):
        return []

    removed: list[str] = []

    if only_webpipe:
        for name in list(servers.keys()):
            if name != "webpipe":
                servers.pop(name, None)
                removed.append(name)
        cfg["mcpServers"] = servers
        return sorted(set(removed))

    if prune_legacy:
        legacy = [
            # Common legacy names from earlier setup.
            "tavily-remote-mcp",
            "firecrawl-mcp",
        ]
        for name in legacy:
            if name in servers:
                servers.pop(name, None)
                removed.append(name)

    cfg["mcpServers"] = servers
    return sorted(set(removed))


def main() -> None:
    paths = default_paths()

    if not paths.webpipe_manifest.exists():
        fail(
            "could not find webpipe manifest at "
            + str(paths.webpipe_manifest)
            + "\nSet WEBPIPE_WORKSPACE_ROOT to your workspace root (the folder containing `webpipe/`)."
        )

    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--launch-mode",
        choices=["bin", "cargo"],
        default="bin",
        help="How Cursor should launch webpipe: 'bin' (stable prebuilt binary) or 'cargo' (rebuild on launch).",
    )
    ap.add_argument(
        "--copy-keys",
        action="store_true",
        help="Copy existing API keys from other MCP server env blocks into the webpipe server env (no printing).",
    )
    ap.add_argument(
        "--copy-env-files",
        action="store_true",
        help="Also copy web-relevant keys from repo .env files into the webpipe server env (no printing).",
    )
    ap.add_argument(
        "--env-file",
        action="append",
        default=[],
        help="A repo .env file path (relative to workspace root) to consider when --copy-env-files is set. Repeatable.",
    )
    ap.add_argument(
        "--overwrite",
        action="store_true",
        help="When copying keys, overwrite existing webpipe env values (still never prints values).",
    )
    ap.add_argument(
        "--copy-mcp-backups",
        action="store_true",
        help="Copy web-relevant keys from ~/.cursor/mcp.json.bak-* files into webpipe env (no printing).",
    )
    ap.add_argument(
        "--mcp-backup",
        action="append",
        default=[],
        help="A specific ~/.cursor/mcp.json.bak-* file to read (repeatable). If omitted, auto-discovers in ~/.cursor/.",
    )
    ap.add_argument(
        "--prune-legacy",
        action="store_true",
        help="Remove known legacy MCP server entries (currently: tavily-remote-mcp, firecrawl-mcp).",
    )
    ap.add_argument(
        "--only-webpipe",
        action="store_true",
        help="Remove ALL MCP servers except 'webpipe'.",
    )
    ap.add_argument(
        "--dry-run",
        action="store_true",
        help="Print planned changes (no writes, no backups).",
    )
    args = ap.parse_args()

    cfg = load_json(paths.mcp_json)

    ensure_webpipe_server(cfg, paths, launch_mode=str(args.launch_mode))
    copied = maybe_copy_env_keys(cfg, copy_keys=bool(args.copy_keys))
    copied_from_env: list[str] = []
    if bool(args.copy_env_files):
        rels = [Path(p) for p in (args.env_file or [])]
        env_files = [(paths.workspace_root / p) for p in rels]
        copied_from_env = maybe_copy_keys_from_env_files(
            cfg,
            env_files=env_files,
            overwrite=bool(args.overwrite),
        )
    copied_from_backups: list[str] = []
    if bool(args.copy_mcp_backups):
        explicit = [Path(p).expanduser() for p in (args.mcp_backup or [])]
        if explicit:
            backups = explicit
        else:
            # Auto-discover in ~/.cursor (newest-first by filename timestamp in suffix).
            cur = Path.home() / ".cursor"
            backups = sorted(cur.glob("mcp.json.bak-*"), reverse=True)
        copied_from_backups = maybe_copy_keys_from_mcp_backups(
            cfg,
            backup_paths=backups,
            overwrite=bool(args.overwrite),
        )
    removed = prune_servers(
        cfg,
        prune_legacy=bool(args.prune_legacy),
        only_webpipe=bool(args.only_webpipe),
    )

    if bool(args.dry_run):
        print(f"DRY RUN: would install/update 'webpipe' server in {paths.mcp_json}")
        if str(args.launch_mode) == "bin" and not paths.webpipe_bin.exists():
            print(
                f"DRY RUN: NOTE: webpipe binary not found at {paths.webpipe_bin} "
                "(build it with: cargo build --release --manifest-path webpipe/crates/webpipe-mcp/Cargo.toml)"
            )
        if copied:
            print(f"DRY RUN: would copy keys into webpipe env: {', '.join(sorted(set(copied)))}")
        if copied_from_env:
            print(
                "DRY RUN: would copy keys from repo .env files into webpipe env: "
                + ", ".join(sorted(set(copied_from_env)))
            )
        if copied_from_backups:
            print(
                "DRY RUN: would copy keys from mcp.json backups into webpipe env: "
                + ", ".join(sorted(set(copied_from_backups)))
            )
        if removed:
            print(f"DRY RUN: would remove MCP servers: {', '.join(removed)}")
        return

    backup = atomic_backup(paths.mcp_json)
    write_json(paths.mcp_json, cfg)

    # Success message without leaking file contents.
    print(f"OK: installed/updated 'webpipe' server in {paths.mcp_json}")
    print(f"Backup: {backup}")
    if str(args.launch_mode) == "bin" and not paths.webpipe_bin.exists():
        print(
            f"NOTE: webpipe binary not found at {paths.webpipe_bin}. "
            "Build it with: cargo build --release --manifest-path webpipe/crates/webpipe-mcp/Cargo.toml"
        )
    if copied:
        print(f"Copied keys into webpipe env: {', '.join(sorted(set(copied)))}")
    if copied_from_env:
        print(
            "Copied keys from repo .env files into webpipe env: "
            + ", ".join(sorted(set(copied_from_env)))
        )
    if copied_from_backups:
        print(
            "Copied keys from mcp.json backups into webpipe env: "
            + ", ".join(sorted(set(copied_from_backups)))
        )
    if removed:
        print(f"Removed MCP servers: {', '.join(removed)}")
    print("Next: add WEBPIPE_BRAVE_API_KEY in the webpipe server env if desired.")


if __name__ == "__main__":
    main()

