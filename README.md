# webpipe

Local "search → fetch → extract" plumbing, exposed as an **MCP stdio** server.

`webpipe` is designed for **bounded, deterministic evidence gathering**:
- cache-first fetch by default
- explicit limits on bytes/chars/results
- stable, schema-versioned JSON outputs
- **zero API keys required** — the default tool surface works out of the box

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

Optional: to enable search providers, set API keys in `mcp.json` (see env vars below), but **don't commit keys**.

## Default tool surface (zero keys required)

The Normal toolset works with no API keys configured:

| Tool | Purpose |
|---|---|
| `webpipe_meta` | Server config, capabilities, and runtime stats (method=usage/usage_reset) |
| `search_evidence` | **Main tool**: optional search → fetch → extract → ranked top chunks |
| `web_extract` | Extract readable text from a URL you already have |
| `web_fetch` | Fetch raw bytes/headers from a URL |
| `arxiv_search` | Search arXiv academic papers |
| `arxiv_enrich` | Fetch full metadata for a specific arXiv paper |
| `paper_search` | Search papers via Semantic Scholar / OpenAlex |

Tools that require API keys (shown only when configured):

| Tool | Key required |
|---|---|
| `web_perplexity` | `WEBPIPE_PERPLEXITY_API_KEY` |

## Sanity check

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
        tools = msg.get("result", {}).get("tools", [])
        print(f"{len(tools)} tools registered:")
        for t in tools:
            print(f"  {t['name']}")
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
- **`offline`**: *no network egress* (except `localhost`); use cache-only workflows.
- **`anonymous`**: network allowed **only when routed via an explicit proxy** (fail-closed).

Related env vars:

- **`WEBPIPE_ANON_PROXY`**: proxy URL used in anonymous mode.
  - For local/reqwest fetches, Tor users typically use `socks5h://127.0.0.1:9050`.
  - For Playwright render (`fetch_backend="render"`), use an HTTP proxy (e.g. `http://127.0.0.1:8118`).
- **`WEBPIPE_RENDER_DISABLE=1`**: force-disable render backend.

## Avoid Cargo build locks (dev ergonomics)

If you run multiple `cargo` commands concurrently, you can hit:
`Blocking waiting for file lock on build directory`.

For `webpipe` work, isolate build output by setting a per-repo target dir:

```bash
CARGO_TARGET_DIR=target-webpipe cargo test -p webpipe --tests -- --skip live_
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

There are two "modes" in which `webpipe` is used:

- **Cursor user (MCP tools)**: you (or Cursor agents) call tools like `search_evidence` directly from chat. The tool response is a **single JSON payload** that Cursor saves/displays as text.
- **Agent inside one tool call**: a higher-level agent/tool (outside of `webpipe`, or inside your own app) calls `webpipe` tools as steps in a workflow. The same JSON payloads are used, but you typically branch on fields like `warning_codes`, `error.code`, and `top_chunks`.

In both cases, the "structured output" is the JSON object:

- `ok: true|false`
- `schema_version`, `kind`, `elapsed_ms`
- optional `warning_codes` + `warning_hints`
- on error: `error: { code, message, hint, retryable }`

One important UX detail: most MCP clients (including Cursor) render **`content[0]`** (human-facing Markdown)
and keep the canonical machine payload in **`structuredContent`**. If you expected "the extracted text" to
show up inline, set `include_text=true` on `web_extract` or `search_evidence` (bounded by `max_chars`).

## Tool cookbook (Cursor / MCP users)

These examples are written as **tool arguments** (what you paste into the tool call UI).
Look at `ok`, `warning_codes`, and the tool-specific fields described below.

### `webpipe_meta` (what's configured)

```json
{}
```

Look for:
- `configured.*`: which providers/backends are available
- `available.*`: which search providers and optional tools are currently configured
- `supported.*`: allowed enum values
- `defaults.*`: recommended starting values

Check usage/cost stats:
```json
{"method": "usage"}
```

Reset usage counters:
```json
{"method": "usage_reset"}
```

### `search_evidence` (main evidence tool)

URLs-mode (no search; deterministic, works without search API keys):

```json
{
  "urls": ["https://example.com/docs/install", "https://example.com/docs/getting-started"],
  "query": "installation steps",
  "url_selection_mode": "query_rank",
  "fetch_backend": "local",
  "max_urls": 2,
  "top_chunks": 4
}
```

Search-mode (requires a search provider key: Brave, Tavily, or SearXNG):

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

Compact response (agent loops; ~10x smaller payload):

```json
{
  "query": "installation steps",
  "urls": ["https://example.com/docs/install"],
  "max_urls": 1,
  "top_chunks": 3,
  "minimal_output": true
}
```

Look for:
- `top_chunks[]`: the best evidence snippets (bounded)
- `results[]`: per-URL details (omitted when `minimal_output=true`)
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

### `web_perplexity` (when key is configured)

This tool is only visible when `WEBPIPE_PERPLEXITY_API_KEY` is set:

```json
{
  "query": "What are the trade-offs between retrieval-augmented generation and long context?",
  "model": "sonar",
  "timeout_ms": 20000
}
```

## Tool cookbook (agent/workflow authors)

If you're writing an agent that uses `webpipe` as a subsystem, the most reliable pattern is:

1) **Start bounded**: small `max_results`, `max_urls`, `top_chunks`, `max_chars`.
2) **Branch on warnings**:
   - `blocked_by_js_challenge` → try `fetch_backend="firecrawl"` (if configured) or choose a different URL
   - `empty_extraction` / `main_content_low_signal` → increase `max_bytes`/`max_chars`, or switch backend
3) **Prefer urls-mode for determinism**: once you have candidate URLs, pass `urls=[...]` and use `url_selection_mode=query_rank`.
4) **Use cache to get repeatable behavior**:
   - warm cache with `no_network=false, cache_write=true`
   - then evaluate with `no_network=true` to eliminate network flakiness
5) **Use `minimal_output=true` for tight loops** where you only need `top_chunks` and `warning_codes`.

You can treat `ok=false` as "tool call failed" and use `error.hint` as the "next move".

Environment variables (optional, provider-dependent):
- **Brave search**: `WEBPIPE_BRAVE_API_KEY` (or `BRAVE_SEARCH_API_KEY`)
- **Tavily search**: `WEBPIPE_TAVILY_API_KEY` (or `TAVILY_API_KEY`)
- **SearXNG search** (self-hosted): `WEBPIPE_SEARXNG_ENDPOINT` (or `WEBPIPE_SEARXNG_ENDPOINTS`)
- **Firecrawl fetch**: `WEBPIPE_FIRECRAWL_API_KEY` (or `FIRECRAWL_API_KEY`)
- **Perplexity synthesis**: `WEBPIPE_PERPLEXITY_API_KEY` (or `PERPLEXITY_API_KEY`)
- **ArXiv endpoint override** (debug): `WEBPIPE_ARXIV_ENDPOINT` (default `https://export.arxiv.org/api/query`)

## Quick test

```bash
cargo test -p webpipe --features stdio -- --skip live_
```
