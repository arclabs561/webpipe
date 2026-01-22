#!/usr/bin/env python3
"""
Audit .env files under a root directory WITHOUT printing secret values.

Output includes:
- files discovered
- key names per file
- likely-secret key names
- duplicates across files
- placeholder/empty detection (best-effort, value is never printed)

This is intended for local hygiene reviews (not a secret scanner).
"""

from __future__ import annotations

import argparse
import json
import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any, cast


_ASSIGN_RE = re.compile(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)\s*$")


def _looks_like_secret_key(k: str) -> bool:
    u = k.upper()
    needles = [
        "API_KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PRIVATE_KEY",
        "ACCESS_KEY",
        "SESSION",
        "COOKIE",
    ]
    return any(n in u for n in needles)


def _classify_value(raw: str) -> str:
    """
    Classify value *without* returning it.
    """
    s = raw.strip()
    if s == "":
        return "empty"
    # strip quotes for classification only
    if (len(s) >= 2) and ((s[0] == s[-1] == '"') or (s[0] == s[-1] == "'")):
        inner = s[1:-1]
    else:
        inner = s
    inner_stripped = inner.strip()
    if inner_stripped == "":
        return "empty"
    if inner_stripped in {"<SET_ME>", "<REDACTED>", "CHANGEME"}:
        return "placeholder"
    if "your_" in inner_stripped.lower() or "replace" in inner_stripped.lower():
        return "placeholder"
    if inner_stripped in {"0", "1", "true", "false"}:
        return "boolish"
    if inner_stripped.isdigit():
        return "numeric"
    return "present"


@dataclass(frozen=True)
class ParsedEnv:
    keys: list[str]
    key_meta: dict[str, dict[str, Any]]
    parse_errors: int


def parse_env_file(path: Path) -> ParsedEnv:
    keys: list[str] = []
    key_meta: dict[str, dict[str, Any]] = {}
    parse_errors = 0

    try:
        raw = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        # Fall back to latin-1 best-effort; we still do not surface values.
        raw = path.read_text(encoding="latin-1")

    for line in raw.splitlines():
        s = line.strip()
        if not s or s.startswith("#"):
            continue
        m = _ASSIGN_RE.match(line)
        if not m:
            parse_errors += 1
            continue
        k = m.group(1)
        v = m.group(2)
        keys.append(k)
        key_meta[k] = {
            "likely_secret_key": _looks_like_secret_key(k),
            "value_class": _classify_value(v),
        }

    return ParsedEnv(keys=keys, key_meta=key_meta, parse_errors=parse_errors)


def iter_env_files(root: Path, *, max_files: int) -> list[Path]:
    # Explicit allowlist of env-like filenames (avoid grabbing random *.env assets).
    allowed = {
        ".env",
        ".env.local",
        ".env.production",
        ".env.development",
        ".env.test",
        ".env.example",
        "dev.env",
    }

    # Also allow common prefixed variants: .env.* and *.env
    def is_env_name(name: str) -> bool:
        if name in allowed:
            return True
        if name.startswith(".env."):
            return True
        if name.endswith(".env"):
            return True
        return False

    deny_dirs = {
        ".git",
        "node_modules",
        "target",
        ".venv",
        "dist",
        "build",
        ".next",
        ".turbo",
        "_scratch",
        "_private",
        "_archive",
        "_trash",
        "integration_test_tmp",
    }

    out: list[Path] = []
    for dirpath, dirnames, filenames in os.walk(root):
        # prune
        dirnames[:] = [d for d in dirnames if d not in deny_dirs and not d.startswith(".git")]
        for fn in filenames:
            if is_env_name(fn):
                out.append(Path(dirpath) / fn)
                if max_files > 0 and len(out) >= max_files:
                    break
        if max_files > 0 and len(out) >= max_files:
            break

    # Stable ordering for diffs
    out.sort(key=lambda p: str(p))
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--root", default=".", help="Root directory to scan (default: .)")
    ap.add_argument(
        "--path",
        action="append",
        default=[],
        help="Specific .env file path to parse (repeatable). If set, no directory walk is performed.",
    )
    ap.add_argument("--json", action="store_true", help="Emit JSON (default: text)")
    ap.add_argument("--max-files", type=int, default=500, help="Hard cap on files scanned")
    args = ap.parse_args()

    root = Path(args.root).resolve()
    max_files = max(0, int(args.max_files))
    explicit_paths = [Path(p).expanduser() for p in cast(list[str], args.path)]
    if explicit_paths:
        files = [p if p.is_absolute() else (root / p) for p in explicit_paths]
    else:
        files = iter_env_files(root, max_files=max_files)

    file_reports: list[dict[str, Any]] = []
    key_to_files: dict[str, list[str]] = {}
    likely_secret_keys: dict[str, list[str]] = {}
    total_parse_errors = 0

    for p in files:
        parsed = parse_env_file(p)
        total_parse_errors += parsed.parse_errors
        rel = str(p.relative_to(root))

        for k in parsed.keys:
            key_to_files.setdefault(k, []).append(rel)
            if parsed.key_meta.get(k, {}).get("likely_secret_key"):
                likely_secret_keys.setdefault(k, []).append(rel)

        file_reports.append(
            {
                "path": rel,
                "key_count": len(parsed.keys),
                "keys": sorted(set(parsed.keys)),
                "parse_errors": parsed.parse_errors,
                "value_classes": {
                    "empty": sum(1 for m in parsed.key_meta.values() if m["value_class"] == "empty"),
                    "placeholder": sum(
                        1 for m in parsed.key_meta.values() if m["value_class"] == "placeholder"
                    ),
                    "present": sum(
                        1 for m in parsed.key_meta.values() if m["value_class"] == "present"
                    ),
                    "numeric": sum(
                        1 for m in parsed.key_meta.values() if m["value_class"] == "numeric"
                    ),
                    "boolish": sum(
                        1 for m in parsed.key_meta.values() if m["value_class"] == "boolish"
                    ),
                },
            }
        )

    duplicates = {k: v for (k, v) in key_to_files.items() if len(set(v)) >= 2}

    payload = {
        "schema_version": 1,
        "root": str(root),
        "file_count": len(files),
        "total_parse_errors": total_parse_errors,
        "files": file_reports,
        "duplicates": {k: sorted(set(v)) for (k, v) in sorted(duplicates.items())},
        "likely_secret_keys": {k: sorted(set(v)) for (k, v) in sorted(likely_secret_keys.items())},
    }

    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=False))
        return

    files_payload = cast(list[dict[str, Any]], payload["files"])
    duplicates_payload = cast(dict[str, list[str]], payload["duplicates"])

    print(f"root: {payload['root']}")
    print(f"env_files: {payload['file_count']}")
    print(f"parse_errors_total: {payload['total_parse_errors']}")
    print("---")
    for f in files_payload:
        vc = f["value_classes"]
        print(
            f"- {f['path']}: keys={f['key_count']} present={vc['present']} placeholder={vc['placeholder']} empty={vc['empty']} parse_errors={f['parse_errors']}"
        )
    print("---")
    print(f"duplicate_keys: {len(duplicates_payload)}")
    # show just a small sample in text mode (still no values)
    for k, paths in list(duplicates_payload.items())[:30]:
        print(f"  - {k}: {', '.join(paths)}")


if __name__ == "__main__":
    main()

