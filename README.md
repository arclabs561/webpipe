# webpipe

Local "search → fetch → extract" plumbing, exposed as an **MCP stdio** server.

- cache-first fetch by default
- explicit limits on bytes/chars/results
- stable, schema-versioned JSON outputs
- **zero API keys required** — the default tool surface works out of the box

## Cursor MCP setup

Add this to your `~/.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "webpipe": {
      "command": "cargo",
      "args": ["run", "--release", "--bin", "webpipe", "--features", "stdio", "--", "mcp-stdio"],
      "cwd": "/path/to/webpipe"
    }
  }
}
```

## Default tool surface (zero keys required)

| Tool | Purpose |
|---|---|
| `webpipe_meta` | Server config, capabilities, and runtime stats |
| `search_evidence` | **Main tool**: search → fetch → extract → ranked top chunks |
| `web_extract` | Extract readable text from a URL you already have |
| `arxiv` | arXiv papers: search by topic (`query`) or get metadata for one paper (`id_or_url`) |
| `paper_search` | Search papers via Semantic Scholar / OpenAlex |

Tools that require API keys (shown only when configured):

| Tool | Key required |
|---|---|
| `web_perplexity` | `WEBPIPE_PERPLEXITY_API_KEY` |

> `web_fetch`, `arxiv_search`, and `arxiv_enrich` are still callable as aliases but no longer appear in the default surface.

## Tool cookbook

### `search_evidence` (main evidence tool)

URLs-mode (no search key needed):
```json
{
  "urls": ["https://example.com/docs/install"],
  "query": "installation steps",
  "max_urls": 2,
  "top_chunks": 4
}
```

Search-mode (requires Brave, Tavily, or SearXNG key):
```json
{
  "query": "tool-using LLM agents",
  "provider": "auto",
  "max_results": 5,
  "max_urls": 3,
  "top_chunks": 5
}
```

Compact for tight agent loops:
```json
{ "urls": ["https://example.com"], "query": "...", "max_urls": 1, "top_chunks": 3, "minimal_output": true }
```

### `web_extract` (single URL)

```json
{ "url": "https://example.com/docs", "include_text": true, "max_chars": 20000 }
```

### `arxiv` (search or enrich)

Search by topic:
```json
{ "query": "tool-using agents", "categories": ["cs.AI", "cs.CL"], "per_page": 10 }
```

Get metadata for a specific paper:
```json
{ "id_or_url": "2401.12345", "include_discussions": true }
```

### `web_perplexity` (when key is configured)

```json
{ "query": "trade-offs between RAG and long context", "model": "sonar" }
```

## Install

```bash
# From crates.io:
cargo install webpipe

# From repo root:
cargo install --path crates/webpipe-mcp --bin webpipe --features stdio --force

# Run as MCP stdio server:
webpipe mcp-stdio
```

## Quick test

```bash
cargo test -p webpipe --features stdio -- --skip live_
```

If another cargo process holds the build-directory lock:
```bash
CARGO_TARGET_DIR=/tmp/webpipe-test-target cargo test -p webpipe --features stdio -- --skip live_
```

## Privacy modes

Set `WEBPIPE_PRIVACY_MODE` to:
- **`normal`** (default): network allowed
- **`offline`**: no network egress (use cache only)
- **`anonymous`**: network only via explicit proxy (`WEBPIPE_ANON_PROXY`)

## Optional env vars

| Var | Purpose |
|---|---|
| `WEBPIPE_BRAVE_API_KEY` | Brave search |
| `WEBPIPE_TAVILY_API_KEY` | Tavily search |
| `WEBPIPE_SEARXNG_ENDPOINT` | Self-hosted SearXNG |
| `WEBPIPE_FIRECRAWL_API_KEY` | Firecrawl remote fetch |
| `WEBPIPE_PERPLEXITY_API_KEY` | Perplexity synthesis |
| `WEBPIPE_ANON_PROXY` | Proxy for anonymous mode (e.g. `socks5h://127.0.0.1:9050`) |

## CLI (no Cursor needed)

```bash
# List tools as an MCP client sees them:
webpipe mcp-list-tools

# Call a tool and print Markdown:
webpipe mcp-call --tool search_evidence --args-json '{"query":"example","urls":["https://example.com"],"max_urls":1}'
```

## Workspace layout

- `crates/webpipe-core`: backend-agnostic types + traits
- `crates/webpipe-local`: local implementations (reqwest + cache + extractors)
- `crates/webpipe-mcp`: CLI + MCP stdio server (`bin webpipe`)
