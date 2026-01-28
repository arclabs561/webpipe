## Archived eval/VLM harness (removed from public surface)

`webpipe` is intended to be a small, public-facing MCP server for **bounded web search/fetch/extract**.

Over time, it accumulated a lot of evaluation and judging harness code (matrix runs, critic loops, VLM
critique tooling, transcript summarizers). Those are useful, but they are not the core “public surface”
for `webpipe`.

### What changed

- The `webpipe-mcp` default feature set is now **`["stdio"]` only**.
- The optional VLM CLI commands are gated behind the **`vlm`** feature and are **not enabled by default**.
- The `eval-*` CLI/test surface in `webpipe-mcp` has been removed from the active test suite to keep
  public CI fast and focused.

### Where the judge tooling lives now

The artifact-first judge utilities (matrix score/export/judge + transcript/VLM summarization) live in
`dev/judgekit/` (intended to be a private repo).

