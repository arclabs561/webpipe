# webpipe

Local “search → fetch → extract” plumbing, exposed as an **MCP stdio** server.

`webpipe` is designed for **bounded, deterministic evidence gathering**:
- cache-first fetch by default
- explicit limits on bytes/chars/results
- stable, schema-versioned JSON outputs

## Workspace layout

- `crates/webpipe`: **public facade crate** (re-exports `webpipe-core`)
- `crates/webpipe-core`: backend-agnostic types + traits
- `crates/webpipe-local`: local implementations (reqwest + filesystem cache + extractors)
- `crates/webpipe-mcp`: MCP stdio server + CLI harness (`webpipe`)

## Install + run

```bash
# From the repo root:
cargo install --path crates/webpipe-mcp --bin webpipe --features stdio --force

# Run as an MCP stdio server:
webpipe mcp-stdio
```

## Cursor MCP setup

Use `mcp.example.json` as a starting point (it intentionally contains **no API keys**).

Environment variables (optional, provider-dependent):
- **Brave search**: `WEBPIPE_BRAVE_API_KEY` (or `BRAVE_SEARCH_API_KEY`)
- **Tavily search**: `WEBPIPE_TAVILY_API_KEY` (or `TAVILY_API_KEY`)
- **SearXNG search** (self-hosted): `WEBPIPE_SEARXNG_ENDPOINT`
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

