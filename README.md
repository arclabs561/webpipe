# webpipe

Local “search → fetch → extract” plumbing, exposed as an **MCP stdio** server.

`webpipe` is designed for **bounded, deterministic evidence gathering**:
- cache-first fetch by default
- explicit limits on bytes/chars/results
- stable, schema-versioned JSON outputs

## Cursor MCP setup

Add this to your `~/.cursor/mcp.json` (adjust path to your repo):

```json
{
  "mcpServers": {
    "webpipe": {
      "command": "cargo",
      "args": [
        "run",
        "--release",
        "--bin",
        "webpipe",
        "--features",
        "stdio",
        "--",
        "mcp-stdio"
      ],
      "cwd": "/path/to/webpipe"
    }
  }
}
```

Optional: to enable Brave-backed search, set `WEBPIPE_BRAVE_API_KEY` in your environment (or in `mcp.json`), but **don’t commit keys**.

## Sanity Check

Run this to verify the server is working (returns JSON):

```bash
python3 - <<'PY'
import json, subprocess, time, select

p = subprocess.Popen(
    ["cargo", "run", "-q", "--bin", "webpipe", "--features", "stdio", "--", "mcp-stdio"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.DEVNULL,
    text=True,
    bufsize=1,
)

def send(obj):
    p.stdin.write(json.dumps(obj) + "\n")
    p.stdin.flush()

# Minimal MCP handshake: initialize -> initialized -> tools/list
send({
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "webpipe-readme-smoke", "version": "0"},
    },
})
send({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}})
send({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}})

start = time.time()
while time.time() - start < 10:
    r, _, _ = select.select([p.stdout], [], [], 0.2)
    if not r:
        continue
    line = p.stdout.readline()
    if not line:
        break
    msg = json.loads(line)
    if msg.get("id") == 2:
        print(json.dumps(msg, indent=2))
        break

p.terminate()
PY
```

## Safety defaults

`webpipe` drops request-secret headers by default:
- `Authorization`
- `Cookie`
- `Proxy-Authorization`

If a caller supplies them, responses include a warning:
- `warning_codes` contains `unsafe_request_headers_dropped`
- `request.dropped_request_headers` lists header *names only*

To opt in (only for trusted endpoints), set `WEBPIPE_ALLOW_UNSAFE_HEADERS=true`.

## Privacy modes (no telemetry / offline / anonymous)

`webpipe` itself does **not** send analytics/telemetry. The remaining privacy question is **how network egress happens**.

Set `WEBPIPE_PRIVACY_MODE` to one of:

- **`normal`** (default): network allowed.
- **`offline`**: *no network egress* (except `localhost`); discovery/search is disabled; use cache-only workflows.
- **`anonymous`**: network allowed **only when routed via an explicit proxy** (fail-closed). Some remote backends are disabled.

Related env vars:

- **`WEBPIPE_ANON_PROXY`**: proxy URL used in anonymous mode.
  - For local/reqwest fetches, Tor users typically use `socks5h://127.0.0.1:9050`.
  - For Playwright render (`fetch_backend="render"`), `socks5h://...` is not supported; use an **HTTP proxy** endpoint (Tor users often run Privoxy and set `WEBPIPE_ANON_PROXY=http://127.0.0.1:8118`).
- **`WEBPIPE_RENDER_DISABLE=1`**: force-disable render backend (deterministic “not configured”).

## Avoid Cargo build locks (dev ergonomics)

If you run multiple `cargo` commands concurrently, you can hit:
`Blocking waiting for file lock on build directory`.

For `webpipe` work, you can isolate build output by setting a per-repo target dir:

```bash
CARGO_TARGET_DIR=target-webpipe cargo test -p webpipe-mcp --tests -- --skip live_
```

## Manual exploration from the CLI (no Cursor needed)

If you want to see *exactly* what Cursor would see (tool list + Markdown + structured JSON) without
opening the UI, use the CLI harness in `webpipe`:

```bash
# List tools as an MCP client sees them:
webpipe mcp-list-tools

# Call one MCP tool (spawns one `mcp-stdio` child) and print Markdown:
webpipe mcp-call --tool search_evidence --args-json '{"query":"example","urls":["https://example.com"],"max_urls":1}'
```

For **comparisons** (same server process; avoids spawn overhead dominating):

```bash
# Compare two argument sets in one spawned MCP server.
webpipe mcp-call --tool search_evidence --args-json-file .generated/mcp_call_compare_parallel.json --timeout-ms 45000 --print-each true
```

This is the recommended way to evaluate knobs like `max_parallel_urls`, because it holds the
server process and caches constant while only changing tool arguments.

## Workspace layout

- `crates/webpipe`: historical facade crate (no longer a workspace member)
- `crates/webpipe-core`: backend-agnostic types + traits
- `crates/webpipe-local`: local implementations (reqwest + filesystem cache + extractors)
- `crates/webpipe-mcp`: the `webpipe` crate (CLI + MCP stdio server; bin `webpipe`)

## Install + run

```bash
# From crates.io:
cargo install webpipe

# From the repo root:
cargo install --path crates/webpipe-mcp --bin webpipe --features stdio --force

# Run as an MCP stdio server:
webpipe mcp-stdio
```

## Two audiences (how to think about the tools)

There are two “modes” in which `webpipe` is used:

- **Cursor user (MCP tools)**: you (or Cursor agents) call tools like `web_search_extract` directly from chat. The tool response is a **single JSON payload** that Cursor saves/displays as text.
- **Agent inside one tool call**: a higher-level agent/tool (outside of `webpipe`, or inside your own app) calls `webpipe` tools as steps in a workflow. The same JSON payloads are used, but you typically branch on fields like `warning_codes`, `error.code`, and `top_chunks`.

In both cases, the “structured output” is the JSON object:

- `ok: true|false`
- `schema_version`, `kind`, `elapsed_ms`
- optional `warnings` + `warning_codes` + `warning_hints`
- on error: `error: { code, message, hint, retryable }`

One important UX detail: most MCP clients (including Cursor) render **`content[0]`** (human-facing Markdown)
and keep the canonical machine payload in **`structured_content`**. If you expected “the extracted text” to
show up inline, set `include_text=true` on `web_extract` / `web_search_extract` (it is still bounded).

## Tool cookbook (Cursor / MCP users)

These examples are written as **tool arguments** (what you paste into the tool call UI).
Outputs are JSON-as-text; look at the `ok`, `warning_codes`, and the tool-specific fields described below.

### `webpipe_meta` (what’s configured)

```json
{}
```

Look for:
- `configured.*`: which providers/backends are available
- `supported.*`: allowed enum values
- `defaults.*`: what you get if you omit fields

### `web_search_extract` (the main “do the job” tool)

Online query-mode:

```json
{
  "query": "latest arxiv papers on tool-using LLM agents",
  "provider": "auto",
  "auto_mode": "fallback",
  "fetch_backend": "local",
  "agentic": true,
  "max_results": 5,
  "max_urls": 3,
  "top_chunks": 5,
  "max_chunk_chars": 500
}
```

JS-heavy pages (render in a headless browser first):

```json
{
  "query": "headers is an async function",
  "urls": ["https://nextjs.org/docs/app/api-reference/functions/headers"],
  "fetch_backend": "render",
  "max_urls": 1,
  "top_chunks": 6,
  "max_chars": 20000
}
```

JS-heavy pages (try local first, then fallback to render when local is empty/low-signal):

```json
{
  "query": "headers is an async function",
  "urls": ["https://nextjs.org/docs/app/api-reference/functions/headers"],
  "fetch_backend": "local",
  "render_fallback_on_empty_extraction": true,
  "render_fallback_on_low_signal": true,
  "max_urls": 1,
  "top_chunks": 6,
  "max_chars": 20000
}
```

Offline-ish urls-mode (no search; optionally rank chunks by query):

```json
{
  "query": "installation steps",
  "urls": ["https://example.com/docs/install", "https://example.com/docs/getting-started"],
  "url_selection_mode": "query_rank",
  "fetch_backend": "local",
  "no_network": false,
  "max_urls": 2,
  "max_parallel_urls": 2,
  "top_chunks": 4
}
```

Look for:
- `top_chunks[]`: the best evidence snippets (bounded)
- `results[]`: per-URL details
- `warning_hints`: concrete next moves when extraction is empty/low-signal/blocked

### `web_extract` (single URL, extraction-first)

```json
{
  "url": "https://example.com/docs/install",
  "fetch_backend": "local",
  "include_text": true,
  "max_chars": 20000,
  "include_links": false
}
```

Use this when you already know the URL and want extraction, not discovery.

### `web_fetch` (single URL, bytes/text-first)

```json
{
  "url": "https://example.com/",
  "fetch_backend": "local",
  "include_text": false,
  "include_headers": false,
  "max_bytes": 5000000
}
```

Use this when you need status/content-type/bytes and only sometimes need bounded text.

### `web_search` (search only)

```json
{
  "query": "rmcp mcp server examples",
  "provider": "auto",
  "auto_mode": "fallback",
  "max_results": 10
}
```

### `web_cache_search_extract` (no network; cache corpus only)

```json
{
  "query": "route handlers",
  "max_docs": 50,
  "top_chunks": 5
}
```

Use this for deterministic “did we already fetch something relevant?” checks.

### `web_deep_research` (evidence gather + optional synthesis)

Evidence-only mode (most inspectable):

```json
{
  "query": "What are the main recent approaches to tool-using LLM agents?",
  "synthesize": false,
  "include_evidence": true,
  "provider": "auto",
  "fetch_backend": "local",
  "max_urls": 3,
  "top_chunks": 5
}
```

### `arxiv_search` / `arxiv_enrich`

```json
{
  "query": "tool-using agents",
  "categories": ["cs.AI", "cs.CL"],
  "per_page": 10
}
```

```json
{
  "id_or_url": "https://arxiv.org/abs/2401.12345",
  "include_discussions": true,
  "max_discussion_urls": 2
}
```

## Tool cookbook (agent/workflow authors)

If you’re writing an agent that uses `webpipe` as a subsystem, the most reliable pattern is:

1) **Start bounded**: small `max_results`, `max_urls`, `top_chunks`, `max_chars`.
2) **Branch on warnings**:
   - `blocked_by_js_challenge` → try `fetch_backend="firecrawl"` (if configured) or choose a different URL
   - `empty_extraction` / `main_content_low_signal` → increase `max_bytes`/`max_chars`, or switch backend
3) **Prefer urls-mode for determinism**: once you have candidate URLs, pass `urls=[...]` and use `url_selection_mode=query_rank`.
4) **Use cache to get repeatable behavior**:
   - warm cache with `no_network=false, cache_write=true`
   - then evaluate with `no_network=true` to eliminate network flakiness

You can treat `ok=false` as “tool call failed” and use `error.hint` as the “next move”.

Environment variables (optional, provider-dependent):
- **Brave search**: `WEBPIPE_BRAVE_API_KEY` (or `BRAVE_SEARCH_API_KEY`)
- **Tavily search**: `WEBPIPE_TAVILY_API_KEY` (or `TAVILY_API_KEY`)
- **SearXNG search** (self-hosted): `WEBPIPE_SEARXNG_ENDPOINT` (or `WEBPIPE_SEARXNG_ENDPOINTS`)
- **Firecrawl fetch**: `WEBPIPE_FIRECRAWL_API_KEY` (or `FIRECRAWL_API_KEY`)
- **Perplexity deep research**: `WEBPIPE_PERPLEXITY_API_KEY` (or `PERPLEXITY_API_KEY`)
- **ArXiv endpoint override** (debug): `WEBPIPE_ARXIV_ENDPOINT` (default `https://export.arxiv.org/api/query`)

## Safety defaults

`webpipe` drops request-secret headers by default:
- `Authorization`
- `Cookie`
- `Proxy-Authorization`

If a caller supplies them, responses include a warning:
- `warning_codes` contains `unsafe_request_headers_dropped`
- `request.dropped_request_headers` lists header *names only*

To opt in (only for trusted endpoints), set `WEBPIPE_ALLOW_UNSAFE_HEADERS=true`.

## Quick test

```bash
cargo test -p webpipe-mcp --features stdio
```
