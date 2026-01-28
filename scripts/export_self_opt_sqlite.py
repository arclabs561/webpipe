#!/usr/bin/env python3
"""
One-shot exporter: build a SQLite database for webpipe self-optimization.

Sources (best-effort, metadata-only):
- Cursor tool-use telemetry via `chatvault aggregate-tool-calls` (counts + earliest/latest ids)
- webpipe eval/judge transcripts (`*.transcript.jsonl`) if present on disk
- webpipe eval artifacts (`webpipe-eval-judge-swarm-*.json`, etc.) if present

Privacy posture:
- This exporter intentionally avoids storing raw extracted page text, prompts, or model raw outputs.
- It stores only counts, timings, booleans, and small categorical fields (tool name, stage, backend).
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import os
import re
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any, Dict, Iterable, Optional, Tuple


def utc_now_iso() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def die(msg: str) -> "NoReturn":
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(2)


def ensure_parent_dir(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)


def run_chatvault_aggregate_tool_calls(
    chatvault_bin: Path,
    max_keys: int,
    top: int,
    include_prefixes: list[str],
    exclude_substrings: list[str],
) -> Dict[str, Any]:
    """
    Runs chatvault with output_path to avoid stdout truncation.
    Returns the parsed JSON payload (full, from the file).
    """
    with tempfile.TemporaryDirectory(prefix="webpipe-self-opt-") as td:
        out_path = Path(td) / "chatvault_aggregate_tool_calls.json"
        cmd = [
            str(chatvault_bin),
            "aggregate-tool-calls",
            "--max-keys",
            str(max_keys),
            "--top",
            str(top),
            "--verbose",
            "--output-path",
            str(out_path),
            "--return-inline=false",
        ]
        for p in include_prefixes:
            cmd += ["--include-prefix", p]
        for s in exclude_substrings:
            cmd += ["--exclude-substring", s]

        # The command prints a small summary to stdout; ignore it.
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        return json.loads(out_path.read_text("utf-8"))


def sql_connect(path: Path) -> sqlite3.Connection:
    con = sqlite3.connect(str(path))
    con.row_factory = sqlite3.Row
    con.execute("PRAGMA journal_mode=WAL;")
    con.execute("PRAGMA synchronous=NORMAL;")
    con.execute("PRAGMA foreign_keys=ON;")
    return con


ISSUE_VOCAB: tuple[str, ...] = (
    "missing_key_facts",
    "too_short",
    "boilerplate",
    "low_signal",
    "off_topic",
    "truncated",
    "did_not_hit_expected_url",
)

CRITIC_ISSUE_VOCAB: tuple[str, ...] = (
    "missing_structured_content",
    "missing_or_bad_kind",
    "unexpected_kind",
    "tool_failed",
    "missing_urls",
    "empty_top_chunks",
    "low_signal",
    "warnings_present",
    "missing_markdown",
    "markdown_is_json",
    "markdown_missing_request_section",
    "markdown_missing_summary_section",
)


def normalize_critic_issue_vocab(s: str) -> Optional[str]:
    """
    Critic issues should already be controlled vocabulary; keep mapping strict.
    """
    t = s.strip()
    if not t:
        return None
    if t in CRITIC_ISSUE_VOCAB:
        return t
    # Back-compat / minor drift mapping (best-effort).
    tl = t.lower().replace(" ", "_")
    if tl in CRITIC_ISSUE_VOCAB:
        return tl
    return None


def normalize_critic_issue_list(xs: Any) -> tuple[Optional[str], Optional[str], int]:
    if not (isinstance(xs, list) and all(isinstance(x, str) for x in xs)):
        return None, None, 0
    raw_json = json.dumps(xs, sort_keys=True)
    norm: list[str] = []
    unknown = 0
    for s in xs:
        n = normalize_critic_issue_vocab(s)
        if n is None:
            unknown += 1
            continue
        if n not in norm:
            norm.append(n)
    norm_json = json.dumps(norm, sort_keys=True)
    return raw_json, norm_json, unknown


def normalize_issue_vocab(s: str) -> Optional[str]:
    """
    Map older/free-form judge strings into our controlled vocabulary.
    Returns None when no safe mapping exists.
    """
    t = s.strip().lower()
    if not t:
        return None
    # Normalize separators.
    t = t.replace("_", " ").replace("-", " ")

    # Prefer specific signals first.
    if "expected url" in t or "did not hit expected" in t or "reach the expected" in t:
        return "did_not_hit_expected_url"
    if "trunc" in t or "invalid json" in t or "unfinished" in t or "incomplete" in t:
        return "truncated"
    if "off topic" in t or "unrelated" in t:
        return "off_topic"
    if "low signal" in t or "low-signal" in t or "gunk" in t:
        return "low_signal"
    if "boilerplate" in t or "nav" in t or "toc" in t or "app shell" in t:
        return "boilerplate"
    if "too short" in t or "brevity" in t:
        return "too_short"
    if "missing key" in t or ("missing" in t and ("facts" in t or "results" in t or "explanations" in t)):
        return "missing_key_facts"

    # Exact-match fallback for already-normalized strings.
    k = t.replace(" ", "_")
    if k in ISSUE_VOCAB:
        return k
    return None


def normalize_issue_list(xs: Any) -> tuple[Optional[str], Optional[str], int]:
    """
    Returns (raw_json, norm_json, unknown_count).
    """
    if not (isinstance(xs, list) and all(isinstance(x, str) for x in xs)):
        return None, None, 0
    raw_json = json.dumps(xs, sort_keys=True)
    norm: list[str] = []
    unknown = 0
    for s in xs:
        n = normalize_issue_vocab(s)
        if n is None:
            unknown += 1
            continue
        if n not in norm:
            norm.append(n)
    norm_json = json.dumps(norm, sort_keys=True)
    return raw_json, norm_json, unknown


def _json_load_list(s: Optional[str]) -> list[Any]:
    if not isinstance(s, str) or not s.strip():
        return []
    try:
        v = json.loads(s)
    except Exception:
        return []
    return v if isinstance(v, list) else []


def emit_vlm_report(con: sqlite3.Connection, out_path: Path, top_k_fixes: int = 10) -> None:
    """
    Write a small, actionable report from ingested VLM runs.
    Output is JSON (privacy-safe: no raw page text, only counts + fix strings).
    """
    cur = con.cursor()
    total_runs = cur.execute("select count(*) from webpipe_eval_vlm_runs").fetchone()[0]
    total_inputs = cur.execute("select count(*) from webpipe_eval_vlm_per_input").fetchone()[0]

    by_profile: Dict[str, Dict[str, Any]] = {}
    for r in cur.execute(
        """
        select r.run_key, r.goal_profiles_json, r.model, r.temperature,
               i.score_0_10, i.p0_count, i.p1_count, i.p2_count,
               i.consensus_fixes_json
        from webpipe_eval_vlm_runs r
        join webpipe_eval_vlm_per_input i on i.run_key = r.run_key
        """
    ):
        run_key, gp_json, model, temp, score, p0, p1, p2, fixes_json = r
        profiles = _json_load_list(gp_json) or ["<no_goal_profile>"]
        fixes = [x for x in _json_load_list(fixes_json) if isinstance(x, str)]
        for prof in profiles:
            if not isinstance(prof, str) or not prof.strip():
                continue
            k = prof.strip()
            b = by_profile.setdefault(
                k,
                {
                    "runs": set(),
                    "inputs": 0,
                    "sum_score": 0.0,
                    "score_n": 0,
                    "p0": 0,
                    "p1": 0,
                    "p2": 0,
                    "fix_counts": {},
                    "models": {},
                },
            )
            b["runs"].add(run_key)
            b["inputs"] += 1
            if isinstance(score, (int, float)):
                b["sum_score"] += float(score)
                b["score_n"] += 1
            b["p0"] += int(p0 or 0)
            b["p1"] += int(p1 or 0)
            b["p2"] += int(p2 or 0)
            for f in fixes:
                ff = f.strip()
                if not ff:
                    continue
                b["fix_counts"][ff] = b["fix_counts"].get(ff, 0) + 1
            mk = f"{model or '<unknown_model>'}@{temp if temp is not None else 'na'}"
            b["models"][mk] = b["models"].get(mk, 0) + 1

    # Finalize: make JSON-friendly + compute averages and top fixes.
    prof_rows: list[Dict[str, Any]] = []
    for prof, b in by_profile.items():
        runs_n = len(b["runs"])
        inputs_n = int(b["inputs"])
        score_avg = None
        if b["score_n"] > 0:
            score_avg = b["sum_score"] / float(b["score_n"])
        fix_counts = [(k, v) for k, v in b["fix_counts"].items()]
        fix_counts.sort(key=lambda kv: (-kv[1], kv[0]))
        top_fixes = [{"fix": k, "count": v} for k, v in fix_counts[: max(0, top_k_fixes)]]
        models = [(k, v) for k, v in b["models"].items()]
        models.sort(key=lambda kv: (-kv[1], kv[0]))
        prof_rows.append(
            {
                "goal_profile": prof,
                "runs": runs_n,
                "inputs": inputs_n,
                "avg_score_0_10": score_avg,
                "p0_total": int(b["p0"]),
                "p1_total": int(b["p1"]),
                "p2_total": int(b["p2"]),
                "p0_per_input": (float(b["p0"]) / inputs_n) if inputs_n else None,
                "p1_per_input": (float(b["p1"]) / inputs_n) if inputs_n else None,
                "top_consensus_fixes": top_fixes,
                "model_mix": [{"model_temp": k, "count": v} for k, v in models],
            }
        )
    # Rank: P0 density, then P1 density, then avg score desc.
    def _rank_key(d: Dict[str, Any]) -> tuple:
        p0 = d.get("p0_per_input")
        p1 = d.get("p1_per_input")
        s = d.get("avg_score_0_10")
        return (
            -(p0 if isinstance(p0, (int, float)) else 0.0),
            -(p1 if isinstance(p1, (int, float)) else 0.0),
            -(s if isinstance(s, (int, float)) else 0.0),
            d.get("goal_profile", ""),
        )

    prof_rows.sort(key=_rank_key)

    payload = {
        "schema_version": 1,
        "kind": "webpipe_self_opt_vlm_report",
        "generated_at_utc": utc_now_iso(),
        "totals": {"vlm_runs": int(total_runs), "vlm_inputs": int(total_inputs)},
        "by_goal_profile": prof_rows,
    }
    ensure_parent_dir(out_path)
    out_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def emit_critic_report(
    con: sqlite3.Connection,
    out_path: Path,
    top_k: int = 20,
    src_path_substring: Optional[str] = None,
) -> None:
    """
    Write a small, actionable report from ingested critic transcript events.
    Output is JSON (privacy-safe: no raw page text, only issue counts + boolean rates).
    """
    cur = con.cursor()
    where = "stage='critic'"
    params: tuple[Any, ...] = ()
    if isinstance(src_path_substring, str) and src_path_substring.strip():
        where += " and instr(src_path, ?) > 0"
        params = (src_path_substring.strip(),)

    total = cur.execute(f"select count(*) from webpipe_transcript_events where {where}", params).fetchone()[0]
    ok_md = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and critic_markdown_ok=1",
        params,
    ).fetchone()[0]
    ok_struct = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and critic_structured_ok=1",
        params,
    ).fetchone()[0]

    # Expand critic_issues_norm_json arrays into counts.
    counts: Dict[str, int] = {}
    unknown_total = 0
    for (norm_json, unk) in cur.execute(
        f"select critic_issues_norm_json, critic_issues_unknown_count from webpipe_transcript_events where {where}",
        params,
    ):
        unknown_total += int(unk or 0)
        issues = _json_load_list(norm_json) if isinstance(norm_json, str) else []
        for it in issues:
            if not isinstance(it, str) or not it.strip():
                continue
            k = it.strip()
            counts[k] = counts.get(k, 0) + 1

    top = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[: int(top_k)]
    payload = {
        "schema_version": 1,
        "kind": "webpipe_self_opt_critic_report",
        "generated_at_utc": utc_now_iso(),
        "totals": {
            "critic_events": int(total),
            "critic_markdown_ok": int(ok_md),
            "critic_structured_ok": int(ok_struct),
            "unknown_issue_count": int(unknown_total),
        },
        "top_issue_counts": [{"issue": k, "count": int(v)} for (k, v) in top],
    }
    ensure_parent_dir(out_path)
    out_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def emit_judge_musings_report(
    con: sqlite3.Connection,
    out_path: Path,
    top_k: int = 20,
    src_path_substring: Optional[str] = None,
) -> None:
    """
    Write a small, actionable report from ingested judge musings (dimensions only).
    Output is JSON (privacy-safe: dimensions + counts only).
    """
    cur = con.cursor()
    where = "stage='judge'"
    params: tuple[Any, ...] = ()
    if isinstance(src_path_substring, str) and src_path_substring.strip():
        where += " and instr(src_path, ?) > 0"
        params = (src_path_substring.strip(),)

    total_judge_events = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where}",
        params,
    ).fetchone()[0]
    total_with_musings = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and judge_musing_dimensions_json is not null",
        params,
    ).fetchone()[0]
    unknown_total = cur.execute(
        f"select coalesce(sum(judge_musing_dimensions_unknown_count),0) from webpipe_transcript_events where {where}",
        params,
    ).fetchone()[0]

    counts: Dict[str, int] = {}
    for (dims_json,) in cur.execute(
        f"select judge_musing_dimensions_json from webpipe_transcript_events where {where} and judge_musing_dimensions_json is not null",
        params,
    ):
        dims = _json_load_list(dims_json) if isinstance(dims_json, str) else []
        for d in dims:
            if not isinstance(d, str) or not d.strip():
                continue
            k = d.strip()
            counts[k] = counts.get(k, 0) + 1

    top = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[: int(top_k)]
    payload = {
        "schema_version": 1,
        "kind": "webpipe_self_opt_judge_musings_report",
        "generated_at_utc": utc_now_iso(),
        "totals": {
            "judge_events": int(total_judge_events),
            "judge_events_with_musings": int(total_with_musings),
            "unknown_dimension_count": int(unknown_total),
        },
        "top_dimension_counts": [{"dimension": k, "count": int(v)} for (k, v) in top],
    }
    ensure_parent_dir(out_path)
    out_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def emit_judge_context_report(
    con: sqlite3.Connection,
    out_path: Path,
    src_path_substring: Optional[str] = None,
) -> None:
    """
    Report whether judges had enough context (counts + basic distributions).
    Output is JSON (privacy-safe).
    """
    cur = con.cursor()

    where = "stage='judge'"
    params: tuple[Any, ...] = ()
    if isinstance(src_path_substring, str) and src_path_substring.strip():
        where += " and instr(src_path, ?) > 0"
        params = (src_path_substring.strip(),)

    total = cur.execute(f"select count(*) from webpipe_transcript_events where {where}", params).fetchone()[0]
    has_ctx = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and judge_ctx_evidence_chars is not null",
        params,
    ).fetchone()[0]
    truncated = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and judge_ctx_evidence_truncated=1",
        params,
    ).fetchone()[0]
    low_urls = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and judge_ctx_observed_url_count is not null and judge_ctx_observed_url_count<=0",
        params,
    ).fetchone()[0]
    low_evidence = cur.execute(
        f"select count(*) from webpipe_transcript_events where {where} and judge_ctx_evidence_chars is not null and judge_ctx_evidence_chars<400",
        params,
    ).fetchone()[0]

    # Bucket evidence sizes for quick visibility.
    buckets = {"<400": 0, "400-799": 0, "800-1499": 0, "1500+": 0}
    for (n,) in cur.execute(
        f"select judge_ctx_evidence_chars from webpipe_transcript_events where {where} and judge_ctx_evidence_chars is not null",
        params,
    ):
        if not isinstance(n, int):
            continue
        if n < 400:
            buckets["<400"] += 1
        elif n < 800:
            buckets["400-799"] += 1
        elif n < 1500:
            buckets["800-1499"] += 1
        else:
            buckets["1500+"] += 1

    # Worst cases by evidence_chars (smallest) and by score (lowest), limited to 10.
    smallest = cur.execute(
        f"""
        select call_id, llm_overall_score, judge_ctx_observed_url_count, judge_ctx_warnings_count, judge_ctx_evidence_chars, judge_ctx_evidence_truncated
        from webpipe_transcript_events
        where {where} and judge_ctx_evidence_chars is not null
        order by judge_ctx_evidence_chars asc
        limit 10
        """,
        params,
    ).fetchall()
    lowest_score = cur.execute(
        f"""
        select call_id, llm_overall_score, judge_ctx_observed_url_count, judge_ctx_warnings_count, judge_ctx_evidence_chars, judge_ctx_evidence_truncated
        from webpipe_transcript_events
        where {where} and llm_overall_score is not null
        order by llm_overall_score asc
        limit 10
        """,
        params,
    ).fetchall()

    def row_to_obj(r):
        (call_id, score, urls, warns, ev_chars, ev_tr) = r
        return {
            "call_id": call_id,
            "overall_score": score,
            "observed_url_count": urls,
            "warnings_count": warns,
            "evidence_chars": ev_chars,
            "evidence_truncated": ev_tr,
        }

    payload = {
        "schema_version": 1,
        "kind": "webpipe_self_opt_judge_context_report",
        "generated_at_utc": utc_now_iso(),
        "totals": {
            "judge_events": int(total),
            "judge_events_with_context": int(has_ctx),
            "evidence_truncated": int(truncated),
            "observed_url_count_le_0": int(low_urls),
            "evidence_chars_lt_400": int(low_evidence),
        },
        "evidence_chars_buckets": buckets,
        "worst_by_evidence_chars": [row_to_obj(r) for r in smallest],
        "worst_by_score": [row_to_obj(r) for r in lowest_score],
    }
    ensure_parent_dir(out_path)
    out_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def sql_init(con: sqlite3.Connection) -> None:
    con.executescript(
        """
        CREATE TABLE IF NOT EXISTS meta (
          k TEXT PRIMARY KEY,
          v TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS cursor_tool_counts (
          name TEXT PRIMARY KEY,
          count INTEGER NOT NULL,
          first_rowid INTEGER,
          last_rowid INTEGER,
          first_trace_id TEXT,
          last_trace_id TEXT,
          first_blob_prefix_hex TEXT,
          last_blob_prefix_hex TEXT,
          source_db_path TEXT,
          max_keys INTEGER,
          filters_json TEXT,
          generated_at_utc TEXT
        );

        CREATE TABLE IF NOT EXISTS webpipe_transcript_files (
          path TEXT PRIMARY KEY,
          mtime_epoch_s INTEGER,
          bytes INTEGER
        );

        CREATE TABLE IF NOT EXISTS webpipe_transcript_events (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          src_path TEXT NOT NULL,
          seq INTEGER,
          generated_at_epoch_s INTEGER,
          run_kind TEXT,
          stage TEXT,
          call_id TEXT,
          -- tool stage
          tool_name TEXT,
          tool_elapsed_ms INTEGER,
          tool_ok INTEGER,
          tool_provider TEXT,
          tool_fetch_backend TEXT,
          tool_warning_count INTEGER,
          tool_cache_hit INTEGER,
          tool_args_truncated INTEGER,
          tool_result_truncated INTEGER,
          tool_args_json_ok INTEGER,
          tool_result_json_ok INTEGER,
          tool_args_sha256 TEXT,
          tool_result_sha256 TEXT,
          -- extracted args for web_search_extract (context knobs)
          wse_requested_provider TEXT,
          wse_auto_mode TEXT,
          wse_selection_mode TEXT,
          wse_fetch_backend TEXT,
          wse_agentic INTEGER,
          wse_agentic_selector TEXT,
          wse_url_selection_mode TEXT,
          wse_max_urls INTEGER,
          wse_max_chars INTEGER,
          wse_top_chunks INTEGER,
          wse_max_chunk_chars INTEGER,
          wse_include_text INTEGER,
          wse_include_links INTEGER,
          wse_cache_read INTEGER,
          wse_cache_write INTEGER,
          wse_no_network INTEGER,
          -- parsed result summary (when tool_result_json_ok=1)
          wse_results_count INTEGER,
          wse_top_chunks_count INTEGER,
          -- judge stage (LLM)
          attempt INTEGER,
          llm_backend TEXT,
          llm_model_effective TEXT,
          llm_json_mode INTEGER,
          llm_timeout_ms INTEGER,
          llm_timing_ms INTEGER,
          llm_error_present INTEGER,
          llm_parse_ok INTEGER,
          llm_schema_ok INTEGER,
          prompt_system_chars INTEGER,
          prompt_user_chars INTEGER,
          prompt_truncated INTEGER,
          prompt_system_sha256 TEXT,
          prompt_user_sha256 TEXT,
          response_raw_chars INTEGER,
          response_raw_truncated INTEGER,
          response_raw_sha256 TEXT,
          llm_overall_score REAL,
          -- judge context completeness (from transcript "context")
          judge_ctx_observed_url_count INTEGER,
          judge_ctx_warnings_count INTEGER,
          judge_ctx_evidence_chars INTEGER,
          judge_ctx_evidence_truncated INTEGER,
          -- judge musings (aggregatable; store dimensions only)
          judge_musings_count INTEGER,
          judge_musing_dimensions_json TEXT,
          judge_musing_dimensions_unknown_count INTEGER,
          -- VLM (eval-vlm-run) stage: derived from response.parsed + prompt fields
          vlm_profile TEXT,
          vlm_goal_profiles_json TEXT,
          vlm_score_0_10 REAL,
          vlm_verdict TEXT,
          vlm_issues_count INTEGER,
          vlm_p0_count INTEGER,
          vlm_p1_count INTEGER,
          vlm_p2_count INTEGER,
          vlm_top_3_fixes_json TEXT,
          -- critic stage (deterministic)
          critic_issues_json TEXT,
          critic_issues_norm_json TEXT,
          critic_issues_unknown_count INTEGER,
          critic_markdown_ok INTEGER,
          critic_structured_ok INTEGER,
          FOREIGN KEY (src_path) REFERENCES webpipe_transcript_files(path)
        );

        CREATE INDEX IF NOT EXISTS idx_webpipe_transcript_events_stage
          ON webpipe_transcript_events(run_kind, stage);
        CREATE INDEX IF NOT EXISTS idx_webpipe_transcript_events_tool
          ON webpipe_transcript_events(tool_name);

        -- webpipe eval artifacts (judge swarm, search/fetch/search-extract)
        CREATE TABLE IF NOT EXISTS webpipe_eval_artifact_files (
          path TEXT PRIMARY KEY,
          kind TEXT,
          schema_version INTEGER,
          generated_at_epoch_s INTEGER,
          mtime_epoch_s INTEGER,
          bytes INTEGER
        );

        CREATE TABLE IF NOT EXISTS webpipe_eval_judge_swarm_runs (
          run_key TEXT PRIMARY KEY, -- derived from file path
          generated_at_epoch_s INTEGER,
          llm_backend TEXT,
          llm_model_effective TEXT,
          json_mode INTEGER,
          provider TEXT,
          auto_mode TEXT,
          selection_mode TEXT,
          fetch_backend TEXT,
          trial_set TEXT,
          max_queries INTEGER,
          seed INTEGER
        );

        CREATE TABLE IF NOT EXISTS webpipe_eval_judge_meta (
          run_key TEXT PRIMARY KEY,
          overall_assessment TEXT,
          cross_judge_agreement TEXT,
          top_systemic_failures_json TEXT,
          top_systemic_failures_norm_json TEXT,
          top_systemic_failures_unknown_count INTEGER,
          top_3_fixes_json TEXT,
          recommended_next_experiments_json TEXT,
          top_dimensions_json TEXT,
          top_musings_json TEXT,
          FOREIGN KEY (run_key) REFERENCES webpipe_eval_judge_swarm_runs(run_key)
        );

        CREATE TABLE IF NOT EXISTS webpipe_eval_judge_agent_reports (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          run_key TEXT NOT NULL,
          judge_id TEXT,
          totals_queries INTEGER,
          totals_trials INTEGER,
          totals_llm_ok INTEGER,
          totals_llm_failed INTEGER,
          totals_llm_parse_failed INTEGER,
          hit_expected_url_any_trial INTEGER,
          overall_confidence REAL,
          top_failure_modes_json TEXT,
          top_failure_modes_norm_json TEXT,
          top_failure_modes_unknown_count INTEGER,
          FOREIGN KEY (run_key) REFERENCES webpipe_eval_judge_swarm_runs(run_key)
        );

        CREATE TABLE IF NOT EXISTS webpipe_eval_judge_trials (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          run_key TEXT NOT NULL,
          judge_id TEXT,
          query_id TEXT,
          trial_id TEXT,
          agentic INTEGER,
          agentic_selector TEXT,
          fetch_backend TEXT,
          url_selection_mode TEXT,
          elapsed_ms INTEGER,
          ok INTEGER,
          observed_url_count INTEGER,
          warnings_count INTEGER,
          judge_text_truncated INTEGER,
          judge_overall_score REAL,
          judge_answerable INTEGER,
          judge_relevant INTEGER,
          judge_confidence REAL,
          judge_issues_json TEXT,
          judge_issues_norm_json TEXT,
          judge_issues_unknown_count INTEGER,
          hit_expected_url INTEGER,
          FOREIGN KEY (run_key) REFERENCES webpipe_eval_judge_swarm_runs(run_key)
        );

        CREATE INDEX IF NOT EXISTS idx_webpipe_eval_judge_trials_score
          ON webpipe_eval_judge_trials(judge_overall_score);
        CREATE INDEX IF NOT EXISTS idx_webpipe_eval_judge_trials_cfg
          ON webpipe_eval_judge_trials(agentic, agentic_selector, url_selection_mode, fetch_backend);

        -- VLM eval summaries (eval-vlm-run)
        CREATE TABLE IF NOT EXISTS webpipe_eval_vlm_runs (
          run_key TEXT PRIMARY KEY,
          generated_at_epoch_s INTEGER,
          model TEXT,
          temperature REAL,
          trials INTEGER,
          images INTEGER,
          totals_runs INTEGER,
          totals_parsed_ok INTEGER,
          goals_count INTEGER,
          goal_profiles_json TEXT,
          out_dir TEXT,
          transcript_jsonl TEXT,
          transcript_events INTEGER,
          transcript_vlm_events INTEGER,
          transcript_vlm_schema_ok INTEGER,
          top_3_fixes_json TEXT
        );

        CREATE TABLE IF NOT EXISTS webpipe_eval_vlm_per_input (
          id INTEGER PRIMARY KEY AUTOINCREMENT,
          run_key TEXT NOT NULL,
          image_index INTEGER,
          image_path TEXT,
          parsed_ok INTEGER,
          score_0_10 REAL,
          issues_count INTEGER,
          p0_count INTEGER,
          p1_count INTEGER,
          p2_count INTEGER,
          consensus_fixes_json TEXT,
          top_3_fixes_json TEXT,
          FOREIGN KEY (run_key) REFERENCES webpipe_eval_vlm_runs(run_key)
        );
        """
    )


def upsert_meta(con: sqlite3.Connection, k: str, v: str) -> None:
    con.execute(
        "INSERT INTO meta(k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v=excluded.v",
        (k, v),
    )


def blob_prefix_from_id_meta(id_meta: Any) -> Optional[str]:
    if not isinstance(id_meta, dict):
        return None
    if id_meta.get("kind") != "cursor_blob":
        return None
    v = id_meta.get("cursor_blob_prefix_hex")
    return v if isinstance(v, str) else None


def ingest_cursor_tool_counts(
    con: sqlite3.Connection,
    payload: Dict[str, Any],
    max_keys: int,
) -> None:
    generated_at = utc_now_iso()
    source_db_path = (
        payload.get("source", {}).get("db_path")
        if isinstance(payload.get("source"), dict)
        else None
    )
    filters_json = json.dumps(payload.get("filters", {}), sort_keys=True)

    top_tools = payload.get("top_tools")
    if not isinstance(top_tools, list):
        return

    for t in top_tools:
        if not isinstance(t, dict):
            continue
        name = t.get("name")
        count = t.get("count")
        if not isinstance(name, str) or not isinstance(count, int):
            continue

        fs = t.get("first_seen") if isinstance(t.get("first_seen"), dict) else {}
        ls = t.get("last_seen") if isinstance(t.get("last_seen"), dict) else {}

        first_rowid = fs.get("rowid") if isinstance(fs.get("rowid"), int) else None
        last_rowid = ls.get("rowid") if isinstance(ls.get("rowid"), int) else None
        first_trace_id = fs.get("trace_id") if isinstance(fs.get("trace_id"), str) else None
        last_trace_id = ls.get("trace_id") if isinstance(ls.get("trace_id"), str) else None
        first_blob_prefix = blob_prefix_from_id_meta(fs.get("id_meta"))
        last_blob_prefix = blob_prefix_from_id_meta(ls.get("id_meta"))

        con.execute(
            """
            INSERT INTO cursor_tool_counts(
              name, count, first_rowid, last_rowid,
              first_trace_id, last_trace_id,
              first_blob_prefix_hex, last_blob_prefix_hex,
              source_db_path, max_keys, filters_json, generated_at_utc
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(name) DO UPDATE SET
              count=excluded.count,
              first_rowid=excluded.first_rowid,
              last_rowid=excluded.last_rowid,
              first_trace_id=excluded.first_trace_id,
              last_trace_id=excluded.last_trace_id,
              first_blob_prefix_hex=excluded.first_blob_prefix_hex,
              last_blob_prefix_hex=excluded.last_blob_prefix_hex,
              source_db_path=excluded.source_db_path,
              max_keys=excluded.max_keys,
              filters_json=excluded.filters_json,
              generated_at_utc=excluded.generated_at_utc
            """,
            (
                name,
                count,
                first_rowid,
                last_rowid,
                first_trace_id,
                last_trace_id,
                first_blob_prefix,
                last_blob_prefix,
                source_db_path,
                max_keys,
                filters_json,
                generated_at,
            ),
        )


def safe_bool(v: Any) -> Optional[int]:
    if isinstance(v, bool):
        return 1 if v else 0
    return None


def safe_int(v: Any) -> Optional[int]:
    return v if isinstance(v, int) else None


def safe_str(v: Any) -> Optional[str]:
    return v if isinstance(v, str) else None


def sha256_text(s: Optional[str]) -> Optional[str]:
    if not isinstance(s, str):
        return None
    h = hashlib.sha256(s.encode("utf-8", errors="ignore")).hexdigest()
    return h


def parse_tx_trunc_field(v: Any) -> Tuple[Optional[str], Optional[int], Optional[int]]:
    """
    Parse a `tx_trunc`/`json_trunc` object:
      { "text": "...", "truncated": true|false }
    Returns (text, truncated_int, text_chars).
    """
    # Some transcript fields are stored as raw strings (not wrapped).
    if isinstance(v, str):
        return (v, 0, len(v))
    if not isinstance(v, dict):
        return (None, None, None)
    text = v.get("text")
    truncated = v.get("truncated")
    if not isinstance(text, str):
        return (None, safe_bool(truncated), None)
    return (text, safe_bool(truncated), len(text))


def try_parse_json(text: Optional[str]) -> Tuple[Optional[Dict[str, Any]], Optional[int]]:
    if not isinstance(text, str):
        return (None, None)
    try:
        obj = json.loads(text)
        return (obj if isinstance(obj, dict) else None, 1)
    except Exception:
        return (None, 0)


def extract_web_search_extract_metadata_from_result_json(result_obj: Any) -> Dict[str, Any]:
    """
    Return small, privacy-safe metadata from a parsed `web_search_extract` result payload.
    """
    if not isinstance(result_obj, dict):
        return {}
    out: Dict[str, Any] = {}
    out["tool_ok"] = safe_bool(result_obj.get("ok"))
    out["tool_provider"] = safe_str(result_obj.get("provider"))
    out["tool_fetch_backend"] = safe_str(result_obj.get("fetch_backend"))

    warnings = result_obj.get("warnings")
    if isinstance(warnings, list):
        out["tool_warning_count"] = len([x for x in warnings if isinstance(x, (str, dict))])

    fetch_source = result_obj.get("fetch_source")
    if isinstance(fetch_source, str):
        out["tool_cache_hit"] = 1 if fetch_source == "cache" else 0
    return out


def extract_llm_score(parsed: Any) -> Optional[float]:
    if not isinstance(parsed, dict):
        return None
    v = parsed.get("overall_score")
    if isinstance(v, (int, float)):
        return float(v)
    return None


def schema_ok_for_stage(stage: Optional[str], parsed: Any) -> Optional[int]:
    """
    Best-effort schema validation for judge outputs (no raw text stored).
    Returns 1/0/None.
    """
    if parsed is None:
        return None
    if not isinstance(parsed, dict):
        return 0
    if stage == "judge":
        need = ["overall_score", "relevant", "answerable", "confidence", "issues"]
        if not all(k in parsed for k in need):
            return 0
        if not isinstance(parsed.get("issues"), list):
            return 0
        return 1
    if stage == "judge_overall":
        need = ["overall_assessment", "top_failure_modes", "recommended_defaults", "confidence"]
        if not all(k in parsed for k in need):
            return 0
        if not isinstance(parsed.get("top_failure_modes"), list):
            return 0
        if not isinstance(parsed.get("recommended_defaults"), dict):
            return 0
        return 1
    if stage == "vlm_openrouter":
        # VLM critique schema (tight; avoids prose drift).
        need = ["overall", "score_0_10", "verdict", "strengths", "issues", "top_3_fixes"]
        if not all(k in parsed for k in need):
            return 0
        if not isinstance(parsed.get("strengths"), list):
            return 0
        if not isinstance(parsed.get("issues"), list):
            return 0
        if not isinstance(parsed.get("top_3_fixes"), list):
            return 0
        return 1
    if stage == "critic":
        # Deterministic critic schema (tight; avoids prose drift).
        need = ["issues", "structured_ok", "markdown_ok", "results_count", "top_chunks_count"]
        if not all(k in parsed for k in need):
            return 0
        if not isinstance(parsed.get("issues"), list):
            return 0
        return 1
    # Other LLM-ish stages exist (meta/task_solve); keep unknown.
    return None


def ingest_webpipe_transcripts(con: sqlite3.Connection, webpipe_root: Path) -> Tuple[int, int]:
    """
    Scan for `*.transcript.jsonl` under webpipe root and ingest.
    Returns (files_ingested, events_ingested).
    """
    files = []
    for dirpath, dirnames, filenames in os.walk(webpipe_root):
        # include dot dirs; don't modify dirnames
        for fn in filenames:
            # Historically, transcripts were named `*.transcript.jsonl`, but some commands/tests
            # use custom names. Treat "contains 'transcript' + endswith .jsonl" as a transcript.
            if fn.endswith(".transcript.jsonl") or ("transcript" in fn and fn.endswith(".jsonl")):
                files.append(Path(dirpath) / fn)

    files_ingested = 0
    events_ingested = 0

    for p in files:
        try:
            st = p.stat()
        except OSError:
            continue

        con.execute(
            "INSERT OR REPLACE INTO webpipe_transcript_files(path, mtime_epoch_s, bytes) VALUES (?, ?, ?)",
            (str(p), int(st.st_mtime), int(st.st_size)),
        )
        files_ingested += 1

        with p.open("r", encoding="utf-8", errors="ignore") as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    ev = json.loads(line)
                except Exception:
                    continue
                if not isinstance(ev, dict):
                    continue
                if ev.get("kind") != "webpipe_eval_transcript_event":
                    continue

                run_kind = safe_str(ev.get("run_kind"))
                stage = safe_str(ev.get("stage"))
                call_id = safe_str(ev.get("call_id"))
                seq = safe_int(ev.get("seq"))
                generated_at_epoch_s = safe_int(ev.get("generated_at_epoch_s"))

                tool_name = None
                tool_elapsed_ms = None
                tool_meta: Dict[str, Any] = {}
                tool_args_truncated = None
                tool_result_truncated = None
                tool_args_json_ok = None
                tool_result_json_ok = None
                tool_args_sha = None
                tool_result_sha = None
                wse_args: Dict[str, Any] = {}
                wse_res_counts: Dict[str, Any] = {}

                tool = ev.get("tool")
                if isinstance(tool, dict):
                    tool_name = safe_str(tool.get("name"))
                    tool_elapsed_ms = safe_int(tool.get("elapsed_ms"))
                    # Tool args are stored as `json_trunc` => {text, truncated}.
                    # Tool results are increasingly stored as structured `result_summary` to avoid
                    # JSON truncation (which would make downstream analysis misleading).
                    args_text, tool_args_truncated, _args_chars = parse_tx_trunc_field(tool.get("args"))
                    res_text, tool_result_truncated, _res_chars = parse_tx_trunc_field(tool.get("result"))
                    tool_args_sha = sha256_text(args_text)
                    tool_result_sha = sha256_text(res_text)

                    args_json, tool_args_json_ok = try_parse_json(args_text)
                    res_json, tool_result_json_ok = try_parse_json(res_text)

                    # Only attempt structured extraction if parse succeeded and the JSON wasn't truncated.
                    if tool_name == "web_search_extract":
                        # Prefer result_summary when present (parseable + bounded).
                        rs0 = tool.get("result_summary")
                        if isinstance(rs0, dict):
                            # These are the fields we currently log in transcripts.
                            wse_res_counts = {
                                "wse_results_count": safe_int(rs0.get("results_count")),
                                "wse_top_chunks_count": safe_int(rs0.get("top_chunks_count")),
                            }
                            tool_meta["tool_ok"] = safe_bool(rs0.get("ok"))
                            tool_meta["tool_provider"] = safe_str(rs0.get("provider"))
                            tool_meta["tool_fetch_backend"] = safe_str(rs0.get("fetch_backend"))

                        if args_json is not None:
                            # Capture knobs that matter for analysis (all scalar/bool).
                            wse_args = {
                                "wse_requested_provider": safe_str(args_json.get("provider")),
                                "wse_auto_mode": safe_str(args_json.get("auto_mode")),
                                "wse_selection_mode": safe_str(args_json.get("selection_mode")),
                                "wse_fetch_backend": safe_str(args_json.get("fetch_backend")),
                                "wse_agentic": safe_bool(args_json.get("agentic")),
                                "wse_agentic_selector": safe_str(args_json.get("agentic_selector")),
                                "wse_url_selection_mode": safe_str(args_json.get("url_selection_mode")),
                                "wse_max_urls": safe_int(args_json.get("max_urls")),
                                "wse_max_chars": safe_int(args_json.get("max_chars")),
                                "wse_top_chunks": safe_int(args_json.get("top_chunks")),
                                "wse_max_chunk_chars": safe_int(args_json.get("max_chunk_chars")),
                                "wse_include_text": safe_bool(args_json.get("include_text")),
                                "wse_include_links": safe_bool(args_json.get("include_links")),
                                "wse_cache_read": safe_bool(args_json.get("cache_read")),
                                "wse_cache_write": safe_bool(args_json.get("cache_write")),
                                "wse_no_network": safe_bool(args_json.get("no_network")),
                            }
                        if res_json is not None and not wse_res_counts:
                            tool_meta = extract_web_search_extract_metadata_from_result_json(res_json)
                            # Counts only (no text).
                            rs = res_json.get("results")
                            tc = res_json.get("top_chunks")
                            wse_res_counts = {
                                "wse_results_count": len(rs) if isinstance(rs, list) else None,
                                "wse_top_chunks_count": len(tc) if isinstance(tc, list) else None,
                            }
                    elif res_json is not None and isinstance(res_json, dict):
                        tool_meta["tool_ok"] = safe_bool(res_json.get("ok"))

                attempt = safe_int(ev.get("attempt"))
                llm = ev.get("llm") if isinstance(ev.get("llm"), dict) else None
                llm_backend = safe_str(llm.get("backend")) if llm else None
                llm_model_effective = safe_str(llm.get("model_effective")) if llm else None
                llm_json_mode = safe_bool(llm.get("json_mode")) if llm else None
                llm_timeout_ms = safe_int(llm.get("timeout_ms")) if llm else None
                llm_timing_ms = safe_int(ev.get("timing_ms"))

                prompt_system_chars = None
                prompt_user_chars = None
                prompt_truncated = None
                prompt_system_sha = None
                prompt_user_sha = None
                vlm_profile = None
                vlm_goal_profiles_json = None
                prompt = ev.get("prompt") if isinstance(ev.get("prompt"), dict) else None
                if prompt:
                    sys_text, sys_tr, sys_chars = parse_tx_trunc_field(prompt.get("system"))
                    usr_text, usr_tr, usr_chars = parse_tx_trunc_field(prompt.get("user"))
                    prompt_system_chars = sys_chars
                    prompt_user_chars = usr_chars
                    prompt_system_sha = sha256_text(sys_text)
                    prompt_user_sha = sha256_text(usr_text)
                    # prompt_truncated is true if either field was truncated, or explicit flag exists.
                    pt = prompt.get("prompt_truncated")
                    pt_b = safe_bool(pt)
                    if pt_b is None:
                        if sys_tr == 1 or usr_tr == 1:
                            prompt_truncated = 1
                        elif sys_tr == 0 and usr_tr == 0:
                            prompt_truncated = 0
                    else:
                        prompt_truncated = pt_b

                    # VLM prompt metadata (small + aggregatable).
                    vlm_profile = safe_str(prompt.get("profile"))
                    gps = prompt.get("goal_profiles")
                    if isinstance(gps, list) and all(isinstance(x, str) for x in gps):
                        vlm_goal_profiles_json = json.dumps(gps, sort_keys=True)

                response_raw_chars = None
                response_raw_truncated = None
                response_raw_sha = None
                resp = ev.get("response") if isinstance(ev.get("response"), dict) else None
                llm_error_present = None
                llm_parse_ok = None
                llm_schema_ok = None
                llm_overall_score = None
                critic_issues_json = None
                critic_issues_norm_json = None
                critic_issues_unknown_count = None
                critic_markdown_ok = None
                critic_structured_ok = None
                judge_ctx_observed_url_count = None
                judge_ctx_warnings_count = None
                judge_ctx_evidence_chars = None
                judge_ctx_evidence_truncated = None
                judge_musings_count = None
                judge_musing_dimensions_json = None
                judge_musing_dimensions_unknown_count = None
                vlm_score_0_10 = None
                vlm_verdict = None
                vlm_issues_count = None
                vlm_p0_count = None
                vlm_p1_count = None
                vlm_p2_count = None
                vlm_top_3_fixes_json = None
                if resp:
                    err = resp.get("error")
                    llm_error_present = 1 if (isinstance(err, str) and err.strip()) else 0
                    parsed = resp.get("parsed")
                    llm_parse_ok = 1 if parsed is not None else 0
                    llm_overall_score = extract_llm_score(parsed)
                    llm_schema_ok = schema_ok_for_stage(stage, parsed)

                    # Judge context completeness (counts only; privacy-safe).
                    if stage == "judge":
                        ctx = ev.get("context") if isinstance(ev.get("context"), dict) else None
                        if isinstance(ctx, dict):
                            judge_ctx_observed_url_count = safe_int(ctx.get("observed_url_count"))
                            judge_ctx_warnings_count = safe_int(ctx.get("warnings_count"))
                            judge_ctx_evidence_chars = safe_int(ctx.get("evidence_chars"))
                            judge_ctx_evidence_truncated = safe_bool(ctx.get("evidence_truncated"))

                    # Critic structured extraction (bounded; aggregatable).
                    if stage == "critic" and isinstance(parsed, dict):
                        raw, norm, unk = normalize_critic_issue_list(parsed.get("issues"))
                        critic_issues_json = raw
                        critic_issues_norm_json = norm
                        critic_issues_unknown_count = int(unk)
                        critic_markdown_ok = safe_bool(parsed.get("markdown_ok"))
                        critic_structured_ok = safe_bool(parsed.get("structured_ok"))

                    # Judge musings: store dimensions only (notes may contain page-specific text).
                    if stage == "judge" and isinstance(parsed, dict):
                        mus = parsed.get("musings")
                        dims: list[str] = []
                        unk = 0
                        if isinstance(mus, list):
                            for it in mus:
                                if not isinstance(it, dict):
                                    unk += 1
                                    continue
                                d = it.get("dimension")
                                if not isinstance(d, str):
                                    unk += 1
                                    continue
                                dd = d.strip()
                                if not dd or len(dd) > 32:
                                    unk += 1
                                    continue
                                if dd not in dims:
                                    dims.append(dd)
                        if mus is not None:
                            judge_musings_count = len(mus) if isinstance(mus, list) else 0
                            judge_musing_dimensions_json = json.dumps(dims, sort_keys=True)
                            judge_musing_dimensions_unknown_count = int(unk)

                    # VLM structured extraction (bounded; no raw screenshot content).
                    if stage == "vlm_openrouter" and isinstance(parsed, dict):
                        v = parsed.get("score_0_10")
                        if isinstance(v, (int, float)):
                            vlm_score_0_10 = float(v)
                        vv = parsed.get("verdict")
                        if isinstance(vv, str) and vv.strip():
                            vlm_verdict = vv.strip()
                        issues = parsed.get("issues")
                        if isinstance(issues, list):
                            vlm_issues_count = len(issues)
                            p0 = p1 = p2 = 0
                            for it in issues:
                                if not isinstance(it, dict):
                                    continue
                                sev = it.get("severity")
                                if not isinstance(sev, str):
                                    continue
                                s = sev.strip().upper()
                                if s == "P0":
                                    p0 += 1
                                elif s == "P1":
                                    p1 += 1
                                elif s == "P2":
                                    p2 += 1
                            vlm_p0_count, vlm_p1_count, vlm_p2_count = p0, p1, p2
                        t3 = parsed.get("top_3_fixes")
                        if isinstance(t3, list) and all(isinstance(x, str) for x in t3):
                            vlm_top_3_fixes_json = json.dumps(t3, sort_keys=True)

                    raw_text, raw_tr, raw_chars = parse_tx_trunc_field(resp.get("raw"))
                    response_raw_chars = raw_chars
                    response_raw_truncated = raw_tr
                    response_raw_sha = sha256_text(raw_text)

                con.execute(
                    """
                    INSERT INTO webpipe_transcript_events(
                      src_path, seq, generated_at_epoch_s, run_kind, stage, call_id,
                      tool_name, tool_elapsed_ms, tool_ok, tool_provider, tool_fetch_backend,
                      tool_warning_count, tool_cache_hit,
                      tool_args_truncated, tool_result_truncated, tool_args_json_ok, tool_result_json_ok,
                      tool_args_sha256, tool_result_sha256,
                      wse_requested_provider, wse_auto_mode, wse_selection_mode, wse_fetch_backend,
                      wse_agentic, wse_agentic_selector, wse_url_selection_mode,
                      wse_max_urls, wse_max_chars, wse_top_chunks, wse_max_chunk_chars,
                      wse_include_text, wse_include_links, wse_cache_read, wse_cache_write, wse_no_network,
                      wse_results_count, wse_top_chunks_count,
                      attempt, llm_backend, llm_model_effective, llm_json_mode, llm_timeout_ms,
                      llm_timing_ms, llm_error_present, llm_parse_ok, llm_schema_ok,
                      prompt_system_chars, prompt_user_chars, prompt_truncated,
                      prompt_system_sha256, prompt_user_sha256,
                      response_raw_chars, response_raw_truncated,
                      response_raw_sha256,
                      llm_overall_score,
                      judge_ctx_observed_url_count, judge_ctx_warnings_count, judge_ctx_evidence_chars, judge_ctx_evidence_truncated,
                      judge_musings_count, judge_musing_dimensions_json, judge_musing_dimensions_unknown_count,
                      vlm_profile, vlm_goal_profiles_json,
                      vlm_score_0_10, vlm_verdict, vlm_issues_count,
                      vlm_p0_count, vlm_p1_count, vlm_p2_count,
                      vlm_top_3_fixes_json,
                      critic_issues_json, critic_issues_norm_json, critic_issues_unknown_count,
                      critic_markdown_ok, critic_structured_ok
                    ) VALUES (
                      ?, ?, ?, ?, ?, ?,
                      ?, ?, ?, ?, ?,
                      ?, ?,
                      ?, ?, ?, ?,
                      ?, ?,
                      ?, ?, ?, ?,
                      ?, ?, ?,
                      ?, ?, ?, ?,
                      ?, ?, ?, ?, ?,
                      ?, ?,
                      ?, ?, ?, ?, ?,
                      ?, ?, ?, ?,
                      ?, ?, ?,
                      ?, ?,
                      ?, ?,
                      ?,
                      ?,
                      ?, ?, ?, ?,
                      ?, ?, ?,
                      ?, ?,
                      ?, ?, ?,
                      ?, ?, ?,
                      ?,
                      ?, ?, ?,
                      ?, ?
                    )
                    """,
                    (
                        str(p),
                        seq,
                        generated_at_epoch_s,
                        run_kind,
                        stage,
                        call_id,
                        tool_name,
                        tool_elapsed_ms,
                        tool_meta.get("tool_ok"),
                        tool_meta.get("tool_provider"),
                        tool_meta.get("tool_fetch_backend"),
                        tool_meta.get("tool_warning_count"),
                        tool_meta.get("tool_cache_hit"),
                        tool_args_truncated,
                        tool_result_truncated,
                        tool_args_json_ok,
                        tool_result_json_ok,
                        tool_args_sha,
                        tool_result_sha,
                        wse_args.get("wse_requested_provider"),
                        wse_args.get("wse_auto_mode"),
                        wse_args.get("wse_selection_mode"),
                        wse_args.get("wse_fetch_backend"),
                        wse_args.get("wse_agentic"),
                        wse_args.get("wse_agentic_selector"),
                        wse_args.get("wse_url_selection_mode"),
                        wse_args.get("wse_max_urls"),
                        wse_args.get("wse_max_chars"),
                        wse_args.get("wse_top_chunks"),
                        wse_args.get("wse_max_chunk_chars"),
                        wse_args.get("wse_include_text"),
                        wse_args.get("wse_include_links"),
                        wse_args.get("wse_cache_read"),
                        wse_args.get("wse_cache_write"),
                        wse_args.get("wse_no_network"),
                        wse_res_counts.get("wse_results_count"),
                        wse_res_counts.get("wse_top_chunks_count"),
                        attempt,
                        llm_backend,
                        llm_model_effective,
                        llm_json_mode,
                        llm_timeout_ms,
                        llm_timing_ms,
                        llm_error_present,
                        llm_parse_ok,
                        llm_schema_ok,
                        prompt_system_chars,
                        prompt_user_chars,
                        prompt_truncated,
                        prompt_system_sha,
                        prompt_user_sha,
                        response_raw_chars,
                        response_raw_truncated,
                        response_raw_sha,
                        llm_overall_score,
                        judge_ctx_observed_url_count,
                        judge_ctx_warnings_count,
                        judge_ctx_evidence_chars,
                        judge_ctx_evidence_truncated,
                        judge_musings_count,
                        judge_musing_dimensions_json,
                        judge_musing_dimensions_unknown_count,
                        vlm_profile,
                        vlm_goal_profiles_json,
                        vlm_score_0_10,
                        vlm_verdict,
                        vlm_issues_count,
                        vlm_p0_count,
                        vlm_p1_count,
                        vlm_p2_count,
                        vlm_top_3_fixes_json,
                        critic_issues_json,
                        critic_issues_norm_json,
                        critic_issues_unknown_count,
                        critic_markdown_ok,
                        critic_structured_ok,
                    ),
                )
                events_ingested += 1

    return files_ingested, events_ingested


def _ingest_eval_file_record(con: sqlite3.Connection, p: Path, kind: Optional[str], schema_version: Optional[int], generated_at: Optional[int]) -> None:
    try:
        st = p.stat()
    except OSError:
        return
    con.execute(
        """
        INSERT OR REPLACE INTO webpipe_eval_artifact_files(path, kind, schema_version, generated_at_epoch_s, mtime_epoch_s, bytes)
        VALUES (?, ?, ?, ?, ?, ?)
        """,
        (str(p), kind, schema_version, generated_at, int(st.st_mtime), int(st.st_size)),
    )


def ingest_webpipe_eval_judge_swarm(con: sqlite3.Connection, webpipe_root: Path) -> int:
    """
    Ingest `.generated/webpipe-eval-judge-swarm-*.json` artifacts.
    Stores only configuration + numeric/judgment fields (no prose, no raw judge_text).
    Returns number of runs ingested.
    """
    gen_dir = webpipe_root / ".generated"
    if not gen_dir.is_dir():
        return 0

    # Judge-swarm artifacts have existed under a few filename conventions over time.
    # Prefer explicit patterns first (bounded), then fall back to a shape-based scan.
    paths: list[Path] = []
    for pat in [
        "webpipe-eval-judge-swarm-*.json",
        "judge-swarm-*.json",
        "tmp-eval-judge-swarm.json",
        "tmp-live-swarm*.json",
    ]:
        paths.extend(gen_dir.glob(pat))
    # De-dup, stable order.
    paths = sorted({p.resolve() for p in paths})
    runs = 0
    for p in paths:
        try:
            obj = json.loads(p.read_text("utf-8"))
        except Exception:
            continue
        if not isinstance(obj, dict):
            continue
        kind = safe_str(obj.get("kind"))
        schema_version = safe_int(obj.get("schema_version"))
        generated_at = safe_int(obj.get("generated_at_epoch_s"))

        # Back-compat: some older artifacts omitted kind/schema_version.
        # Infer from shape so historical runs remain analyzable.
        inferred_judge_swarm = False
        if kind is None:
            if isinstance(obj.get("inputs"), dict) and isinstance(obj.get("judge_reports"), list):
                inferred_judge_swarm = True
                kind = "webpipe_eval_judge_swarm"
        if schema_version is None and (kind == "webpipe_eval_judge_swarm" or inferred_judge_swarm):
            schema_version = 1
        _ingest_eval_file_record(con, p, kind, schema_version, generated_at)
        if kind != "webpipe_eval_judge_swarm":
            continue

        # run_key stable-ish: relative path under repo root
        run_key = str(p.relative_to(webpipe_root))

        inputs = obj.get("inputs") if isinstance(obj.get("inputs"), dict) else {}
        llm_backend = safe_str(inputs.get("llm_backend"))
        llm_model_effective = safe_str(inputs.get("llm_model_effective"))
        json_mode = safe_bool(inputs.get("json_mode"))
        provider = safe_str(inputs.get("provider"))
        auto_mode = safe_str(inputs.get("auto_mode"))
        selection_mode = safe_str(inputs.get("selection_mode"))
        fetch_backend = safe_str(inputs.get("fetch_backend"))
        trial_set = safe_str(inputs.get("trial_set"))
        max_queries = safe_int(inputs.get("max_queries"))
        seed = safe_int(inputs.get("seed"))

        con.execute(
            """
            INSERT OR REPLACE INTO webpipe_eval_judge_swarm_runs(
              run_key, generated_at_epoch_s, llm_backend, llm_model_effective, json_mode,
              provider, auto_mode, selection_mode, fetch_backend, trial_set, max_queries, seed
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                run_key,
                generated_at,
                llm_backend,
                llm_model_effective,
                json_mode,
                provider,
                auto_mode,
                selection_mode,
                fetch_backend,
                trial_set,
                max_queries,
                seed,
            ),
        )

        # Meta-judge summary (small, aggregatable fields).
        meta = obj.get("meta") if isinstance(obj.get("meta"), dict) else {}
        if isinstance(meta, dict):
            overall_assessment = safe_str(meta.get("overall_assessment"))
            cross_judge_agreement = safe_str(meta.get("cross_judge_agreement"))
            top_systemic_failures_json, top_systemic_failures_norm_json, top_systemic_failures_unknown_count = (
                normalize_issue_list(meta.get("top_systemic_failures"))
            )
            top_3_fixes_json = None
            v = meta.get("top_3_fixes")
            if isinstance(v, list) and all(isinstance(x, str) for x in v):
                top_3_fixes_json = json.dumps(v, sort_keys=True)
            recommended_next_experiments_json = None
            v = meta.get("recommended_next_experiments")
            if isinstance(v, list) and all(isinstance(x, str) for x in v):
                recommended_next_experiments_json = json.dumps(v, sort_keys=True)
            top_dimensions_json = None
            v = meta.get("top_dimensions")
            if isinstance(v, list) and all(isinstance(x, str) for x in v):
                top_dimensions_json = json.dumps(v, sort_keys=True)
            top_musings_json = None
            v = meta.get("top_musings")
            if isinstance(v, list) and all(isinstance(x, str) for x in v):
                top_musings_json = json.dumps(v, sort_keys=True)
            con.execute(
                """
                INSERT OR REPLACE INTO webpipe_eval_judge_meta(
                  run_key, overall_assessment, cross_judge_agreement,
                  top_systemic_failures_json, top_systemic_failures_norm_json, top_systemic_failures_unknown_count,
                  top_3_fixes_json, recommended_next_experiments_json,
                  top_dimensions_json, top_musings_json
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    run_key,
                    overall_assessment,
                    cross_judge_agreement,
                    top_systemic_failures_json,
                    top_systemic_failures_norm_json,
                    top_systemic_failures_unknown_count,
                    top_3_fixes_json,
                    recommended_next_experiments_json,
                    top_dimensions_json,
                    top_musings_json,
                ),
            )

        judge_reports = obj.get("judge_reports")
        if not isinstance(judge_reports, list):
            runs += 1
            continue

        for jr in judge_reports:
            if not isinstance(jr, dict):
                continue
            judge_id = safe_str(jr.get("judge_id"))
            totals = jr.get("totals") if isinstance(jr.get("totals"), dict) else {}

            totals_queries = safe_int(totals.get("queries"))
            totals_trials = safe_int(totals.get("trials"))
            totals_llm_ok = safe_int(totals.get("llm_ok"))
            totals_llm_failed = safe_int(totals.get("llm_failed"))
            totals_llm_parse_failed = safe_int(totals.get("llm_parse_failed"))
            hit_expected = safe_int(totals.get("hit_expected_url_any_trial"))

            overall = jr.get("overall") if isinstance(jr.get("overall"), dict) else {}
            overall_conf = overall.get("confidence")
            overall_confidence = float(overall_conf) if isinstance(overall_conf, (int, float)) else None

            tfm = overall.get("top_failure_modes")
            top_failure_modes_json, top_failure_modes_norm_json, top_failure_modes_unknown_count = (
                normalize_issue_list(tfm)
            )

            con.execute(
                """
                INSERT INTO webpipe_eval_judge_agent_reports(
                  run_key, judge_id,
                  totals_queries, totals_trials, totals_llm_ok, totals_llm_failed, totals_llm_parse_failed,
                  hit_expected_url_any_trial, overall_confidence,
                  top_failure_modes_json, top_failure_modes_norm_json, top_failure_modes_unknown_count
                ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                """,
                (
                    run_key,
                    judge_id,
                    totals_queries,
                    totals_trials,
                    totals_llm_ok,
                    totals_llm_failed,
                    totals_llm_parse_failed,
                    hit_expected,
                    overall_confidence,
                    top_failure_modes_json,
                    top_failure_modes_norm_json,
                    top_failure_modes_unknown_count,
                ),
            )

            # per_query → trials (store numeric + categorical only)
            per_query = jr.get("per_query")
            if not isinstance(per_query, list):
                continue
            for pq in per_query:
                if not isinstance(pq, dict):
                    continue
                query_id = safe_str(pq.get("query_id"))
                trials = pq.get("trials")
                if not isinstance(trials, list):
                    continue
                for tr in trials:
                    if not isinstance(tr, dict):
                        continue
                    trial_id = safe_str(tr.get("trial_id"))
                    agentic = safe_bool(tr.get("agentic"))
                    agentic_selector = safe_str(tr.get("agentic_selector"))
                    tr_fetch_backend = safe_str(tr.get("fetch_backend"))
                    url_selection_mode = safe_str(tr.get("url_selection_mode"))
                    elapsed_ms = safe_int(tr.get("elapsed_ms"))
                    ok = safe_bool(tr.get("ok"))
                    observed_urls = tr.get("observed_urls")
                    observed_url_count = len(observed_urls) if isinstance(observed_urls, list) else None
                    warnings = tr.get("warnings")
                    warnings_count = len(warnings) if isinstance(warnings, list) else None
                    judge_text_truncated = safe_bool(tr.get("judge_text_truncated"))
                    hit_expected_url = safe_bool(tr.get("hit_expected_url"))

                    judge = tr.get("judge") if isinstance(tr.get("judge"), dict) else {}
                    judge_overall_score = None
                    v = judge.get("overall_score")
                    if isinstance(v, (int, float)):
                        judge_overall_score = float(v)
                    judge_answerable = safe_bool(judge.get("answerable"))
                    judge_relevant = safe_bool(judge.get("relevant"))
                    vc = judge.get("confidence")
                    judge_confidence = float(vc) if isinstance(vc, (int, float)) else None
                    issues = judge.get("issues")
                    judge_issues_json, judge_issues_norm_json, judge_issues_unknown_count = (
                        normalize_issue_list(issues)
                    )

                    con.execute(
                        """
                        INSERT INTO webpipe_eval_judge_trials(
                          run_key, judge_id, query_id, trial_id,
                          agentic, agentic_selector, fetch_backend, url_selection_mode,
                          elapsed_ms, ok, observed_url_count, warnings_count, judge_text_truncated,
                          judge_overall_score, judge_answerable, judge_relevant, judge_confidence,
                          judge_issues_json, judge_issues_norm_json, judge_issues_unknown_count,
                          hit_expected_url
                        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                        """,
                        (
                            run_key,
                            judge_id,
                            query_id,
                            trial_id,
                            agentic,
                            agentic_selector,
                            tr_fetch_backend,
                            url_selection_mode,
                            elapsed_ms,
                            ok,
                            observed_url_count,
                            warnings_count,
                            judge_text_truncated,
                            judge_overall_score,
                            judge_answerable,
                            judge_relevant,
                            judge_confidence,
                            judge_issues_json,
                            judge_issues_norm_json,
                            judge_issues_unknown_count,
                            hit_expected_url,
                        ),
                    )

        runs += 1
    return runs


def ingest_webpipe_eval_vlm_runs(con: sqlite3.Connection, webpipe_root: Path) -> int:
    """
    Ingest `.generated/**` VLM run summaries produced by `webpipe eval-vlm-run`.
    Returns number of run summaries ingested.
    """
    gen_dir = webpipe_root / ".generated"
    if not gen_dir.is_dir():
        return 0

    # Filename conventions:
    # - default: .generated/webpipe-eval-vlm-run-<epoch>/summary.json
    # - some callers may write a flat file; accept both.
    paths: list[Path] = []
    for pat in [
        "webpipe-eval-vlm-run-*/summary.json",
        "webpipe-eval-vlm-run-*.json",
        "webpipe-eval-vlm-*.json",
    ]:
        paths.extend(gen_dir.glob(pat))
    paths = sorted({p.resolve() for p in paths})

    runs = 0
    for p in paths:
        try:
            obj = json.loads(p.read_text("utf-8"))
        except Exception:
            continue
        if not isinstance(obj, dict):
            continue
        if safe_str(obj.get("kind")) != "webpipe_eval_vlm_run":
            continue
        sv = safe_int(obj.get("schema_version"))
        if sv is not None and sv != 1:
            continue

        generated_at = safe_int(obj.get("generated_at_epoch_s"))
        run_key = str(p.relative_to(webpipe_root))

        inputs = obj.get("inputs") if isinstance(obj.get("inputs"), dict) else {}
        totals = obj.get("totals") if isinstance(obj.get("totals"), dict) else {}
        outputs = obj.get("outputs") if isinstance(obj.get("outputs"), dict) else {}

        model = safe_str(inputs.get("model"))
        temperature = inputs.get("temperature")
        temperature_f = float(temperature) if isinstance(temperature, (int, float)) else None
        trials = safe_int(inputs.get("trials"))
        images = safe_int(totals.get("images"))
        totals_runs = safe_int(totals.get("runs"))
        totals_parsed_ok = safe_int(totals.get("parsed_ok"))

        goals = inputs.get("goals")
        goals_count = len(goals) if isinstance(goals, list) else None
        gp = inputs.get("goal_profiles")
        goal_profiles_json = None
        if isinstance(gp, list) and all(isinstance(x, str) for x in gp):
            goal_profiles_json = json.dumps(gp, sort_keys=True)

        out_dir = safe_str(outputs.get("out_dir"))
        transcript_jsonl = safe_str(outputs.get("transcript_jsonl"))
        transcript_events = None
        transcript_vlm_events = None
        transcript_vlm_schema_ok = None

        # If the transcript exists, compute a few cheap, aggregatable counts.
        if isinstance(transcript_jsonl, str) and transcript_jsonl.strip():
            tp = Path(transcript_jsonl)
            if not tp.is_absolute():
                tp = (webpipe_root / tp).resolve()
            if tp.is_file():
                total = 0
                vlm = 0
                vlm_ok = 0
                try:
                    with tp.open("r", encoding="utf-8", errors="ignore") as f:
                        for line in f:
                            line = line.strip()
                            if not line:
                                continue
                            total += 1
                            try:
                                ev = json.loads(line)
                            except Exception:
                                continue
                            if not isinstance(ev, dict):
                                continue
                            if ev.get("kind") != "webpipe_eval_transcript_event":
                                continue
                            if safe_str(ev.get("stage")) != "vlm_openrouter":
                                continue
                            vlm += 1
                            resp = ev.get("response") if isinstance(ev.get("response"), dict) else None
                            parsed = resp.get("parsed") if resp else None
                            if schema_ok_for_stage("vlm_openrouter", parsed) == 1:
                                vlm_ok += 1
                except OSError:
                    pass
                transcript_events = total
                transcript_vlm_events = vlm
                transcript_vlm_schema_ok = vlm_ok

        t3 = obj.get("top_3_fixes")
        top_3_fixes_json = None
        if isinstance(t3, list) and all(isinstance(x, str) for x in t3):
            top_3_fixes_json = json.dumps(t3, sort_keys=True)

        _ingest_eval_file_record(con, p, "webpipe_eval_vlm_run", 1, generated_at)

        con.execute(
            """
            INSERT OR REPLACE INTO webpipe_eval_vlm_runs(
              run_key, generated_at_epoch_s, model, temperature, trials, images,
              totals_runs, totals_parsed_ok, goals_count, goal_profiles_json,
              out_dir, transcript_jsonl,
              transcript_events, transcript_vlm_events, transcript_vlm_schema_ok,
              top_3_fixes_json
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """,
            (
                run_key,
                generated_at,
                model,
                temperature_f,
                trials,
                images,
                totals_runs,
                totals_parsed_ok,
                goals_count,
                goal_profiles_json,
                out_dir,
                transcript_jsonl,
                transcript_events,
                transcript_vlm_events,
                transcript_vlm_schema_ok,
                top_3_fixes_json,
            ),
        )

        per_input = obj.get("per_input")
        if isinstance(per_input, list):
            for it in per_input:
                if not isinstance(it, dict):
                    continue
                image_index = safe_int(it.get("image_index"))
                image_path = safe_str(it.get("image_path"))
                parsed_ok = safe_bool(it.get("parsed_ok"))
                score_0_10 = None
                s = it.get("score_0_10")
                if isinstance(s, (int, float)):
                    score_0_10 = float(s)
                issues = it.get("issues")
                issues_count = len(issues) if isinstance(issues, list) else None

                p0 = p1 = p2 = 0
                if isinstance(issues, list):
                    for iss in issues:
                        if not isinstance(iss, dict):
                            continue
                        sev = iss.get("severity")
                        if not isinstance(sev, str):
                            continue
                        ss = sev.strip().upper()
                        if ss == "P0":
                            p0 += 1
                        elif ss == "P1":
                            p1 += 1
                        elif ss == "P2":
                            p2 += 1

                consensus_fixes_json = None
                trials_obj = it.get("trials")
                if isinstance(trials_obj, dict):
                    cf = trials_obj.get("consensus_fixes")
                    if isinstance(cf, list) and all(isinstance(x, str) for x in cf):
                        consensus_fixes_json = json.dumps(cf, sort_keys=True)

                top_3_fixes_json2 = None
                t3i = it.get("top_3_fixes")
                if isinstance(t3i, list) and all(isinstance(x, str) for x in t3i):
                    top_3_fixes_json2 = json.dumps(t3i, sort_keys=True)

                con.execute(
                    """
                    INSERT INTO webpipe_eval_vlm_per_input(
                      run_key, image_index, image_path, parsed_ok, score_0_10,
                      issues_count, p0_count, p1_count, p2_count,
                      consensus_fixes_json, top_3_fixes_json
                    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                    """,
                    (
                        run_key,
                        image_index,
                        image_path,
                        parsed_ok,
                        score_0_10,
                        issues_count,
                        p0,
                        p1,
                        p2,
                        consensus_fixes_json,
                        top_3_fixes_json2,
                    ),
                )

        runs += 1

    return runs


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--out",
        type=Path,
        default=Path(__file__).resolve().parents[1] / ".generated" / "webpipe_self_opt.sqlite3",
        help="Output SQLite path (default: webpipe/.generated/webpipe_self_opt.sqlite3).",
    )
    ap.add_argument(
        "--max-keys",
        type=int,
        default=50_000,
        help="Max agentKv keys to scan via chatvault (default: 50000).",
    )
    ap.add_argument(
        "--top",
        type=int,
        default=5_000,
        help="How many tool names to retain (default: 5000).",
    )
    ap.add_argument(
        "--chatvault-bin",
        type=Path,
        default=Path(os.path.expanduser("~/.cargo/bin/chatvault")),
        help="Path to chatvault binary (default: ~/.cargo/bin/chatvault).",
    )
    ap.add_argument(
        "--include-prefix",
        action="append",
        default=[],
        help="Prefix filter for tool names (repeatable).",
    )
    ap.add_argument(
        "--exclude-substring",
        action="append",
        default=[],
        help="Substring exclusion filter for tool names (repeatable).",
    )
    ap.add_argument(
        "--webpipe-root",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="webpipe repo root (default: inferred from this script path).",
    )
    ap.add_argument(
        "--vlm-report-out",
        type=Path,
        default=None,
        help="Optional: write a small VLM report JSON to this path.",
    )
    ap.add_argument(
        "--critic-report-out",
        type=Path,
        default=None,
        help="Optional: write a small critic report JSON to this path.",
    )
    ap.add_argument(
        "--judge-musings-report-out",
        type=Path,
        default=None,
        help="Optional: write a small judge musings report JSON to this path.",
    )
    ap.add_argument(
        "--judge-context-report-out",
        type=Path,
        default=None,
        help="Optional: write a small judge context report JSON to this path.",
    )
    ap.add_argument(
        "--report-src-substring",
        type=str,
        default=None,
        help="Optional: restrict report aggregation to transcript events whose src_path contains this substring.",
    )
    args = ap.parse_args()

    include_prefixes = args.include_prefix or ["web_", "tavily", "firecrawl", "brave"]
    exclude_substrings = args.exclude_substring or ["smoke", "live_", "_opt_in"]

    if not args.chatvault_bin.exists():
        die(f"chatvault binary not found at {args.chatvault_bin} (set --chatvault-bin)")

    out = args.out
    ensure_parent_dir(out)
    if out.exists():
        out.unlink()

    con = sql_connect(out)
    try:
        sql_init(con)
        upsert_meta(con, "generated_at_utc", utc_now_iso())
        upsert_meta(con, "webpipe_root", str(args.webpipe_root))
        upsert_meta(con, "chatvault_bin", str(args.chatvault_bin))

        payload = run_chatvault_aggregate_tool_calls(
            chatvault_bin=args.chatvault_bin,
            max_keys=args.max_keys,
            top=args.top,
            include_prefixes=include_prefixes,
            exclude_substrings=exclude_substrings,
        )
        ingest_cursor_tool_counts(con, payload, max_keys=args.max_keys)

        files_ingested, events_ingested = ingest_webpipe_transcripts(con, args.webpipe_root)
        upsert_meta(con, "webpipe_transcripts_files_ingested", str(files_ingested))
        upsert_meta(con, "webpipe_transcripts_events_ingested", str(events_ingested))

        judge_runs = ingest_webpipe_eval_judge_swarm(con, args.webpipe_root)
        upsert_meta(con, "webpipe_eval_judge_swarm_runs_ingested", str(judge_runs))

        vlm_runs = ingest_webpipe_eval_vlm_runs(con, args.webpipe_root)
        upsert_meta(con, "webpipe_eval_vlm_runs_ingested", str(vlm_runs))
        if args.vlm_report_out is not None:
            emit_vlm_report(con, args.vlm_report_out)
        if args.critic_report_out is not None:
            emit_critic_report(con, args.critic_report_out, src_path_substring=args.report_src_substring)
        if args.judge_musings_report_out is not None:
            emit_judge_musings_report(con, args.judge_musings_report_out, src_path_substring=args.report_src_substring)
        if args.judge_context_report_out is not None:
            emit_judge_context_report(con, args.judge_context_report_out, src_path_substring=args.report_src_substring)

        con.commit()
    finally:
        con.close()

    print(str(out))


if __name__ == "__main__":
    main()

