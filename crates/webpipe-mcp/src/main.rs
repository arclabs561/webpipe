#![recursion_limit = "256"]

use anyhow::Result;
use clap::{Parser, Subcommand};
mod eval;

#[derive(Parser, Debug)]
#[command(name = "webpipe")]
#[command(about = "Local fetch/search plumbing (MCP stdio server)", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Commands {
    /// Run as an MCP stdio server (for Cursor / MCP clients).
    #[cfg(feature = "stdio")]
    McpStdio,
    /// Run a local search evaluation harness (writes a JSON artifact).
    EvalSearch(EvalSearchCmd),
    /// Run a local fetch evaluation harness (writes a JSON artifact).
    EvalFetch(EvalFetchCmd),
    /// Run a local search->fetch->extract evaluation harness (writes a JSON artifact).
    #[cfg(feature = "stdio")]
    EvalSearchExtract(EvalSearchExtractCmd),
    /// Run a small E2E matrix over a versioned query set (writes a JSONL artifact).
    #[cfg(feature = "stdio")]
    EvalMatrix(EvalMatrixCmd),
    /// Score an eval-matrix JSONL artifact against an E2E qrels file (json).
    EvalMatrixScore(EvalMatrixScoreCmd),
    /// Export eval-matrix JSONL rows into a judge-ready JSONL dataset (optionally joined with qrels).
    EvalMatrixExport(EvalMatrixExportCmd),
    /// Deterministic baseline judge over exported eval-matrix examples (json).
    EvalMatrixJudge(EvalMatrixJudgeCmd),
    /// Run eval-matrix -> score -> export -> judge (one-shot).
    ///
    /// This is a convenience wrapper that invokes the existing subcommands with explicit
    /// artifact paths, so you can run a full end-to-end evaluation with one call.
    EvalMatrixRun(EvalMatrixRunCmd),
    /// Diagnose configuration/launch issues (json; no secrets).
    Doctor(DoctorCmd),
    /// Print version info.
    Version(VersionCmd),
    /// Score an eval_search_extract artifact against a qrels file (json).
    EvalQrels(EvalQrelsCmd),
}

#[derive(clap::Args, Debug)]
struct EvalQrelsCmd {
    /// Eval artifact path produced by `eval-search-extract`.
    #[arg(long)]
    eval_artifact: std::path::PathBuf,
    /// Qrels file (json), see `crates/webpipe-mcp/fixtures/qrels_seed.json`.
    #[arg(long)]
    qrels: std::path::PathBuf,
    /// Output JSON path (default: .generated/webpipe-eval-qrels-<epoch>.json)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalSearchExtractCmd {
    /// Provider to use. Allowed: auto, brave, tavily, searxng
    #[arg(long, default_value = "auto")]
    provider: String,
    /// When provider="auto", choose routing mode. Allowed: fallback, merge, mab
    #[arg(long, default_value = "fallback")]
    auto_mode: String,
    /// How to select `top_chunks` across URLs. Allowed: score, pareto
    #[arg(long, default_value = "score")]
    selection_mode: String,
    /// Optional: run multiple selection modes and include a summary comparison.
    ///
    /// Example: "score,pareto"
    #[arg(long)]
    compare_selection_modes: Option<String>,
    /// Which fetch backend to use. Allowed: local, firecrawl
    #[arg(long, default_value = "local")]
    fetch_backend: String,
    /// If true, and fetch_backend="local", retry a single URL with Firecrawl when local extraction yields empty text.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = false)]
    firecrawl_fallback_on_empty_extraction: bool,

    /// Query (repeatable). If you pass urls, query is optional.
    #[arg(long)]
    query: Vec<String>,
    /// File containing queries (one per line; blank lines and #comments ignored).
    #[arg(long)]
    queries_file: Vec<std::path::PathBuf>,
    /// Seed query dataset (json), e.g. `crates/webpipe-mcp/fixtures/queries_seed.json`.
    ///
    /// When provided, query runs will include `query_id` so they can be scored against qrels.
    #[arg(long)]
    queries_json: Vec<std::path::PathBuf>,

    /// URL (repeatable). If provided, skip search and hydrate these URLs directly.
    #[arg(long)]
    url: Vec<String>,
    /// File containing URLs (one per line; blank lines and #comments ignored).
    #[arg(long)]
    urls_file: Vec<std::path::PathBuf>,
    /// When urls=[...] is provided, how to pick URLs under max_urls. Allowed: auto, preserve, query_rank
    #[arg(long, default_value = "auto")]
    url_selection_mode: String,

    /// Enable agentic URL discovery loop (bounded by max_urls).
    ///
    /// This allows the evaluator to follow links and replace weak initial picks,
    /// rather than assuming the seed list ordering or URL-string heuristics are sufficient.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = false)]
    agentic: bool,

    /// Agentic selector strategy. Allowed: auto, lexical, llm
    ///
    /// - auto: use OpenRouter if configured, else lexical
    /// - lexical: deterministic URL/link scoring only
    /// - llm: force OpenRouter planner (falls back to lexical if not configured)
    #[arg(long, default_value = "auto")]
    agentic_selector: String,

    /// High-level width vs depth preset. Allowed: balanced, wide, deep
    ///
    /// Note: this only fills defaults for optional knobs like --max-results/--max-urls when
    /// those flags are omitted.
    #[arg(long, default_value = "balanced")]
    exploration: String,

    /// When agentic=true, allow up to this many search rounds total.
    #[arg(long)]
    agentic_max_search_rounds: Option<usize>,

    /// When agentic=true, maximum frontier size.
    #[arg(long)]
    agentic_frontier_max: Option<usize>,

    /// Max planner (LLM) calls for a single request.
    #[arg(long)]
    planner_max_calls: Option<usize>,

    #[arg(long)]
    max_results: Option<usize>,
    #[arg(long)]
    max_urls: Option<usize>,
    #[arg(long, default_value_t = 20_000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 5_000_000)]
    max_bytes: u64,
    /// If true, and fetch_backend="local", retry a URL with a higher max_bytes when the body was truncated.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = false)]
    retry_on_truncation: bool,
    /// When retry_on_truncation=true, max_bytes to use for the retry.
    #[arg(long)]
    truncation_retry_max_bytes: Option<u64>,
    /// Width used for HTML->text extraction (default: 100).
    #[arg(long, default_value_t = 100)]
    width: usize,
    /// Max chars of extracted text per URL.
    #[arg(long)]
    max_chars: Option<usize>,
    /// Max chunks per URL (also used as the across-URL `top_chunks` cap).
    #[arg(long)]
    top_chunks: Option<usize>,
    /// Max chars per chunk.
    #[arg(long)]
    max_chunk_chars: Option<usize>,
    /// Include extracted links.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    include_links: bool,
    #[arg(long, default_value_t = 25)]
    max_links: usize,
    /// Include full extracted text in per-URL results.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = false)]
    include_text: bool,

    /// Output JSON path (default: .generated/webpipe-eval-search-extract-<epoch>.json)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalMatrixCmd {
    /// E2E query dataset (json), e.g. `crates/webpipe-mcp/fixtures/e2e_queries_v1.json`.
    #[arg(long)]
    queries_json: std::path::PathBuf,
    /// Base URL used to expand `url_paths` entries in the dataset.
    ///
    /// Example: http://127.0.0.1:8080
    #[arg(long)]
    base_url: String,
    /// Provider to use for the "search" leg. Allowed: auto, brave, tavily, searxng
    #[arg(long, default_value = "searxng")]
    provider: String,
    /// When provider="auto", choose routing mode. Allowed: fallback, merge, mab
    #[arg(long, default_value = "fallback")]
    auto_mode: String,
    /// How to select `top_chunks` across URLs. Allowed: score, pareto
    #[arg(long, default_value = "score")]
    selection_mode: String,
    /// Which fetch backend to use. Allowed: local, firecrawl
    #[arg(long, default_value = "local")]
    fetch_backend: String,
    /// Max search results to request (default: 1; max: 20).
    #[arg(long, default_value_t = 1)]
    max_results: usize,
    /// Max URLs to process (default: 1; max: 10).
    #[arg(long, default_value_t = 1)]
    max_urls: usize,
    /// Fetch timeout per URL (ms).
    #[arg(long, default_value_t = 20_000)]
    timeout_ms: u64,
    /// Max bytes per URL.
    #[arg(long, default_value_t = 2_000_000)]
    max_bytes: u64,
    /// Across-URL top chunks cap.
    #[arg(long, default_value_t = 2)]
    top_chunks: usize,
    /// Max chars per chunk.
    #[arg(long, default_value_t = 200)]
    max_chunk_chars: usize,
    /// Output JSONL path (default: .generated/webpipe-eval-matrix-<epoch>.jsonl)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalMatrixScoreCmd {
    /// Matrix artifact path produced by `eval-matrix` (jsonl).
    #[arg(long)]
    matrix_artifact: std::path::PathBuf,
    /// E2E qrels file (json), see `crates/webpipe-mcp/fixtures/e2e_qrels_v1.json`.
    #[arg(long)]
    qrels: std::path::PathBuf,
    /// Output JSON path (default: .generated/webpipe-eval-matrix-score-<epoch>.json)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalMatrixExportCmd {
    /// Matrix artifact path produced by `eval-matrix` (jsonl).
    #[arg(long)]
    matrix_artifact: std::path::PathBuf,
    /// Optional E2E qrels file (json) to attach expected URL needles and a hit label.
    #[arg(long)]
    qrels: Option<std::path::PathBuf>,
    /// Max characters per exported text field (keeps artifacts bounded).
    #[arg(long, default_value_t = 4000)]
    max_text_chars: usize,
    /// Output JSONL path (default: .generated/webpipe-eval-matrix-export-<epoch>.jsonl)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalMatrixJudgeCmd {
    /// Exported examples path produced by `eval-matrix-export` (jsonl).
    #[arg(long)]
    examples_artifact: std::path::PathBuf,
    /// Output JSON path (default: .generated/webpipe-eval-matrix-judge-<epoch>.json)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalMatrixRunCmd {
    /// E2E query dataset (json), e.g. `crates/webpipe-mcp/fixtures/e2e_queries_v1.json`.
    #[arg(long)]
    queries_json: std::path::PathBuf,
    /// E2E qrels file (json), e.g. `crates/webpipe-mcp/fixtures/e2e_qrels_v1.json`.
    #[arg(long)]
    qrels: std::path::PathBuf,
    /// Base URL used to expand `url_paths` entries in the dataset.
    #[arg(long)]
    base_url: String,
    /// Provider to use for the "search" leg. Allowed: auto, brave, tavily, searxng
    #[arg(long, default_value = "searxng")]
    provider: String,
    /// When provider="auto", choose routing mode. Allowed: fallback, merge, mab
    #[arg(long, default_value = "fallback")]
    auto_mode: String,
    /// How to select `top_chunks` across URLs. Allowed: score, pareto
    #[arg(long, default_value = "score")]
    selection_mode: String,
    /// Which fetch backend to use. Allowed: local, firecrawl
    #[arg(long, default_value = "local")]
    fetch_backend: String,
    /// Output directory for all artifacts (default: .generated).
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
    /// Max characters per exported judge_text (keeps artifacts bounded).
    #[arg(long, default_value_t = 4000)]
    max_text_chars: usize,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
    /// Optional git SHA to include in the run manifest (best-effort if omitted).
    #[arg(long)]
    git_sha: Option<String>,
    /// Output format: json|text
    #[arg(long = "output", alias = "format", default_value = "text")]
    output: String,
}

#[derive(clap::Args, Debug)]
struct EvalSearchCmd {
    /// Provider list (comma-separated). Allowed: brave,tavily
    #[arg(long, default_value = "brave,tavily")]
    providers: String,
    /// Query (repeatable).
    #[arg(long)]
    query: Vec<String>,
    /// File containing queries (one per line; blank lines and #comments ignored).
    #[arg(long)]
    queries_file: Vec<std::path::PathBuf>,
    #[arg(long, default_value_t = 10)]
    max_results: usize,
    #[arg(long)]
    language: Option<String>,
    #[arg(long)]
    country: Option<String>,
    /// Output JSON path (default: .generated/webpipe-eval-search-<epoch>.json)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct EvalFetchCmd {
    /// Fetcher list (comma-separated). Allowed: local,firecrawl
    #[arg(long, default_value = "local,firecrawl")]
    fetchers: String,
    /// URL (repeatable).
    #[arg(long)]
    url: Vec<String>,
    /// File containing URLs (one per line; blank lines and #comments ignored).
    #[arg(long)]
    urls_file: Vec<std::path::PathBuf>,
    #[arg(long, default_value_t = 30_000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 5_000_000)]
    max_bytes: u64,
    /// Max chars stored per output text (keeps artifacts bounded).
    #[arg(long, default_value_t = 10_000)]
    max_text_chars: usize,
    /// Also run a local HTML->text extraction pass and include it in the artifact.
    #[arg(long, default_value_t = false)]
    extract: bool,
    /// Width used for HTML->text extraction (default: 100).
    #[arg(long, default_value_t = 100)]
    extract_width: usize,
    /// Output JSON path (default: .generated/webpipe-eval-fetch-<epoch>.json)
    #[arg(long)]
    out: Option<std::path::PathBuf>,
    /// Override "now" for deterministic outputs.
    #[arg(long)]
    now_epoch_s: Option<u64>,
}

#[derive(clap::Args, Debug)]
struct DoctorCmd {
    /// Output format: json|text
    #[arg(long = "output", alias = "format", default_value = "json")]
    output: String,
    /// Attempt a local stdio MCP handshake (list_tools) to prove Cursor can start the server.
    ///
    /// This is a self-check: it spawns a child `webpipe mcp-stdio` process and calls `list_tools`.
    /// It does not perform any network fetch/search, and it does not print any secret values.
    #[arg(long, action = clap::ArgAction::Set, default_value_t = true)]
    check_stdio: bool,
    /// Timeout for the stdio handshake (ms).
    #[arg(long, default_value_t = 3000)]
    timeout_ms: u64,
}

#[derive(clap::Args, Debug)]
struct VersionCmd {
    /// Output format: json|text
    #[arg(long = "output", alias = "format", default_value = "json")]
    output: String,
}

#[cfg(feature = "stdio")]
mod mcp {
    use super::*;
    use rmcp::{
        handler::server::router::tool::ToolRouter as RmcpToolRouter,
        handler::server::wrapper::Parameters,
        model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
        tool, tool_handler, tool_router,
        transport::stdio,
        ErrorData as McpError, ServiceExt,
    };
    use schemars::JsonSchema;
    use serde::Deserialize;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use webpipe_core::{
        Error as WebpipeError, FetchBackend, FetchCachePolicy, FetchRequest, FetchSource,
        SearchProvider, SearchQuery,
    };
    use webpipe_local::LocalFetcher;

    const SCHEMA_VERSION: u64 = 1;

    fn p<T>(v: T) -> Parameters<Option<T>> {
        Parameters(Some(v))
    }

    #[path = "envelope.rs"]
    mod envelope;
    use envelope::*;

    // ---- Minimal self-contained helpers (public-repo friendly) ----
    //
    // Historically, this repo used workspace-local crates for text normalization, Pareto selection,
    // and routing windows. For a standalone public repository, we keep small, deterministic
    // implementations here instead.

    mod textprep {
        /// Deterministic, lightweight normalization used for stable keys.
        ///
        /// Goals:
        /// - never panic
        /// - ASCII-lowercase, whitespace-normalized
        /// - keep it small (not a full tokenizer)
        pub fn scrub(s: &str) -> String {
            let mut out = String::new();
            let mut last_space = true;
            for ch in s.chars() {
                let c = ch.to_ascii_lowercase();
                if c.is_ascii_alphanumeric() {
                    out.push(c);
                    last_space = false;
                } else if c.is_whitespace() || c == '-' || c == '_' || c == '/' {
                    if !last_space {
                        out.push(' ');
                        last_space = true;
                    }
                } else {
                    // drop other punctuation/control
                }
            }
            out.trim().to_string()
        }

        pub mod stopwords {
            // Small, stable list: enough to avoid obvious junk tokens.
            pub const ENGLISH: [&str; 47] = [
                "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "has", "have",
                "how", "i", "in", "is", "it", "its", "me", "my", "of", "on", "or", "our", "s",
                "she", "that", "the", "their", "them", "there", "they", "this", "to", "was", "we",
                "were", "what", "when", "where", "which", "who", "why", "will", "with", "you",
                "your",
            ];
        }
    }

    mod pare {
        /// Return indices of the Pareto frontier (maximize all dimensions).
        pub fn pareto_indices(metrics: &[Vec<f32>]) -> Option<Vec<usize>> {
            if metrics.is_empty() {
                return None;
            }
            let dim = metrics[0].len();
            if dim == 0 || metrics.iter().any(|v| v.len() != dim) {
                return None;
            }

            fn dominates(a: &[f32], b: &[f32]) -> bool {
                // a dominates b if a >= b in all dims and a > b in at least one.
                let mut any_strict = false;
                for i in 0..a.len() {
                    if a[i] < b[i] {
                        return false;
                    }
                    if a[i] > b[i] {
                        any_strict = true;
                    }
                }
                any_strict
            }

            let mut frontier: Vec<usize> = Vec::new();
            'outer: for i in 0..metrics.len() {
                for j in 0..metrics.len() {
                    if i == j {
                        continue;
                    }
                    if dominates(&metrics[j], &metrics[i]) {
                        continue 'outer;
                    }
                }
                frontier.push(i);
            }
            Some(frontier)
        }
    }

    mod muxer {
        use serde::Serialize;
        use std::collections::VecDeque;

        #[derive(Debug, Clone, Copy)]
        pub struct Outcome {
            pub ok: bool,
            pub http_429: bool,
            pub junk: bool,
            pub hard_junk: bool,
            pub cost_units: u64,
            pub elapsed_ms: u64,
        }

        #[derive(Debug, Clone, Default, Serialize)]
        pub struct Summary {
            pub calls: u64,
            pub ok: u64,
            pub http_429: u64,
            pub junk: u64,
            pub hard_junk: u64,
            pub cost_units: u64,
            pub elapsed_ms_sum: u64,
        }

        impl Summary {
            pub fn ok_rate(&self) -> f64 {
                if self.calls == 0 {
                    0.0
                } else {
                    (self.ok as f64) / (self.calls as f64)
                }
            }
            pub fn http_429_rate(&self) -> f64 {
                if self.calls == 0 {
                    0.0
                } else {
                    (self.http_429 as f64) / (self.calls as f64)
                }
            }
            pub fn junk_rate(&self) -> f64 {
                if self.calls == 0 {
                    0.0
                } else {
                    (self.junk as f64) / (self.calls as f64)
                }
            }
            pub fn hard_junk_rate(&self) -> f64 {
                if self.calls == 0 {
                    0.0
                } else {
                    (self.hard_junk as f64) / (self.calls as f64)
                }
            }
            pub fn mean_cost_units(&self) -> f64 {
                if self.calls == 0 {
                    0.0
                } else {
                    (self.cost_units as f64) / (self.calls as f64)
                }
            }
            pub fn mean_latency_ms(&self) -> f64 {
                if self.calls == 0 {
                    0.0
                } else {
                    (self.elapsed_ms_sum as f64) / (self.calls as f64)
                }
            }
        }

        #[derive(Debug, Clone)]
        pub struct Window {
            cap: usize,
            buf: VecDeque<Outcome>,
        }

        impl Window {
            pub fn new(cap: usize) -> Self {
                Self {
                    cap: cap.max(1),
                    buf: VecDeque::new(),
                }
            }

            pub fn push(&mut self, o: Outcome) {
                self.buf.push_back(o);
                while self.buf.len() > self.cap {
                    self.buf.pop_front();
                }
            }

            pub fn set_last_junk_level(&mut self, junk: bool, hard_junk: bool) {
                if let Some(last) = self.buf.back_mut() {
                    last.junk = junk;
                    last.hard_junk = hard_junk;
                }
            }

            pub fn summary(&self) -> Summary {
                let mut s = Summary::default();
                for o in &self.buf {
                    s.calls += 1;
                    s.ok += o.ok as u64;
                    s.http_429 += o.http_429 as u64;
                    s.junk += o.junk as u64;
                    s.hard_junk += o.hard_junk as u64;
                    s.cost_units = s.cost_units.saturating_add(o.cost_units);
                    s.elapsed_ms_sum = s.elapsed_ms_sum.saturating_add(o.elapsed_ms);
                }
                s
            }
        }

        #[derive(Debug, Clone)]
        pub struct MabConfig {
            pub exploration_c: f64,
            pub cost_weight: f64,
            pub latency_weight: f64,
            pub junk_weight: f64,
            pub hard_junk_weight: f64,
            pub max_junk_rate: Option<f64>,
            pub max_hard_junk_rate: Option<f64>,
            pub max_http_429_rate: Option<f64>,
            pub max_mean_cost_units: Option<f64>,
        }

        impl Default for MabConfig {
            fn default() -> Self {
                Self {
                    exploration_c: 0.7,
                    cost_weight: 0.0,
                    latency_weight: 0.0,
                    junk_weight: 0.0,
                    hard_junk_weight: 0.0,
                    max_junk_rate: None,
                    max_hard_junk_rate: None,
                    max_http_429_rate: None,
                    max_mean_cost_units: None,
                }
            }
        }

        #[derive(Debug, Clone, Serialize)]
        pub struct CandidateRow {
            pub name: String,
            pub calls: u64,
            pub ok_rate: f64,
            pub http_429_rate: f64,
            pub junk_rate: f64,
            pub hard_junk_rate: f64,
            pub mean_cost_units: f64,
            pub mean_latency_ms: f64,
            pub score: f64,
            pub filtered: bool,
        }

        #[derive(Debug, Clone, Serialize)]
        pub struct MabSelection {
            pub chosen: String,
            pub frontier: Vec<String>,
            pub candidates: Vec<CandidateRow>,
        }

        pub fn select_mab(
            order: &[String],
            summaries: &std::collections::BTreeMap<String, Summary>,
            cfg: &MabConfig,
        ) -> MabSelection {
            let total_calls: u64 = summaries.values().map(|s| s.calls).sum();
            let ln_total = ((total_calls.max(1)) as f64).ln();

            let mut rows: Vec<CandidateRow> = Vec::new();
            for name in order {
                let s = summaries.get(name).cloned().unwrap_or_default();
                let ok_rate = s.ok_rate();
                let http_429_rate = s.http_429_rate();
                let junk_rate = s.junk_rate();
                let hard_junk_rate = s.hard_junk_rate();
                let mean_cost_units = s.mean_cost_units();
                let mean_latency_ms = s.mean_latency_ms();

                let mut filtered = false;
                if let Some(max) = cfg.max_http_429_rate {
                    filtered |= http_429_rate > max;
                }
                if let Some(max) = cfg.max_junk_rate {
                    filtered |= junk_rate > max;
                }
                if let Some(max) = cfg.max_hard_junk_rate {
                    filtered |= hard_junk_rate > max;
                }
                if let Some(max) = cfg.max_mean_cost_units {
                    filtered |= mean_cost_units > max;
                }

                // Deterministic UCB-ish objective (maximize).
                // We use ok_rate as the primary reward and subtract weighted costs/risks.
                let base = ok_rate
                    - cfg.cost_weight * mean_cost_units
                    - cfg.latency_weight * (mean_latency_ms / 1000.0)
                    - cfg.junk_weight * junk_rate
                    - cfg.hard_junk_weight * hard_junk_rate;
                let explore = if s.calls == 0 {
                    cfg.exploration_c
                } else {
                    cfg.exploration_c * ((ln_total / (s.calls as f64)).sqrt())
                };
                let score = base + explore;

                rows.push(CandidateRow {
                    name: name.clone(),
                    calls: s.calls,
                    ok_rate,
                    http_429_rate,
                    junk_rate,
                    hard_junk_rate,
                    mean_cost_units,
                    mean_latency_ms,
                    score,
                    filtered,
                });
            }

            // If constraints filtered everything, fall back to unfiltered.
            let any_unfiltered = rows.iter().any(|r| !r.filtered);
            if !any_unfiltered {
                for r in &mut rows {
                    r.filtered = false;
                }
            }

            let mut chosen = String::new();
            let mut best = f64::NEG_INFINITY;
            for r in &rows {
                if r.filtered {
                    continue;
                }
                // Deterministic tie-breakers when scores are equal:
                // prefer lower hard-junk, then lower junk, then lower 429s, then lower cost/latency,
                // then earlier order.
                let eps = 1e-12;
                let better = if r.score > best + eps {
                    true
                } else if (r.score - best).abs() <= eps {
                    // Compare against current best row (if any).
                    if chosen.is_empty() {
                        true
                    } else {
                        let cur = rows.iter().find(|x| x.name == chosen);
                        if let Some(cur) = cur {
                            (
                                r.hard_junk_rate,
                                r.junk_rate,
                                r.http_429_rate,
                                r.mean_cost_units,
                                r.mean_latency_ms,
                            ) < (
                                cur.hard_junk_rate,
                                cur.junk_rate,
                                cur.http_429_rate,
                                cur.mean_cost_units,
                                cur.mean_latency_ms,
                            )
                        } else {
                            true
                        }
                    }
                } else {
                    false
                };
                if better {
                    best = r.score;
                    chosen = r.name.clone();
                }
            }
            if chosen.is_empty() {
                chosen = order.first().cloned().unwrap_or_default();
            }

            // Frontier: best-effort list of top candidates by score (bounded, stable).
            let mut frontier = rows
                .iter()
                .filter(|r| !r.filtered)
                .cloned()
                .collect::<Vec<_>>();
            frontier.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let frontier_names = frontier.into_iter().take(3).map(|r| r.name).collect();

            MabSelection {
                chosen,
                frontier: frontier_names,
                candidates: rows,
            }
        }
    }

    fn cache_dir_from_env() -> Option<PathBuf> {
        std::env::var("WEBPIPE_CACHE_DIR").ok().map(PathBuf::from)
    }

    pub(crate) fn default_cache_dir() -> PathBuf {
        // Prefer a persistent per-user cache directory (so warm-cache + no_network works across
        // restarts), with a temp-dir fallback.
        dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("webpipe-cache")
    }

    fn has_env(k: &str) -> bool {
        std::env::var(k).ok().is_some_and(|v| !v.trim().is_empty())
    }

    fn normalize_warning_code(w: &'static str) -> &'static str {
        match w {
            // Canonicalize historical drift.
            "text_truncated_by_max_text_chars" => "text_truncated_by_max_chars",
            // Canonicalize “capability unavailable” variants.
            "links_unavailable_for_firecrawl" => "links_unavailable",
            "links_unavailable_for_pdf" => "links_unavailable",
            "headers_unavailable_for_firecrawl" => "headers_unavailable",
            "text_unavailable_for_pdf_use_web_extract" => "text_unavailable_for_pdf",
            // Default: pass through existing stable codes.
            _ => w,
        }
    }

    fn url_looks_like_auth_or_challenge(url: &str) -> bool {
        // Generic (non-domain-specific) heuristics to avoid agentic loops getting stuck on
        // auth walls / JS challenges / consent gates.
        let u = url.to_ascii_lowercase();
        // Path-ish signals.
        for needle in [
            "/login",
            "/log-in",
            "/signin",
            "/sign-in",
            "/signup",
            "/sign-up",
            "/register",
            "/auth",
            "/oauth",
            "/sso",
            "/captcha",
            "/challenge",
            "/consent",
        ] {
            if u.contains(needle) {
                return true;
            }
        }
        // Query-ish signals.
        for needle in ["returnurl=", "redirect=", "continue=", "next=", "callback="] {
            if u.contains(needle) {
                return true;
            }
        }
        // Common bot mitigation tokens (best-effort).
        for needle in ["cf_chl", "cloudflare", "turnstile"] {
            if u.contains(needle) {
                return true;
            }
        }
        false
    }

    fn looks_like_js_challenge(status: u16, extracted_text: &str, title: Option<&str>) -> bool {
        // Generic “JS required / challenge page” detector, used for warnings + agentic bailout.
        if status == 403 || status == 429 {
            // Many challenge pages show up as 403/429.
            return true;
        }
        let t = extracted_text.to_ascii_lowercase();
        if t.contains("enable javascript")
            || t.contains("challenge-error-text")
            || t.contains("just a moment")
        {
            return true;
        }
        if let Some(tt) = title {
            let tl = tt.to_ascii_lowercase();
            if tl.contains("just a moment") || tl.contains("enable javascript") {
                return true;
            }
        }
        false
    }

    fn structure_looks_like_ui_shell(
        structure: &webpipe_local::extract::ExtractedStructure,
    ) -> bool {
        // Generic heuristic: UI shells tend to be mostly short list items / nav labels.
        // We avoid host-specific rules; this is purely shape-based.
        let total = structure.blocks.len();
        if total == 0 {
            return false;
        }
        let listish = structure
            .blocks
            .iter()
            .filter(|b| b.kind == "list_item")
            .count();
        let short_blocks = structure
            .blocks
            .iter()
            .filter(|b| b.text.chars().count() <= 40)
            .count();
        let list_ratio = (listish as f32) / (total as f32);
        let short_ratio = (short_blocks as f32) / (total as f32);
        if (list_ratio >= 0.60 && short_ratio >= 0.60) || (list_ratio >= 0.85) {
            return true;
        }

        // UI shells also frequently contain auth/consent words even when list items are longer.
        if list_ratio >= 0.60 {
            let sl = structure.structure_text.to_ascii_lowercase();
            for needle in [
                "sign up", "log in", "login", "cookie", "consent", "privacy", "terms",
            ] {
                if sl.contains(needle) {
                    return true;
                }
            }
        }

        false
    }

    fn warning_codes_from(warnings: &[&'static str]) -> Vec<&'static str> {
        warnings.iter().map(|w| normalize_warning_code(w)).collect()
    }

    fn is_http_status(msg: &str, code: u16) -> bool {
        msg.contains(&format!("HTTP {code}")) || msg.contains(&format!("{code} Too Many Requests"))
    }

    fn parse_searxng_arm(name: &str) -> Option<usize> {
        name.strip_prefix("searxng#")
            .and_then(|s| s.trim().parse::<usize>().ok())
    }

    #[derive(Debug, Clone, Copy)]
    struct SeedSpec {
        id: &'static str,
        url: &'static str,
        kind: &'static str,
        note: &'static str,
    }

    fn seed_registry() -> &'static [SeedSpec] {
        &[
            SeedSpec {
                id: "public-apis",
                url: "https://raw.githubusercontent.com/public-apis/public-apis/master/README.md",
                kind: "awesome_list",
                note: "Public APIs directory (huge; good for keyless URL seeds).",
            },
            SeedSpec {
                id: "awesome-selfhosted",
                url: "https://raw.githubusercontent.com/awesome-selfhosted/awesome-selfhosted/master/README.md",
                kind: "awesome_list",
                note: "Self-hosted services; many have public endpoints/docs.",
            },
            SeedSpec {
                id: "awesome",
                url: "https://raw.githubusercontent.com/sindresorhus/awesome/master/readme.md",
                kind: "awesome_list",
                note: "Meta-index of awesome lists (good pivot for more seeds).",
            },
            SeedSpec {
                id: "awesome-python",
                url: "https://raw.githubusercontent.com/vinta/awesome-python/master/README.md",
                kind: "awesome_list",
                note: "Python ecosystem index; good for tooling + libraries for adapters.",
            },
            SeedSpec {
                id: "free-for-dev",
                url: "https://raw.githubusercontent.com/ripienaar/free-for-dev/master/README.md",
                kind: "awesome_list",
                note: "Free tiers across many providers (useful for alternatives when rate-limited).",
            },
            SeedSpec {
                id: "awesome-public-datasets",
                url: "https://raw.githubusercontent.com/awesomedata/awesome-public-datasets/master/README.rst",
                kind: "awesome_list",
                note: "Public datasets index (good seed for offline-ish evidence gathering).",
            },
            SeedSpec {
                id: "awesome-go",
                url: "https://raw.githubusercontent.com/avelino/awesome-go/main/README.md",
                kind: "awesome_list",
                note: "Go ecosystem index (useful for tooling/services).",
            },
            SeedSpec {
                id: "awesome-sysadmin",
                url: "https://raw.githubusercontent.com/awesome-foss/awesome-sysadmin/master/README.md",
                kind: "awesome_list",
                note: "Sysadmin tooling index (good for deployable/self-hostable alternatives).",
            },
        ]
    }

    fn select_seed_urls(
        registry: &[SeedSpec],
        urls: Option<Vec<String>>,
        seed_ids: Option<Vec<String>>,
        max_urls: usize,
    ) -> (Vec<(String, String)>, Vec<String>) {
        if let Some(us) = urls {
            let picked = us
                .into_iter()
                .filter(|u| !u.trim().is_empty())
                .take(max_urls)
                .enumerate()
                .map(|(i, u)| (format!("custom#{i}"), u))
                .collect::<Vec<_>>();
            return (picked, Vec::new());
        }

        let mut unknown_seed_ids: Vec<String> = Vec::new();

        let chosen: Vec<SeedSpec> = if let Some(ids) = seed_ids {
            let mut out: Vec<SeedSpec> = Vec::new();
            let mut seen = std::collections::BTreeSet::<String>::new();
            for id0 in ids {
                let id = id0.trim().to_string();
                if id.is_empty() {
                    continue;
                }
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(s) = registry.iter().find(|s| s.id == id) {
                    out.push(*s);
                } else {
                    unknown_seed_ids.push(id);
                }
            }
            out
        } else {
            registry.to_vec()
        };

        let picked = chosen
            .into_iter()
            .take(max_urls)
            .map(|s| (s.id.to_string(), s.url.to_string()))
            .collect::<Vec<_>>();

        (picked, unknown_seed_ids)
    }

    fn search_failed_hint(provider: &str, msg: &str, default_hint: &str) -> String {
        if is_http_status(msg, 429) {
            return format!(
                "{provider} is rate-limiting (HTTP 429). Retry later; reduce max_results; use urls=[...] offline mode; or switch providers."
            );
        }
        default_hint.to_string()
    }

    fn tool_result(payload: serde_json::Value) -> CallToolResult {
        // Always attach structured content for machine consumers, and include a text fallback
        // for older clients/tests that only read `content[0].text`.
        let mut r = CallToolResult::structured(payload.clone());
        r.content = vec![Content::text(payload.to_string())];
        r
    }

    fn payload_from_result(r: &CallToolResult) -> serde_json::Value {
        if let Some(v) = r.structured_content.clone() {
            return v;
        }
        let s = r
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default();
        serde_json::from_str(&s).unwrap_or_else(|_| serde_json::json!({}))
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebpipeUsageArgs {}

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebSeedUrlsArgs {
        /// Max seed urls to return (bounded).
        #[serde(default)]
        max: Option<usize>,
        /// If true, return an additional `ids` array for convenience.
        #[serde(default)]
        include_ids: Option<bool>,
        /// If true, return only ids+urls (omit kind/note fields) in the `seeds` list.
        ///
        /// This is useful when you want maximum output stability (ids/urls only).
        #[serde(default)]
        ids_only: Option<bool>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebSeedSearchExtractArgs {
        /// Query to use for chunk selection.
        query: String,
        /// Optional explicit URL list. If provided, we use these instead of built-in seeds.
        #[serde(default)]
        urls: Option<Vec<String>>,
        /// Optional seed ids from `web_seed_urls`. Ignored when `urls` is provided.
        #[serde(default)]
        seed_ids: Option<Vec<String>>,
        /// Max URLs to fetch/extract (bounded).
        #[serde(default)]
        max_urls: Option<usize>,
        /// Which fetch backend to use (default: local). Allowed: local, firecrawl
        #[serde(default)]
        fetch_backend: Option<String>,
        /// If true, do not perform any network calls. For local fetch_backend, this means:
        /// - allow cache reads
        /// - error on cache miss
        ///
        /// For firecrawl fetch_backend, this always errors (firecrawl is network-only).
        #[serde(default)]
        no_network: Option<bool>,
        /// Width for text wrapping (default: 100).
        #[serde(default)]
        width: Option<usize>,
        /// Max chars in output text (default: 20_000).
        #[serde(default)]
        max_chars: Option<usize>,
        /// Max bytes to fetch (default: 2_000_000).
        #[serde(default)]
        max_bytes: Option<u64>,
        /// How many chunks to return per URL (default: 5).
        #[serde(default)]
        top_chunks: Option<usize>,
        /// Cap merged chunks per seed id (default: top_chunks).
        #[serde(default)]
        merged_max_per_seed: Option<usize>,
        /// Max chars per chunk snippet (default: 500).
        #[serde(default)]
        max_chunk_chars: Option<usize>,
        /// Allow cache reads (default: true).
        #[serde(default)]
        cache_read: Option<bool>,
        /// Allow cache writes (default: true).
        #[serde(default)]
        cache_write: Option<bool>,
        /// Optional cache TTL in seconds.
        #[serde(default)]
        cache_ttl_s: Option<u64>,
        /// Include full extracted text (default: false; usually too big for seeds).
        #[serde(default)]
        include_text: Option<bool>,
        /// Compact output (default: true).
        #[serde(default)]
        compact: Option<bool>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebFetchArgs {
        /// URL to fetch (required).
        #[serde(default)]
        url: Option<String>,
        /// Which fetch backend to use (default: local). Allowed: local, firecrawl
        #[serde(default)]
        fetch_backend: Option<String>,
        /// If true, do not perform any network calls. For local fetch_backend, this means:
        /// - allow cache reads
        /// - error on cache miss
        ///
        /// For firecrawl fetch_backend, this always errors (firecrawl is network-only).
        #[serde(default)]
        no_network: Option<bool>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        max_bytes: Option<u64>,
        /// Max chars in output text (default: 20_000).
        #[serde(default)]
        max_text_chars: Option<usize>,
        #[serde(default)]
        headers: Option<BTreeMap<String, String>>,
        #[serde(default)]
        cache_read: Option<bool>,
        #[serde(default)]
        cache_write: Option<bool>,
        #[serde(default)]
        cache_ttl_s: Option<u64>,
        /// Include response body text in output (default: false).
        #[serde(default)]
        include_text: Option<bool>,
        /// Include response headers in output (default: false).
        #[serde(default)]
        include_headers: Option<bool>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebExtractArgs {
        /// URL to fetch+extract (required).
        #[serde(default)]
        url: Option<String>,
        /// Which fetch backend to use (default: local). Allowed: local, firecrawl
        #[serde(default)]
        fetch_backend: Option<String>,
        /// If true, do not perform any network calls. For local fetch_backend, this means:
        /// - allow cache reads
        /// - error on cache miss
        ///
        /// For firecrawl fetch_backend, this always errors (firecrawl is network-only).
        #[serde(default)]
        no_network: Option<bool>,
        /// Width for text wrapping (default: 100).
        #[serde(default)]
        width: Option<usize>,
        /// Max chars in output text (default: 20_000).
        #[serde(default)]
        max_chars: Option<usize>,
        /// Optional query: if set, return top matching chunks.
        #[serde(default)]
        query: Option<String>,
        /// Max chunks to return when `query` is set (default: 5).
        #[serde(default)]
        top_chunks: Option<usize>,
        /// Max chars per chunk when `query` is set (default: 500).
        #[serde(default)]
        max_chunk_chars: Option<usize>,
        /// Include the full extracted text (default: true when query is omitted; false when query is set).
        #[serde(default)]
        include_text: Option<bool>,
        /// Include extracted links (default: false).
        #[serde(default)]
        include_links: Option<bool>,
        /// Max links to return (default: 50).
        #[serde(default)]
        max_links: Option<usize>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        #[serde(default)]
        max_bytes: Option<u64>,
        #[serde(default)]
        cache_read: Option<bool>,
        #[serde(default)]
        cache_write: Option<bool>,
        #[serde(default)]
        cache_ttl_s: Option<u64>,
        /// Include structured extraction (outline + blocks) (default: false).
        #[serde(default)]
        include_structure: Option<bool>,
        /// Max outline items returned in structure (default: 25; max: 200).
        #[serde(default)]
        max_outline_items: Option<usize>,
        /// Max blocks returned in structure (default: 40; max: 200).
        #[serde(default)]
        max_blocks: Option<usize>,
        /// Max chars per block in structure (default: 400; max: 2000).
        #[serde(default)]
        max_block_chars: Option<usize>,
        /// If true, compute semantic chunk scores using embeddings (default: false; requires feature).
        #[serde(default)]
        semantic_rerank: Option<bool>,
        /// Max semantic chunks to return when semantic_rerank=true (default: 5; max: 50).
        #[serde(default)]
        semantic_top_k: Option<usize>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebSearchArgs {
        /// Search query (required).
        #[serde(default)]
        query: Option<String>,
        /// Which provider to use (default: brave). Allowed: auto, brave, tavily, searxng
        #[serde(default)]
        provider: Option<String>,
        /// When provider="auto", choose routing mode:
        /// - "fallback" (default): pick the best available provider (brave-first) and fall back on failure.
        /// - "merge": query all configured providers and merge/dedup results (bounded).
        /// - "mab": adaptive + deterministic choice based on in-process usage stats.
        #[serde(default)]
        auto_mode: Option<String>,
        #[serde(default)]
        max_results: Option<usize>,
        #[serde(default)]
        language: Option<String>,
        #[serde(default)]
        country: Option<String>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct ArxivSearchArgs {
        /// Search query (required).
        #[serde(default)]
        query: Option<String>,
        /// Optional ArXiv categories like "cs.LG", "stat.ML".
        #[serde(default)]
        categories: Option<Vec<String>>,
        /// Optional filter by publication years.
        #[serde(default)]
        years: Option<Vec<u32>>,
        /// Page number (1-based). Default: 1.
        #[serde(default)]
        page: Option<usize>,
        /// Results per page. Default: 10; max: 50.
        #[serde(default)]
        per_page: Option<usize>,
        /// Timeout per request (ms). Default: 20_000.
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// If true, rerank returned papers using semantic embeddings (feature-gated).
        #[serde(default)]
        semantic_rerank: Option<bool>,
        /// Max papers to keep after semantic rerank. Default: per_page; max: 50.
        #[serde(default)]
        semantic_top_k: Option<usize>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct ArxivEnrichArgs {
        /// ArXiv ID (e.g. "0805.3415") or an arxiv.org/abs URL.
        #[serde(default)]
        id_or_url: Option<String>,
        /// If true, find best-effort discussion/commentary via web search (bounded).
        #[serde(default)]
        include_discussions: Option<bool>,
        /// Max discussion pages to fetch/extract (default: 2; max: 5).
        #[serde(default)]
        max_discussion_urls: Option<usize>,
        /// Timeout per discussion fetch (ms). Default: 20_000.
        #[serde(default)]
        timeout_ms: Option<u64>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebCacheSearchExtractArgs {
        /// Search query (required).
        #[serde(default)]
        query: Option<String>,
        /// Max cached documents to inspect (default: 50; max: 500).
        #[serde(default)]
        max_docs: Option<usize>,
        /// Max chars extracted per cached doc (default: 20_000; max: 200_000).
        #[serde(default)]
        max_chars: Option<usize>,
        /// Max bytes to read per cached doc body (default: 5_000_000).
        #[serde(default)]
        max_bytes: Option<u64>,
        /// Width for HTML-to-text wrapping (default: 100).
        #[serde(default)]
        width: Option<usize>,
        /// Max chunks per doc (default: 5; max: 50).
        #[serde(default)]
        top_chunks: Option<usize>,
        /// Max chars per chunk (default: 500; max: 5_000).
        #[serde(default)]
        max_chunk_chars: Option<usize>,
        /// Include structured extraction (default: false).
        #[serde(default)]
        include_structure: Option<bool>,
        #[serde(default)]
        max_outline_items: Option<usize>,
        #[serde(default)]
        max_blocks: Option<usize>,
        #[serde(default)]
        max_block_chars: Option<usize>,
        /// If true, include per-doc extracted text (default: false).
        #[serde(default)]
        include_text: Option<bool>,
        /// Max cache entries to scan for recency filtering (default: 2000; max: 20000).
        #[serde(default)]
        max_scan_entries: Option<usize>,
        /// If true, compute semantic chunk scores using embeddings (default: false; feature-gated).
        #[serde(default)]
        semantic_rerank: Option<bool>,
        /// Max semantic chunks per doc when semantic_rerank=true (default: 5; max: 50).
        #[serde(default)]
        semantic_top_k: Option<usize>,
        /// If true, return a more compact per-doc shape (default: true).
        ///
        /// This reduces duplication between top-level fields and `extract.*` by keeping:
        /// - stable identifiers (url/final_url/status/content_type/bytes/score)
        /// - `extract` (engine + chunks + optional structure/text)
        /// - warnings + warning_codes + warning_hints
        /// - semantic (if present)
        #[serde(default)]
        compact: Option<bool>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    pub(crate) struct WebSearchExtractArgs {
        /// Search query. Required unless `urls` is provided.
        #[serde(default)]
        pub(crate) query: Option<String>,
        /// If provided, skip search and hydrate these URLs directly (offline-friendly).
        #[serde(default)]
        pub(crate) urls: Option<Vec<String>>,
        /// When urls=[...] is provided, how to choose which URLs to process under max_urls.
        ///
        /// - "auto" (default): if query is non-empty, use query_rank; else preserve
        /// - "preserve": preserve input order (after dedup)
        /// - "query_rank": stable rerank by query-token matches in the URL string
        #[serde(default)]
        pub(crate) url_selection_mode: Option<String>,
        /// Which search provider to use if `query` is used (default: auto). Allowed: auto, brave, tavily, searxng
        #[serde(default)]
        pub(crate) provider: Option<String>,
        /// When provider="auto", choose routing mode (default: "fallback"). Allowed: fallback, merge, mab
        #[serde(default)]
        pub(crate) auto_mode: Option<String>,
        /// How to select `top_chunks` across URLs (default: "score"). Allowed: score, pareto
        ///
        /// - "score": sort by chunk score (descending)
        /// - "pareto": non-dominated selection across score/cost/warnings (bounded)
        #[serde(default)]
        pub(crate) selection_mode: Option<String>,
        /// Which fetch backend to use (default: local). Allowed: local, firecrawl
        #[serde(default)]
        pub(crate) fetch_backend: Option<String>,
        /// If true, do not perform any network calls:
        /// - query mode is disabled (you must pass urls=[...])
        /// - per-URL fetch becomes cache-only; cache misses return ok=false per URL
        /// - firecrawl fallback is disabled
        #[serde(default)]
        pub(crate) no_network: Option<bool>,
        /// If true, and fetch_backend="local", retry a single URL with Firecrawl when local extraction yields empty text.
        ///
        /// This is a bounded, per-URL fallback: it only triggers when local extraction produced `empty_extraction`,
        /// and only if Firecrawl is configured.
        #[serde(default)]
        pub(crate) firecrawl_fallback_on_empty_extraction: Option<bool>,
        /// If true, and fetch_backend="local", retry a single URL with Firecrawl when local extraction
        /// yields non-empty but clearly low-signal output (e.g. JS bundle/app-shell gunk).
        ///
        /// This is bounded and per-URL (like the empty-extraction fallback), and only triggers when
        /// Firecrawl is configured.
        #[serde(default)]
        pub(crate) firecrawl_fallback_on_low_signal: Option<bool>,
        /// Max search results to request (default: 5; max: 20).
        #[serde(default)]
        pub(crate) max_results: Option<usize>,
        /// Max URLs to actually fetch/extract (default: 3; max: 10).
        #[serde(default)]
        pub(crate) max_urls: Option<usize>,
        /// Fetch timeout per URL (ms).
        #[serde(default)]
        pub(crate) timeout_ms: Option<u64>,
        /// Max bytes per URL.
        #[serde(default)]
        pub(crate) max_bytes: Option<u64>,
        /// If true, and fetch_backend="local", retry a single URL with a higher max_bytes
        /// when the body was truncated by max_bytes (default: false).
        ///
        /// This is an opt-in, per-URL retry to handle very large HTML pages (common on JS-heavy docs).
        /// The retry is bounded by `truncation_retry_max_bytes` (or a conservative default cap).
        #[serde(default)]
        pub(crate) retry_on_truncation: Option<bool>,
        /// When retry_on_truncation=true, the max_bytes to use for the retry.
        ///
        /// If omitted, webpipe uses a best-effort default (currently 2x max_bytes, capped).
        #[serde(default)]
        pub(crate) truncation_retry_max_bytes: Option<u64>,
        /// Width used for HTML->text extraction (default: 100).
        #[serde(default)]
        pub(crate) width: Option<usize>,
        /// Max chars of extracted text per URL (default: 20_000; max: 200_000).
        #[serde(default)]
        pub(crate) max_chars: Option<usize>,
        /// Max chunks per URL (default: 5; max: 50).
        #[serde(default)]
        pub(crate) top_chunks: Option<usize>,
        /// Max chars per chunk (default: 500; max: 5_000).
        #[serde(default)]
        pub(crate) max_chunk_chars: Option<usize>,
        /// Include extracted links (default: false).
        #[serde(default)]
        pub(crate) include_links: Option<bool>,
        /// Include structured extraction (outline + blocks) per URL (default: false).
        #[serde(default)]
        pub(crate) include_structure: Option<bool>,
        /// Max outline items returned in structure (default: 25; max: 200).
        #[serde(default)]
        pub(crate) max_outline_items: Option<usize>,
        /// Max blocks returned in structure (default: 40; max: 200).
        #[serde(default)]
        pub(crate) max_blocks: Option<usize>,
        /// Max chars per block in structure (default: 400; max: 2000).
        #[serde(default)]
        pub(crate) max_block_chars: Option<usize>,
        /// If true, compute semantic chunk scores per URL using embeddings (default: false; requires feature).
        #[serde(default)]
        pub(crate) semantic_rerank: Option<bool>,
        /// Max semantic chunks per URL when semantic_rerank=true (default: 5; max: 50).
        #[serde(default)]
        pub(crate) semantic_top_k: Option<usize>,
        /// Max links per URL (default: 25; max: 500).
        #[serde(default)]
        pub(crate) max_links: Option<usize>,
        /// Include full extracted text in per-URL results (default: false).
        #[serde(default)]
        pub(crate) include_text: Option<bool>,
        #[serde(default)]
        pub(crate) cache_read: Option<bool>,
        #[serde(default)]
        pub(crate) cache_write: Option<bool>,
        #[serde(default)]
        pub(crate) cache_ttl_s: Option<u64>,

        /// High-level width vs depth preset (optional).
        ///
        /// This does not override explicitly provided fields; it only fills in defaults.
        ///
        /// - "balanced" (default): current defaults
        /// - "wide": more breadth (more search + frontier) with shallower per-page extraction
        /// - "deep": fewer pages, deeper per-page extraction
        #[serde(default)]
        pub(crate) exploration: Option<String>,

        /// Enable an internal, bounded agentic loop:
        /// - fetch one URL
        /// - extract links (internally) and add to a frontier
        /// - re-rank which URL to fetch next (deterministic, bounded by max_urls)
        ///
        /// This helps “discover the right page” from adjacent pages rather than relying on URL-only heuristics.
        #[serde(default)]
        pub(crate) agentic: Option<bool>,

        /// Agentic selection strategy (when agentic=true):
        /// - "lexical" (default): deterministic URL-token ranking + link priors
        /// - "llm": ask OpenRouter (via `axi` structured output) to choose the next URL from a bounded candidate set
        ///   (falls back to lexical if not configured)
        #[serde(default)]
        pub(crate) agentic_selector: Option<String>,

        /// When agentic=true, allow up to this many search rounds total (default: 1).
        ///
        /// - 1 means “single search only” (current default behavior).
        /// - 2+ enables a bounded “search more” escape hatch.
        #[serde(default)]
        pub(crate) agentic_max_search_rounds: Option<usize>,

        /// When agentic=true, maximum frontier size (default: 200).
        #[serde(default)]
        pub(crate) agentic_frontier_max: Option<usize>,

        /// Max planner (LLM) calls for a single request (default: WEBPIPE_PLANNER_MAX_CALLS or 1).
        #[serde(default)]
        pub(crate) planner_max_calls: Option<usize>,

        /// If true, return a more compact output shape (default: true).
        ///
        /// This removes:
        /// - large agentic traces (replaced with a short summary)
        /// - some redundant per-URL fields not needed for downstream agent steps
        #[serde(default)]
        pub(crate) compact: Option<bool>,
    }

    #[derive(Debug, Deserialize, JsonSchema, Default)]
    struct WebDeepResearchArgs {
        /// User question / research prompt.
        query: String,

        /// If true, enforce a strict “auditability” contract (default: false):
        /// - reject Perplexity browsing (`search_mode` must be unset)
        /// - require include_evidence=true so the evidence is returned
        #[serde(default)]
        audit: Option<bool>,

        /// If false, skip synthesis and return only the evidence pack (default: true).
        ///
        /// Useful for: debugging evidence quality, offline-ish evaluation, and keeping runs inspectable.
        #[serde(default)]
        synthesize: Option<bool>,

        /// Optional: skip web search and use these URLs as the evidence set (offline-friendly).
        #[serde(default)]
        urls: Option<Vec<String>>,

        /// Which search provider to use for evidence gathering (default: auto).
        #[serde(default)]
        provider: Option<String>,
        /// When provider="auto", choose routing mode (default: "fallback"). Allowed: fallback, merge
        #[serde(default)]
        auto_mode: Option<String>,
        /// How to select `top_chunks` across URLs (default: "score"). Allowed: score, pareto
        #[serde(default)]
        selection_mode: Option<String>,

        /// High-level width vs depth preset (optional).
        ///
        /// This does not override explicitly provided fields; it only fills in defaults.
        ///
        /// - "balanced" (default): current defaults
        /// - "wide": more breadth (more search + more evidence URLs) with shallower per-page extraction
        /// - "deep": fewer evidence URLs, deeper per-page extraction
        #[serde(default)]
        exploration: Option<String>,
        /// Which fetch backend to use for evidence gathering (default: local). Allowed: local, firecrawl
        #[serde(default)]
        fetch_backend: Option<String>,
        /// If true, do not perform any network calls. This requires urls=[...] (offline evidence only),
        /// and disables Perplexity browsing via search_mode.
        #[serde(default)]
        no_network: Option<bool>,
        /// Max search results to request (default: 5; max: 20).
        #[serde(default)]
        max_results: Option<usize>,
        /// Max URLs to actually fetch/extract (default: 3; max: 10).
        #[serde(default)]
        max_urls: Option<usize>,

        /// Fetch timeout per URL (ms).
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// Max bytes per URL.
        #[serde(default)]
        max_bytes: Option<u64>,
        /// Width used for HTML->text extraction (default: 100).
        #[serde(default)]
        width: Option<usize>,
        /// Max chars of extracted text per URL (default: 20_000; max: 200_000).
        #[serde(default)]
        max_chars: Option<usize>,
        /// Max chunks per URL (default: 5; max: 50).
        #[serde(default)]
        top_chunks: Option<usize>,
        /// Max chars per chunk (default: 500; max: 5_000).
        #[serde(default)]
        max_chunk_chars: Option<usize>,
        /// Include extracted links (default: false).
        #[serde(default)]
        include_links: Option<bool>,
        /// Max links per URL (default: 25; max: 500).
        #[serde(default)]
        max_links: Option<usize>,

        /// Optionally include a bounded ArXiv search as extra evidence.
        ///
        /// This is separate from `web_search` providers (it uses ArXiv's Atom API).
        ///
        /// Allowed:
        /// - "off": never query ArXiv
        /// - "auto": query ArXiv only when the prompt looks paper-like
        /// - "on": always query ArXiv (unless no_network=true)
        #[serde(default)]
        arxiv_mode: Option<String>,
        /// Max ArXiv papers to include (default: 3; max: 10).
        #[serde(default)]
        arxiv_max_papers: Option<usize>,
        /// Optional ArXiv categories (e.g. ["cs.CL", "cs.LG"]).
        #[serde(default)]
        arxiv_categories: Option<Vec<String>>,
        /// Optional years filter for ArXiv (e.g. [2023, 2024]).
        #[serde(default)]
        arxiv_years: Option<Vec<u32>>,
        /// ArXiv query timeout (ms) (default: 8_000; max: 30_000).
        #[serde(default)]
        arxiv_timeout_ms: Option<u64>,

        /// Perplexity model to use (default: sonar-deep-research).
        #[serde(default)]
        model: Option<String>,

        /// Model name for non-Perplexity synthesis backends (e.g. OpenAI-compatible local gateways).
        ///
        /// - Used by `llm_backend="openai_compat"`.
        /// - Ignored by `llm_backend="ollama"` (which uses WEBPIPE_OLLAMA_MODEL).
        #[serde(default)]
        llm_model: Option<String>,
        /// Perplexity: optional search mode (provider-side browsing). If set, Perplexity may use extra sources
        /// beyond the evidence pack, reducing inspectability/determinism.
        #[serde(default)]
        search_mode: Option<String>,
        /// Perplexity-specific: reasoning effort for deep research models (low|medium|high).
        #[serde(default)]
        reasoning_effort: Option<String>,
        #[serde(default)]
        max_tokens: Option<u64>,
        #[serde(default)]
        temperature: Option<f64>,
        #[serde(default)]
        top_p: Option<f64>,

        /// Bound answer text returned in this tool output (default: 20_000; max: 200_000).
        #[serde(default)]
        max_answer_chars: Option<usize>,

        /// Also include the evidence pack in the output (default: true).
        #[serde(default)]
        include_evidence: Option<bool>,

        /// Override "now" for deterministic outputs.
        #[serde(default)]
        now_epoch_s: Option<u64>,

        /// Which LLM backend to use for synthesis.
        ///
        /// - "auto" (default): Perplexity if configured (and no_network=false), else OpenAI-compatible if configured, else Ollama if enabled.
        /// - "perplexity": require Perplexity API (network-only).
        /// - "ollama": use local Ollama (best-effort; defaults to localhost).
        /// - "openai_compat": call an OpenAI-compatible `/v1/chat/completions` endpoint (works well with `axi-gateway`).
        #[serde(default)]
        llm_backend: Option<String>,
    }

    #[derive(Debug, Clone)]
    struct ChunkCandidate {
        url: String,
        score: u64,
        start_char: usize,
        end_char: usize,
        text: String,
        warnings_count: usize,
        cache_hit: bool,
    }

    #[derive(Debug, Clone, Default, serde::Serialize)]
    struct ProviderUsage {
        calls: u64,
        ok: u64,
        cost_units: u64,
        elapsed_ms_sum: u64,
        http_429: u64,
    }

    #[derive(Debug, Clone, serde::Serialize)]
    struct UsageStats {
        started_at_epoch_s: u64,
        tool_calls: std::collections::BTreeMap<String, u64>,
        search_providers: std::collections::BTreeMap<String, ProviderUsage>,
        #[serde(skip)]
        search_windows: std::collections::BTreeMap<String, muxer::Window>,
        #[serde(skip)]
        search_windows_by_query_key:
            std::collections::BTreeMap<String, std::collections::BTreeMap<String, muxer::Window>>,
        #[serde(skip)]
        routing_last_seen_tick: u64,
        #[serde(skip)]
        routing_query_last_seen: std::collections::BTreeMap<String, u64>,
        search_window_cap: usize,
        routing_max_contexts: usize,
        #[serde(skip)]
        routing_context: RoutingContext,
        llm_backends: std::collections::BTreeMap<String, ProviderUsage>,
        fetch_backends: std::collections::BTreeMap<String, ProviderUsage>,
        warning_counts: std::collections::BTreeMap<String, u64>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RoutingContext {
        Global,
        QueryKey,
        Both,
    }

    impl UsageStats {
        fn new(now_epoch_s: u64) -> Self {
            let cap = std::env::var("WEBPIPE_ROUTING_WINDOW")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(50)
                .clamp(1, 500);
            let routing_max_contexts = std::env::var("WEBPIPE_ROUTING_MAX_CONTEXTS")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(200)
                .clamp(1, 2000);
            let routing_context = match std::env::var("WEBPIPE_ROUTING_CONTEXT")
                .ok()
                .unwrap_or_else(|| "global".to_string())
                .trim()
            {
                "query_key" | "querykey" | "qk" => RoutingContext::QueryKey,
                "both" => RoutingContext::Both,
                _ => RoutingContext::Global,
            };
            Self {
                started_at_epoch_s: now_epoch_s,
                tool_calls: std::collections::BTreeMap::new(),
                search_providers: std::collections::BTreeMap::new(),
                search_windows: std::collections::BTreeMap::new(),
                search_windows_by_query_key: std::collections::BTreeMap::new(),
                routing_last_seen_tick: 0,
                routing_query_last_seen: std::collections::BTreeMap::new(),
                search_window_cap: cap,
                routing_max_contexts,
                routing_context,
                llm_backends: std::collections::BTreeMap::new(),
                fetch_backends: std::collections::BTreeMap::new(),
                warning_counts: std::collections::BTreeMap::new(),
            }
        }
    }

    fn now_epoch_s() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_secs()
    }

    #[derive(Clone)]
    pub(crate) struct WebpipeMcp {
        tool_router: RmcpToolRouter<Self>,
        fetcher: Arc<LocalFetcher>,
        http: reqwest::Client,
        stats: Arc<std::sync::Mutex<UsageStats>>,
    }

    #[tool_router]
    impl WebpipeMcp {
        pub(crate) fn new() -> Result<Self, McpError> {
            let cache_dir =
                cache_dir_from_env().or_else(|| Some(default_cache_dir()));
            let fetcher = LocalFetcher::new(cache_dir)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            Ok(Self {
                tool_router: Self::tool_router(),
                fetcher: Arc::new(fetcher),
                http: reqwest::Client::builder()
                    .user_agent("webpipe-mcp/0.1")
                    .build()
                    .map_err(|e| McpError::internal_error(e.to_string(), None))?,
                stats: Arc::new(std::sync::Mutex::new(UsageStats::new(now_epoch_s()))),
            })
        }

        fn stats_lock(&self) -> std::sync::MutexGuard<'_, UsageStats> {
            self.stats.lock().unwrap_or_else(|e| e.into_inner())
        }

        fn stats_inc_tool(&self, kind: &str) {
            let mut s = self.stats_lock();
            *s.tool_calls.entry(kind.to_string()).or_insert(0) += 1;
        }

        fn stats_record_provider(
            map: &mut std::collections::BTreeMap<String, ProviderUsage>,
            name: &str,
            ok: bool,
            cost_units: u64,
            elapsed_ms: u64,
            http_429: bool,
        ) {
            let entry = map.entry(name.to_string()).or_default();
            entry.calls += 1;
            if ok {
                entry.ok += 1;
            }
            entry.cost_units = entry.cost_units.saturating_add(cost_units);
            entry.elapsed_ms_sum = entry.elapsed_ms_sum.saturating_add(elapsed_ms);
            if http_429 {
                entry.http_429 += 1;
            }
        }

        fn stats_record_search_provider_qk(
            &self,
            name: &str,
            ok: bool,
            cost_units: u64,
            elapsed_ms: u64,
            err: Option<&str>,
            query_key: Option<&str>,
        ) {
            let http_429 = err.is_some_and(|m| is_http_status(m, 429));
            let mut s = self.stats_lock();
            Self::stats_record_provider(
                &mut s.search_providers,
                name,
                ok,
                cost_units,
                elapsed_ms,
                http_429,
            );
            let cap = s.search_window_cap;
            let record_global = matches!(
                s.routing_context,
                RoutingContext::Global | RoutingContext::Both
            );
            let record_qk = matches!(
                s.routing_context,
                RoutingContext::QueryKey | RoutingContext::Both
            ) && query_key.is_some_and(|qk| !qk.trim().is_empty());

            if record_global {
                let w = s
                    .search_windows
                    .entry(name.to_string())
                    .or_insert_with(|| muxer::Window::new(cap));
                w.push(muxer::Outcome {
                    ok,
                    http_429,
                    junk: false,
                    hard_junk: false,
                    cost_units,
                    elapsed_ms,
                });
            }

            if record_qk {
                let qk = query_key.unwrap().trim().to_string();
                s.routing_last_seen_tick = s.routing_last_seen_tick.saturating_add(1);
                let tick = s.routing_last_seen_tick;
                s.routing_query_last_seen.insert(qk.clone(), tick);
                let per_qk = s.search_windows_by_query_key.entry(qk.clone()).or_default();
                let w = per_qk
                    .entry(name.to_string())
                    .or_insert_with(|| muxer::Window::new(cap));
                w.push(muxer::Outcome {
                    ok,
                    http_429,
                    junk: false,
                    hard_junk: false,
                    cost_units,
                    elapsed_ms,
                });

                while s.search_windows_by_query_key.len() > s.routing_max_contexts {
                    let Some((evict_qk, _tick)) = s
                        .routing_query_last_seen
                        .iter()
                        .min_by_key(|(_k, v)| *v)
                        .map(|(k, v)| (k.clone(), *v))
                    else {
                        break;
                    };
                    s.search_windows_by_query_key.remove(&evict_qk);
                    s.routing_query_last_seen.remove(&evict_qk);
                }
            }
        }

        fn stats_set_last_search_outcome_junk_level_qk(
            &self,
            name: &str,
            junk: bool,
            hard_junk: bool,
            query_key: Option<&str>,
        ) {
            let mut s = self.stats_lock();
            if matches!(
                s.routing_context,
                RoutingContext::Global | RoutingContext::Both
            ) {
                if let Some(w) = s.search_windows.get_mut(name) {
                    w.set_last_junk_level(junk, hard_junk);
                }
            }
            if matches!(
                s.routing_context,
                RoutingContext::QueryKey | RoutingContext::Both
            ) && query_key.is_some_and(|qk| !qk.trim().is_empty())
            {
                let qk = query_key.unwrap().trim();
                if let Some(per_qk) = s.search_windows_by_query_key.get_mut(qk) {
                    if let Some(w) = per_qk.get_mut(name) {
                        w.set_last_junk_level(junk, hard_junk);
                    }
                }
            }
        }

        fn snapshot_search_summaries_for_query_key(
            &self,
            query_key: Option<&str>,
        ) -> (
            std::collections::BTreeMap<String, muxer::Summary>,
            &'static str,
        ) {
            let s = self.stats_lock();
            let global = || {
                let mut m = std::collections::BTreeMap::new();
                for (k, w) in &s.search_windows {
                    m.insert(k.clone(), w.summary());
                }
                (m, "global")
            };
            match s.routing_context {
                RoutingContext::Global => global(),
                RoutingContext::QueryKey | RoutingContext::Both => {
                    if let Some(qk) = query_key.map(|x| x.trim()).filter(|x| !x.is_empty()) {
                        if let Some(per_qk) = s.search_windows_by_query_key.get(qk) {
                            let mut m = std::collections::BTreeMap::new();
                            for (k, w) in per_qk {
                                m.insert(k.clone(), w.summary());
                            }
                            return (m, "query_key");
                        }
                    }
                    global()
                }
            }
        }

        fn compute_search_junk_label(
            hard_junk_urls: usize,
            soft_junk_urls: usize,
            total_urls_ok: usize,
        ) -> bool {
            if total_urls_ok == 0 {
                return false;
            }
            if hard_junk_urls > 0 {
                return true;
            }
            let min_soft = std::env::var("WEBPIPE_ROUTING_SOFT_JUNK_MIN")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(2)
                .clamp(1, 50);
            let frac = std::env::var("WEBPIPE_ROUTING_SOFT_JUNK_FRAC")
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
                .unwrap_or(0.5)
                .clamp(0.0, 1.0);
            if soft_junk_urls < min_soft {
                return false;
            }
            (soft_junk_urls as f64) >= frac * (total_urls_ok as f64)
        }

        fn stats_record_llm_backend(
            &self,
            name: &str,
            ok: bool,
            elapsed_ms: u64,
            err: Option<&str>,
        ) {
            let http_429 = err.is_some_and(|m| is_http_status(m, 429));
            let mut s = self.stats_lock();
            Self::stats_record_provider(&mut s.llm_backends, name, ok, 0, elapsed_ms, http_429);
        }

        fn stats_record_fetch_backend(
            &self,
            name: &str,
            ok: bool,
            elapsed_ms: u64,
            err: Option<&str>,
        ) {
            let http_429 = err.is_some_and(|m| is_http_status(m, 429));
            let mut s = self.stats_lock();
            Self::stats_record_provider(&mut s.fetch_backends, name, ok, 0, elapsed_ms, http_429);
        }

        fn stats_record_warnings(&self, warnings: &[&'static str]) {
            let mut s = self.stats_lock();
            for &w in warnings {
                *s.warning_counts
                    .entry(normalize_warning_code(w).to_string())
                    .or_insert(0) += 1;
            }
        }

        fn truncate_to_chars(s: &str, max_chars: usize) -> (String, usize, bool) {
            if max_chars == 0 {
                return (String::new(), 0, !s.is_empty());
            }
            // Find a valid UTF-8 boundary for the first `max_chars` chars, in one pass.
            let mut n = 0usize;
            let mut cut = s.len();
            for (i, _) in s.char_indices() {
                if n == max_chars {
                    cut = i;
                    break;
                }
                n += 1;
            }
            let clipped = cut < s.len();
            // If we clipped, we know we produced exactly `max_chars` chars.
            let n_out = if clipped { max_chars } else { n };
            (s[..cut].to_string(), n_out, clipped)
        }

        fn approx_bytes_len(s: &str) -> usize {
            s.len()
        }

        fn content_type_is_pdf(ct: Option<&str>) -> bool {
            ct.unwrap_or("")
                .trim()
                .to_ascii_lowercase()
                .starts_with("application/pdf")
        }

        fn url_looks_like_pdf(url: &str) -> bool {
            let u = url.trim();
            if u.is_empty() {
                return false;
            }
            if let Ok(p) = reqwest::Url::parse(u) {
                let path_lc = p.path().to_ascii_lowercase();
                // Common forms:
                // - .../file.pdf
                // - .../pdf/... (many hosts, including arXiv)
                path_lc.ends_with(".pdf") || path_lc.contains("/pdf/")
            } else {
                let u_lc = u.to_ascii_lowercase();
                u_lc.ends_with(".pdf") || u_lc.contains("/pdf/")
            }
        }

        fn looks_like_bundle_gunk(s: &str) -> bool {
            // Common “JS app shell” signatures we saw in the wild (e.g. Next.js hydration payloads).
            let t = s.trim();
            if t.is_empty() {
                return false;
            }
            let lc = t.to_ascii_lowercase();
            lc.contains("self.__next_s")
                || lc.contains("suppresshydrationwarning")
                || lc.contains("__webpack")
                || lc.contains("webpackchunk")
                || lc.contains("window.__nuxt")
                || lc.contains("(function(a,b,c,d)")
                || lc.contains(".push([0,{\"suppresshydrationwarning\"")
        }

        fn chunk_is_low_signal(c: &webpipe_local::extract::ScoredChunk) -> bool {
            let t = c.text.trim();
            if t.is_empty() {
                return true;
            }
            if Self::looks_like_bundle_gunk(t) {
                return true;
            }
            // Lightweight “readability” heuristic:
            // penalize chunks that are mostly punctuation/escaped JSON.
            let mut letters = 0usize;
            let mut digits = 0usize;
            let mut spaces = 0usize;
            let mut other = 0usize;
            for ch in t.chars() {
                if ch.is_alphabetic() {
                    letters += 1;
                } else if ch.is_ascii_digit() {
                    digits += 1;
                } else if ch.is_whitespace() {
                    spaces += 1;
                } else {
                    other += 1;
                }
            }
            let denom = letters + digits + spaces + other;
            if denom < 120 {
                return false;
            }
            let alpha_like = letters + spaces;
            // If < ~1/3 of chars are alphabetic/space, this is usually “gunk”.
            alpha_like.saturating_mul(3) < denom
        }

        fn filter_low_signal_chunks(
            chunks: Vec<webpipe_local::extract::ScoredChunk>,
        ) -> (Vec<webpipe_local::extract::ScoredChunk>, bool) {
            if chunks.is_empty() {
                return (chunks, false);
            }
            let mut out: Vec<webpipe_local::extract::ScoredChunk> = chunks
                .iter()
                .filter(|c| !Self::chunk_is_low_signal(c))
                .cloned()
                .collect();
            // Never return an empty list if we had chunks; fall back to original.
            if out.is_empty() {
                out = chunks;
                return (out, false);
            }
            let filtered = out.len() < chunks.len();
            (out, filtered)
        }

        fn score_key(score: u64) -> i64 {
            // Keep ordering stable without float total-order deps.
            // (Scores come from our own chunker; treat as an integer signal.)
            score.min(i64::MAX as u64) as i64
        }

        fn select_top_chunks(
            mut candidates: Vec<ChunkCandidate>,
            top_k: usize,
            selection_mode: &str,
        ) -> Vec<ChunkCandidate> {
            if candidates.is_empty() || top_k == 0 {
                return Vec::new();
            }

            match selection_mode {
                "pareto" => {
                    // Multi-objective selection:
                    // - maximize score
                    // - prefer cache hits (cheaper / more reproducible)
                    // - prefer fewer warnings (higher reliability)
                    // - prefer shorter chunks (bounded evidence packs)
                    let metrics: Vec<Vec<f32>> = candidates
                        .iter()
                        .map(|c| {
                            let score = c.score as f32;
                            let cache = if c.cache_hit { 1.0 } else { 0.0 };
                            let warnings = -(c.warnings_count as f32);
                            let len = -(c.text.chars().count() as f32);
                            vec![score, cache, warnings, len]
                        })
                        .collect();

                    let frontier = pare::pareto_indices(&metrics);
                    let mut picked: Vec<ChunkCandidate> = Vec::new();

                    if let Some(frontier) = frontier {
                        let mut idxs = frontier;
                        // Stable, deterministic tie-break: high score, then fewer warnings, then cache, then URL.
                        idxs.sort_by(|&ia, &ib| {
                            let a = &candidates[ia];
                            let b = &candidates[ib];
                            let ka = Self::score_key(a.score);
                            let kb = Self::score_key(b.score);
                            kb.cmp(&ka)
                                .then_with(|| a.warnings_count.cmp(&b.warnings_count))
                                .then_with(|| (b.cache_hit as u8).cmp(&(a.cache_hit as u8)))
                                .then_with(|| a.url.cmp(&b.url))
                        });

                        for i in idxs {
                            picked.push(candidates[i].clone());
                            if picked.len() >= top_k {
                                break;
                            }
                        }
                    }

                    if picked.len() < top_k {
                        // Fill remaining slots by score (descending), skipping already-picked items.
                        let mut seen = std::collections::BTreeSet::<(String, usize, usize)>::new();
                        for c in &picked {
                            seen.insert((c.url.clone(), c.start_char, c.end_char));
                        }

                        candidates.sort_by(|a, b| {
                            let ka = Self::score_key(a.score);
                            let kb = Self::score_key(b.score);
                            kb.cmp(&ka)
                                .then_with(|| a.warnings_count.cmp(&b.warnings_count))
                                .then_with(|| (b.cache_hit as u8).cmp(&(a.cache_hit as u8)))
                                .then_with(|| a.url.cmp(&b.url))
                        });

                        for c in candidates {
                            if picked.len() >= top_k {
                                break;
                            }
                            if seen.insert((c.url.clone(), c.start_char, c.end_char)) {
                                picked.push(c);
                            }
                        }
                    }

                    picked.truncate(top_k);
                    picked
                }
                // Default: score-only ranking (back-compat behavior).
                _ => {
                    candidates.sort_by(|a, b| {
                        let ka = Self::score_key(a.score);
                        let kb = Self::score_key(b.score);
                        kb.cmp(&ka)
                            .then_with(|| a.url.cmp(&b.url))
                            .then_with(|| a.start_char.cmp(&b.start_char))
                    });
                    candidates.truncate(top_k);
                    candidates
                }
            }
        }

        fn query_key(query: &str) -> Option<String> {
            let q = query.trim();
            if q.is_empty() {
                None
            } else {
                Some(textprep::scrub(q))
            }
        }

        #[allow(dead_code)]
        pub(crate) fn openrouter_api_key_from_env() -> Option<String> {
            std::env::var("WEBPIPE_OPENROUTER_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("OPENROUTER_API_KEY")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
        }

        pub(crate) fn openrouter_model_from_env() -> String {
            std::env::var("WEBPIPE_OPENROUTER_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("AXI_OPENROUTER_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .or_else(|| {
                    std::env::var("OPENROUTER_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                // Default planner model: keep it fast/cheap and recent.
                // Override with WEBPIPE_OPENROUTER_MODEL / AXI_OPENROUTER_MODEL / OPENROUTER_MODEL.
                .unwrap_or_else(|| "openai/gpt-4o-mini".to_string())
        }

        #[allow(dead_code)]
        pub(crate) fn openai_api_key_from_env() -> Option<String> {
            std::env::var("WEBPIPE_OPENAI_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("OPENAI_API_KEY")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
        }

        pub(crate) fn openai_model_from_env() -> String {
            std::env::var("WEBPIPE_OPENAI_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("AXI_OPENAI_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .or_else(|| {
                    std::env::var("OPENAI_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .unwrap_or_else(|| "gpt-4o-mini".to_string())
        }

        #[allow(dead_code)]
        pub(crate) fn groq_api_key_from_env() -> Option<String> {
            std::env::var("WEBPIPE_GROQ_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("GROQ_API_KEY")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
        }

        pub(crate) fn groq_model_from_env() -> String {
            std::env::var("WEBPIPE_GROQ_MODEL")
                .ok()
                .filter(|v| !v.trim().is_empty())
                .or_else(|| {
                    std::env::var("AXI_GROQ_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .or_else(|| {
                    std::env::var("GROQ_MODEL")
                        .ok()
                        .filter(|v| !v.trim().is_empty())
                })
                .unwrap_or_else(|| "llama-3.1-8b-instant".to_string())
        }

        pub(crate) fn planner_max_calls_from_env() -> usize {
            std::env::var("WEBPIPE_PLANNER_MAX_CALLS")
                .ok()
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(1)
                .min(10)
        }

        fn select_urls_for_hydration(
            urls: Vec<String>,
            max_urls: usize,
            query: &str,
            url_selection_mode: &str,
        ) -> Vec<String> {
            if urls.is_empty() {
                return Vec::new();
            }
            let max_urls = max_urls.clamp(1, 10);

            match url_selection_mode {
                "auto" => {
                    if Self::query_key(query).is_some() {
                        Self::select_urls_for_hydration(urls, max_urls, query, "query_rank")
                    } else {
                        Self::select_urls_for_hydration(urls, max_urls, query, "preserve")
                    }
                }
                // Note: auto_plus is implemented in the main web_search_extract flow because it needs
                // bounded fetch+hint extraction.
                "query_rank" => {
                    let qkey = Self::query_key(query).unwrap_or_default();
                    let q_toks: Vec<&str> = qkey
                        .split(|ch: char| !ch.is_alphanumeric())
                        .filter(|t| t.len() >= 2)
                        .collect();
                    if q_toks.is_empty() {
                        return urls.into_iter().take(max_urls).collect();
                    }

                    let mut scored: Vec<(usize, u64, String)> = urls
                        .into_iter()
                        .enumerate()
                        .map(|(i, u)| {
                            let ukey = textprep::scrub(&u);
                            let mut s = 0u64;
                            for t in &q_toks {
                                if ukey.contains(t) {
                                    s += 1;
                                }
                            }
                            (i, s, u)
                        })
                        .collect();

                    // Stable rerank: higher score first; tie-break by original order.
                    scored.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                    scored
                        .into_iter()
                        .take(max_urls)
                        .map(|(_i, _s, u)| u)
                        .collect()
                }
                _ => urls.into_iter().take(max_urls).collect(),
            }
        }

        #[tool(description = "Report webpipe configuration + version (no secrets)")]
        async fn webpipe_meta(&self) -> Result<CallToolResult, McpError> {
            let t0 = std::time::Instant::now();
            self.stats_inc_tool("webpipe_meta");

            // Only report booleans / key names, never values.
            let tavily_configured = has_env("WEBPIPE_TAVILY_API_KEY") || has_env("TAVILY_API_KEY");
            let firecrawl_configured =
                has_env("WEBPIPE_FIRECRAWL_API_KEY") || has_env("FIRECRAWL_API_KEY");
            let brave_configured =
                has_env("WEBPIPE_BRAVE_API_KEY") || has_env("BRAVE_SEARCH_API_KEY");
            let searxng_configured =
                has_env("WEBPIPE_SEARXNG_ENDPOINT") || has_env("WEBPIPE_SEARXNG_ENDPOINTS");
            let perplexity_configured =
                has_env("WEBPIPE_PERPLEXITY_API_KEY") || has_env("PERPLEXITY_API_KEY");
            let openrouter_configured =
                has_env("WEBPIPE_OPENROUTER_API_KEY") || has_env("OPENROUTER_API_KEY");
            let openai_configured = has_env("WEBPIPE_OPENAI_API_KEY") || has_env("OPENAI_API_KEY");
            let groq_configured = has_env("WEBPIPE_GROQ_API_KEY") || has_env("GROQ_API_KEY");
            let gemini_configured = has_env("WEBPIPE_GEMINI_API_KEY")
                || has_env("GEMINI_API_KEY")
                || has_env("WEBPIPE_GOOGLE_API_KEY")
                || has_env("GOOGLE_API_KEY");

            let local_tools = serde_json::json!({
                "yt_dlp": webpipe_local::shellout::has("yt-dlp"),
                "pandoc": webpipe_local::shellout::has("pandoc"),
                "ffmpeg": webpipe_local::shellout::has("ffmpeg"),
                "tesseract": webpipe_local::shellout::has("tesseract"),
                "pdftotext": webpipe_local::shellout::has("pdftotext"),
                "mutool": webpipe_local::shellout::has("mutool"),
            });

            let mut providers = Vec::new();
            if brave_configured {
                providers.push("brave");
            }
            if tavily_configured {
                providers.push("tavily");
            }
            if searxng_configured {
                providers.push("searxng");
            }

            let mut remote_fetch = Vec::new();
            if firecrawl_configured {
                remote_fetch.push("firecrawl");
            }

            // Opinionated cost-aware default:
            // - Default `web_search.provider` stays "brave" (cheaper baseline; stable).
            // - "auto" prefers Brave when configured; uses other configured providers as fallbacks.
            let recommended_provider_order: Vec<&'static str> = if brave_configured {
                let mut v = vec!["brave"];
                // Prefer self-hosted “free-ish” SearXNG before paid providers when available.
                if searxng_configured {
                    v.push("searxng");
                }
                if tavily_configured {
                    v.push("tavily");
                }
                v
            } else if searxng_configured {
                let mut v = vec!["searxng"];
                if tavily_configured {
                    v.push("tavily");
                }
                v
            } else if tavily_configured {
                vec!["tavily"]
            } else {
                vec![]
            };

            // Help debug “which binary is Cursor actually running?” without leaking secrets.
            // We only include a short tail of the path.
            let exe = std::env::current_exe().ok().map(|p| {
                let comps: Vec<String> = p
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect();
                let n = 3usize;
                if comps.len() <= n {
                    comps.join("/")
                } else {
                    format!(".../{}", comps[comps.len() - n..].join("/"))
                }
            });

            let mut payload = serde_json::json!({
                "ok": true,
                "name": "webpipe",
                "version": env!("CARGO_PKG_VERSION"),
                "cache_dir": cache_dir_from_env().map(|p| p.to_string_lossy().to_string()),
                "binary": {
                    "exe": exe,
                },
                "capabilities": {
                    "pdf_extract": true,
                    "semantic_rerank": false,
                    "embeddings_openai": false,
                    "embeddings_tei": false,
                    "vision_gemini": cfg!(feature = "vision-gemini")
                },
                "supported": {
                    // Tool surface is intentionally small and ordered by “what users reach for first”.
                    // (UX note: some clients render tools in this order; others don’t, but this is a
                    // stable, human-friendly reference regardless.)
                    "mcp_tools": [
                        "webpipe_meta",
                        "webpipe_usage",
                        "webpipe_usage_reset",
                        "web_seed_urls",
                        "web_seed_search_extract",
                        "web_fetch",
                        "web_extract",
                        "web_search",
                        "web_search_extract",
                        "web_cache_search_extract",
                        "web_deep_research",
                        "arxiv_search",
                        "arxiv_enrich"
                    ],
                    "mcp_tool_groups": {
                        "meta": ["webpipe_meta", "webpipe_usage", "webpipe_usage_reset"],
                        "seeds": ["web_seed_urls", "web_seed_search_extract"],
                        "fetch_extract": ["web_fetch", "web_extract"],
                        "search": ["web_search", "web_search_extract", "web_cache_search_extract"],
                        "research": ["web_deep_research", "arxiv_search", "arxiv_enrich"]
                    },
                    // Values for web_search.provider
                    "providers": ["auto", "brave", "tavily", "searxng"],
                    // Values for web_search.auto_mode (when provider="auto")
                    "auto_modes": ["fallback", "merge", "mab"],
                    // Values for extraction engines
                    "extraction_engines": ["html2text", "html_main", "html_hint", "text", "json", "xml", "markdown", "pdf-extract", "pdf-pdftotext", "pdf-mutool", "youtube_transcript", "pandoc", "image", "image_ocr", "media", "media_subtitles", "gemini_vision"],
                    // Environment knobs (names only; no values) for opportunistic local tooling + multimodal.
                    "knobs": [
                        "WEBPIPE_SEARXNG_ENDPOINT",
                        "WEBPIPE_SEARXNG_ENDPOINTS",
                        "WEBPIPE_ARXIV_ENDPOINT",
                        "WEBPIPE_PERPLEXITY_ENDPOINT",
                        "WEBPIPE_PDF_SHELLOUT",
                        "WEBPIPE_PDF_SHELLOUT_MAX_PAGES",
                        "WEBPIPE_YOUTUBE_TRANSCRIPTS",
                        "WEBPIPE_YOUTUBE_LANGS",
                        "WEBPIPE_YOUTUBE_MAX_CHARS",
                        "WEBPIPE_PANDOC",
                        "WEBPIPE_PANDOC_TIMEOUT_MS",
                        "WEBPIPE_PANDOC_MAX_CHARS",
                        "WEBPIPE_OCR",
                        "WEBPIPE_OCR_TIMEOUT_MS",
                        "WEBPIPE_OCR_MAX_CHARS",
                        "WEBPIPE_MEDIA_SUBTITLES",
                        "WEBPIPE_MEDIA_SUBS_TIMEOUT_MS",
                        "WEBPIPE_MEDIA_SUBS_MAX_CHARS",
                        "WEBPIPE_VISION",
                        "WEBPIPE_GEMINI_API_KEY",
                        "GEMINI_API_KEY",
                        "WEBPIPE_GOOGLE_API_KEY",
                        "GOOGLE_API_KEY",
                        "WEBPIPE_GEMINI_MODEL",
                        "WEBPIPE_GEMINI_TIMEOUT_MS",
                        "WEBPIPE_GEMINI_MAX_CHARS",
                        "WEBPIPE_GEMINI_PROMPT",
                        "WEBPIPE_GEMINI_BASE_URL"
                    ],
                    // Values for web_search_extract.selection_mode / web_deep_research.selection_mode
                    "selection_modes": ["score", "pareto"],
                    // Values for web_search_extract.url_selection_mode
                    "url_selection_modes": ["auto", "auto_plus", "preserve", "query_rank"],
                    // Values for web_search_extract.agentic_selector
                    "agentic_selectors": ["auto", "lexical", "llm"],
                    // Values for web_search_extract.exploration
                    "explorations": ["balanced", "wide", "deep"],
                    // Values for web_* fetch_backend
                    "fetch_backends": ["local", "firecrawl"],
                    // Note: firecrawl is a remote fetch backend (used by CLI eval harness and optionally by MCP tools when requested).
                    "remote_fetch": ["firecrawl"],
                    "llm_models": ["sonar-deep-research"],
                    "llm_planner_providers": ["openrouter", "openai", "groq"]
                },
                "defaults": {
                    "web_search": {
                        "provider": "brave",
                        "provider_auto_order": recommended_provider_order,
                        "auto_mode": "fallback",
                        "max_results": 10,
                        "max_results_max": 20
                    },
                    "web_fetch": {
                        "fetch_backend": "local",
                            "no_network": false,
                        "timeout_ms": 15_000,
                        "max_bytes": 5_000_000,
                        "max_text_chars": 20_000,
                        "max_text_chars_max": 200_000,
                        "include_text": false,
                        "include_headers": false
                    },
                    "web_extract": {
                        "fetch_backend": "local",
                            "no_network": false,
                        "timeout_ms": 20_000,
                        "max_bytes": 5_000_000,
                        "width": 100,
                        "max_chars": 20_000,
                        "max_chars_max": 200_000,
                        "top_chunks": 5,
                        "top_chunks_max": 50,
                        "max_chunk_chars": 500,
                        "max_chunk_chars_max": 5_000,
                        "include_text": true,
                        "include_links": false,
                        "max_links": 50,
                        "max_links_max": 500,
                        "include_structure": false,
                        "max_outline_items": 25,
                        "max_blocks": 40,
                        "max_block_chars": 400,
                        "semantic_rerank": false,
                        "semantic_top_k": 5
                    },
                    "web_cache_search_extract": {
                        "max_docs": 50,
                        "max_docs_max": 500,
                        "max_scan_entries": 2000,
                        "max_scan_entries_max": 20000,
                        "width": 100,
                        "max_chars": 20_000,
                        "max_chars_max": 200_000,
                        "top_chunks": 5,
                        "top_chunks_max": 50,
                        "max_chunk_chars": 500,
                        "max_chunk_chars_max": 5_000,
                        "include_text": false,
                        "include_structure": false,
                        "max_outline_items": 25,
                        "max_blocks": 40,
                        "max_block_chars": 400,
                        "semantic_rerank": false,
                        "semantic_top_k": 5,
                        "compact": true
                    },
                    "web_search_extract": {
                        "provider": "auto",
                        "auto_mode": "fallback",
                        "selection_mode": "score",
                        "fetch_backend": "local",
                            "no_network": false,
                        "firecrawl_fallback_on_empty_extraction": false,
                        "firecrawl_fallback_on_low_signal": false,
                        "exploration": "balanced",
                        "max_results": 5,
                        "max_urls": 3,
                        "url_selection_mode": "auto",
                        "agentic": true,
                        "agentic_selector": "auto",
                        "agentic_max_search_rounds": 1,
                        "agentic_frontier_max": 200,
                        "planner_max_calls": 1,
                        "timeout_ms": 20_000,
                        "max_bytes": 5_000_000,
                        "width": 100,
                        "max_chars": 20_000,
                        "max_chars_max": 200_000,
                        "top_chunks": 5,
                        "top_chunks_max": 50,
                        "max_chunk_chars": 500,
                        "max_chunk_chars_max": 5_000,
                        "include_links": false,
                        "max_links": 25,
                        "max_links_max": 500,
                        "include_text": false,
                        "include_structure": false,
                        "max_outline_items": 25,
                        "max_blocks": 40,
                        "max_block_chars": 400,
                        "semantic_rerank": false,
                        "semantic_top_k": 5,
                        "compact": true
                    },
                    "web_deep_research": {
                        "provider": "auto",
                        "auto_mode": "fallback",
                        "selection_mode": "score",
                        "fetch_backend": "local",
                        "exploration": "balanced",
                        "max_results": 5,
                        "max_urls": 3,
                        "timeout_ms": 20_000,
                        "max_bytes": 5_000_000,
                        "width": 100,
                        "max_chars": 20_000,
                        "top_chunks": 5,
                        "max_chunk_chars": 500,
                        "include_links": false,
                        "max_links": 25,
                        "model": "sonar-deep-research",
                        "search_mode": null,
                        "reasoning_effort": "medium",
                        "max_answer_chars": 20_000
                    }
                },
                "configured": {
                    "providers": {
                        "brave": brave_configured,
                        "tavily": tavily_configured,
                        "searxng": searxng_configured
                    },
                    "fetch": {
                        "local": true
                    },
                    "remote_fetch": {
                        "firecrawl": firecrawl_configured
                    },
                    "llm": {
                        "perplexity": perplexity_configured,
                        "openrouter": openrouter_configured,
                        "openrouter_model": if openrouter_configured {
                            Some(Self::openrouter_model_from_env())
                        } else {
                            None
                        },
                        "openai": openai_configured,
                        "openai_model": if openai_configured {
                            Some(Self::openai_model_from_env())
                        } else {
                            None
                        },
                        "groq": groq_configured,
                        "groq_model": if groq_configured {
                            Some(Self::groq_model_from_env())
                        } else {
                            None
                        }
                    },
                    "vision": {
                        "gemini": gemini_configured
                    }
                },
                "available": {
                    "providers": providers,
                    "remote_fetch": remote_fetch
                },
                "local_tools": local_tools
            });
            add_envelope_fields(&mut payload, "webpipe_meta", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(
            description = "Return curated seed URLs (awesome lists) for keyless search workflows"
        )]
        async fn web_seed_urls(
            &self,
            params: Parameters<Option<WebSeedUrlsArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let t0 = std::time::Instant::now();
            self.stats_inc_tool("web_seed_urls");
            let args = params.0.unwrap_or_default();
            let max = args.max.unwrap_or(25).clamp(1, 50);
            let include_ids = args.include_ids.unwrap_or(true);
            let ids_only = args.ids_only.unwrap_or(false);

            // Curated seed registry (single source of truth).
            let seeds = seed_registry();

            let seeds_out = if ids_only {
                seeds.iter().take(max).map(|s| serde_json::json!({
                    "id": s.id,
                    "url": s.url
                })).collect::<Vec<_>>()
            } else {
                seeds.iter().take(max).map(|s| serde_json::json!({
                    "id": s.id,
                    "url": s.url,
                    "kind": s.kind,
                    "note": s.note
                })).collect::<Vec<_>>()
            };

            let mut payload = serde_json::json!({
                "ok": true,
                "max": max,
                "seeds": seeds_out,
                "notes": [
                    "Use these with web_extract or web_search_extract(urls=[...], url_selection_mode=query_rank) to avoid paid search providers.",
                    "Prefer small bounds (max_urls/top_chunks/max_chars) and cache_read=true to stay deterministic."
                ]
            });
            if include_ids {
                payload["ids"] = serde_json::json!(
                    seeds.iter().take(max).map(|s| s.id).collect::<Vec<_>>()
                );
            }
            add_envelope_fields(&mut payload, "web_seed_urls", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(
            description = "Fetch+extract a bounded set of seed URLs and return merged top chunks for a query"
        )]
        async fn web_seed_search_extract(
            &self,
            params: Parameters<Option<WebSeedSearchExtractArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let t0 = std::time::Instant::now();
            self.stats_inc_tool("web_seed_search_extract");

            let args = params.0.unwrap_or_default();
            let query = args.query.trim().to_string();
            if query.is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": "",
                    "error": error_obj(ErrorCode::InvalidParams, "query must be non-empty", "Provide a query string.")
                });
                add_envelope_fields(&mut payload, "web_seed_search_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let max_urls = args.max_urls.unwrap_or(4).clamp(1, 10);
            let width = args.width.unwrap_or(100).clamp(20, 240);
            let max_chars = args.max_chars.unwrap_or(20_000).min(200_000);
            let max_bytes = args.max_bytes.unwrap_or(2_000_000).min(10_000_000);
            let top_chunks = args.top_chunks.unwrap_or(5).clamp(1, 25);
            let merged_max_per_seed = args.merged_max_per_seed.unwrap_or(top_chunks).clamp(1, 25);
            let max_chunk_chars = args.max_chunk_chars.unwrap_or(500).clamp(50, 5_000);
            let include_text = args.include_text.unwrap_or(false);
            let compact = args.compact.unwrap_or(true);

            let fetch_backend = args.fetch_backend.unwrap_or_else(|| "local".to_string());
            let no_network = args.no_network.unwrap_or(false);
            let cache_read = args.cache_read.unwrap_or(true);
            let cache_write = args.cache_write.unwrap_or(true);
            let cache_ttl_s = args.cache_ttl_s;

            let mut warnings: Vec<&'static str> = Vec::new();
            let seed_ids_were_provided = args.urls.is_none() && args.seed_ids.as_ref().is_some();
            let (urls, unknown_seed_ids) = select_seed_urls(
                seed_registry(),
                args.urls.clone(),
                args.seed_ids.clone(),
                max_urls,
            );
            if !unknown_seed_ids.is_empty() {
                warnings.push("unknown_seed_id");
            }
            if seed_ids_were_provided && urls.is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "seed_ids did not match any known seeds",
                        "Call web_seed_urls to see valid ids, or pass urls=[...] explicitly."
                    ),
                    "unknown_seed_ids": unknown_seed_ids,
                });
                add_envelope_fields(&mut payload, "web_seed_search_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            #[derive(Debug, Clone, serde::Serialize)]
            struct PerUrlResult {
                id: String,
                url: String,
                ok: bool,
                extraction_engine: Option<String>,
                chunks: Vec<serde_json::Value>,
                warnings: Vec<String>,
                error: Option<serde_json::Value>,
            }

            let mut per_url: Vec<PerUrlResult> = Vec::new();
            let mut merged: Vec<(u64, String, serde_json::Value)> = Vec::new();

            for (id, url) in urls.iter() {
                let r = self
                    .web_extract(p(WebExtractArgs {
                        url: Some(url.clone()),
                        fetch_backend: Some(fetch_backend.clone()),
                        no_network: Some(no_network),
                        width: Some(width),
                        max_chars: Some(max_chars),
                        query: Some(query.clone()),
                        top_chunks: Some(top_chunks),
                        max_chunk_chars: Some(max_chunk_chars),
                        max_bytes: Some(max_bytes),
                        include_links: Some(false),
                        max_links: Some(0),
                        include_text: Some(include_text),
                        include_structure: Some(false),
                        max_outline_items: None,
                        max_blocks: None,
                        max_block_chars: None,
                        semantic_rerank: Some(false),
                        semantic_top_k: None,
                        cache_read: Some(cache_read),
                        cache_write: Some(cache_write),
                        cache_ttl_s,
                        timeout_ms: None,
                    }))
                    .await?;
                let v = payload_from_result(&r);

                let ok = v["ok"].as_bool().unwrap_or(false);
                let engine = v
                    .get("extraction")
                    .and_then(|x| x.get("engine"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                let chunks = v.get("chunks").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                let warnings = v
                    .get("warnings")
                    .and_then(|x| x.as_array())
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect::<Vec<_>>();
                let error = v.get("error").cloned();

                if ok {
                    for c in chunks.iter() {
                        let score = c.get("score").and_then(|x| x.as_u64()).unwrap_or(0);
                        merged.push((score, id.clone(), c.clone()));
                    }
                }

                per_url.push(PerUrlResult {
                    id: id.clone(),
                    url: url.clone(),
                    ok,
                    extraction_engine: engine,
                    chunks,
                    warnings,
                    error,
                });
            }

            // Merge: higher score first, then stable tiebreak by id.
            merged.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            let max_total_chunks = (max_urls * top_chunks).clamp(1, 50);
            let mut per_seed_counts: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            let mut merged_chunks: Vec<serde_json::Value> = Vec::new();
            for (score, id, mut c) in merged.into_iter() {
                if merged_chunks.len() >= max_total_chunks {
                    break;
                }
                let n = per_seed_counts.get(&id).copied().unwrap_or(0);
                if n >= merged_max_per_seed {
                    continue;
                }
                per_seed_counts.insert(id.clone(), n + 1);
                c["seed_id"] = serde_json::json!(id);
                c["score"] = serde_json::json!(score);
                merged_chunks.push(c);
            }

            let mut payload = serde_json::json!({
                "ok": true,
                "query": query,
                "request": {
                    "max_urls": max_urls,
                    "fetch_backend": fetch_backend,
                    "no_network": no_network,
                    "width": width,
                    "max_chars": max_chars,
                    "max_bytes": max_bytes,
                    "top_chunks": top_chunks,
                    "merged_max_per_seed": merged_max_per_seed,
                    "max_chunk_chars": max_chunk_chars,
                    "include_text": include_text,
                    "compact": compact,
                    "cache_read": cache_read,
                    "cache_write": cache_write,
                    "cache_ttl_s": cache_ttl_s
                },
                "urls": urls.iter().map(|(id, url)| serde_json::json!({"id": id, "url": url})).collect::<Vec<_>>(),
                "results": {
                    "merged_chunks": merged_chunks,
                    "per_url": per_url
                }
            });

            if no_network && payload["results"]["per_url"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|x| {
                    x.get("error")
                        .and_then(|e| e.get("code"))
                        .and_then(|c| c.as_str())
                        == Some("cache_error")
                })
            {
                warnings.push("no_network_may_require_warm_cache");
            }
            if !unknown_seed_ids.is_empty() {
                payload["unknown_seed_ids"] = serde_json::json!(unknown_seed_ids);
            }
            if !warnings.is_empty() {
                payload["warnings"] = serde_json::json!(warnings);
                let codes = warning_codes_from(&warnings);
                payload["warning_codes"] = serde_json::json!(codes.clone());
                payload["warning_hints"] = warning_hints_from(&codes);
            }

            if compact {
                // Keep per_url but drop potentially large echoed chunks there; merged view is enough.
                if let Some(arr) = payload["results"]["per_url"].as_array_mut() {
                    for it in arr {
                        it["chunks"] = serde_json::json!([]);
                    }
                }
            }

            add_envelope_fields(&mut payload, "web_seed_search_extract", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(description = "Report in-process usage/cost stats since server start (no secrets)")]
        async fn webpipe_usage(
            &self,
            _params: Parameters<Option<WebpipeUsageArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let kind = "webpipe_usage";
            let t0 = std::time::Instant::now();
            self.stats_inc_tool(kind);

            fn env_u64(key: &str) -> Option<u64> {
                std::env::var(key)
                    .ok()
                    .and_then(|v| v.trim().parse::<u64>().ok())
            }
            fn env_f64(key: &str) -> Option<f64> {
                std::env::var(key)
                    .ok()
                    .and_then(|v| v.trim().parse::<f64>().ok())
            }

            // Optional, user-configured budgeting/pricing knobs.
            // These are intentionally “dumb”: we only multiply cost_units by usd_per_unit if the user sets it.
            // We do not hardcode provider pricing in code.
            let tavily_budget_units = env_u64("WEBPIPE_TAVILY_BUDGET_UNITS");
            let tavily_budget_usd = env_f64("WEBPIPE_TAVILY_BUDGET_USD");
            let tavily_usd_per_unit = env_f64("WEBPIPE_TAVILY_USD_PER_COST_UNIT");

            let brave_budget_units = env_u64("WEBPIPE_BRAVE_BUDGET_UNITS");
            let brave_budget_usd = env_f64("WEBPIPE_BRAVE_BUDGET_USD");
            let brave_usd_per_unit = env_f64("WEBPIPE_BRAVE_USD_PER_COST_UNIT");

            let s = self.stats_lock();
            let now = now_epoch_s();

            let tavily_units = s
                .search_providers
                .get("tavily")
                .map(|p| p.cost_units)
                .unwrap_or(0);
            let brave_units = s
                .search_providers
                .get("brave")
                .map(|p| p.cost_units)
                .unwrap_or(0);

            let tool_calls = s.tool_calls.clone();
            let search_providers = s.search_providers.clone();
            let search_window_cap = s.search_window_cap;
            let routing_context = match s.routing_context {
                RoutingContext::Global => "global",
                RoutingContext::QueryKey => "query_key",
                RoutingContext::Both => "both",
            };
            let routing_max_contexts = s.routing_max_contexts;
            let routing_contexts_in_memory = s.search_windows_by_query_key.len();
            let mut search_window_summaries: std::collections::BTreeMap<String, muxer::Summary> =
                std::collections::BTreeMap::new();
            for (k, w) in &s.search_windows {
                search_window_summaries.insert(k.clone(), w.summary());
            }
            let llm_backends = s.llm_backends.clone();
            let fetch_backends = s.fetch_backends.clone();
            let warning_counts = s.warning_counts.clone();

            let tavily_est_usd = tavily_usd_per_unit.map(|k| (tavily_units as f64) * k);
            let brave_est_usd = brave_usd_per_unit.map(|k| (brave_units as f64) * k);

            let mut payload = serde_json::json!({
                "ok": true,
                "started_at_epoch_s": s.started_at_epoch_s,
                "now_epoch_s": now,
                "tool_calls": tool_calls,
                "usage": {
                    "search_providers": search_providers,
                    "search_window": {
                        "cap": search_window_cap,
                        "summaries": search_window_summaries,
                        "routing_context": routing_context,
                        "routing_max_contexts": routing_max_contexts,
                        "routing_contexts_in_memory": routing_contexts_in_memory
                    },
                    "llm_backends": llm_backends,
                    "fetch_backends": fetch_backends
                },
                "warnings": {
                    "counts": warning_counts
                },
                "pricing_config": {
                    "tavily": {
                        "budget_units": tavily_budget_units,
                        "budget_usd": tavily_budget_usd,
                        "usd_per_cost_unit": tavily_usd_per_unit
                    },
                    "brave": {
                        "budget_units": brave_budget_units,
                        "budget_usd": brave_budget_usd,
                        "usd_per_cost_unit": brave_usd_per_unit
                    }
                },
                "estimates": {
                    "tavily": { "cost_units": tavily_units, "estimated_usd": tavily_est_usd },
                    "brave": { "cost_units": brave_units, "estimated_usd": brave_est_usd }
                }
            });

            add_envelope_fields(&mut payload, kind, t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(description = "Reset in-process usage/cost stats (explicit side effect)")]
        async fn webpipe_usage_reset(&self) -> Result<CallToolResult, McpError> {
            let kind = "webpipe_usage_reset";
            let t0 = std::time::Instant::now();
            self.stats_inc_tool(kind);

            let now = now_epoch_s();
            let mut s = self.stats_lock();
            *s = UsageStats::new(now);

            let mut payload = serde_json::json!({
                "ok": true,
                "reset_at_epoch_s": now
            });
            add_envelope_fields(&mut payload, kind, t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(description = "Search ArXiv (Atom API) with bounded results")]
        async fn arxiv_search(
            &self,
            params: Parameters<Option<ArxivSearchArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("arxiv_search");
            let t0 = std::time::Instant::now();

            let query = args.query.clone().unwrap_or_default().trim().to_string();
            if query.is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": args.query.unwrap_or_default(),
                    "error": error_obj(ErrorCode::InvalidParams, "query must be non-empty", "Pass a non-empty query string.")
                });
                add_envelope_fields(&mut payload, "arxiv_search", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let page = args.page.unwrap_or(1).max(1);
            let per_page = args.per_page.unwrap_or(10).clamp(1, 50);
            let timeout_ms = args.timeout_ms.unwrap_or(20_000);
            let categories = args.categories.unwrap_or_default();
            let years = args.years.unwrap_or_default();

            let semantic_rerank = args.semantic_rerank.unwrap_or(false);
            let semantic_top_k = args.semantic_top_k.unwrap_or(per_page).clamp(1, 50);

            let mut resp = webpipe_local::arxiv::arxiv_search(
                self.http.clone(),
                query.clone(),
                categories.clone(),
                years.clone(),
                page,
                per_page,
                timeout_ms,
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let mut semantic: serde_json::Value = serde_json::Value::Null;
            if semantic_rerank {
                let cands: Vec<(usize, usize, String)> = resp
                    .papers
                    .iter()
                    .map(|p| {
                        let text =
                            format!("__ARXIV_ID__:{}\n{}\n\n{}", p.arxiv_id, p.title, p.summary);
                        (0, text.chars().count(), text)
                    })
                    .collect();
                let sem =
                    webpipe_local::semantic::semantic_rerank_chunks(&query, &cands, semantic_top_k);
                if sem.ok {
                    let mut by_id = std::collections::BTreeMap::<
                        String,
                        webpipe_local::arxiv::ArxivPaper,
                    >::new();
                    for p in &resp.papers {
                        by_id.insert(p.arxiv_id.clone(), p.clone());
                    }
                    let mut out: Vec<webpipe_local::arxiv::ArxivPaper> = Vec::new();
                    let mut seen = std::collections::HashSet::<String>::new();
                    for ch in &sem.chunks {
                        let first = ch.text.lines().next().unwrap_or("");
                        let id = first.strip_prefix("__ARXIV_ID__:").unwrap_or("").trim();
                        if id.is_empty() {
                            continue;
                        }
                        if let Some(p) = by_id.get(id) {
                            if seen.insert(p.arxiv_id.clone()) {
                                out.push(p.clone());
                            }
                        }
                        if out.len() >= semantic_top_k {
                            break;
                        }
                    }
                    if !out.is_empty() {
                        resp.papers = out;
                    } else {
                        resp.warnings.push("semantic_rerank_no_matches");
                    }
                } else {
                    resp.warnings.extend(sem.warnings.iter().copied());
                }
                semantic = serde_json::to_value(sem).unwrap_or(serde_json::Value::Null);
            }

            let mut payload = serde_json::json!({
                "ok": resp.ok,
                "query": resp.query,
                "page": resp.page,
                "per_page": resp.per_page,
                "total_results": resp.total_results,
                "papers": resp.papers,
                "warnings": resp.warnings,
                "request": {
                    "query": query,
                    "categories": categories,
                    "years": years,
                    "page": page,
                    "per_page": per_page,
                    "timeout_ms": timeout_ms,
                    "semantic_rerank": semantic_rerank,
                    "semantic_top_k": semantic_top_k
                }
            });
            if !semantic.is_null() {
                payload["semantic"] = semantic;
            }
            add_envelope_fields(&mut payload, "arxiv_search", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(description = "Fetch ArXiv metadata and optionally find discussion pages (bounded)")]
        async fn arxiv_enrich(
            &self,
            params: Parameters<Option<ArxivEnrichArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("arxiv_enrich");
            let t0 = std::time::Instant::now();

            let raw = args.id_or_url.unwrap_or_default().trim().to_string();
            if raw.is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "error": error_obj(ErrorCode::InvalidParams, "id_or_url must be non-empty", "Pass an arXiv ID like \"0805.3415\" or a full abs URL.")
                });
                add_envelope_fields(&mut payload, "arxiv_enrich", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let arxiv_id = if raw.contains("arxiv.org/") {
                raw.split("/abs/")
                    .nth(1)
                    .unwrap_or(&raw)
                    .trim()
                    .trim_matches('/')
                    .to_string()
            } else {
                raw.clone()
            };
            let abs_url = webpipe_local::arxiv::arxiv_abs_url(&arxiv_id);

            let paper = webpipe_local::arxiv::arxiv_lookup_by_id(
                self.http.clone(),
                arxiv_id.clone(),
                args.timeout_ms.unwrap_or(20_000),
            )
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

            let mut payload = serde_json::json!({
                "ok": true,
                "arxiv_id": arxiv_id,
                "abs_url": abs_url,
                "paper": paper,
            });

            let include_discussions = args.include_discussions.unwrap_or(false);
            if include_discussions {
                let max_discussion_urls = args.max_discussion_urls.unwrap_or(2).clamp(1, 5);

                // Cheap + bounded: do a web search for discussion/commentary pages.
                // (Extraction can be done separately via `web_search_extract` if desired.)
                let q = format!("{} discussion", abs_url);
                let r = self
                    .web_search(p(WebSearchArgs {
                        query: Some(q),
                        provider: Some("auto".to_string()),
                        auto_mode: Some("mab".to_string()),
                        max_results: Some(max_discussion_urls),
                        language: None,
                        country: None,
                    }))
                    .await?;
                payload["discussion_search"] = payload_from_result(&r);
            }

            add_envelope_fields(&mut payload, "arxiv_enrich", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(
            description = "Cheap Tavily-style: search (optional) -> local fetch -> local extract -> top chunks (bounded)"
        )]
        pub(crate) async fn web_search_extract(
            &self,
            params: Parameters<Option<WebSearchExtractArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("web_search_extract");
            let t0 = std::time::Instant::now();

            // Track which knobs were explicitly provided so presets can fill only defaults.
            let max_results_user = args.max_results.is_some();
            let max_urls_user = args.max_urls.is_some();
            let max_chars_user = args.max_chars.is_some();
            let top_chunks_user = args.top_chunks.is_some();
            let max_chunk_chars_user = args.max_chunk_chars.is_some();

            let mut max_results = args.max_results.unwrap_or(5).clamp(1, 20);
            let mut max_urls = args.max_urls.unwrap_or(3).clamp(1, 10);
            let timeout_ms = args.timeout_ms.unwrap_or(20_000);
            let max_bytes = args.max_bytes.unwrap_or(5_000_000);
            let width = args.width.unwrap_or(100).clamp(20, 240);
            let mut max_chars = args.max_chars.unwrap_or(20_000).min(200_000);
            let mut top_chunks = args.top_chunks.unwrap_or(5).min(50);
            let mut max_chunk_chars = args.max_chunk_chars.unwrap_or(500).min(5_000);
            let include_links = args.include_links.unwrap_or(false);
            let max_links = args.max_links.unwrap_or(25).min(500);
            let include_text = args.include_text.unwrap_or(false);
            let include_structure = args.include_structure.unwrap_or(false);
            let max_outline_items = args.max_outline_items.unwrap_or(25).min(200);
            let max_blocks = args.max_blocks.unwrap_or(40).min(200);
            let max_block_chars = args.max_block_chars.unwrap_or(400).min(2000);
            let semantic_rerank = args.semantic_rerank.unwrap_or(false);
            let semantic_top_k = args.semantic_top_k.unwrap_or(5).min(50);
            let compact = args.compact.unwrap_or(true);
            let retry_on_truncation = args.retry_on_truncation.unwrap_or(false);
            let truncation_retry_max_bytes = args.truncation_retry_max_bytes;
            // Default to agentic loop (bounded by max_urls) for better “query → right page” behavior.
            // Callers can opt out with agentic=false.
            let agentic = args.agentic.unwrap_or(true);
            let agentic_selector = args.agentic_selector.unwrap_or_else(|| "auto".to_string());

            let cache_read = args.cache_read.unwrap_or(true);
            let cache_write = args.cache_write.unwrap_or(true);
            let cache_ttl_s = args.cache_ttl_s;

            let requested_provider = args.provider.unwrap_or_else(|| "auto".to_string());
            let requested_auto_mode = args.auto_mode.unwrap_or_else(|| "fallback".to_string());
            let selection_mode = args.selection_mode.unwrap_or_else(|| "score".to_string());
            let fetch_backend = args.fetch_backend.unwrap_or_else(|| "local".to_string());
            let no_network = args.no_network.unwrap_or(false);
            let url_selection_mode = args
                .url_selection_mode
                .unwrap_or_else(|| "auto".to_string());
            let firecrawl_fallback_on_empty_extraction = if no_network {
                false
            } else {
                args.firecrawl_fallback_on_empty_extraction.unwrap_or(false)
            };
            let firecrawl_fallback_on_low_signal = if no_network {
                false
            } else {
                args.firecrawl_fallback_on_low_signal.unwrap_or(false)
            };

            // Width vs depth preset: only fills in defaults when those fields were not explicitly set.
            let exploration = args
                .exploration
                .clone()
                .unwrap_or_else(|| "balanced".to_string());
            match exploration.as_str() {
                "balanced" => {}
                "wide" => {
                    if !max_results_user {
                        max_results = 10;
                    }
                    if !max_urls_user {
                        max_urls = 5;
                    }
                    if !max_chars_user {
                        max_chars = 10_000;
                    }
                    if !top_chunks_user {
                        top_chunks = 3;
                    }
                    if !max_chunk_chars_user {
                        max_chunk_chars = 400;
                    }
                }
                "deep" => {
                    if !max_results_user {
                        max_results = 5;
                    }
                    if !max_urls_user {
                        max_urls = 2;
                    }
                    if !max_chars_user {
                        max_chars = 60_000;
                    }
                    if !top_chunks_user {
                        top_chunks = 10;
                    }
                    if !max_chunk_chars_user {
                        max_chunk_chars = 800;
                    }
                }
                _ => {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "provider": requested_provider,
                        "exploration": exploration,
                        "query": args.query.clone().unwrap_or_default(),
                        "error": error_obj(
                            ErrorCode::InvalidParams,
                            "unknown exploration preset",
                            "Allowed exploration values: balanced, wide, deep"
                        ),
                    });
                    add_envelope_fields(
                        &mut payload,
                        "web_search_extract",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }
            }
            if url_selection_mode.as_str() != "auto"
                && url_selection_mode.as_str() != "auto_plus"
                && url_selection_mode.as_str() != "preserve"
                && url_selection_mode.as_str() != "query_rank"
            {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": requested_provider,
                    "auto_mode": requested_auto_mode,
                    "url_selection_mode": url_selection_mode,
                    "fetch_backend": fetch_backend,
                    "query": args.query.clone().unwrap_or_default(),
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown url_selection_mode",
                        "Allowed url_selection_mode values: auto, auto_plus, preserve, query_rank"
                    ),
                });
                add_envelope_fields(&mut payload, "web_search_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if selection_mode.as_str() != "score" && selection_mode.as_str() != "pareto" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": requested_provider,
                    "auto_mode": requested_auto_mode,
                    "selection_mode": selection_mode,
                    "fetch_backend": fetch_backend,
                    "query": args.query.clone().unwrap_or_default(),
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown selection_mode",
                        "Allowed selection_mode values: score, pareto"
                    ),
                });
                add_envelope_fields(&mut payload, "web_search_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if fetch_backend.as_str() != "local" && fetch_backend.as_str() != "firecrawl" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": requested_provider,
                    "fetch_backend": fetch_backend,
                    "query": args.query.clone().unwrap_or_default(),
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown fetch_backend",
                        "Allowed fetch_backends: local, firecrawl"
                    ),
                });
                add_envelope_fields(&mut payload, "web_search_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if agentic_selector.as_str() != "auto"
                && agentic_selector.as_str() != "lexical"
                && agentic_selector.as_str() != "llm"
            {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": requested_provider,
                    "fetch_backend": fetch_backend,
                    "query": args.query.clone().unwrap_or_default(),
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown agentic_selector",
                        "Allowed agentic_selector values: auto, lexical, llm"
                    ),
                });
                add_envelope_fields(&mut payload, "web_search_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            // `fetch_backend` selects the primary backend. Separately, we may use a bounded Firecrawl
            // fallback in local mode when extraction yields empty or low-signal output (opt-in flags).
            let firecrawl_primary = if fetch_backend == "firecrawl" {
                match webpipe_local::firecrawl::FirecrawlClient::from_env(self.http.clone()) {
                    Ok(c) => Some(c),
                    Err(e) => {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "provider": requested_provider,
                            "fetch_backend": fetch_backend,
                            "query": args.query.clone().unwrap_or_default(),
                            "error": error_obj(
                                ErrorCode::NotConfigured,
                                e.to_string(),
                                "Set WEBPIPE_FIRECRAWL_API_KEY (or FIRECRAWL_API_KEY) to use fetch_backend=\"firecrawl\"."
                            ),
                        });
                        add_envelope_fields(
                            &mut payload,
                            "web_search_extract",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                }
            } else {
                None
            };
            let firecrawl_fallback = if !no_network
                && fetch_backend == "local"
                && (firecrawl_fallback_on_empty_extraction || firecrawl_fallback_on_low_signal)
            {
                webpipe_local::firecrawl::FirecrawlClient::from_env(self.http.clone()).ok()
            } else {
                None
            };

            let mut search_steps: Vec<serde_json::Value> = Vec::new();
            let mut search_rounds: usize = 0;
            let mut search_backend_provider: Option<String> = None;

            let (query, urls): (String, Vec<String>) = if let Some(urls) = args.urls {
                let q = args.query.unwrap_or_default();
                (q, urls)
            } else {
                if no_network {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "provider": requested_provider,
                        "auto_mode": requested_auto_mode,
                        "fetch_backend": fetch_backend,
                        "no_network": true,
                        "query": args.query.unwrap_or_default(),
                        "error": error_obj(
                            ErrorCode::NotSupported,
                            "no_network=true requires urls=[...] (search is network-only)",
                            "Pass urls=[...] to run offline, or set no_network=false."
                        ),
                        "request": { "provider": requested_provider, "auto_mode": requested_auto_mode, "max_results": max_results, "max_urls": max_urls }
                    });
                    add_envelope_fields(
                        &mut payload,
                        "web_search_extract",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }
                let q = match args.query {
                    Some(q) if !q.trim().is_empty() => q,
                    _ => {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "provider": requested_provider,
                            "query": args.query.unwrap_or_default(),
                            "error": error_obj(
                                ErrorCode::InvalidParams,
                                "either `urls` must be provided, or `query` must be a non-empty string",
                                "Pass urls=[...] for offline mode, or query=\"...\" to run a search first."
                            ),
                            "request": {
                                "provider": requested_provider,
                                "max_results": max_results,
                                "max_urls": max_urls
                            }
                        });
                        add_envelope_fields(
                            &mut payload,
                            "web_search_extract",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                };

                // Use our existing web_search tool logic to keep routing/fallback consistent.
                let sr = self
                    .web_search(p(WebSearchArgs {
                        query: Some(q.clone()),
                        provider: Some(requested_provider.clone()),
                        auto_mode: Some(requested_auto_mode.clone()),
                        max_results: Some(max_results),
                        language: None,
                        country: None,
                    }))
                    .await?;
                let sv = payload_from_result(&sr);
                if sv.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                    // Bubble the search failure, but keep this tool's kind.
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "provider": requested_provider,
                        "query": q,
                        "search": sv,
                        "error": error_obj(
                            ErrorCode::SearchFailed,
                            "search failed",
                            "Fix the search error above (keys/config), or provide urls=[...] to run in offline mode."
                        )
                    });
                    add_envelope_fields(
                        &mut payload,
                        "web_search_extract",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }

                search_backend_provider = sv
                    .get("backend_provider")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| {
                        sv.get("provider")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string())
                    });

                let mut out_all = Vec::new();
                if let Some(rs) = sv.get("results").and_then(|v| v.as_array()) {
                    for r in rs {
                        if let Some(u) = r.get("url").and_then(|v| v.as_str()) {
                            out_all.push(u.to_string());
                            if out_all.len() >= max_results {
                                break;
                            }
                        }
                    }
                }
                let cap = if agentic { max_results } else { max_urls };
                let mut out: Vec<String> = Vec::new();
                let mut dropped_auth = 0usize;
                // Prefer non-auth-wall URLs when we have enough candidates.
                for u in &out_all {
                    if out.len() >= cap {
                        break;
                    }
                    if !url_looks_like_auth_or_challenge(u) {
                        out.push(u.clone());
                    } else {
                        dropped_auth += 1;
                    }
                }
                // If we still need URLs, allow auth-wall URLs as a fallback (better than returning nothing).
                if out.len() < cap {
                    for u in &out_all {
                        if out.len() >= cap {
                            break;
                        }
                        if out.iter().any(|x| x == u) {
                            continue;
                        }
                        out.push(u.clone());
                    }
                }

                // Keep a compact, stable summary of the search step for agent provenance.
                // (Do not duplicate results; we already emit per-URL fetch/extract outputs.)
                // NOTE: `provider` is the requested provider; `backend_provider` may be "merge" or a concrete provider.
                search_steps.push(serde_json::json!({
                    "ok": true,
                    "requested_provider": requested_provider,
                    "auto_mode": requested_auto_mode,
                    "backend_provider": sv.get("backend_provider").cloned().unwrap_or(serde_json::Value::Null),
                    "result_count": sv.get("results").and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0),
                    "warnings": sv.get("warnings").cloned().unwrap_or(serde_json::Value::Null),
                    "providers": sv.get("providers").cloned().unwrap_or(serde_json::Value::Null),
                    "selection": sv.get("selection").cloned().unwrap_or(serde_json::Value::Null),
                    "auth_wall_urls_skipped": dropped_auth,
                }));
                search_rounds = 1;

                (q, out)
            };

            // Dedup URLs while preserving the original order. This keeps `max_urls`
            // semantics predictable and avoids doing redundant work.
            let urls = {
                let mut seen = std::collections::HashSet::<String>::new();
                let mut out = Vec::new();
                for u in urls {
                    let us = u.trim();
                    if us.is_empty() {
                        continue;
                    }
                    let s = us.to_string();
                    if seen.insert(s.clone()) {
                        out.push(s);
                    }
                }
                out
            };

            // URL selection under max_urls. For the "auto_plus" mode, we do a bounded pre-pass:
            // fetch small bytes (cache-first) and rank by title/h1 hints + URL tokens.
            let selected_urls: Vec<String> = if url_selection_mode.as_str() == "auto_plus" {
                // If we don't have a query, auto_plus degenerates to preserve.
                let qkey = Self::query_key(&query).unwrap_or_default();
                if qkey.is_empty() {
                    urls.iter().take(max_urls).cloned().collect()
                } else {
                    let mut q_toks: Vec<String> = qkey
                        .split(|ch: char| !ch.is_alphanumeric())
                        .filter_map(|t| {
                            let t = t.trim();
                            (t.len() >= 2).then_some(t.to_string())
                        })
                        .collect();
                    // De-noise: drop generic stopwords and a few domain-generic tokens that
                    // otherwise swamp hint/title matching.
                    {
                        let stop: std::collections::HashSet<&'static str> =
                            textprep::stopwords::ENGLISH.iter().copied().collect();
                        let domain_stop: std::collections::HashSet<&'static str> = [
                            "api",
                            "docs",
                            "doc",
                            "documentation",
                            "reference",
                            "guide",
                            "page",
                            "latest",
                        ]
                        .into_iter()
                        .collect();
                        q_toks.retain(|t| {
                            !stop.contains(t.as_str()) && !domain_stop.contains(t.as_str())
                        });
                        q_toks.sort();
                        q_toks.dedup();
                    }

                    // Tiny, explicit query expansion for common MCP stdio failure terms.
                    // This is intentionally small + deterministic (no LLM), and helps map
                    // “TransportClosed/ConnectionClosed” error queries to the transports spec.
                    let q_join = q_toks.join(" ");
                    if q_join.contains("transportclosed") || q_join.contains("connectionclosed") {
                        q_toks.extend(
                            ["transport", "transports", "stdio", "spec", "specification"]
                                .into_iter()
                                .map(|s| s.to_string()),
                        );
                        q_toks.sort();
                        q_toks.dedup();
                    }

                    if q_toks.is_empty() {
                        urls.iter().take(max_urls).cloned().collect()
                    } else {
                        // Token rarity weights (cheap DF over URL strings).
                        let token_weights: std::collections::HashMap<String, u64> = {
                            let mut df: std::collections::HashMap<String, u64> =
                                q_toks.iter().map(|t| (t.clone(), 0u64)).collect();
                            for u in &urls {
                                let u_scrub = textprep::scrub(u);
                                for t in &q_toks {
                                    if u_scrub.contains(t) {
                                        if let Some(v) = df.get_mut(t) {
                                            *v += 1;
                                        }
                                    }
                                }
                            }
                            // Weight is inverse document frequency-ish. Keep integer arithmetic.
                            // Higher weight for rarer tokens so “firecrawl” beats generic words like “field”.
                            df.into_iter()
                                .map(|(t, c)| {
                                    let w = 1_000u64 / (1 + c);
                                    (t, w.max(1))
                                })
                                .collect()
                        };

                        // First pass over ALL URLs using URL-string token match (cheap, no IO).
                        let mut by_url_score: Vec<(usize, u64, String)> = urls
                            .iter()
                            .enumerate()
                            .map(|(i, u)| {
                                let u_scrub = textprep::scrub(u);
                                let mut s = 0u64;
                                for t in &q_toks {
                                    if u_scrub.contains(t) {
                                        s = s.saturating_add(*token_weights.get(t).unwrap_or(&1));
                                    }
                                }
                                (i, s, u.clone())
                            })
                            .collect();
                        by_url_score.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

                        // Bound prefetch work: refine only the top-N by URL score, capped.
                        // This keeps IO predictable while still considering URLs anywhere in the list.
                        let probe_n = std::cmp::min(
                            urls.len(),
                            std::cmp::min(max_urls.saturating_mul(12), 60),
                        );
                        let probe: Vec<(usize, u64, String)> =
                            by_url_score.iter().take(probe_n).cloned().collect();

                        let mut scored: Vec<(usize, u64, u64, bool, String)> = Vec::new();
                        for (i, url_score, u) in probe {
                            // Cache-first, bounded fetch of small bytes for hint extraction.
                            let (hint_score, cache_hit) = match self
                                .fetcher
                                .fetch(&webpipe_core::FetchRequest {
                                    url: u.clone(),
                                    timeout_ms: Some(timeout_ms.min(5_000)),
                                    max_bytes: Some(max_bytes.min(200_000)),
                                    headers: BTreeMap::new(),
                                    cache: webpipe_core::FetchCachePolicy {
                                        read: true,
                                        write: cache_write,
                                        ttl_s: cache_ttl_s,
                                    },
                                })
                                .await
                            {
                                Ok(fr) => {
                                    let cache_hit = fr.source == webpipe_core::FetchSource::Cache;
                                    let html = fr.text_lossy();
                                    let hint = webpipe_local::extract::html_hint_text(&html, 400);
                                    let hint_scrub = textprep::scrub(&hint);
                                    let mut s = 0u64;
                                    for t in &q_toks {
                                        if hint_scrub.contains(t) {
                                            s = s.saturating_add(
                                                *token_weights.get(t).unwrap_or(&1),
                                            );
                                        }
                                    }
                                    (s, cache_hit)
                                }
                                Err(_) => (0u64, false),
                            };

                            scored.push((i, hint_score, url_score, cache_hit, u));
                        }

                        // Sort: hint_score, then url_score, then cache_hit, then original order.
                        scored.sort_by(|a, b| {
                            b.1.cmp(&a.1)
                                .then_with(|| b.2.cmp(&a.2))
                                .then_with(|| (b.3 as u8).cmp(&(a.3 as u8)))
                                .then_with(|| a.0.cmp(&b.0))
                        });
                        let mut picked: Vec<String> = scored
                            .into_iter()
                            .take(max_urls)
                            .map(|(_, _, _, _, u)| u)
                            .collect();

                        if picked.len() < max_urls {
                            let mut seen = std::collections::HashSet::<String>::new();
                            for u in &picked {
                                seen.insert(u.clone());
                            }
                            for (_i, _s, u) in by_url_score {
                                if picked.len() >= max_urls {
                                    break;
                                }
                                if seen.insert(u.clone()) {
                                    picked.push(u);
                                }
                            }
                        }

                        picked
                    }
                }
            } else {
                Self::select_urls_for_hydration(
                    urls.clone(),
                    max_urls,
                    &query,
                    url_selection_mode.as_str(),
                )
            };

            let mut per_url = Vec::new();
            let mut all_chunks: Vec<ChunkCandidate> = Vec::new();
            let mut total_urls_ok: usize = 0;
            let mut hard_junk_urls: usize = 0;
            let mut soft_junk_urls: usize = 0;

            let query_key = Self::query_key(&query);

            let mut agentic_trace: Vec<serde_json::Value> = Vec::new();
            fn canonicalize_url_no_frag(url: &str) -> Option<String> {
                let s = url.trim();
                if s.is_empty() {
                    return None;
                }
                if let Ok(mut u) = reqwest::Url::parse(s) {
                    u.set_fragment(None);
                    Some(u.to_string())
                } else {
                    None
                }
            }

            // Agentic loop: frontier can expand via link discovery and can replace weak initial picks.
            // Bounded by `max_urls` total fetches.
            let mut frontier: Vec<String> = if agentic {
                urls.clone()
            } else {
                selected_urls.clone()
            };
            let mut seen_frontier = std::collections::HashSet::<String>::new();
            frontier.retain(|u| {
                canonicalize_url_no_frag(u)
                    .map(|k| seen_frontier.insert(k))
                    .unwrap_or(false)
            });

            // Query tokens used for deterministic frontier ranking (no special-cased domains).
            let qkey = Self::query_key(&query).unwrap_or_default();
            let mut q_toks: Vec<String> = qkey
                .split(|ch: char| !ch.is_alphanumeric())
                .filter_map(|t| {
                    let t = t.trim();
                    (t.len() >= 2).then_some(t.to_string())
                })
                .collect();
            {
                let stop: std::collections::HashSet<&'static str> =
                    textprep::stopwords::ENGLISH.iter().copied().collect();
                let domain_stop: std::collections::HashSet<&'static str> = [
                    "api",
                    "docs",
                    "doc",
                    "documentation",
                    "reference",
                    "guide",
                    "page",
                    "latest",
                ]
                .into_iter()
                .collect();
                q_toks.retain(|t| !stop.contains(t.as_str()) && !domain_stop.contains(t.as_str()));
                q_toks.sort();
                q_toks.dedup();
            }
            let q_join = q_toks.join(" ");
            if q_join.contains("transportclosed") || q_join.contains("connectionclosed") {
                q_toks.extend(
                    ["transport", "transports", "stdio", "spec", "specification"]
                        .into_iter()
                        .map(|s| s.to_string()),
                );
                q_toks.sort();
                q_toks.dedup();
            }

            // Per-URL “prior” relevance propagated from fetched pages to their discovered links.
            // Keyed by canonical URL (fragmentless).
            let mut priors = std::collections::HashMap::<String, u64>::new();
            // Best-effort label per discovered URL (anchor text), to help the planner pick.
            let mut link_labels = std::collections::HashMap::<String, String>::new();

            // Public-repo friendly: keep agentic selection deterministic (lexical only).
            // `agentic_selector="llm"` is accepted but treated as lexical in this build.
            let _planner_model_available = false;

            // If Firecrawl is configured, allow the agentic selector to opt into it for a single URL.
            // This is bounded (at most `max_urls` total fetches) and opt-in per-URL.
            let firecrawl_agentic = if !no_network
                && (has_env("WEBPIPE_FIRECRAWL_API_KEY") || has_env("FIRECRAWL_API_KEY"))
            {
                webpipe_local::firecrawl::FirecrawlClient::from_env(self.http.clone()).ok()
            } else {
                None
            };
            let mut firecrawl_disabled = false;
            let mut agentic_force_firecrawl_next: bool = false;
            let planner_max_calls = args
                .planner_max_calls
                .unwrap_or_else(Self::planner_max_calls_from_env)
                .min(10);
            let _planner_calls: usize = 0;
            let max_search_rounds = args
                .agentic_max_search_rounds
                .or_else(|| {
                    std::env::var("WEBPIPE_AGENTIC_MAX_SEARCH_ROUNDS")
                        .ok()
                        .and_then(|v| v.trim().parse::<usize>().ok())
                })
                .unwrap_or(1)
                .clamp(1, 5);
            let frontier_max = args.agentic_frontier_max.unwrap_or(200).clamp(50, 2_000);
            let mut stuck_streak: usize = 0;

            while per_url.len() < max_urls {
                if frontier.is_empty() {
                    // Agentic escape hatch: do another bounded search round when we ran out of frontier.
                    // This is cost-aware: default max_search_rounds=1 unless explicitly enabled.
                    if !agentic
                        || no_network
                        || query.trim().is_empty()
                        || search_rounds >= max_search_rounds
                    {
                        break;
                    }

                    // Try to broaden search results. Keep this bounded and mostly deterministic.
                    // If the user requested a concrete provider, we try the other configured provider once.
                    let brave_ok =
                        has_env("WEBPIPE_BRAVE_API_KEY") || has_env("BRAVE_SEARCH_API_KEY");
                    let tavily_ok = has_env("WEBPIPE_TAVILY_API_KEY") || has_env("TAVILY_API_KEY");
                    let (prov2, auto2) = if requested_provider.as_str() == "auto" {
                        ("auto".to_string(), "merge".to_string())
                    } else if requested_provider.as_str() == "brave" && tavily_ok {
                        ("tavily".to_string(), requested_auto_mode.clone())
                    } else if requested_provider.as_str() == "tavily" && brave_ok {
                        ("brave".to_string(), requested_auto_mode.clone())
                    } else {
                        // Fall back to auto routing (it will return not_configured if neither is set).
                        ("auto".to_string(), "merge".to_string())
                    };

                    search_rounds = search_rounds.saturating_add(1);
                    let query2 = query.trim().to_string();
                    let sr2 = self
                        .web_search(p(WebSearchArgs {
                            query: Some(query2.clone()),
                            provider: Some(prov2.clone()),
                            auto_mode: Some(auto2.clone()),
                            max_results: Some(max_results),
                            language: None,
                            country: None,
                        }))
                        .await?;
                    let sv2 = payload_from_result(&sr2);
                    search_steps.push(serde_json::json!({
                        "ok": sv2.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
                        "requested_provider": prov2,
                        "auto_mode": auto2,
                        "query_rewrite": "",
                        "backend_provider": sv2.get("backend_provider").cloned().unwrap_or(serde_json::Value::Null),
                        "result_count": sv2.get("results").and_then(|x| x.as_array()).map(|a| a.len()).unwrap_or(0),
                        "warnings": sv2.get("warnings").cloned().unwrap_or(serde_json::Value::Null),
                        "providers": sv2.get("providers").cloned().unwrap_or(serde_json::Value::Null),
                        "selection": sv2.get("selection").cloned().unwrap_or(serde_json::Value::Null),
                    }));

                    if sv2.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                        break;
                    }

                    let mut added = 0usize;
                    if let Some(rs) = sv2.get("results").and_then(|v| v.as_array()) {
                        for r in rs {
                            if let Some(u) = r.get("url").and_then(|v| v.as_str()) {
                                if let Some(k) = canonicalize_url_no_frag(u) {
                                    if seen_frontier.insert(k) {
                                        frontier.push(u.to_string());
                                        added += 1;
                                        if frontier.len() >= frontier_max {
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    agentic_trace.push(serde_json::json!({
                        "search_more": true,
                        "round": search_rounds,
                        "frontier_added": added,
                        "frontier_len_after": frontier.len(),
                    }));

                    // Continue to selection now that we have a frontier again.
                    continue;
                }

                let next_url: String = if agentic {
                    // Token rarity weights over current frontier (cheap DF over URL strings).
                    let token_weights: std::collections::HashMap<String, u64> = {
                        let mut df: std::collections::HashMap<String, u64> =
                            q_toks.iter().map(|t| (t.clone(), 0u64)).collect();
                        for u in &frontier {
                            let u_scrub = textprep::scrub(u);
                            for t in &q_toks {
                                if u_scrub.contains(t) {
                                    if let Some(v) = df.get_mut(t) {
                                        *v += 1;
                                    }
                                }
                            }
                        }
                        df.into_iter()
                            .map(|(t, c)| {
                                let w = 1_000u64 / (1 + c);
                                (t, w.max(1))
                            })
                            .collect()
                    };

                    // Compute (url_score, prior_score) for each frontier URL, then pick a Pareto-optimal
                    // candidate and break ties deterministically. Optionally, ask an LLM to choose
                    // among a bounded candidate set (falls back to lexical if unavailable).
                    let mut url_scores: Vec<u64> = Vec::with_capacity(frontier.len());
                    let mut prior_scores: Vec<u64> = Vec::with_capacity(frontier.len());
                    let mut canon: Vec<String> = Vec::with_capacity(frontier.len());
                    for u in &frontier {
                        let u_scrub = textprep::scrub(u);
                        let mut s = 0u64;
                        for t in &q_toks {
                            if u_scrub.contains(t) {
                                s = s.saturating_add(*token_weights.get(t).unwrap_or(&1));
                            }
                        }
                        // Generic penalty: avoid auth/challenge/consent URLs unless they are clearly relevant.
                        if url_looks_like_auth_or_challenge(u) {
                            s /= 4;
                        }
                        let k = canonicalize_url_no_frag(u).unwrap_or_else(|| u.clone());
                        let p = *priors.get(&k).unwrap_or(&0);
                        url_scores.push(s);
                        prior_scores.push(p);
                        canon.push(k);
                    }

                    let metrics: Vec<Vec<f32>> = (0..frontier.len())
                        .map(|i| vec![url_scores[i] as f32, prior_scores[i] as f32])
                        .collect();
                    let mut idxs = pare::pareto_indices(&metrics)
                        .unwrap_or_else(|| (0..frontier.len()).collect());
                    // Prefer higher prior (content-backed), then higher url_score, then stable order.
                    idxs.sort_by(|&ia, &ib| {
                        prior_scores[ib]
                            .cmp(&prior_scores[ia])
                            .then_with(|| url_scores[ib].cmp(&url_scores[ia]))
                            .then_with(|| ia.cmp(&ib))
                    });
                    let mut best_i = idxs.first().copied().unwrap_or(0);

                    let selector_used: &'static str = "lexical";
                    if best_i >= frontier.len() {
                        best_i = idxs.first().copied().unwrap_or(0);
                    }

                    let picked = frontier.swap_remove(best_i);
                    let best_s = url_scores.get(best_i).copied().unwrap_or(0);
                    let trace_obj = serde_json::json!({
                        "picked_url": canonicalize_url_no_frag(&picked).unwrap_or_else(|| picked.clone()),
                        "score": best_s,
                        "prior": *priors.get(&canonicalize_url_no_frag(&picked).unwrap_or_else(|| picked.clone())).unwrap_or(&0),
                        "selector": selector_used,
                        "frontier_len_before": frontier.len() + 1
                    });
                    agentic_trace.push(trace_obj);
                    picked
                } else {
                    frontier.remove(0)
                };

                let url_owned = next_url;
                let url = &url_owned;
                let per_t0 = std::time::Instant::now();
                let mut attempts: serde_json::Value = serde_json::Value::Null;
                let use_firecrawl_agentic = !firecrawl_disabled
                    && agentic_force_firecrawl_next
                    && firecrawl_agentic.is_some();
                if agentic_force_firecrawl_next {
                    agentic_force_firecrawl_next = false;
                }
                let mut used_firecrawl_agentic = false;
                let (
                    raw_text,
                    raw_bytes,
                    final_url,
                    status,
                    content_type,
                    bytes_len,
                    truncated,
                    fetch_source,
                    used_firecrawl_fallback,
                    extracted_obj,
                ) = if let Some(fc) = if use_firecrawl_agentic {
                    firecrawl_agentic.as_ref()
                } else {
                    firecrawl_primary.as_ref()
                } {
                    let max_age_ms = cache_ttl_s.map(|s| s.saturating_mul(1000));
                    let r = match fc.fetch_markdown(url, timeout_ms, max_age_ms).await {
                        Ok(r) => r,
                        Err(e) => {
                            firecrawl_disabled = true;
                            per_url.push(serde_json::json!({
                                    "url": url,
                                    "ok": false,
                                    "stage": "fetch",
                                    "fetch_backend": "firecrawl",
                                    "error": error_obj(ErrorCode::FetchFailed, e.to_string(), "Firecrawl fetch failed for this URL."),
                                    "elapsed_ms": per_t0.elapsed().as_millis()
                                }));
                            continue;
                        }
                    };
                    let md = r.markdown;
                    let md_bytes = md.as_bytes().to_vec();
                    let bytes_len = Self::approx_bytes_len(&md);
                    used_firecrawl_agentic = use_firecrawl_agentic;
                    attempts = serde_json::json!({
                        (if use_firecrawl_agentic { "firecrawl_agentic" } else { "firecrawl" }): {
                            "ok": true,
                            "elapsed_ms": r.elapsed_ms,
                            "bytes": bytes_len
                        }
                    });
                    (
                        md.clone(),
                        md_bytes,
                        url.clone(),
                        200u16,
                        Some("text/markdown".to_string()),
                        bytes_len,
                        false,
                        "network",
                        false,
                        webpipe_local::extract::ExtractedText {
                            engine: "firecrawl",
                            text: md,
                            warnings: Vec::new(),
                        },
                    )
                } else {
                    let req = FetchRequest {
                        url: url.clone(),
                        timeout_ms: Some(timeout_ms),
                        max_bytes: Some(max_bytes),
                        headers: BTreeMap::new(),
                        cache: FetchCachePolicy {
                            read: cache_read || no_network,
                            write: if no_network { false } else { cache_write },
                            ttl_s: cache_ttl_s,
                        },
                    };
                    let mut fetched = if no_network {
                        match self.fetcher.cache_get(&req) {
                            Ok(Some(r)) => r,
                            Ok(None) => {
                                per_url.push(serde_json::json!({
                                        "url": url,
                                        "ok": false,
                                        "stage": "fetch",
                                        "fetch_backend": "local",
                                        "no_network": true,
                                        "error": error_obj(
                                            ErrorCode::NotSupported,
                                            "cache miss in no_network mode",
                                            "Warm the cache first (run without no_network), or set no_network=false."
                                        ),
                                        "elapsed_ms": per_t0.elapsed().as_millis()
                                    }));
                                continue;
                            }
                            Err(e) => {
                                per_url.push(serde_json::json!({
                                        "url": url,
                                        "ok": false,
                                        "stage": "fetch",
                                        "fetch_backend": "local",
                                        "no_network": true,
                                        "error": error_obj(ErrorCode::CacheError, e.to_string(), "Cache read failed in no_network mode."),
                                        "elapsed_ms": per_t0.elapsed().as_millis()
                                    }));
                                continue;
                            }
                        }
                    } else {
                        match self.fetcher.fetch(&req).await {
                            Ok(r) => r,
                            Err(e) => {
                                per_url.push(serde_json::json!({
                                        "url": url,
                                        "ok": false,
                                        "stage": "fetch",
                                        "fetch_backend": "local",
                                        "error": error_obj(ErrorCode::FetchFailed, e.to_string(), "Fetch failed for this URL."),
                                        "elapsed_ms": per_t0.elapsed().as_millis()
                                    }));
                                continue;
                            }
                        }
                    };

                    // Optional truncation retry (local-only, opt-in).
                    //
                    // If the response was truncated by max_bytes, try again with a larger cap so
                    // we can recover content that lives beyond the initial prefix (common on JS-heavy docs).
                    //
                    // IMPORTANT: only do this when explicitly requested; callers sometimes set small max_bytes
                    // intentionally to limit bandwidth/latency.
                    let mut local_attempt_obj: Option<serde_json::Value> = None;
                    let mut local_retry_obj: Option<serde_json::Value> = None;
                    if !no_network && retry_on_truncation && fetched.truncated && max_bytes > 0 {
                        let retry_cap_default = max_bytes.saturating_mul(2);
                        let retry_cap = truncation_retry_max_bytes
                            .unwrap_or(retry_cap_default)
                            .max(max_bytes.saturating_add(1))
                            .min(5_000_000);
                        if retry_cap > max_bytes {
                            // Snapshot first attempt for diagnostics.
                            local_attempt_obj = Some(serde_json::json!({
                                "ok": true,
                                "final_url": fetched.final_url.clone(),
                                "status": fetched.status,
                                "content_type": fetched.content_type.clone(),
                                "bytes": fetched.bytes.len(),
                                "truncated": true,
                                "source": match fetched.source { FetchSource::Cache => "cache", FetchSource::Network => "network" },
                                "max_bytes": max_bytes
                            }));
                            let req2 = FetchRequest {
                                url: url.clone(),
                                timeout_ms: Some(timeout_ms),
                                max_bytes: Some(retry_cap),
                                headers: BTreeMap::new(),
                                cache: FetchCachePolicy {
                                    read: cache_read,
                                    write: cache_write,
                                    ttl_s: cache_ttl_s,
                                },
                            };
                            if let Ok(r2) = self.fetcher.fetch(&req2).await {
                                local_retry_obj = Some(serde_json::json!({
                                    "ok": true,
                                    "final_url": r2.final_url.clone(),
                                    "status": r2.status,
                                    "content_type": r2.content_type.clone(),
                                    "bytes": r2.bytes.len(),
                                    "truncated": r2.truncated,
                                    "source": match r2.source { FetchSource::Cache => "cache", FetchSource::Network => "network" },
                                    "max_bytes": retry_cap
                                }));
                                fetched = r2;
                            } else {
                                local_retry_obj = Some(serde_json::json!({
                                    "ok": false,
                                    "error": "retry_fetch_failed",
                                    "max_bytes": retry_cap
                                }));
                            }
                        }
                    }
                    if local_retry_obj.is_some() {
                        let mut a = serde_json::Map::new();
                        if let Some(obj) = local_attempt_obj.take() {
                            a.insert("local".to_string(), obj);
                        }
                        if let Some(obj) = local_retry_obj.take() {
                            a.insert("local_retry".to_string(), obj);
                        }
                        attempts = serde_json::Value::Object(a);
                    }
                    let local_final_url = fetched.final_url.clone();
                    let local_status = fetched.status;
                    let local_content_type = fetched.content_type.clone();
                    let local_bytes_len = fetched.bytes.len();
                    let local_truncated = fetched.truncated;
                    let local_source = match fetched.source {
                        FetchSource::Cache => "cache",
                        FetchSource::Network => "network",
                    };

                    // Attempt local extraction first; if it's empty and fallback is enabled + configured,
                    // retry *just this URL* via Firecrawl.
                    let local_pdf_like = Self::content_type_is_pdf(local_content_type.as_deref())
                        || Self::url_looks_like_pdf(&local_final_url)
                        || webpipe_local::extract::bytes_look_like_pdf(&fetched.bytes);
                    let local_raw_text = if local_pdf_like {
                        String::new()
                    } else {
                        fetched.text_lossy()
                    };
                    let local_extracted_obj = webpipe_local::extract::best_effort_text_from_bytes(
                        &fetched.bytes,
                        local_content_type.as_deref(),
                        &local_final_url,
                        width,
                        500,
                    );
                    let (_local_text, local_text_chars, _local_text_clipped) =
                        Self::truncate_to_chars(&local_extracted_obj.text, max_chars);
                    let local_empty_extraction = local_text_chars == 0 && local_bytes_len > 0;
                    let local_low_signal = !local_empty_extraction
                        && local_text_chars > 0
                        && Self::looks_like_bundle_gunk(&local_extracted_obj.text);

                    if (local_empty_extraction && firecrawl_fallback_on_empty_extraction)
                        || (local_low_signal && firecrawl_fallback_on_low_signal)
                    {
                        if let Some(fc) = firecrawl_fallback.as_ref() {
                            let max_age_ms = cache_ttl_s.map(|s| s.saturating_mul(1000));
                            match fc.fetch_markdown(url, timeout_ms, max_age_ms).await {
                                Ok(r) => {
                                    let md = r.markdown;
                                    let md_bytes = md.as_bytes().to_vec();
                                    let fc_bytes = Self::approx_bytes_len(&md);
                                    let mut a = serde_json::Map::new();
                                    if let Some(obj) = local_attempt_obj.take() {
                                        a.insert("local".to_string(), obj);
                                    } else {
                                        a.insert(
                                            "local".to_string(),
                                            serde_json::json!({
                                                "ok": true,
                                                "final_url": local_final_url,
                                                "status": local_status,
                                                "content_type": local_content_type,
                                                "bytes": local_bytes_len,
                                                "truncated": local_truncated,
                                                "source": local_source,
                                                "empty_extraction": local_empty_extraction,
                                                "low_signal": local_low_signal
                                            }),
                                        );
                                    }
                                    if let Some(obj) = local_retry_obj.take() {
                                        a.insert("local_retry".to_string(), obj);
                                    }
                                    a.insert(
                                        "firecrawl".to_string(),
                                        serde_json::json!({
                                            "ok": true,
                                            "elapsed_ms": r.elapsed_ms,
                                            "bytes": fc_bytes
                                        }),
                                    );
                                    attempts = serde_json::Value::Object(a);
                                    (
                                        md.clone(),
                                        md_bytes,
                                        url.clone(),
                                        200u16,
                                        Some("text/markdown".to_string()),
                                        fc_bytes,
                                        false,
                                        "network",
                                        true,
                                        webpipe_local::extract::ExtractedText {
                                            engine: "firecrawl",
                                            text: md,
                                            warnings: vec![if local_empty_extraction {
                                                "firecrawl_fallback_on_empty_extraction"
                                            } else {
                                                "firecrawl_fallback_on_low_signal"
                                            }],
                                        },
                                    )
                                }
                                Err(e) => {
                                    let mut a = serde_json::Map::new();
                                    if let Some(obj) = local_attempt_obj.take() {
                                        a.insert("local".to_string(), obj);
                                    } else {
                                        a.insert(
                                            "local".to_string(),
                                            serde_json::json!({
                                                "ok": true,
                                                "final_url": local_final_url,
                                                "status": local_status,
                                                "content_type": local_content_type,
                                                "bytes": local_bytes_len,
                                                "truncated": local_truncated,
                                                "source": local_source,
                                                "empty_extraction": local_empty_extraction,
                                                "low_signal": local_low_signal
                                            }),
                                        );
                                    }
                                    if let Some(obj) = local_retry_obj.take() {
                                        a.insert("local_retry".to_string(), obj);
                                    }
                                    a.insert(
                                        "firecrawl".to_string(),
                                        serde_json::json!({
                                            "ok": false,
                                            "error": e.to_string()
                                        }),
                                    );
                                    attempts = serde_json::Value::Object(a);
                                    let bytes = fetched.bytes;
                                    (
                                        local_raw_text,
                                        bytes,
                                        local_final_url,
                                        local_status,
                                        local_content_type,
                                        local_bytes_len,
                                        local_truncated,
                                        local_source,
                                        false,
                                        local_extracted_obj,
                                    )
                                }
                            }
                        } else {
                            let mut a = serde_json::Map::new();
                            if let Some(obj) = local_attempt_obj.take() {
                                a.insert("local".to_string(), obj);
                            } else {
                                a.insert(
                                    "local".to_string(),
                                    serde_json::json!({
                                        "ok": true,
                                        "final_url": local_final_url,
                                        "status": local_status,
                                        "content_type": local_content_type,
                                        "bytes": local_bytes_len,
                                        "truncated": local_truncated,
                                        "source": local_source,
                                        "empty_extraction": local_empty_extraction,
                                        "low_signal": local_low_signal
                                    }),
                                );
                            }
                            if let Some(obj) = local_retry_obj.take() {
                                a.insert("local_retry".to_string(), obj);
                            }
                            a.insert(
                                "firecrawl".to_string(),
                                serde_json::json!({
                                    "ok": false,
                                    "error": "not_configured"
                                }),
                            );
                            attempts = serde_json::Value::Object(a);
                            let bytes = fetched.bytes;
                            (
                                local_raw_text,
                                bytes,
                                local_final_url,
                                local_status,
                                local_content_type,
                                local_bytes_len,
                                local_truncated,
                                local_source,
                                false,
                                local_extracted_obj,
                            )
                        }
                    } else {
                        if local_empty_extraction {
                            let mut local_obj = serde_json::json!({
                                "ok": true,
                                "final_url": local_final_url,
                                "status": local_status,
                                "content_type": local_content_type,
                                "bytes": local_bytes_len,
                                "truncated": local_truncated,
                                "source": local_source,
                            });
                            if local_empty_extraction {
                                local_obj["empty_extraction"] = serde_json::json!(true);
                            }
                            if local_pdf_like {
                                local_obj["pdf_like"] = serde_json::json!(true);
                            }
                            attempts = serde_json::json!({ "local": local_obj });
                        }
                        let bytes = fetched.bytes;
                        (
                            local_raw_text,
                            bytes,
                            local_final_url,
                            local_status,
                            local_content_type,
                            local_bytes_len,
                            local_truncated,
                            local_source,
                            false,
                            local_extracted_obj,
                        )
                    }
                };
                #[cfg(feature = "vision-gemini")]
                let mut pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                    &raw_bytes,
                    content_type.as_deref(),
                    final_url.as_str(),
                    extracted_obj,
                    webpipe_local::extract::ExtractPipelineCfg {
                        query: Some(&query),
                        width,
                        max_chars,
                        top_chunks,
                        max_chunk_chars,
                        include_structure,
                        max_outline_items,
                        max_blocks,
                        max_block_chars,
                    },
                );
                #[cfg(not(feature = "vision-gemini"))]
                let pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                    &raw_bytes,
                    content_type.as_deref(),
                    final_url.as_str(),
                    extracted_obj,
                    webpipe_local::extract::ExtractPipelineCfg {
                        query: Some(&query),
                        width,
                        max_chars,
                        top_chunks,
                        max_chunk_chars,
                        include_structure,
                        max_outline_items,
                        max_blocks,
                        max_block_chars,
                    },
                );

                // Opportunistic multimodal for web_search_extract results: only when
                // - feature enabled
                // - no_network is false
                // - local extraction is empty
                // - bytes look like an image
                // - GEMINI key present
                #[cfg(feature = "vision-gemini")]
                {
                    let vision_mode = webpipe_local::vision_gemini::gemini_enabled_mode_from_env();
                    let is_image_like = content_type
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .starts_with("image/")
                        || webpipe_local::extract::bytes_look_like_image(&raw_bytes);
                    if !no_network
                        && vision_mode != "off"
                        && is_image_like
                        && pipeline.text_chars == 0
                        && !raw_bytes.is_empty()
                        && webpipe_local::vision_gemini::gemini_api_key_from_env().is_some()
                    {
                        let mime0 = content_type
                            .as_deref()
                            .unwrap_or("application/octet-stream");
                        let mime = mime0.split(';').next().unwrap_or(mime0).trim();
                        match webpipe_local::vision_gemini::gemini_image_to_text(
                            self.http.clone(),
                            &raw_bytes,
                            mime,
                        )
                        .await
                        {
                            Ok((_model_id, text)) => {
                                let ex = webpipe_local::extract::ExtractedText {
                                    engine: "gemini_vision",
                                    text,
                                    warnings: vec!["gemini_used"],
                                };
                                pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                                    &raw_bytes,
                                    content_type.as_deref(),
                                    final_url.as_str(),
                                    ex,
                                    webpipe_local::extract::ExtractPipelineCfg {
                                        query: Some(&query),
                                        width,
                                        max_chars,
                                        top_chunks,
                                        max_chunk_chars,
                                        include_structure,
                                        max_outline_items,
                                        max_blocks,
                                        max_block_chars,
                                    },
                                );
                            }
                            Err(code) => {
                                if vision_mode == "strict" {
                                    pipeline.extracted.warnings.push(code);
                                } else {
                                    pipeline.extracted.warnings.push("gemini_failed");
                                }
                            }
                        }
                    }
                }
                let extracted_obj = pipeline.extracted;
                let text = extracted_obj.text.clone();
                let text_chars = pipeline.text_chars;
                let text_clipped = pipeline.text_truncated;
                let structure_opt = pipeline.structure;
                let chunks0 = pipeline.chunks;
                let empty_extraction = text_chars == 0 && bytes_len > 0;
                let is_pdf_like = Self::content_type_is_pdf(content_type.as_deref())
                    || Self::url_looks_like_pdf(final_url.as_str())
                    || webpipe_local::extract::bytes_look_like_pdf(&raw_bytes)
                    || extracted_obj.engine.starts_with("pdf-");

                let links = if include_links && fetch_backend == "local" && !is_pdf_like {
                    // Link extraction should be based on the raw document (not the chosen text-extraction
                    // engine). Otherwise, pages that select `html_main`/`html_hint` would incorrectly show
                    // `links: []` even though the HTML contains lots of links.
                    //
                    // Keep this conservative: only attempt link extraction for "text-like" content.
                    let ct0 = content_type
                        .as_deref()
                        .unwrap_or("")
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    let engine0: &str = extracted_obj.engine;
                    let text_like = ct0.starts_with("text/")
                        || engine0.starts_with("html")
                        || engine0 == "markdown";
                    if text_like {
                        let base = Some(final_url.as_str());
                        webpipe_local::links::extract_links(&raw_text, base, max_links)
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                };

                let mut warnings: Vec<&'static str> = Vec::new();
                if truncated {
                    warnings.push("body_truncated_by_max_bytes");
                }
                if let Some(o) = attempts.as_object() {
                    if let Some(lr) = o.get("local_retry") {
                        if lr.get("ok").and_then(|v| v.as_bool()) == Some(true) {
                            warnings.push("retried_due_to_truncation");
                        } else {
                            warnings.push("truncation_retry_failed");
                        }
                    }
                }
                if text_clipped {
                    warnings.push("text_truncated_by_max_chars");
                }
                if empty_extraction {
                    warnings.push("empty_extraction");
                }
                if used_firecrawl_agentic {
                    warnings.push("firecrawl_agentic");
                }
                for w in &extracted_obj.warnings {
                    warnings.push(*w);
                }
                if include_links
                    && (fetch_backend == "firecrawl"
                        || used_firecrawl_fallback
                        || used_firecrawl_agentic)
                {
                    warnings.push("links_unavailable_for_firecrawl");
                }
                if include_links && is_pdf_like {
                    warnings.push("links_unavailable_for_pdf");
                }
                if no_network {
                    warnings.push("cache_only");
                }
                // Surface “JS wall / auth wall” in a stable way so the agent can choose a different URL.
                // Use extracted text (best effort) + title (if available) + status.
                let title_opt = structure_opt
                    .as_ref()
                    .and_then(|s| s.title.as_ref())
                    .map(|s| s.as_str());
                if looks_like_js_challenge(status, &extracted_obj.text, title_opt) {
                    warnings.push("blocked_by_js_challenge");
                }
                if extracted_obj.engine == "html_main"
                    && bytes_len >= 50_000
                    && structure_opt
                        .as_ref()
                        .is_some_and(structure_looks_like_ui_shell)
                {
                    warnings.push("main_content_low_signal");
                }

                // Reduce “JS bundle gunk” in returned chunks (common on Next.js/SPA docs).
                // This is display-only: raw_text is still used for internal discovery.
                let (chunks, filtered_low_signal) = Self::filter_low_signal_chunks(chunks0);
                if filtered_low_signal {
                    warnings.push("chunks_filtered_low_signal");
                }

                let page_signal = chunks.iter().map(|c| c.score).max().unwrap_or(0);
                // Stuck detection: repeated low-signal pages should trigger an extra search round (if allowed),
                // rather than burning the remaining URL budget on adjacent low-quality pages.
                if page_signal == 0 || warnings.contains(&"empty_extraction") {
                    stuck_streak = stuck_streak.saturating_add(1);
                } else {
                    stuck_streak = 0;
                }
                if warnings.contains(&"blocked_by_js_challenge") {
                    stuck_streak = stuck_streak.saturating_add(1);
                }

                // Per-provider “junk” feedback (best-effort, deterministic):
                // - hard junk: blocked_by_js_challenge
                // - soft junk: empty_extraction / main_content_low_signal / chunks_filtered_low_signal
                total_urls_ok = total_urls_ok.saturating_add(1);
                let url_blocked = warnings
                    .iter()
                    .any(|w| normalize_warning_code(w) == "blocked_by_js_challenge");
                let url_soft = warnings.iter().any(|w| {
                    matches!(
                        normalize_warning_code(w),
                        "empty_extraction"
                            | "main_content_low_signal"
                            | "chunks_filtered_low_signal"
                    )
                });
                if url_blocked {
                    hard_junk_urls = hard_junk_urls.saturating_add(1);
                } else if url_soft {
                    soft_junk_urls = soft_junk_urls.saturating_add(1);
                }

                // Collect chunk candidates with per-URL provenance signals for selection.
                let warnings_count = warnings.len();
                let cache_hit = fetch_source == "cache";
                for c in &chunks {
                    all_chunks.push(ChunkCandidate {
                        url: url.clone(),
                        score: c.score,
                        start_char: c.start_char,
                        end_char: c.end_char,
                        text: c.text.clone(),
                        warnings_count,
                        cache_hit,
                    });
                }

                let mut one = serde_json::json!({
                    "url": url,
                    "ok": true,
                    "fetch_backend": if used_firecrawl_fallback || used_firecrawl_agentic { "firecrawl" } else { fetch_backend.as_str() },
                    "final_url": final_url,
                    "status": status,
                    "content_type": content_type,
                    "bytes": bytes_len,
                    "truncated": truncated,
                    "fetch_source": fetch_source,
                    // Always include attempts (null or object) so compact mode stays diagnosable.
                    "attempts": attempts,
                    "extract": {
                        "engine": extracted_obj.engine,
                        "width": width,
                        "max_chars": max_chars,
                        "text_chars": text_chars,
                        "text_truncated": text_clipped,
                        "top_chunks": top_chunks,
                        "max_chunk_chars": max_chunk_chars,
                        "chunks": chunks
                    },
                    "elapsed_ms": per_t0.elapsed().as_millis()
                });
                if !warnings.is_empty() {
                    one["warnings"] = serde_json::json!(warnings);
                    let codes = warning_codes_from(&warnings);
                    one["warning_codes"] = serde_json::json!(codes.clone());
                    one["warning_hints"] = warning_hints_from(&codes);
                    self.stats_record_warnings(&warnings);
                }
                if include_text {
                    one["extract"]["text"] = serde_json::json!(text);
                }
                if include_links {
                    if used_firecrawl_fallback {
                        one["extract"]["links"] = serde_json::json!([]);
                    } else {
                        one["extract"]["links"] = serde_json::json!(links);
                    }
                }
                if include_structure {
                    if let Some(s) = structure_opt.as_ref() {
                        one["extract"]["structure"] = serde_json::json!(s);
                    }
                }
                if semantic_rerank && !query.trim().is_empty() {
                    let cands: Vec<(usize, usize, String)> = chunks
                        .iter()
                        .map(|c| (c.start_char, c.end_char, c.text.clone()))
                        .collect();
                    let q0 = query.clone();
                    let sem = tokio::task::spawn_blocking(move || {
                        webpipe_local::semantic::semantic_rerank_chunks(&q0, &cands, semantic_top_k)
                    })
                    .await
                    .unwrap_or_else(|_| {
                        webpipe_local::semantic::SemanticRerankResult {
                            ok: false,
                            backend: "unknown".to_string(),
                            model_id: None,
                            cache_hits: 0,
                            cache_misses: 0,
                            chunks: Vec::new(),
                            warnings: vec!["semantic_rerank_task_failed"],
                        }
                    });
                    one["extract"]["semantic"] = serde_json::json!(sem);
                }
                if compact {
                    // Reduce duplication: `top_chunks` already carries the evidence text,
                    // so per-URL compact mode drops `extract.chunks` (and some knob echoes).
                    if let Some(ex) = one.get_mut("extract").and_then(|v| v.as_object_mut()) {
                        ex.remove("chunks");
                        ex.remove("width");
                        ex.remove("max_chars");
                        ex.remove("top_chunks");
                        ex.remove("max_chunk_chars");
                    }

                    // Keep only stable identifiers + extract payload + warnings.
                    let mut out = serde_json::Map::new();
                    for k in [
                        "url",
                        "final_url",
                        "ok",
                        "fetch_backend",
                        "attempts",
                        "status",
                        "content_type",
                        "bytes",
                        "elapsed_ms",
                        "warnings",
                        "warning_codes",
                        "warning_hints",
                        "extract",
                    ] {
                        if let Some(v) = one.get(k) {
                            out.insert(k.to_string(), v.clone());
                        }
                    }
                    one = serde_json::Value::Object(out);
                }
                per_url.push(one);
                // Internal discovery: even if include_links=false, expand frontier when agentic.
                // This should work for:
                // - HTML extraction engines (html2text/html_main/html_hint)
                // - Firecrawl fallback (markdown) and markdown-ish content
                if agentic && fetch_backend == "local" && !is_pdf_like {
                    let base = Some(final_url.as_str());
                    let ct0 = content_type
                        .as_deref()
                        .unwrap_or("")
                        .split(';')
                        .next()
                        .unwrap_or("")
                        .trim()
                        .to_ascii_lowercase();
                    let markdown_like = used_firecrawl_fallback
                        || used_firecrawl_agentic
                        || extracted_obj.engine == "markdown"
                        || ct0 == "text/markdown"
                        || ct0 == "text/x-markdown";
                    let discovered = if markdown_like {
                        // Markdown-ish: recover discovery via markdown link parsing.
                        webpipe_local::links::extract_markdown_link_candidates(
                            &raw_text,
                            base,
                            max_links.min(50),
                        )
                    } else {
                        webpipe_local::links::extract_link_candidates(
                            &raw_text,
                            base,
                            max_links.min(50),
                        )
                    };
                    // Propagate content relevance to discovered links. This is the “loop”:
                    // good pages contribute candidate URLs more strongly than weak pages.
                    let parent_relevance = chunks.iter().map(|c| c.score).max().unwrap_or(0);
                    let mut added = 0usize;
                    for cand in discovered {
                        let u = cand.url;
                        if frontier.len() >= frontier_max {
                            break;
                        }
                        if let Some(k) = canonicalize_url_no_frag(&u) {
                            if seen_frontier.insert(k.clone()) {
                                // Record/update prior relevance for this discovered URL.
                                let entry = priors.entry(k.clone()).or_insert(0);
                                if !cand.text.trim().is_empty() {
                                    let (lbl, _n, _clip) = Self::truncate_to_chars(&cand.text, 120);
                                    let replace = match link_labels.get(&k) {
                                        None => true,
                                        Some(existing) => lbl.len() > existing.len(),
                                    };
                                    if replace {
                                        link_labels.insert(k.clone(), lbl);
                                    }
                                }
                                // Use anchor text when available; it's often more semantic than the URL.
                                let anchor_scrub = textprep::scrub(&cand.text);
                                let url_scrub = textprep::scrub(&u);
                                let mut hits = 0u64;
                                for t in &q_toks {
                                    if anchor_scrub.contains(t.as_str()) {
                                        hits += 2; // anchor text counts more than URL string
                                    } else if url_scrub.contains(t.as_str()) {
                                        hits += 1;
                                    }
                                }
                                // Generic bias toward primary artifacts: PDF links often contain the
                                // full document even when the anchor text is just "PDF" and doesn't
                                // match the query tokens (e.g., arXiv).
                                let pdf_like_link = {
                                    let a_lc = cand.text.to_ascii_lowercase();
                                    a_lc.split_whitespace().any(|w| w == "pdf")
                                        || Self::url_looks_like_pdf(&u)
                                };
                                if pdf_like_link {
                                    hits = hits.max(1);
                                }
                                // IMPORTANT: allow “hub page → relevant link” hops even when the hub page
                                // itself has no query-matching chunks (parent_relevance==0). In that case,
                                // we still want anchor/URL token matches to influence the next pick.
                                let base_parent = if parent_relevance == 0 {
                                    1
                                } else {
                                    parent_relevance
                                };
                                let prior_add = base_parent.saturating_mul(hits.min(10));
                                *entry = (*entry).max(prior_add);
                                frontier.push(u);
                                added += 1;
                            }
                        }
                    }
                    if let Some(last) = agentic_trace.last_mut() {
                        if let Some(obj) = last.as_object_mut() {
                            obj.insert("frontier_added".to_string(), serde_json::json!(added));
                            obj.insert(
                                "frontier_len_after".to_string(),
                                serde_json::json!(frontier.len()),
                            );
                        }
                    }
                }

                // If we're stuck, proactively broaden the frontier via another search round.
                if agentic
                    && !no_network
                    && !query.trim().is_empty()
                    && stuck_streak >= 2
                    && search_rounds < max_search_rounds
                {
                    frontier.clear();
                    agentic_trace.push(serde_json::json!({
                        "stuck": true,
                        "stuck_streak": stuck_streak,
                        "action": "search_more",
                    }));
                    stuck_streak = 0;
                }
            }

            let selected = Self::select_top_chunks(all_chunks, top_chunks, selection_mode.as_str());
            let top_chunks_out: Vec<serde_json::Value> = selected
                .into_iter()
                .map(|c| {
                    serde_json::json!({
                        "url": c.url,
                        "score": c.score,
                        "start_char": c.start_char,
                        "end_char": c.end_char,
                        "text": c.text
                    })
                })
                .collect();

            let mut payload = serde_json::json!({
                "ok": true,
                "provider": "web_search_extract",
                "query": query,
                "request": {
                    "provider": requested_provider,
                        "auto_mode": requested_auto_mode,
                    "selection_mode": selection_mode,
                    "exploration": exploration,
                    "max_results": max_results,
                    "max_urls": max_urls,
                    "url_selection_mode": url_selection_mode,
                    "timeout_ms": timeout_ms,
                    "max_bytes": max_bytes,
                    "retry_on_truncation": retry_on_truncation,
                    "truncation_retry_max_bytes": truncation_retry_max_bytes,
                    "width": width,
                    "max_chars": max_chars,
                    "top_chunks": top_chunks,
                    "max_chunk_chars": max_chunk_chars,
                    "include_links": include_links,
                    "max_links": max_links,
                    "include_text": include_text,
                    "agentic": agentic,
                    "agentic_max_search_rounds": max_search_rounds,
                    "agentic_frontier_max": frontier_max,
                    "planner_max_calls": planner_max_calls,
                        "no_network": no_network,
                    "firecrawl_fallback_on_empty_extraction": firecrawl_fallback_on_empty_extraction,
                    "firecrawl_fallback_on_low_signal": firecrawl_fallback_on_low_signal,
                    "cache": { "read": cache_read, "write": cache_write, "ttl_s": cache_ttl_s },
                    "compact": compact
                },
                "url_count_in": urls.len(),
                "url_count_used": per_url.len(),
                "results": per_url,
                "top_chunks": top_chunks_out
            });
            if !search_steps.is_empty() {
                payload["search"] = serde_json::json!({ "steps": search_steps });
            }
            if let Some(ref k) = query_key {
                payload["query_key"] = serde_json::json!(k);
            }
            // E2E/debugging: always surface which provider actually supplied results, even when
            // the requested provider was "auto" or "merge".
            //
            // For urls-mode (no search step), we leave this unset (or set to null) rather than
            // inventing a synthetic provider name.
            if let Some(p) = search_backend_provider.as_deref() {
                payload["backend_provider"] = serde_json::json!(p);
            } else {
                payload["backend_provider"] = serde_json::Value::Null;
            }
            if agentic {
                if compact {
                    let mut stuck_events = 0usize;
                    let mut frontier_added_total = 0usize;
                    for t in &agentic_trace {
                        if t.get("stuck").and_then(|v| v.as_bool()) == Some(true) {
                            stuck_events = stuck_events.saturating_add(1);
                        }
                        frontier_added_total = frontier_added_total.saturating_add(
                            t.get("frontier_added")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0) as usize,
                        );
                    }
                    payload["agentic"] = serde_json::json!({
                        "enabled": true,
                        "trace_len": agentic_trace.len(),
                        "search_rounds": search_rounds,
                        "stuck_events": stuck_events,
                        "frontier_added_total": frontier_added_total,
                        "frontier_len_final": frontier.len(),
                        "urls_fetched": per_url.len(),
                    });
                } else {
                    payload["agentic"] = serde_json::json!({
                        "enabled": true,
                        "trace": agentic_trace
                    });
                }
            }

            if let Some(p) = search_backend_provider.as_deref() {
                // Only apply this feedback to single-provider choices (not "merge").
                if (p == "brave" || p == "tavily") && total_urls_ok > 0 {
                    let hard_junk = hard_junk_urls > 0;
                    let soft_junk = Self::compute_search_junk_label(
                        hard_junk_urls,
                        soft_junk_urls,
                        total_urls_ok,
                    );
                    let junk = hard_junk || soft_junk;
                    self.stats_set_last_search_outcome_junk_level_qk(
                        p,
                        junk,
                        hard_junk,
                        query_key.as_deref(),
                    );
                }
            }
            add_envelope_fields(&mut payload, "web_search_extract", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(
            description = "Cache-only search: scan WEBPIPE_CACHE_DIR -> extract -> top chunks (bounded)"
        )]
        async fn web_cache_search_extract(
            &self,
            params: Parameters<Option<WebCacheSearchExtractArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("web_cache_search_extract");
            let t0 = std::time::Instant::now();

            let query = args.query.clone().unwrap_or_default().trim().to_string();
            if query.is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": args.query.unwrap_or_default(),
                    "error": error_obj(ErrorCode::InvalidParams, "query must be a non-empty string", "Pass a non-empty `query`."),
                });
                add_envelope_fields(
                    &mut payload,
                    "web_cache_search_extract",
                    t0.elapsed().as_millis(),
                );
                return Ok(tool_result(payload));
            }

            let Some(cache_dir) = cache_dir_from_env() else {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "error": error_obj(
                        ErrorCode::NotConfigured,
                        "WEBPIPE_CACHE_DIR is not set",
                        "Set WEBPIPE_CACHE_DIR to enable cache-only search."
                    ),
                });
                add_envelope_fields(
                    &mut payload,
                    "web_cache_search_extract",
                    t0.elapsed().as_millis(),
                );
                return Ok(tool_result(payload));
            };

            let max_docs = args.max_docs.unwrap_or(50).min(500);
            let max_scan_entries = args.max_scan_entries.unwrap_or(2000).min(20000);
            let max_chars = args.max_chars.unwrap_or(20_000).min(200_000);
            let max_bytes = args.max_bytes.unwrap_or(5_000_000);
            let width = args.width.unwrap_or(100).clamp(20, 240);
            let top_chunks = args.top_chunks.unwrap_or(5).min(50);
            let max_chunk_chars = args.max_chunk_chars.unwrap_or(500).min(5_000);
            let include_structure = args.include_structure.unwrap_or(false);
            let max_outline_items = args.max_outline_items.unwrap_or(25).min(200);
            let max_blocks = args.max_blocks.unwrap_or(40).min(200);
            let max_block_chars = args.max_block_chars.unwrap_or(400).min(2000);
            let include_text = args.include_text.unwrap_or(false);
            let semantic_rerank = args.semantic_rerank.unwrap_or(false);
            let semantic_top_k = args.semantic_top_k.unwrap_or(5).min(50);
            let compact = args.compact.unwrap_or(true);

            let cache_dir_s = cache_dir.to_string_lossy().to_string();
            let query_s = query.clone();
            let cache_dir2 = cache_dir.clone();
            let r = tokio::task::spawn_blocking(move || {
                webpipe_local::cache_search::cache_search_extract(
                    &cache_dir2,
                    &query_s,
                    max_docs,
                    max_chars,
                    max_bytes,
                    width,
                    top_chunks,
                    max_chunk_chars,
                    include_structure,
                    max_outline_items,
                    max_blocks,
                    max_block_chars,
                    include_text,
                    max_scan_entries,
                )
            })
            .await
            .map_err(|e| {
                McpError::internal_error(format!("cache search task failed: {e}"), None)
            })?;

            let mut results_json =
                serde_json::to_value(&r.results).unwrap_or_else(|_| serde_json::json!([]));
            if semantic_rerank {
                // Probe once so we can summarize status at top-level (and avoid repeated work).
                let sem_probe = webpipe_local::semantic::semantic_rerank_chunks(&query, &[], 1);
                let sem_probe_is_unavailable = !sem_probe.ok
                    && sem_probe.warnings.iter().any(|w| {
                        *w == "semantic_backend_not_configured" || *w == "semantic_feature_disabled"
                    });
                let sem_probe_json = serde_json::json!(sem_probe.clone());
                if let Some(arr) = results_json.as_array_mut() {
                    for one in arr {
                        let chunks = one
                            .get("chunks")
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default();
                        let mut cands: Vec<(usize, usize, String)> = Vec::new();
                        for c in chunks {
                            let sc =
                                c.get("start_char").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                            let ec =
                                c.get("end_char").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                            let txt = c
                                .get("text")
                                .and_then(|x| x.as_str())
                                .unwrap_or("")
                                .to_string();
                            cands.push((sc, ec, txt));
                        }
                        // Keep per-doc field for back-compat, but when unavailable reuse the probe.
                        let sem = if sem_probe_is_unavailable {
                            sem_probe_json.clone()
                        } else {
                            serde_json::json!(webpipe_local::semantic::semantic_rerank_chunks(
                                &query,
                                &cands,
                                semantic_top_k
                            ))
                        };
                        one.as_object_mut()
                            .map(|m| m.insert("semantic".to_string(), sem));
                    }
                }
            }
            // Unify shape with web_search_extract: add an `extract` object (without removing existing fields).
            if let Some(arr) = results_json.as_array_mut() {
                for one in arr {
                    let engine = one
                        .get("extraction_engine")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let text_chars = one
                        .get("text_chars")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let text_truncated = one
                        .get("text_truncated")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    let chunks = one
                        .get("chunks")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([]));
                    let structure = one.get("structure").cloned();
                    let mut ex = serde_json::json!({
                        "engine": engine,
                        "width": width,
                        "max_chars": max_chars,
                        "text_chars": text_chars,
                        "text_truncated": text_truncated,
                        "top_chunks": top_chunks,
                        "max_chunk_chars": max_chunk_chars,
                        "chunks": chunks,
                    });
                    if include_structure {
                        if let Some(s) = structure {
                            ex["structure"] = s;
                        }
                    }
                    one.as_object_mut()
                        .map(|m| m.insert("extract".to_string(), ex));
                }
            }

            // Add per-doc warning_codes + warning_hints (additive).
            if let Some(arr) = results_json.as_array_mut() {
                for one in arr {
                    if let Some(ws) = one.get("warnings").and_then(|v| v.as_array()) {
                        let mut wv: Vec<&'static str> = Vec::new();
                        for x in ws {
                            if let Some(s) = x.as_str() {
                                // warning strings in cache search are expected to be static codes.
                                // If a non-static string appears, skip it (do not allocate new statics).
                                // This keeps warning_codes bounded + stable.
                                if let Some(s2) = match s {
                                    "cache_doc_reused" => Some("cache_doc_reused"),
                                    "text_truncated_by_max_chars" => {
                                        Some("text_truncated_by_max_chars")
                                    }
                                    "text_truncated_by_max_text_chars" => {
                                        Some("text_truncated_by_max_text_chars")
                                    }
                                    _ => None,
                                } {
                                    wv.push(s2);
                                }
                            }
                        }
                        if !wv.is_empty() {
                            let codes = warning_codes_from(&wv);
                            if let Some(m) = one.as_object_mut() {
                                m.insert(
                                    "warning_codes".to_string(),
                                    serde_json::json!(codes.clone()),
                                );
                                m.insert("warning_hints".to_string(), warning_hints_from(&codes));
                            }
                        }
                    }
                }
            }

            // Optional compact mode: reduce duplication by retaining only key fields + `extract`.
            if compact {
                if let Some(arr) = results_json.as_array_mut() {
                    for one in arr {
                        let mut out = serde_json::Map::new();
                        for k in [
                            "url",
                            "final_url",
                            "status",
                            "content_type",
                            "bytes",
                            "score",
                            "fetched_at_epoch_s",
                            "extraction_engine",
                            "warnings",
                            "warning_codes",
                            "warning_hints",
                            "semantic",
                            "extract",
                        ] {
                            if let Some(v) = one.get(k) {
                                out.insert(k.to_string(), v.clone());
                            }
                        }
                        *one = serde_json::Value::Object(out);
                    }
                }
            }

            let mut payload = serde_json::json!({
                "ok": r.ok,
                "query": query,
                "query_key": Self::query_key(&query),
                "cache_dir": cache_dir_s,
                "scanned_entries": r.scanned_entries,
                "selected_docs": r.selected_docs,
                "results": results_json,
                "request": {
                    "max_docs": max_docs,
                    "max_scan_entries": max_scan_entries,
                    "max_chars": max_chars,
                    "max_bytes": max_bytes,
                    "width": width,
                    "top_chunks": top_chunks,
                    "max_chunk_chars": max_chunk_chars,
                    "include_text": include_text,
                    "include_structure": include_structure,
                    "max_outline_items": max_outline_items,
                    "max_blocks": max_blocks,
                    "max_block_chars": max_block_chars,
                    "semantic_rerank": semantic_rerank,
                    "semantic_top_k": semantic_top_k,
                    "compact": compact
                }
            });
            if semantic_rerank {
                let sem_probe = webpipe_local::semantic::semantic_rerank_chunks(&query, &[], 1);
                let codes: Vec<&'static str> = sem_probe
                    .warnings
                    .iter()
                    .copied()
                    .map(normalize_warning_code)
                    .collect();
                if !codes.is_empty() && !sem_probe.ok {
                    payload["semantic_status"] = serde_json::json!({
                        "ok": false,
                        "backend": sem_probe.backend,
                        "model_id": sem_probe.model_id,
                        "warning_codes": codes.clone(),
                        "warning_hints": warning_hints_from(&codes),
                    });
                }
            }
            if !r.warnings.is_empty() {
                payload["warnings"] = serde_json::json!(r.warnings);
                let codes = warning_codes_from(&r.warnings);
                payload["warning_codes"] = serde_json::json!(codes.clone());
                payload["warning_hints"] = warning_hints_from(&codes);
            }
            add_envelope_fields(
                &mut payload,
                "web_cache_search_extract",
                t0.elapsed().as_millis(),
            );
            Ok(tool_result(payload))
        }

        #[tool(
            description = "Agentic research: search -> fetch/extract -> Perplexity deep research synthesis (bounded)"
        )]
        async fn web_deep_research(
            &self,
            params: Parameters<Option<WebDeepResearchArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("web_deep_research");
            let t0 = std::time::Instant::now();

            let query = args.query.trim().to_string();
            if query.is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": args.query,
                    "error": error_obj(ErrorCode::InvalidParams, "query must be a non-empty string", "Pass a question/prompt as `query`."),
                });
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let max_results_user = args.max_results.is_some();
            let max_urls_user = args.max_urls.is_some();
            let max_chars_user = args.max_chars.is_some();
            let top_chunks_user = args.top_chunks.is_some();
            let max_chunk_chars_user = args.max_chunk_chars.is_some();

            let mut max_results = args.max_results.unwrap_or(5).clamp(1, 20);
            let mut max_urls = args.max_urls.unwrap_or(3).clamp(1, 10);
            let timeout_ms = args.timeout_ms.unwrap_or(20_000);
            let max_bytes = args.max_bytes.unwrap_or(5_000_000);
            let width = args.width.unwrap_or(100).clamp(20, 240);
            let mut max_chars = args.max_chars.unwrap_or(20_000).min(200_000);
            let mut top_chunks = args.top_chunks.unwrap_or(5).min(50);
            let mut max_chunk_chars = args.max_chunk_chars.unwrap_or(500).min(5_000);
            let include_links = args.include_links.unwrap_or(false);
            let max_links = args.max_links.unwrap_or(25).min(500);
            let include_evidence = args.include_evidence.unwrap_or(true);
            let synthesize = args.synthesize.unwrap_or(true);
            let audit = args.audit.unwrap_or(false);

            if audit && !include_evidence {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": args.model.unwrap_or_else(|| "sonar-deep-research".to_string()),
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "audit=true requires include_evidence=true",
                        "Set include_evidence=true (audit mode requires returning the evidence pack)."
                    ),
                });
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if audit && args.search_mode.is_some() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": args.model.unwrap_or_else(|| "sonar-deep-research".to_string()),
                    "error": error_obj(
                        ErrorCode::NotSupported,
                        "audit=true cannot be combined with search_mode",
                        "Unset search_mode (provider-side browsing is not audit-friendly), or set audit=false."
                    ),
                });
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let exploration = args
                .exploration
                .clone()
                .unwrap_or_else(|| "balanced".to_string());
            match exploration.as_str() {
                "balanced" => {}
                "wide" => {
                    if !max_results_user {
                        max_results = 10;
                    }
                    if !max_urls_user {
                        max_urls = 5;
                    }
                    if !max_chars_user {
                        max_chars = 10_000;
                    }
                    if !top_chunks_user {
                        top_chunks = 3;
                    }
                    if !max_chunk_chars_user {
                        max_chunk_chars = 400;
                    }
                }
                "deep" => {
                    if !max_results_user {
                        max_results = 5;
                    }
                    if !max_urls_user {
                        max_urls = 2;
                    }
                    if !max_chars_user {
                        max_chars = 60_000;
                    }
                    if !top_chunks_user {
                        top_chunks = 10;
                    }
                    if !max_chunk_chars_user {
                        max_chunk_chars = 800;
                    }
                }
                _ => {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "query": args.query,
                        "exploration": exploration,
                        "error": error_obj(
                            ErrorCode::InvalidParams,
                            "unknown exploration preset",
                            "Allowed exploration values: balanced, wide, deep"
                        ),
                    });
                    add_envelope_fields(
                        &mut payload,
                        "web_deep_research",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }
            }

            let provider = args.provider.unwrap_or_else(|| "auto".to_string());
            let auto_mode = args
                .auto_mode
                .clone()
                .unwrap_or_else(|| "fallback".to_string());
            let selection_mode = args
                .selection_mode
                .clone()
                .unwrap_or_else(|| "score".to_string());
            let model = args
                .model
                .unwrap_or_else(|| "sonar-deep-research".to_string());
            let fetch_backend = args.fetch_backend.unwrap_or_else(|| "local".to_string());
            let no_network = args.no_network.unwrap_or(false);
            if selection_mode.as_str() != "score" && selection_mode.as_str() != "pareto" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": model,
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown selection_mode",
                        "Allowed selection_mode values: score, pareto"
                    ),
                });
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let max_answer_chars = args.max_answer_chars.unwrap_or(20_000).min(200_000);
            let reasoning_effort = args.reasoning_effort.clone();
            let max_tokens = args.max_tokens;
            let temperature = args.temperature;
            let top_p = args.top_p;
            let llm_backend = args
                .llm_backend
                .clone()
                .unwrap_or_else(|| "auto".to_string());
            let llm_model = args.llm_model.clone();

            // 1) Gather evidence with our own bounded pipeline (so even if Perplexity is flaky, we can inspect what we fed it).
            let firecrawl_fallback_on_empty_extraction = fetch_backend == "local"
                && (has_env("WEBPIPE_FIRECRAWL_API_KEY") || has_env("FIRECRAWL_API_KEY"));

            if no_network && args.urls.is_none() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": model,
                    "error": error_obj(
                        ErrorCode::NotSupported,
                        "no_network=true requires urls=[...] (offline evidence only)",
                        "Pass urls=[...] to run offline, or set no_network=false."
                    ),
                });
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if no_network && args.search_mode.is_some() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": model,
                    "error": error_obj(
                        ErrorCode::NotSupported,
                        "no_network=true cannot be combined with search_mode",
                        "Unset search_mode (Perplexity browsing is network-only), or set no_network=false."
                    ),
                });
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let evidence_r = self
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some(query.clone()),
                    urls: args.urls.clone(),
                    provider: Some(provider.clone()),
                    auto_mode: Some(auto_mode.clone()),
                    selection_mode: Some(selection_mode.clone()),
                    url_selection_mode: Some("query_rank".to_string()),
                    fetch_backend: Some(fetch_backend.clone()),
                    no_network: Some(no_network),
                    firecrawl_fallback_on_empty_extraction: Some(
                        firecrawl_fallback_on_empty_extraction,
                    ),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(max_results),
                    max_urls: Some(max_urls),
                    timeout_ms: Some(timeout_ms),
                    max_bytes: Some(max_bytes),
                    width: Some(width),
                    max_chars: Some(max_chars),
                    top_chunks: Some(top_chunks),
                    max_chunk_chars: Some(max_chunk_chars),
                    include_links: Some(include_links),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(max_links),
                    include_text: Some(false),
                    cache_read: Some(true),
                    cache_write: Some(true),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(false),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: None,
                    agentic_frontier_max: None,
                    planner_max_calls: None,
                    retry_on_truncation: None,
                    truncation_retry_max_bytes: None,
                    // The web_deep_research tool builds its own compact evidence pack below; keep
                    // the upstream evidence call compact by default to avoid huge payloads.
                    compact: Some(true),
                }))
                .await?;

            let evidence = payload_from_result(&evidence_r);
            if evidence.get("ok").and_then(|v| v.as_bool()) != Some(true) {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": model,
                    "error": error_obj(
                        ErrorCode::SearchFailed,
                        "failed to gather evidence via web_search_extract",
                        "Inspect the `evidence` field for details; try a different provider or reduce max_urls/max_results."
                    ),
                    "request": {
                        "provider": provider,
                        "auto_mode": auto_mode,
                        "selection_mode": selection_mode,
                        "fetch_backend": fetch_backend,
                        "max_results": max_results,
                        "max_urls": max_urls
                    }
                });
                if include_evidence {
                    payload["evidence"] = evidence;
                }
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            // Note: we do not run a second hidden evidence pass here. If Firecrawl is available and
            // fetch_backend="local", the per-URL fallback is handled inside web_search_extract.

            // Build a compact evidence pack for the LLM: keep only URLs + top chunks, never full HTML.
            let mut compact_results = Vec::new();
            if let Some(rs) = evidence.get("results").and_then(|v| v.as_array()) {
                for r in rs {
                    compact_results.push(serde_json::json!({
                        "url": r.get("url").cloned().unwrap_or(serde_json::Value::Null),
                        "final_url": r.get("final_url").cloned().unwrap_or(serde_json::Value::Null),
                        "ok": r.get("ok").cloned().unwrap_or(serde_json::Value::Null),
                        "fetch_backend": r.get("fetch_backend").cloned().unwrap_or(serde_json::Value::Null),
                        "status": r.get("status").cloned().unwrap_or(serde_json::Value::Null),
                        "content_type": r.get("content_type").cloned().unwrap_or(serde_json::Value::Null),
                        "bytes": r.get("bytes").cloned().unwrap_or(serde_json::Value::Null),
                        "extract": {
                            "text_chars": r.get("extract").and_then(|e| e.get("text_chars")).cloned().unwrap_or(serde_json::Value::Null),
                            "chunks": r.get("extract").and_then(|e| e.get("chunks")).cloned().unwrap_or(serde_json::Value::Null),
                        },
                        "warnings": r.get("warnings").cloned().unwrap_or(serde_json::Value::Null),
                    }));
                }
            }
            let evidence_for_llm = serde_json::json!({
                "schema_version": 1,
                "kind": "webpipe_evidence_pack",
                "question": query,
                "question_key": Self::query_key(&query),
                "selection": { "provider": provider.clone(), "auto_mode": auto_mode.clone(), "selection_mode": selection_mode.clone(), "fetch_backend": fetch_backend.clone(), "firecrawl_fallback_on_empty_extraction": firecrawl_fallback_on_empty_extraction },
                "top_chunks": evidence.get("top_chunks").cloned().unwrap_or_else(|| serde_json::json!([])),
                "results": compact_results,
                "firecrawl_retry": serde_json::Value::Null,
                "arxiv": serde_json::Value::Null
            });

            // Optionally add bounded ArXiv evidence.
            fn looks_paper_like(q: &str) -> bool {
                let s = q.to_lowercase();
                if s.contains("arxiv") || s.contains("doi:") || s.contains("doi ") {
                    return true;
                }
                let needles = [
                    "paper",
                    "preprint",
                    "abstract",
                    "proceedings",
                    "journal",
                    "conference",
                    "icml",
                    "neurips",
                    "iclr",
                    "acl",
                    "emnlp",
                    "naacl",
                    "cvpr",
                ];
                if needles.iter().any(|n| s.contains(n)) {
                    return true;
                }
                // ArXiv id-ish patterns.
                let chars: Vec<char> = s.chars().collect();
                for i in 0..chars.len().saturating_sub(9) {
                    // yyyy.xxxxx (very rough)
                    let a = chars.get(i..i + 4).unwrap_or(&[]);
                    let b = chars.get(i + 4).copied().unwrap_or('_');
                    let c = chars.get(i + 5..i + 9).unwrap_or(&[]);
                    if a.iter().all(|ch| ch.is_ascii_digit())
                        && b == '.'
                        && c.iter().all(|ch| ch.is_ascii_digit())
                    {
                        return true;
                    }
                }
                false
            }

            let arxiv_mode = args
                .arxiv_mode
                .clone()
                .unwrap_or_else(|| "auto".to_string());
            let arxiv_enabled = match arxiv_mode.as_str() {
                "off" => false,
                "on" => true,
                "auto" => looks_paper_like(&query),
                _ => {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "query": query,
                        "model": model,
                        "arxiv_mode": arxiv_mode,
                        "error": error_obj(
                            ErrorCode::InvalidParams,
                            "unknown arxiv_mode",
                            "Allowed arxiv_mode values: off, auto, on"
                        ),
                    });
                    add_envelope_fields(
                        &mut payload,
                        "web_deep_research",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }
            };

            let arxiv_block = if arxiv_enabled && !no_network {
                let max_papers = args.arxiv_max_papers.unwrap_or(3).clamp(1, 10);
                let categories = args.arxiv_categories.clone().unwrap_or_default();
                let years = args.arxiv_years.clone().unwrap_or_default();
                let arxiv_timeout_ms = args.arxiv_timeout_ms.unwrap_or(8_000).min(30_000);

                match webpipe_local::arxiv::arxiv_search(
                    self.http.clone(),
                    query.clone(),
                    categories,
                    years,
                    1,
                    max_papers,
                    arxiv_timeout_ms,
                )
                .await
                {
                    Ok(r) => {
                        let mut papers = Vec::new();
                        for p in r.papers.into_iter().take(max_papers) {
                            let (summary, _n, _clipped) =
                                Self::truncate_to_chars(&p.summary, max_chunk_chars.min(2_000));
                            papers.push(serde_json::json!({
                                "arxiv_id": p.arxiv_id,
                                "url": p.url,
                                "pdf_url": p.pdf_url,
                                "title": p.title,
                                "summary": summary,
                                "published": p.published,
                                "updated": p.updated,
                                "authors": p.authors,
                                "primary_category": p.primary_category,
                                "categories": p.categories,
                            }));
                        }
                        serde_json::json!({
                            "ok": true,
                            "query": r.query,
                            "page": r.page,
                            "per_page": r.per_page,
                            "total_results": r.total_results,
                            "warnings": r.warnings,
                            "papers": papers,
                        })
                    }
                    Err(e) => {
                        let msg = e.to_string();
                        serde_json::json!({
                            "ok": false,
                            "error": msg,
                        })
                    }
                }
            } else if arxiv_enabled && no_network {
                serde_json::json!({
                    "ok": false,
                    "error": "skipped (no_network=true)",
                })
            } else {
                serde_json::Value::Null
            };

            let mut evidence_for_llm = evidence_for_llm;
            evidence_for_llm["arxiv"] = arxiv_block;
            let evidence_pack = evidence_for_llm.clone();

            // Keep prompt bounded: pass only URLs + top chunks, never full HTML.
            let sys = "You are a careful research assistant. Use the provided evidence pack. Cite sources by URL. If evidence is insufficient, say so.";
            let user = serde_json::json!({
                "question": query,
                "evidence": evidence_for_llm,
            })
            .to_string();
            let (user, _n_user, _user_clipped) = Self::truncate_to_chars(&user, 20_000);

            if !synthesize {
                let mut payload = serde_json::json!({
                    "ok": true,
                    "query": query,
                    "query_key": Self::query_key(&query),
                    "model": model,
                    "request": {
                        "provider": provider,
                        "auto_mode": auto_mode,
                        "selection_mode": selection_mode,
                        "fetch_backend": fetch_backend,
                        "no_network": no_network,
                        "max_results": max_results,
                        "max_urls": max_urls,
                        "timeout_ms": timeout_ms,
                        "max_bytes": max_bytes,
                        "width": width,
                        "max_chars": max_chars,
                        "top_chunks": top_chunks,
                        "max_chunk_chars": max_chunk_chars,
                        "include_links": include_links,
                        "max_links": max_links,
                        "exploration": exploration,
                        "arxiv_mode": arxiv_mode,
                        "arxiv_max_papers": args.arxiv_max_papers.unwrap_or(3).clamp(1, 10),
                        "synthesize": false,
                    },
                    "answer": {
                        "text": "",
                        "skipped": true,
                        "reason": "synthesize=false",
                    },
                });
                if include_evidence {
                    payload["evidence"] = evidence_for_llm;
                }
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            // LLM backend selection.
            // - In no_network mode, network-backed providers must not be called.
            // - “Local network” (localhost only) is allowed for explicit local backends.
            if no_network && llm_backend.as_str() == "perplexity" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": model,
                    "error": error_obj(
                        ErrorCode::NotSupported,
                        "llm_backend=perplexity cannot be used with no_network=true",
                        "Use llm_backend=\"ollama\" or \"openai_compat\" for local synthesis, or set no_network=false."
                    ),
                });
                if include_evidence {
                    payload["evidence"] = evidence;
                }
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let selected_backend: &str = match llm_backend.as_str() {
                "perplexity" => "perplexity",
                "ollama" => "ollama",
                "openai_compat" => "openai_compat",
                "auto" => {
                    // Prefer Perplexity when it's available and we're not in strict offline mode.
                    if !no_network
                        && (has_env("WEBPIPE_PERPLEXITY_API_KEY") || has_env("PERPLEXITY_API_KEY"))
                    {
                        "perplexity"
                    } else if has_env("WEBPIPE_OPENAI_COMPAT_BASE_URL") {
                        "openai_compat"
                    } else if has_env("WEBPIPE_OLLAMA_ENABLE")
                        && std::env::var("WEBPIPE_OLLAMA_ENABLE")
                            .ok()
                            .is_some_and(|v| v.trim().eq_ignore_ascii_case("true"))
                    {
                        "ollama"
                    } else {
                        "none"
                    }
                }
                other => {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "query": query,
                        "model": model,
                        "error": error_obj(
                            ErrorCode::InvalidParams,
                            format!("unknown llm_backend: {other}"),
                            "Allowed llm_backend values: auto, perplexity, ollama, openai_compat"
                        ),
                    });
                    if include_evidence {
                        payload["evidence"] = evidence;
                        payload["evidence_pack"] = evidence_pack.clone();
                    }
                    add_envelope_fields(
                        &mut payload,
                        "web_deep_research",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }
            };

            if selected_backend == "none" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "query": query,
                    "model": model,
                    "error": error_obj(
                        ErrorCode::NotConfigured,
                        "no LLM backend configured for synthesis",
                        "Configure WEBPIPE_PERPLEXITY_API_KEY, or set WEBPIPE_OPENAI_COMPAT_BASE_URL (and llm_model / WEBPIPE_OPENAI_COMPAT_MODEL), or enable Ollama via WEBPIPE_OLLAMA_ENABLE=true."
                    ),
                    "request": { "llm_backend": llm_backend },
                });
                if include_evidence {
                    payload["evidence"] = evidence;
                }
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            // 2) Synthesize via selected backend.
            let mut deep_warnings: Vec<&'static str> = Vec::new();
            let mut citations: Vec<String> = Vec::new();
            if let Some(arr) = evidence.get("top_chunks").and_then(|v| v.as_array()) {
                for c in arr {
                    if let Some(u) = c.get("url").and_then(|x| x.as_str()) {
                        if !citations.iter().any(|x| x == u) {
                            citations.push(u.to_string());
                        }
                    }
                }
            }

            if selected_backend == "ollama" {
                let llm_t0 = std::time::Instant::now();
                let ollama = match webpipe_local::ollama::OllamaClient::from_env(self.http.clone())
                {
                    Ok(c) => c,
                    Err(e) => {
                        self.stats_record_llm_backend(
                            "ollama",
                            false,
                            llm_t0.elapsed().as_millis() as u64,
                            Some(&e.to_string()),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(
                                ErrorCode::NotConfigured,
                                e.to_string(),
                                "Enable Ollama synthesis by setting WEBPIPE_OLLAMA_ENABLE=true (and ensure Ollama is running)."
                            ),
                            "request": { "llm_backend": llm_backend },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                            payload["evidence_pack"] = evidence_pack.clone();
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                };

                if no_network {
                    // Best-effort safety: require localhost base URL when no_network=true.
                    let b = ollama.base_url().to_string();
                    let ok_local =
                        b.contains("127.0.0.1") || b.contains("localhost") || b.contains("[::1]");
                    if !ok_local {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(
                                ErrorCode::NotSupported,
                                "no_network=true requires Ollama to be localhost",
                                "Set WEBPIPE_OLLAMA_BASE_URL to http://127.0.0.1:11434 (or set no_network=false)."
                            ),
                            "request": { "llm_backend": llm_backend, "ollama_base_url": b },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                            payload["evidence_pack"] = evidence_pack.clone();
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                }

                deep_warnings.push("llm_ollama_used");
                let answer = match ollama.chat(sys, &user, timeout_ms).await {
                    Ok(s) => s,
                    Err(e) => {
                        self.stats_record_llm_backend(
                            "ollama",
                            false,
                            llm_t0.elapsed().as_millis() as u64,
                            Some(&e.to_string()),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(ErrorCode::ProviderUnavailable, e.to_string(), "Ollama synthesis failed. Check that Ollama is running and the model name exists."),
                            "request": { "llm_backend": "ollama" },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                            payload["evidence_pack"] = evidence_pack.clone();
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                };
                self.stats_record_llm_backend(
                    "ollama",
                    true,
                    llm_t0.elapsed().as_millis() as u64,
                    None,
                );
                let (answer, _n, clipped) = Self::truncate_to_chars(&answer, max_answer_chars);
                let mut payload = serde_json::json!({
                    "ok": true,
                    "provider": "ollama",
                    "query": query,
                    "request": { "llm_backend": llm_backend, "timeout_ms": timeout_ms, "no_network": no_network },
                    "answer": { "text": answer, "truncated": clipped, "citations": citations },
                });
                if !deep_warnings.is_empty() {
                    payload["warnings"] = serde_json::json!(deep_warnings);
                    let codes = warning_codes_from(&deep_warnings);
                    payload["warning_codes"] = serde_json::json!(codes.clone());
                    payload["warning_hints"] = warning_hints_from(&codes);
                    self.stats_record_warnings(&deep_warnings);
                }
                if include_evidence {
                    payload["evidence"] = evidence;
                    payload["evidence_pack"] = evidence_pack.clone();
                }
                if let Some(now) = args.now_epoch_s {
                    payload["generated_at_epoch_s"] = serde_json::json!(now);
                }
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            if selected_backend == "openai_compat" {
                let llm_t0 = std::time::Instant::now();
                let client = match webpipe_local::openai_compat::OpenAiCompatClient::from_env(
                    self.http.clone(),
                    llm_model.clone(),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        self.stats_record_llm_backend(
                            "openai_compat",
                            false,
                            llm_t0.elapsed().as_millis() as u64,
                            Some(&e.to_string()),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(
                                ErrorCode::NotConfigured,
                                e.to_string(),
                                "Set WEBPIPE_OPENAI_COMPAT_BASE_URL and (llm_model or WEBPIPE_OPENAI_COMPAT_MODEL)."
                            ),
                            "request": { "llm_backend": llm_backend },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                            payload["evidence_pack"] = evidence_pack.clone();
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                };

                if no_network {
                    let b = client.base_url().to_string();
                    let ok_local =
                        b.contains("127.0.0.1") || b.contains("localhost") || b.contains("[::1]");
                    if !ok_local {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(
                                ErrorCode::NotSupported,
                                "no_network=true requires openai_compat base URL to be localhost",
                                "Set WEBPIPE_OPENAI_COMPAT_BASE_URL to http://127.0.0.1:<port> (or set no_network=false)."
                            ),
                            "request": { "llm_backend": llm_backend, "openai_compat_base_url": b },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                            payload["evidence_pack"] = evidence_pack.clone();
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                }

                deep_warnings.push("llm_openai_compat_used");
                let answer = match client
                    .chat(sys, &user, timeout_ms, max_tokens, temperature, top_p)
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        self.stats_record_llm_backend(
                            "openai_compat",
                            false,
                            llm_t0.elapsed().as_millis() as u64,
                            Some(&e.to_string()),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(ErrorCode::ProviderUnavailable, e.to_string(), "OpenAI-compatible synthesis failed. Check the base URL, model name, and auth."),
                            "request": { "llm_backend": "openai_compat" },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                };
                self.stats_record_llm_backend(
                    "openai_compat",
                    true,
                    llm_t0.elapsed().as_millis() as u64,
                    None,
                );
                let (answer, _n, clipped) = Self::truncate_to_chars(&answer, max_answer_chars);
                let mut payload = serde_json::json!({
                    "ok": true,
                    "provider": "openai_compat",
                    "query": query,
                    "request": { "llm_backend": llm_backend, "timeout_ms": timeout_ms, "no_network": no_network },
                    "answer": { "text": answer, "truncated": clipped, "citations": citations },
                });
                if !deep_warnings.is_empty() {
                    payload["warnings"] = serde_json::json!(deep_warnings);
                    let codes = warning_codes_from(&deep_warnings);
                    payload["warning_codes"] = serde_json::json!(codes.clone());
                    payload["warning_hints"] = warning_hints_from(&codes);
                }
                if include_evidence {
                    payload["evidence"] = evidence;
                    payload["evidence_pack"] = evidence_pack.clone();
                }
                if let Some(now) = args.now_epoch_s {
                    payload["generated_at_epoch_s"] = serde_json::json!(now);
                }
                add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            // Otherwise: Perplexity API path (existing behavior).
            let pplx = match webpipe_local::perplexity::PerplexityClient::from_env(
                self.http.clone(),
            ) {
                Ok(c) => c,
                Err(e) => {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "query": query,
                        "model": model,
                        "error": error_obj(ErrorCode::NotConfigured, e.to_string(), "Set WEBPIPE_PERPLEXITY_API_KEY (or PERPLEXITY_API_KEY) in the webpipe MCP server environment."),
                        "request": { "llm_backend": llm_backend },
                    });
                    if include_evidence {
                        payload["evidence"] = evidence;
                        payload["evidence_pack"] = evidence_pack.clone();
                    }
                    add_envelope_fields(
                        &mut payload,
                        "web_deep_research",
                        t0.elapsed().as_millis(),
                    );
                    return Ok(tool_result(payload));
                }
            };

            // By default, disable provider-side browsing. We already gather an evidence pack ourselves;
            // letting the provider browse makes results less inspectable and can be much more expensive.
            //
            // If the provider rejects "off", we retry once without `search_mode` (and surface a warning).
            let mut effective_search_mode = args.search_mode.clone();
            if !no_network && effective_search_mode.is_none() {
                effective_search_mode = Some("off".to_string());
            }

            let req = webpipe_local::perplexity::ChatCompletionsRequest {
                model: model.clone(),
                messages: vec![
                    webpipe_local::perplexity::Message {
                        role: "system".to_string(),
                        content: sys.to_string(),
                    },
                    webpipe_local::perplexity::Message {
                        role: "user".to_string(),
                        content: user,
                    },
                ],
                max_tokens,
                temperature,
                top_p,
                search_mode: effective_search_mode.clone(),
                reasoning_effort: reasoning_effort.clone(),
            };

            let resp = match pplx.chat_completions(req.clone()).await {
                Ok(r) => r,
                Err(e) => {
                    // Retry once if we set the default `search_mode="off"` and the provider rejected it.
                    // (Do not retry if user explicitly set search_mode.)
                    let defaulted_off = args.search_mode.is_none()
                        && effective_search_mode.as_deref() == Some("off")
                        && !no_network;
                    if defaulted_off {
                        deep_warnings.push("perplexity_search_mode_off_rejected");
                        if audit {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "query": query,
                                "model": model,
                                "error": error_obj(
                                    ErrorCode::NotSupported,
                                    "perplexity rejected search_mode=\"off\" (audit mode cannot proceed)",
                                    "Set llm_backend to a local backend (ollama/openai_compat), or set audit=false."
                                ),
                                "request": {
                                    "provider": provider,
                                    "auto_mode": auto_mode,
                                    "selection_mode": selection_mode,
                                    "fetch_backend": fetch_backend,
                                    "max_results": max_results,
                                    "max_urls": max_urls
                                },
                                "warnings": ["perplexity_search_mode_off_rejected"]
                            });
                            let codes =
                                warning_codes_from(&["perplexity_search_mode_off_rejected"]);
                            payload["warning_codes"] = serde_json::json!(codes.clone());
                            payload["warning_hints"] = warning_hints_from(&codes);
                            if include_evidence {
                                payload["evidence"] = evidence;
                                payload["evidence_pack"] = evidence_pack.clone();
                            }
                            add_envelope_fields(
                                &mut payload,
                                "web_deep_research",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                        let mut req2 = req;
                        req2.search_mode = None;
                        if let Ok(r2) = pplx.chat_completions(req2).await {
                            r2
                        } else {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "query": query,
                                "model": model,
                                "error": error_obj(ErrorCode::ProviderUnavailable, e.to_string(), "Retry later or reduce scope. You can still use web_search_extract output as evidence."),
                                "request": {
                                    "provider": provider,
                                    "auto_mode": auto_mode,
                                    "selection_mode": selection_mode,
                                    "fetch_backend": fetch_backend,
                                    "max_results": max_results,
                                    "max_urls": max_urls,
                                },
                            });
                            if include_evidence {
                                payload["evidence"] = evidence;
                            }
                            add_envelope_fields(
                                &mut payload,
                                "web_deep_research",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                    } else {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "query": query,
                            "model": model,
                            "error": error_obj(ErrorCode::ProviderUnavailable, e.to_string(), "Retry later or reduce scope. You can still use web_search_extract output as evidence."),
                            "request": {
                                "provider": provider,
                                "auto_mode": auto_mode,
                                "selection_mode": selection_mode,
                                "fetch_backend": fetch_backend,
                                "max_results": max_results,
                                "max_urls": max_urls,
                            },
                        });
                        if include_evidence {
                            payload["evidence"] = evidence;
                        }
                        add_envelope_fields(
                            &mut payload,
                            "web_deep_research",
                            t0.elapsed().as_millis(),
                        );
                        return Ok(tool_result(payload));
                    }
                }
            };

            let answer_text = resp
                .choices
                .first()
                .map(|c| c.message.content.clone())
                .unwrap_or_default();
            let (answer, _n, clipped) = Self::truncate_to_chars(&answer_text, max_answer_chars);

            let mut payload = serde_json::json!({
                "ok": true,
                "query": query,
                "query_key": Self::query_key(&query),
                "model": model,
                "request": {
                    "provider": provider,
                    "auto_mode": auto_mode,
                    "selection_mode": selection_mode,
                    "fetch_backend": fetch_backend,
                    "no_network": no_network,
                    "max_results": max_results,
                    "max_urls": max_urls,
                    "timeout_ms": timeout_ms,
                    "max_bytes": max_bytes,
                    "width": width,
                    "max_chars": max_chars,
                    "top_chunks": top_chunks,
                    "max_chunk_chars": max_chunk_chars,
                    "include_links": include_links,
                    "max_links": max_links,
                    "search_mode": effective_search_mode,
                    "reasoning_effort": reasoning_effort,
                    "max_answer_chars": max_answer_chars,
                    "max_tokens": max_tokens,
                    "temperature": temperature,
                    "top_p": top_p
                },
                "answer": {
                    "text": answer,
                    "truncated": clipped,
                    "citations": resp.citations.unwrap_or_default()
                },
                "usage": resp.usage,
                "timings_ms": resp.timings_ms
            });
            if !deep_warnings.is_empty() {
                payload["warnings"] = serde_json::json!(deep_warnings);
                let codes = warning_codes_from(&deep_warnings);
                payload["warning_codes"] = serde_json::json!(codes.clone());
                payload["warning_hints"] = warning_hints_from(&codes);
            }
            if include_evidence {
                payload["evidence"] = evidence;
                payload["evidence_pack"] = evidence_pack.clone();
            }
            if let Some(now) = args.now_epoch_s {
                payload["generated_at_epoch_s"] = serde_json::json!(now);
            }
            add_envelope_fields(&mut payload, "web_deep_research", t0.elapsed().as_millis());
            Ok(tool_result(payload))
        }

        #[tool(
            description = "Fetch a URL (cache-first, deterministic JSON output; bounded text by default)"
        )]
        async fn web_fetch(
            &self,
            params: Parameters<Option<WebFetchArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("web_fetch");
            let include_headers = args.include_headers.unwrap_or(false);
            let include_text = args.include_text.unwrap_or(false);
            let max_text_chars = args.max_text_chars.unwrap_or(20_000).min(200_000);
            let fetch_backend = args.fetch_backend.unwrap_or_else(|| "local".to_string());
            let no_network = args.no_network.unwrap_or(false);

            let url = args.url.unwrap_or_default();
            let t0 = std::time::Instant::now();
            if url.trim().is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "url": url,
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "url must be non-empty",
                        "Pass an absolute URL like https://example.com."
                    ),
                    "request": {
                        "fetch_backend": fetch_backend,
                        "no_network": no_network,
                        "timeout_ms": args.timeout_ms.or(Some(15_000)),
                        "max_bytes": args.max_bytes.or(Some(5_000_000)),
                        "cache": { "read": args.cache_read.unwrap_or(true), "write": args.cache_write.unwrap_or(true), "ttl_s": args.cache_ttl_s },
                        "include_text": include_text,
                        "max_text_chars": max_text_chars,
                        "include_headers": include_headers
                    }
                });
                add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if fetch_backend.as_str() != "local" && fetch_backend.as_str() != "firecrawl" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "url": url,
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown fetch_backend",
                        "Allowed fetch_backends: local, firecrawl"
                    ),
                    "request": { "fetch_backend": fetch_backend }
                });
                add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            if no_network && fetch_backend == "firecrawl" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "url": url,
                    "error": error_obj(
                        ErrorCode::NotSupported,
                        "no_network=true cannot be used with fetch_backend=\"firecrawl\"",
                        "Use fetch_backend=\"local\" with a warmed WEBPIPE_CACHE_DIR, or set no_network=false."
                    ),
                    "request": { "fetch_backend": fetch_backend, "no_network": no_network }
                });
                add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            if fetch_backend == "firecrawl" {
                let timeout_ms = args.timeout_ms.unwrap_or(15_000);
                let max_age_ms = args.cache_ttl_s.map(|s| s.saturating_mul(1000));
                let fc = match webpipe_local::firecrawl::FirecrawlClient::from_env(
                    self.http.clone(),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        let msg = e.to_string();
                        self.stats_record_fetch_backend(
                            "firecrawl",
                            false,
                            t0.elapsed().as_millis() as u64,
                            Some(&msg),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(
                                ErrorCode::NotConfigured,
                                msg,
                                "Set WEBPIPE_FIRECRAWL_API_KEY (or FIRECRAWL_API_KEY) to use fetch_backend=\"firecrawl\"."
                            ),
                            "request": { "fetch_backend": fetch_backend, "timeout_ms": timeout_ms }
                        });
                        add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                };
                let r = match fc.fetch_markdown(&url, timeout_ms, max_age_ms).await {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = e.to_string();
                        self.stats_record_fetch_backend(
                            "firecrawl",
                            false,
                            t0.elapsed().as_millis() as u64,
                            Some(&msg),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(
                                ErrorCode::FetchFailed,
                                msg,
                                "Firecrawl fetch failed. If this looks transient, retry later or reduce scope."
                            ),
                            "request": { "fetch_backend": fetch_backend, "timeout_ms": timeout_ms }
                        });
                        add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                };
                let (text, n, text_clipped) = Self::truncate_to_chars(&r.markdown, max_text_chars);
                let mut warnings: Vec<&'static str> = Vec::new();
                if text_clipped {
                    warnings.push("text_truncated_by_max_text_chars");
                }

                let mut payload = serde_json::json!({
                    "ok": true,
                    "fetch_backend": "firecrawl",
                    "url": url,
                    "final_url": url,
                    "status": 200,
                    "content_type": "text/markdown",
                    "bytes": Self::approx_bytes_len(&r.markdown),
                    "truncated": false,
                    "source": "network",
                    "text_chars": n,
                    "text_truncated": text_clipped,
                    "timings_ms": {
                        "total": t0.elapsed().as_millis(),
                        "firecrawl_fetch": r.elapsed_ms
                    }
                });
                add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                payload["request"] = serde_json::json!({
                    "fetch_backend": "firecrawl",
                    "no_network": no_network,
                    "timeout_ms": timeout_ms,
                    "cache_ttl_s": args.cache_ttl_s,
                    "include_text": include_text,
                    "max_text_chars": max_text_chars,
                    "include_headers": include_headers
                });
                if include_text {
                    payload["text"] = serde_json::json!(text);
                }
                if include_headers {
                    payload["headers"] = serde_json::json!({});
                    warnings.push("headers_unavailable_for_firecrawl");
                }
                if !warnings.is_empty() {
                    payload["warnings"] = serde_json::json!(warnings);
                    let codes = warning_codes_from(&warnings);
                    payload["warning_codes"] = serde_json::json!(codes.clone());
                    payload["warning_hints"] = warning_hints_from(&codes);
                    self.stats_record_warnings(&warnings);
                }
                self.stats_record_fetch_backend(
                    "firecrawl",
                    true,
                    t0.elapsed().as_millis() as u64,
                    None,
                );
                return Ok(tool_result(payload));
            }
            let req = FetchRequest {
                url: url.clone(),
                timeout_ms: args.timeout_ms.or(Some(15_000)),
                max_bytes: args.max_bytes.or(Some(5_000_000)),
                headers: BTreeMap::new(), // filled below (after filtering)
                cache: FetchCachePolicy {
                    read: args.cache_read.unwrap_or(true) || no_network,
                    write: if no_network {
                        false
                    } else {
                        args.cache_write.unwrap_or(true)
                    },
                    ttl_s: args.cache_ttl_s,
                },
            };
            // Filter user-provided request headers at the boundary so they don't affect:
            // - request execution (LocalFetcher drops them anyway by default)
            // - cache keys / on-disk cache layout
            // (values are never echoed, but hashed cache keys still count as "touching secrets")
            let allow_unsafe_headers = matches!(
                std::env::var("WEBPIPE_ALLOW_UNSAFE_HEADERS")
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "1" | "true" | "yes" | "on"
            );
            let raw_headers: BTreeMap<String, String> = args.headers.unwrap_or_default();
            let mut dropped_request_headers: Vec<&'static str> = Vec::new();
            let mut filtered_headers: BTreeMap<String, String> = BTreeMap::new();
            for (k, v) in raw_headers {
                let kl = k.trim().to_ascii_lowercase();
                let sensitive = matches!(
                    kl.as_str(),
                    "authorization" | "cookie" | "proxy-authorization"
                );
                if !allow_unsafe_headers && sensitive {
                    dropped_request_headers.push(match kl.as_str() {
                        "authorization" => "authorization",
                        "cookie" => "cookie",
                        _ => "proxy-authorization",
                    });
                    continue;
                }
                filtered_headers.insert(k, v);
            }
            dropped_request_headers.sort();
            dropped_request_headers.dedup();
            let req = FetchRequest {
                headers: filtered_headers,
                ..req
            };

            if no_network {
                match self.fetcher.cache_get(&req) {
                    Ok(Some(resp)) => {
                        let is_pdf_like = Self::content_type_is_pdf(resp.content_type.as_deref())
                            || Self::url_looks_like_pdf(resp.final_url.as_str())
                            || webpipe_local::extract::bytes_look_like_pdf(&resp.bytes);
                        let (text, n, text_clipped) = if include_text && !is_pdf_like {
                            Self::truncate_to_chars(resp.text_lossy().as_ref(), max_text_chars)
                        } else {
                            (String::new(), 0, false)
                        };
                        let mut warnings: Vec<&'static str> = Vec::new();
                        if resp.truncated {
                            warnings.push("body_truncated_by_max_bytes");
                        }
                        if text_clipped {
                            warnings.push("text_truncated_by_max_text_chars");
                        }
                        if is_pdf_like && include_text {
                            warnings.push("text_unavailable_for_pdf_use_web_extract");
                        }
                        warnings.push("cache_only");
                        if !dropped_request_headers.is_empty() {
                            warnings.push("unsafe_request_headers_dropped");
                        }

                        let mut payload = serde_json::json!({
                            "ok": true,
                            "fetch_backend": "local",
                            "url": resp.url,
                            "final_url": resp.final_url,
                            "status": resp.status,
                            "content_type": resp.content_type,
                            "bytes": resp.bytes.len(),
                            "truncated": resp.truncated,
                            "source": "cache",
                            "text_chars": n,
                            "text_truncated": text_clipped,
                            "timings_ms": { "total": t0.elapsed().as_millis() },
                            "request": {
                                "fetch_backend": "local",
                                "no_network": true,
                                "timeout_ms": req.timeout_ms,
                                "max_bytes": req.max_bytes,
                                "cache": { "read": true, "write": false, "ttl_s": req.cache.ttl_s },
                                "include_text": include_text,
                                "max_text_chars": max_text_chars,
                                "include_headers": include_headers
                            },
                            "warnings": warnings
                        });
                        add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                        if !dropped_request_headers.is_empty() {
                            payload["request"]["dropped_request_headers"] =
                                serde_json::json!(dropped_request_headers);
                        }
                        if include_text {
                            payload["text"] = serde_json::json!(text);
                        }
                        if include_headers {
                            payload["headers"] = serde_json::json!(resp.headers);
                        }
                        return Ok(tool_result(payload));
                    }
                    Ok(None) => {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(
                                ErrorCode::NotSupported,
                                "cache miss in no_network mode",
                                "Warm the cache first (run without no_network), or set no_network=false."
                            ),
                            "request": { "fetch_backend": "local", "no_network": true, "cache": { "read": true, "write": false, "ttl_s": req.cache.ttl_s } }
                        });
                        add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                    Err(e) => {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(ErrorCode::CacheError, e.to_string(), "Cache read failed in no_network mode."),
                            "request": { "fetch_backend": "local", "no_network": true }
                        });
                        add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                }
            }

            let resp = match self.fetcher.fetch(&req).await {
                Ok(r) => r,
                Err(e) => {
                    let (code, hint) = match &e {
                        WebpipeError::InvalidUrl(_) => (
                            ErrorCode::InvalidUrl,
                            "Check that the URL is absolute (includes http/https) and is well-formed.",
                        ),
                        WebpipeError::Fetch(_) => (
                            ErrorCode::FetchFailed,
                            "Fetch failed. If this looks transient, retry with a smaller max_bytes and a larger timeout_ms; otherwise try a different URL.",
                        ),
                        WebpipeError::Cache(_) => (
                            ErrorCode::CacheError,
                            "Cache failed. Retry with cache_read=false/cache_write=false to isolate network vs cache issues.",
                        ),
                        WebpipeError::Search(_) => (
                            ErrorCode::UnexpectedError,
                            "Unexpected search error while fetching. This is likely a bug; include this payload when reporting.",
                        ),
                        WebpipeError::Llm(_) => (
                            ErrorCode::UnexpectedError,
                            "Unexpected LLM error while fetching. This is likely a bug; include this payload when reporting.",
                        ),
                        WebpipeError::NotConfigured(_) => (
                            ErrorCode::NotConfigured,
                            "This fetch backend should not require API keys; check your server environment and configuration.",
                        ),
                        WebpipeError::NotSupported(_) => (
                            ErrorCode::NotSupported,
                            "This operation is not supported by the current backend/config.",
                        ),
                    };
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "url": url,
                        "error": error_obj(code, e.to_string(), hint),
                        "request": {
                            "fetch_backend": fetch_backend,
                            "no_network": no_network,
                            "timeout_ms": req.timeout_ms,
                            "max_bytes": req.max_bytes,
                            "cache": { "read": req.cache.read, "write": req.cache.write, "ttl_s": req.cache.ttl_s },
                            "include_text": include_text,
                            "max_text_chars": max_text_chars,
                            "include_headers": include_headers
                        }
                    });
                    if !dropped_request_headers.is_empty() {
                        payload["request"]["dropped_request_headers"] =
                            serde_json::json!(dropped_request_headers);
                    }
                    add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());
                    return Ok(tool_result(payload));
                }
            };

            let is_pdf_like = Self::content_type_is_pdf(resp.content_type.as_deref())
                || Self::url_looks_like_pdf(resp.final_url.as_str())
                || webpipe_local::extract::bytes_look_like_pdf(&resp.bytes);
            let (text, n, text_clipped) = if include_text && !is_pdf_like {
                Self::truncate_to_chars(resp.text_lossy().as_ref(), max_text_chars)
            } else {
                (String::new(), 0, false)
            };
            let mut warnings: Vec<&'static str> = Vec::new();
            if resp.truncated {
                warnings.push("body_truncated_by_max_bytes");
            }
            if text_clipped {
                warnings.push("text_truncated_by_max_text_chars");
            }
            if is_pdf_like && include_text {
                warnings.push("text_unavailable_for_pdf_use_web_extract");
            }
            if !dropped_request_headers.is_empty() {
                warnings.push("unsafe_request_headers_dropped");
            }

            let mut payload = serde_json::json!({
                "ok": true,
                "fetch_backend": "local",
                "url": resp.url,
                "final_url": resp.final_url,
                "status": resp.status,
                "content_type": resp.content_type,
                "bytes": resp.bytes.len(),
                "truncated": resp.truncated,
                "source": match resp.source {
                    FetchSource::Cache => "cache",
                    FetchSource::Network => "network",
                },
                "text_chars": n,
                "text_truncated": text_clipped,
                "timings_ms": {
                    "total": t0.elapsed().as_millis(),
                    "cache_get": resp.timings_ms.get("cache_get").copied().unwrap_or(0),
                    "network_fetch": resp.timings_ms.get("network_fetch").copied().unwrap_or(0),
                    "cache_put": resp.timings_ms.get("cache_put").copied().unwrap_or(0),
                }
            });
            add_envelope_fields(&mut payload, "web_fetch", t0.elapsed().as_millis());

            payload["request"] = serde_json::json!({
                "fetch_backend": "local",
                "no_network": no_network,
                "timeout_ms": req.timeout_ms,
                "max_bytes": req.max_bytes,
                "cache": { "read": req.cache.read, "write": req.cache.write, "ttl_s": req.cache.ttl_s },
                "include_text": include_text,
                "max_text_chars": max_text_chars,
                "include_headers": include_headers
            });
            if !dropped_request_headers.is_empty() {
                payload["request"]["dropped_request_headers"] =
                    serde_json::json!(dropped_request_headers);
            }

            if include_text {
                payload["text"] = serde_json::json!(text);
            }

            if include_headers {
                payload["headers"] = serde_json::json!(resp.headers);
            }
            if !warnings.is_empty() {
                payload["warnings"] = serde_json::json!(warnings);
                let codes = warning_codes_from(&warnings);
                payload["warning_codes"] = serde_json::json!(codes.clone());
                payload["warning_hints"] = warning_hints_from(&codes);
                self.stats_record_warnings(&warnings);
            }
            self.stats_record_fetch_backend("local", true, t0.elapsed().as_millis() as u64, None);

            Ok(tool_result(payload))
        }

        #[tool(
            description = "Search the web (returns ok=false not_configured unless provider keys are set)"
        )]
        async fn web_search(
            &self,
            params: Parameters<Option<WebSearchArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            let max_results = args.max_results.unwrap_or(10).clamp(1, 20);

            let provider_name = args.provider.unwrap_or_else(|| "brave".to_string());
            let auto_mode = args.auto_mode.unwrap_or_else(|| "fallback".to_string());
            let client = self.http.clone();

            // Keep these as plain values so we can reuse them across fallbacks without moving.
            let query = args.query.unwrap_or_default();
            let language = args.language;
            let country = args.country;

            let t0 = std::time::Instant::now();
            self.stats_inc_tool("web_search");
            if query.trim().is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": provider_name,
                    "query": query,
                    "query_key": Self::query_key(&query),
                    "max_results": max_results,
                    "request": {
                        "provider": provider_name,
                        "auto_mode": auto_mode,
                        "query": query,
                        "query_key": Self::query_key(&query),
                        "max_results": max_results,
                        "language": language,
                        "country": country
                    },
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "query must be non-empty",
                        "Pass a non-empty search query."
                    )
                });
                add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let q = SearchQuery {
                query: query.clone(),
                max_results: Some(max_results),
                language: language.clone(),
                country: country.clone(),
            };
            let qk = Self::query_key(&query);

            if provider_name.as_str() != "auto"
                && provider_name.as_str() != "brave"
                && provider_name.as_str() != "tavily"
                && provider_name.as_str() != "searxng"
            {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": provider_name,
                    "query": query,
                    "max_results": max_results,
                    "request": {
                        "provider": provider_name,
                        "auto_mode": auto_mode,
                        "query": query,
                        "max_results": max_results,
                        "language": language,
                        "country": country
                    },
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        format!("unknown provider: {}", provider_name),
                        "provider must be one of: auto, brave, tavily, searxng"
                    )
                });
                add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            if provider_name.as_str() == "auto"
                && auto_mode.as_str() != "fallback"
                && auto_mode.as_str() != "merge"
                && auto_mode.as_str() != "mab"
            {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "provider": "auto",
                    "query": query,
                    "max_results": max_results,
                    "request": {
                        "provider": "auto",
                        "auto_mode": auto_mode,
                        "query": query,
                        "max_results": max_results,
                        "language": language,
                        "country": country
                    },
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown auto_mode",
                        "When provider=\"auto\", auto_mode must be one of: fallback, merge, mab"
                    )
                });
                add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            let resp = match provider_name.as_str() {
                "auto" => {
                    if auto_mode.as_str() == "merge" {
                        // Merge mode: query all configured providers and dedup by URL.
                        // ok=true if at least one provider succeeds.
                        fn canonicalize_url(url: &str) -> String {
                            if let Ok(mut u) = reqwest::Url::parse(url) {
                                u.set_fragment(None);
                                return u.to_string();
                            }
                            url.trim().to_string()
                        }

                        let brave_env =
                            has_env("WEBPIPE_BRAVE_API_KEY") || has_env("BRAVE_SEARCH_API_KEY");
                        let tavily_env =
                            has_env("WEBPIPE_TAVILY_API_KEY") || has_env("TAVILY_API_KEY");
                        let searxng_env = has_env("WEBPIPE_SEARXNG_ENDPOINT")
                            || has_env("WEBPIPE_SEARXNG_ENDPOINTS");
                        if !brave_env && !tavily_env && !searxng_env {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "auto",
                                "backend_provider": "merge",
                                "query": query.clone(),
                                "max_results": max_results,
                                "selection": { "requested_provider": "auto", "auto_mode": "merge", "selected_provider": "merge" },
                                "request": { "provider": "auto", "auto_mode": "merge", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "providers": [],
                                "error": error_obj(
                                    ErrorCode::NotConfigured,
                                    "no web search providers configured",
                                    "Set WEBPIPE_BRAVE_API_KEY / WEBPIPE_TAVILY_API_KEY / WEBPIPE_SEARXNG_ENDPOINT(S), or choose provider explicitly."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }

                        let mut providers = Vec::new();
                        let mut merged: Vec<webpipe_core::SearchResult> = Vec::new();
                        let mut seen = std::collections::BTreeSet::<String>::new();
                        let mut cost_units_total: u64 = 0;

                        if brave_env {
                            let pt0 = std::time::Instant::now();
                            match webpipe_local::search::BraveSearchProvider::from_env(
                                client.clone(),
                            ) {
                                Ok(p) => match p.search(&q).await {
                                    Ok(r) => {
                                        cost_units_total =
                                            cost_units_total.saturating_add(r.cost_units);
                                        for rr in r.results {
                                            let key = canonicalize_url(&rr.url);
                                            if seen.insert(key) {
                                                merged.push(rr);
                                            }
                                            if merged.len() >= max_results {
                                                break;
                                            }
                                        }
                                        providers.push(serde_json::json!({
                                            "name": "brave",
                                            "ok": true,
                                            "cost_units": r.cost_units,
                                            "elapsed_ms": pt0.elapsed().as_millis()
                                        }));
                                        self.stats_record_search_provider_qk(
                                            "brave",
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        providers.push(serde_json::json!({
                                            "name": "brave",
                                            "ok": false,
                                            "error": msg.clone(),
                                            "elapsed_ms": pt0.elapsed().as_millis()
                                        }));
                                        self.stats_record_search_provider_qk(
                                            "brave",
                                            false,
                                            0,
                                            pt0.elapsed().as_millis() as u64,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                    }
                                },
                                Err(e) => {
                                    let msg = e.to_string();
                                    providers.push(serde_json::json!({
                                        "name": "brave",
                                        "ok": false,
                                        "error": msg.clone(),
                                        "elapsed_ms": pt0.elapsed().as_millis()
                                    }));
                                    self.stats_record_search_provider_qk(
                                        "brave",
                                        false,
                                        0,
                                        pt0.elapsed().as_millis() as u64,
                                        Some(&msg),
                                        qk.as_deref(),
                                    );
                                }
                            }
                        }

                        if searxng_env && merged.len() < max_results {
                            let pt0 = std::time::Instant::now();
                            match webpipe_local::search::SearxngSearchProvider::from_env(
                                client.clone(),
                            ) {
                                Ok(p) => match p.search(&q).await {
                                    Ok(r) => {
                                        cost_units_total =
                                            cost_units_total.saturating_add(r.cost_units);
                                        for rr in r.results {
                                            let key = canonicalize_url(&rr.url);
                                            if seen.insert(key) {
                                                merged.push(rr);
                                            }
                                            if merged.len() >= max_results {
                                                break;
                                            }
                                        }
                                        providers.push(serde_json::json!({
                                            "name": "searxng",
                                            "ok": true,
                                            "cost_units": r.cost_units,
                                            "elapsed_ms": pt0.elapsed().as_millis()
                                        }));
                                        self.stats_record_search_provider_qk(
                                            "searxng",
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        providers.push(serde_json::json!({
                                            "name": "searxng",
                                            "ok": false,
                                            "error": msg.clone(),
                                            "elapsed_ms": pt0.elapsed().as_millis()
                                        }));
                                        self.stats_record_search_provider_qk(
                                            "searxng",
                                            false,
                                            0,
                                            pt0.elapsed().as_millis() as u64,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                    }
                                },
                                Err(e) => {
                                    let msg = e.to_string();
                                    providers.push(serde_json::json!({
                                        "name": "searxng",
                                        "ok": false,
                                        "error": msg.clone(),
                                        "elapsed_ms": pt0.elapsed().as_millis()
                                    }));
                                    self.stats_record_search_provider_qk(
                                        "searxng",
                                        false,
                                        0,
                                        pt0.elapsed().as_millis() as u64,
                                        Some(&msg),
                                        qk.as_deref(),
                                    );
                                }
                            }
                        }

                        if tavily_env && merged.len() < max_results {
                            let pt0 = std::time::Instant::now();
                            match webpipe_local::search::TavilySearchProvider::from_env(
                                client.clone(),
                            ) {
                                Ok(p) => match p.search(&q).await {
                                    Ok(r) => {
                                        cost_units_total =
                                            cost_units_total.saturating_add(r.cost_units);
                                        for rr in r.results {
                                            let key = canonicalize_url(&rr.url);
                                            if seen.insert(key) {
                                                merged.push(rr);
                                            }
                                            if merged.len() >= max_results {
                                                break;
                                            }
                                        }
                                        providers.push(serde_json::json!({
                                            "name": "tavily",
                                            "ok": true,
                                            "cost_units": r.cost_units,
                                            "elapsed_ms": pt0.elapsed().as_millis()
                                        }));
                                        self.stats_record_search_provider_qk(
                                            "tavily",
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        providers.push(serde_json::json!({
                                            "name": "tavily",
                                            "ok": false,
                                            "error": msg.clone(),
                                            "elapsed_ms": pt0.elapsed().as_millis()
                                        }));
                                        self.stats_record_search_provider_qk(
                                            "tavily",
                                            false,
                                            0,
                                            pt0.elapsed().as_millis() as u64,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                    }
                                },
                                Err(e) => {
                                    let msg = e.to_string();
                                    providers.push(serde_json::json!({
                                        "name": "tavily",
                                        "ok": false,
                                        "error": msg.clone(),
                                        "elapsed_ms": pt0.elapsed().as_millis()
                                    }));
                                    self.stats_record_search_provider_qk(
                                        "tavily",
                                        false,
                                        0,
                                        pt0.elapsed().as_millis() as u64,
                                        Some(&msg),
                                        qk.as_deref(),
                                    );
                                }
                            }
                        }

                        let ok_any = providers
                            .iter()
                            .any(|p| p.get("ok").and_then(|v| v.as_bool()) == Some(true));
                        if !ok_any {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "auto",
                                "backend_provider": "merge",
                                "query": query.clone(),
                                "max_results": max_results,
                                "selection": { "requested_provider": "auto", "auto_mode": "merge", "selected_provider": "merge" },
                                "request": { "provider": "auto", "auto_mode": "merge", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "providers": providers,
                                "error": error_obj(
                                    ErrorCode::SearchFailed,
                                    "all configured providers failed",
                                    "Inspect `providers` for per-provider errors; retry later or switch provider."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }

                        let mut payload = serde_json::json!({
                            "ok": true,
                            "provider": "auto",
                            "backend_provider": "merge",
                            "query": query.clone(),
                            "max_results": max_results,
                            "request": { "provider": "auto", "auto_mode": "merge", "query": q.query, "max_results": max_results, "language": q.language, "country": q.country },
                            "selection": { "requested_provider": "auto", "auto_mode": "merge", "selected_provider": "merge" },
                            "providers": providers,
                            "cost_units": cost_units_total,
                            "timings_ms": { "total": t0.elapsed().as_millis() },
                            "results": merged
                        });
                        let ok_all = payload["providers"]
                            .as_array()
                            .unwrap_or(&vec![])
                            .iter()
                            .all(|p| p.get("ok").and_then(|v| v.as_bool()) == Some(true));
                        if !ok_all {
                            // Partial results in merge mode: at least one provider failed.
                            // Also mark when Tavily succeeded (so callers can budget for credits).
                            let mut ws: Vec<&'static str> = vec!["partial_results"];
                            let tavily_used = payload["providers"]
                                .as_array()
                                .unwrap_or(&vec![])
                                .iter()
                                .any(|p| {
                                    p.get("name").and_then(|v| v.as_str()) == Some("tavily")
                                        && p.get("ok").and_then(|v| v.as_bool()) == Some(true)
                                });
                            if tavily_used {
                                ws.push("tavily_used");
                            }
                            payload["warnings"] = serde_json::json!(ws);
                            let codes = warning_codes_from(&ws);
                            payload["warning_codes"] = serde_json::json!(codes.clone());
                            payload["warning_hints"] = warning_hints_from(&codes);
                        }
                        add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }

                    if auto_mode.as_str() == "mab" {
                        // Deterministic “bandit-ish” auto selection.
                        // Goal: adapt to observed success/latency/cost without introducing randomness.
                        //
                        // Selection:
                        // - Explore each configured provider at least once (in stable order).
                        // - Then select on a Pareto frontier (multi-objective), and deterministic
                        //   scalarization.
                        fn env_f64(key: &str) -> Option<f64> {
                            std::env::var(key)
                                .ok()
                                .and_then(|v| v.trim().parse::<f64>().ok())
                        }

                        let brave_env =
                            has_env("WEBPIPE_BRAVE_API_KEY") || has_env("BRAVE_SEARCH_API_KEY");
                        let tavily_env =
                            has_env("WEBPIPE_TAVILY_API_KEY") || has_env("TAVILY_API_KEY");
                        let searxng_env = has_env("WEBPIPE_SEARXNG_ENDPOINT")
                            || has_env("WEBPIPE_SEARXNG_ENDPOINTS");
                        if !brave_env && !tavily_env && !searxng_env {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "auto",
                                "query": query.clone(),
                                "max_results": max_results,
                                "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "none" },
                                "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::NotConfigured,
                                    "no web search providers configured",
                                    "Set WEBPIPE_BRAVE_API_KEY / WEBPIPE_TAVILY_API_KEY / WEBPIPE_SEARXNG_ENDPOINT(S), or choose provider explicitly."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }

                        let exploration_c = env_f64("WEBPIPE_MAB_EXPLORATION_C").unwrap_or(0.7);
                        let cost_w = env_f64("WEBPIPE_MAB_COST_WEIGHT").unwrap_or(0.0);
                        let lat_w = env_f64("WEBPIPE_MAB_LATENCY_WEIGHT").unwrap_or(0.0);
                        let junk_w = env_f64("WEBPIPE_MAB_JUNK_WEIGHT").unwrap_or(0.0);
                        let hard_junk_w = env_f64("WEBPIPE_MAB_HARD_JUNK_WEIGHT").unwrap_or(0.0);
                        let max_junk_rate = env_f64("WEBPIPE_ROUTING_MAX_JUNK_RATE");
                        let max_hard_junk_rate = env_f64("WEBPIPE_ROUTING_MAX_HARD_JUNK_RATE");
                        let max_http_429_rate = env_f64("WEBPIPE_ROUTING_MAX_HTTP_429_RATE");
                        let max_mean_cost_units = env_f64("WEBPIPE_ROUTING_MAX_MEAN_COST_UNITS");

                        // Snapshot windowed summaries so we don't hold a mutex across await.
                        // If routing_context includes query_key and we have stats for this query_key, prefer them.
                        let (summaries, routing_context_used) =
                            self.snapshot_search_summaries_for_query_key(qk.as_deref());
                        let tavily_budget_units = std::env::var("WEBPIPE_TAVILY_BUDGET_UNITS")
                            .ok()
                            .and_then(|v| v.trim().parse::<u64>().ok());
                        let brave_budget_units = std::env::var("WEBPIPE_BRAVE_BUDGET_UNITS")
                            .ok()
                            .and_then(|v| v.trim().parse::<u64>().ok());
                        let provider_totals = { self.stats_lock().search_providers.clone() };

                        let searxng_eps = if searxng_env {
                            webpipe_local::search::searxng_endpoints_from_env()
                        } else {
                            Vec::new()
                        };

                        let mut order: Vec<String> = Vec::new();
                        if brave_env {
                            order.push("brave".to_string());
                        }
                        // Prefer self-hosted “free-ish” SearXNG before paid providers when available.
                        if searxng_env {
                            if searxng_eps.len() > 1 {
                                for i in 0..searxng_eps.len() {
                                    order.push(format!("searxng#{i}"));
                                }
                            } else {
                                order.push("searxng".to_string());
                            }
                        }
                        if tavily_env {
                            order.push("tavily".to_string());
                        }
                        // Budget filter (best-effort): if a budget exists and we've exceeded it, don't select that arm.
                        order.retain(|name| {
                            let spent =
                                provider_totals.get(name).map(|p| p.cost_units).unwrap_or(0);
                            match name.as_str() {
                                "tavily" => tavily_budget_units.map(|b| spent < b).unwrap_or(true),
                                "brave" => brave_budget_units.map(|b| spent < b).unwrap_or(true),
                                _ => true,
                            }
                        });
                        if order.is_empty() {
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "auto",
                                "query": query.clone(),
                                "max_results": max_results,
                                "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "none" },
                                "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::NotSupported,
                                    "all configured providers are over budget (or filtered)",
                                    "Raise budgets, reset stats window, or choose provider explicitly."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }

                        let cfg = muxer::MabConfig {
                            exploration_c,
                            cost_weight: cost_w,
                            latency_weight: lat_w,
                            junk_weight: junk_w,
                            hard_junk_weight: hard_junk_w,
                            max_junk_rate,
                            max_hard_junk_rate,
                            max_http_429_rate,
                            max_mean_cost_units,
                        };
                        let sel = muxer::select_mab(&order, &summaries, &cfg);
                        let chosen = sel.chosen.clone();
                        let debug_rows = serde_json::to_value(&sel.candidates)
                            .unwrap_or_else(|_| serde_json::json!([]));
                        let frontier_names = serde_json::json!(sel.frontier);

                        let pt0 = std::time::Instant::now();
                        let (backend_provider, selected_arm, out) = match chosen.as_str() {
                            "tavily" => {
                                let provider =
                                    match webpipe_local::search::TavilySearchProvider::from_env(
                                        client.clone(),
                                    ) {
                                        Ok(p) => p,
                                        Err(e) => {
                                            let msg = e.to_string();
                                            self.stats_record_search_provider_qk(
                                                "tavily",
                                                false,
                                                0,
                                                pt0.elapsed().as_millis() as u64,
                                                Some(&msg),
                                                qk.as_deref(),
                                            );
                                            let mut payload = serde_json::json!({
                                                "ok": false,
                                                "provider": "auto",
                                                "backend_provider": "tavily",
                                                "query": query.clone(),
                                                "max_results": max_results,
                                                "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "tavily", "mab": { "candidates": debug_rows, "frontier": frontier_names, "routing_context_used": routing_context_used, "routing_query_key": qk } },
                                                "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                                "error": error_obj(ErrorCode::NotConfigured, msg, "Tavily was selected but is not configured. Set WEBPIPE_TAVILY_API_KEY (or TAVILY_API_KEY), or use provider=brave.")
                                            });
                                            add_envelope_fields(
                                                &mut payload,
                                                "web_search",
                                                t0.elapsed().as_millis(),
                                            );
                                            return Ok(tool_result(payload));
                                        }
                                    };
                                match provider.search(&q).await {
                                    Ok(r) => {
                                        self.stats_record_search_provider_qk(
                                            "tavily",
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                        ("tavily", None, r)
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        self.stats_record_search_provider_qk(
                                            "tavily",
                                            false,
                                            0,
                                            pt0.elapsed().as_millis() as u64,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                        let hint = search_failed_hint(
                                            "tavily",
                                            &msg,
                                            "Auto (mab) selected Tavily, but it failed. Retry later; reduce max_results; or use provider=brave.",
                                        );
                                        let mut payload = serde_json::json!({
                                            "ok": false,
                                            "provider": "auto",
                                            "backend_provider": "tavily",
                                            "query": query.clone(),
                                            "max_results": max_results,
                                            "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "tavily", "mab": { "candidates": debug_rows, "frontier": frontier_names, "routing_context_used": routing_context_used, "routing_query_key": qk } },
                                            "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                            "error": error_obj(ErrorCode::SearchFailed, msg, hint)
                                        });
                                        add_envelope_fields(
                                            &mut payload,
                                            "web_search",
                                            t0.elapsed().as_millis(),
                                        );
                                        return Ok(tool_result(payload));
                                    }
                                }
                            }
                            s if s == "searxng" || s.starts_with("searxng#") => {
                                let arm_idx = parse_searxng_arm(s);
                                let stats_key = s.to_string();

                                let run = async {
                                    if let Some(i) = arm_idx {
                                        let Some(ep) = searxng_eps.get(i) else {
                                            return Err(WebpipeError::NotSupported(format!(
                                                "searxng arm index out of range: {i}"
                                            )));
                                        };
                                        webpipe_local::search::searxng_search_at_endpoint(
                                            &client,
                                            ep,
                                            &q,
                                        )
                                        .await
                                    } else if searxng_eps.len() == 1 {
                                        webpipe_local::search::searxng_search_at_endpoint(
                                            &client,
                                            &searxng_eps[0],
                                            &q,
                                        )
                                        .await
                                    } else {
                                        let p = webpipe_local::search::SearxngSearchProvider::from_env(
                                            client.clone(),
                                        )?;
                                        p.search(&q).await
                                    }
                                };

                                match run.await {
                                    Ok(r) => {
                                        self.stats_record_search_provider_qk(
                                            &stats_key,
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                        (
                                            "searxng",
                                            arm_idx.map(|i| format!("searxng#{i}")),
                                            r,
                                        )
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        self.stats_record_search_provider_qk(
                                            &stats_key,
                                            false,
                                            0,
                                            pt0.elapsed().as_millis() as u64,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                        let hint = search_failed_hint(
                                            "searxng",
                                            &msg,
                                            "Auto (mab) selected SearXNG, but it failed. Retry later; reduce max_results; or use urls=[...] to skip search.",
                                        );
                                        let mut payload = serde_json::json!({
                                            "ok": false,
                                            "provider": "auto",
                                            "backend_provider": "searxng",
                                            "query": query.clone(),
                                            "max_results": max_results,
                                            "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "searxng", "mab": { "candidates": debug_rows, "frontier": frontier_names, "routing_context_used": routing_context_used, "routing_query_key": qk } },
                                            "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                            "error": error_obj(ErrorCode::SearchFailed, msg, hint)
                                        });
                                        if let Some(i) = arm_idx {
                                            payload["selection"]["selected_arm"] =
                                                serde_json::json!(format!("searxng#{i}"));
                                        }
                                        add_envelope_fields(
                                            &mut payload,
                                            "web_search",
                                            t0.elapsed().as_millis(),
                                        );
                                        return Ok(tool_result(payload));
                                    }
                                }
                            }
                            _ => {
                                let provider =
                                    match webpipe_local::search::BraveSearchProvider::from_env(
                                        client.clone(),
                                    ) {
                                        Ok(p) => p,
                                        Err(e) => {
                                            let msg = e.to_string();
                                            self.stats_record_search_provider_qk(
                                                "brave",
                                                false,
                                                0,
                                                pt0.elapsed().as_millis() as u64,
                                                Some(&msg),
                                                qk.as_deref(),
                                            );
                                            let mut payload = serde_json::json!({
                                                "ok": false,
                                                "provider": "auto",
                                                "backend_provider": "brave",
                                                "query": query.clone(),
                                                "max_results": max_results,
                                                "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "brave", "mab": { "candidates": debug_rows, "frontier": frontier_names, "routing_context_used": routing_context_used, "routing_query_key": qk } },
                                                "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                                "error": error_obj(ErrorCode::NotConfigured, msg, "Brave was selected but is not configured. Set WEBPIPE_BRAVE_API_KEY (or BRAVE_SEARCH_API_KEY), or use provider=tavily.")
                                            });
                                            add_envelope_fields(
                                                &mut payload,
                                                "web_search",
                                                t0.elapsed().as_millis(),
                                            );
                                            return Ok(tool_result(payload));
                                        }
                                    };
                                match provider.search(&q).await {
                                    Ok(r) => {
                                        self.stats_record_search_provider_qk(
                                            "brave",
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                        ("brave", None, r)
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        self.stats_record_search_provider_qk(
                                            "brave",
                                            false,
                                            0,
                                            pt0.elapsed().as_millis() as u64,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                        let hint = search_failed_hint(
                                            "brave",
                                            &msg,
                                            "Auto (mab) selected Brave, but it failed. Retry later; reduce max_results; or use provider=tavily.",
                                        );
                                        let mut payload = serde_json::json!({
                                            "ok": false,
                                            "provider": "auto",
                                            "backend_provider": "brave",
                                            "query": query.clone(),
                                            "max_results": max_results,
                                            "selection": { "requested_provider": "auto", "auto_mode": "mab", "selected_provider": "brave", "mab": { "candidates": debug_rows, "frontier": frontier_names, "routing_context_used": routing_context_used, "routing_query_key": qk } },
                                            "request": { "provider": "auto", "auto_mode": "mab", "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                            "error": error_obj(ErrorCode::SearchFailed, msg, hint)
                                        });
                                        add_envelope_fields(
                                            &mut payload,
                                            "web_search",
                                            t0.elapsed().as_millis(),
                                        );
                                        return Ok(tool_result(payload));
                                    }
                                }
                            }
                        };

                        let mut payload = serde_json::json!({
                            "ok": true,
                            "provider": "auto",
                            "backend_provider": backend_provider,
                            "query": query.clone(),
                            "query_key": Self::query_key(&query),
                            "max_results": max_results,
                            "request": { "provider": "auto", "auto_mode": "mab", "query": q.query, "query_key": Self::query_key(&q.query), "max_results": max_results, "language": q.language, "country": q.country },
                            "selection": { "requested_provider": "auto", "selected_provider": backend_provider, "auto_mode": "mab", "mab": { "candidates": debug_rows, "frontier": frontier_names, "routing_context_used": routing_context_used, "routing_query_key": qk, "exploration_c": exploration_c, "cost_weight": cost_w, "latency_weight": lat_w, "junk_weight": junk_w, "hard_junk_weight": hard_junk_w, "constraints": { "max_junk_rate": max_junk_rate, "max_hard_junk_rate": max_hard_junk_rate, "max_http_429_rate": max_http_429_rate, "max_mean_cost_units": max_mean_cost_units } } },
                            "cost_units": out.cost_units,
                            "timings_ms": { "total": t0.elapsed().as_millis() },
                            "results": out.results
                        });
                        if let Some(arm) = selected_arm {
                            payload["selection"]["selected_arm"] = serde_json::json!(arm);
                        }
                        if backend_provider == "tavily" {
                            payload["warnings"] = serde_json::json!(["tavily_used"]);
                            let codes = warning_codes_from(&["tavily_used"]);
                            payload["warning_codes"] = serde_json::json!(codes.clone());
                            payload["warning_hints"] = warning_hints_from(&codes);
                        }
                        add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }

                    // Fallback mode: “MAB with failover”.
                    //
                    // Build a deterministic provider order based on what's configured, then use the
                    // same MAB selection logic to pick a provider. If that provider fails, retry with
                    // the next best remaining provider, and so on (bounded by configured providers).
                    //
                    // This makes fallback variable-length and “as-MAB-as-possible” without changing
                    // the public `auto_mode` surface.
                    fn env_f64(key: &str) -> Option<f64> {
                        std::env::var(key)
                            .ok()
                            .and_then(|v| v.trim().parse::<f64>().ok())
                    }

                    let brave_env =
                        has_env("WEBPIPE_BRAVE_API_KEY") || has_env("BRAVE_SEARCH_API_KEY");
                    let tavily_env = has_env("WEBPIPE_TAVILY_API_KEY") || has_env("TAVILY_API_KEY");
                    let searxng_env = has_env("WEBPIPE_SEARXNG_ENDPOINT")
                        || has_env("WEBPIPE_SEARXNG_ENDPOINTS");

                    let searxng_eps = if searxng_env {
                        webpipe_local::search::searxng_endpoints_from_env()
                    } else {
                        Vec::new()
                    };

                    let mut order: Vec<String> = Vec::new();
                    if brave_env {
                        order.push("brave".to_string());
                    }
                    if searxng_env {
                        if searxng_eps.len() > 1 {
                            for i in 0..searxng_eps.len() {
                                order.push(format!("searxng#{i}"));
                            }
                        } else {
                            order.push("searxng".to_string());
                        }
                    }
                    if tavily_env {
                        order.push("tavily".to_string());
                    }

                    if order.is_empty() {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "provider": "auto",
                            "query": query.clone(),
                            "max_results": max_results,
                            "selection": { "requested_provider": "auto", "auto_mode": auto_mode, "selected_provider": "none" },
                            "request": { "provider": "auto", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                            "error": error_obj(
                                ErrorCode::NotConfigured,
                                "no web search providers configured",
                            "Set WEBPIPE_BRAVE_API_KEY / WEBPIPE_TAVILY_API_KEY / WEBPIPE_SEARXNG_ENDPOINT(S), or choose provider explicitly."
                            )
                        });
                        add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }

                    // Budget filter (best-effort): keep the same semantics as MAB mode.
                    let tavily_budget_units = std::env::var("WEBPIPE_TAVILY_BUDGET_UNITS")
                        .ok()
                        .and_then(|v| v.trim().parse::<u64>().ok());
                    let brave_budget_units = std::env::var("WEBPIPE_BRAVE_BUDGET_UNITS")
                        .ok()
                        .and_then(|v| v.trim().parse::<u64>().ok());
                    let provider_totals = { self.stats_lock().search_providers.clone() };
                    order.retain(|name| {
                        let spent = provider_totals.get(name).map(|p| p.cost_units).unwrap_or(0);
                        match name.as_str() {
                            "tavily" => tavily_budget_units.map(|b| spent < b).unwrap_or(true),
                            "brave" => brave_budget_units.map(|b| spent < b).unwrap_or(true),
                            _ => true,
                        }
                    });

                    if order.is_empty() {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "provider": "auto",
                            "query": query.clone(),
                            "max_results": max_results,
                            "selection": { "requested_provider": "auto", "auto_mode": auto_mode, "selected_provider": "none" },
                            "request": { "provider": "auto", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                            "error": error_obj(
                                ErrorCode::NotSupported,
                                "all configured providers are over budget (or filtered)",
                                "Raise budgets, reset stats window, or choose provider explicitly."
                            )
                        });
                        add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }

                    let (summaries, routing_context_used) =
                        self.snapshot_search_summaries_for_query_key(qk.as_deref());

                    let cfg = muxer::MabConfig {
                        exploration_c: env_f64("WEBPIPE_MAB_EXPLORATION_C").unwrap_or(0.7),
                        cost_weight: env_f64("WEBPIPE_MAB_COST_WEIGHT").unwrap_or(0.0),
                        latency_weight: env_f64("WEBPIPE_MAB_LATENCY_WEIGHT").unwrap_or(0.0),
                        junk_weight: env_f64("WEBPIPE_MAB_JUNK_WEIGHT").unwrap_or(0.0),
                        hard_junk_weight: env_f64("WEBPIPE_MAB_HARD_JUNK_WEIGHT").unwrap_or(0.0),
                        max_junk_rate: env_f64("WEBPIPE_ROUTING_MAX_JUNK_RATE"),
                        max_hard_junk_rate: env_f64("WEBPIPE_ROUTING_MAX_HARD_JUNK_RATE"),
                        max_http_429_rate: env_f64("WEBPIPE_ROUTING_MAX_HTTP_429_RATE"),
                        max_mean_cost_units: env_f64("WEBPIPE_ROUTING_MAX_MEAN_COST_UNITS"),
                    };

                    // Re-run MAB after each failure, with locally-updated summaries.
                    // This makes fallback responsive within a single call (e.g. 429 -> immediate deprioritization).
                    let mut summaries_local = summaries.clone();
                    let mut remaining = order.clone();

                    // Capture the selection-debug for the first attempt (usually what you want to inspect).
                    let sel0 = muxer::select_mab(&remaining, &summaries_local, &cfg);
                    let debug_rows0 = serde_json::to_value(&sel0.candidates)
                        .unwrap_or_else(|_| serde_json::json!([]));
                    let frontier0 = serde_json::json!(sel0.frontier.clone());

                    let mut attempts: Vec<serde_json::Value> = Vec::new();
                    let mut attempted_chain: Vec<String> = Vec::new();
                    while !remaining.is_empty() {
                        let sel = muxer::select_mab(&remaining, &summaries_local, &cfg);
                        let chosen = sel.chosen.clone();
                        if chosen.is_empty() {
                            break;
                        }
                        attempted_chain.push(chosen.clone());

                        let pt0 = std::time::Instant::now();
                        match chosen.as_str() {
                            "brave" => match webpipe_local::search::BraveSearchProvider::from_env(
                                client.clone(),
                            ) {
                                Ok(p) => match p.search(&q).await {
                                    Ok(r) => {
                                        attempts.push(serde_json::json!({"name":"brave","ok":true,"cost_units":r.cost_units,"elapsed_ms":pt0.elapsed().as_millis()}));
                                        self.stats_record_search_provider_qk(
                                            "brave",
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                        let mut payload = serde_json::json!({
                                            "ok": true,
                                            "provider": "auto",
                                            "backend_provider": "brave",
                                            "query": query.clone(),
                                            "query_key": Self::query_key(&query),
                                            "max_results": max_results,
                                            "request": { "provider": "auto", "auto_mode": auto_mode, "query": q.query, "query_key": Self::query_key(&q.query), "max_results": max_results, "language": q.language, "country": q.country },
                                            "selection": { "requested_provider": "auto", "selected_provider": "brave", "auto_mode": auto_mode, "mab": { "candidates": debug_rows0, "frontier": frontier0, "routing_context_used": routing_context_used, "routing_query_key": qk, "attempted_chain": attempted_chain } },
                                            "providers": attempts,
                                            "cost_units": r.cost_units,
                                            "timings_ms": { "total": t0.elapsed().as_millis() },
                                            "results": r.results,
                                        });
                                        if payload["providers"]
                                            .as_array()
                                            .map(|a| a.len())
                                            .unwrap_or(0)
                                            >= 2
                                        {
                                            payload["warnings"] =
                                                serde_json::json!(["provider_failover"]);
                                            let codes = warning_codes_from(&["provider_failover"]);
                                            payload["warning_codes"] =
                                                serde_json::json!(codes.clone());
                                            payload["warning_hints"] = warning_hints_from(&codes);
                                        }
                                        add_envelope_fields(
                                            &mut payload,
                                            "web_search",
                                            t0.elapsed().as_millis(),
                                        );
                                        return Ok(tool_result(payload));
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        let elapsed_ms = pt0.elapsed().as_millis() as u64;
                                        let http_429 = is_http_status(&msg, 429);
                                        attempts.push(serde_json::json!({"name":"brave","ok":false,"error":msg.clone(),"elapsed_ms":elapsed_ms}));
                                        self.stats_record_search_provider_qk(
                                            "brave",
                                            false,
                                            0,
                                            elapsed_ms,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                        // Update local summaries so the next selection sees the failure immediately.
                                        let s =
                                            summaries_local.entry("brave".to_string()).or_default();
                                        s.calls = s.calls.saturating_add(1);
                                        s.http_429 = s.http_429.saturating_add(http_429 as u64);
                                        s.elapsed_ms_sum =
                                            s.elapsed_ms_sum.saturating_add(elapsed_ms);
                                    }
                                },
                                Err(WebpipeError::NotConfigured(msg)) => {
                                    let elapsed_ms = pt0.elapsed().as_millis() as u64;
                                    let http_429 = is_http_status(&msg, 429);
                                    attempts.push(serde_json::json!({"name":"brave","ok":false,"error":msg.clone(),"elapsed_ms":elapsed_ms}));
                                    self.stats_record_search_provider_qk(
                                        "brave",
                                        false,
                                        0,
                                        elapsed_ms,
                                        Some(&msg),
                                        qk.as_deref(),
                                    );
                                    let s = summaries_local.entry("brave".to_string()).or_default();
                                    s.calls = s.calls.saturating_add(1);
                                    s.http_429 = s.http_429.saturating_add(http_429 as u64);
                                    s.elapsed_ms_sum = s.elapsed_ms_sum.saturating_add(elapsed_ms);
                                }
                                Err(e) => {
                                    return Err(McpError::internal_error(e.to_string(), None))
                                }
                            },
                            s if s == "searxng" || s.starts_with("searxng#") => {
                                let arm_idx = parse_searxng_arm(s);
                                let stats_key = s.to_string();

                                let run = async {
                                    if let Some(i) = arm_idx {
                                        let Some(ep) = searxng_eps.get(i) else {
                                            return Err(WebpipeError::NotSupported(format!(
                                                "searxng arm index out of range: {i}"
                                            )));
                                        };
                                        webpipe_local::search::searxng_search_at_endpoint(
                                            &client,
                                            ep,
                                            &q,
                                        )
                                        .await
                                    } else if searxng_eps.len() == 1 {
                                        webpipe_local::search::searxng_search_at_endpoint(
                                            &client,
                                            &searxng_eps[0],
                                            &q,
                                        )
                                        .await
                                    } else {
                                        let p = webpipe_local::search::SearxngSearchProvider::from_env(
                                            client.clone(),
                                        )?;
                                        p.search(&q).await
                                    }
                                };

                                match run.await {
                                    Ok(r) => {
                                        let mut entry = serde_json::json!({"name":"searxng","ok":true,"cost_units":r.cost_units,"elapsed_ms":pt0.elapsed().as_millis()});
                                        if let Some(i) = arm_idx {
                                            entry["arm"] = serde_json::json!(format!("searxng#{i}"));
                                        }
                                        attempts.push(entry);
                                        self.stats_record_search_provider_qk(
                                            &stats_key,
                                            true,
                                            r.cost_units,
                                            pt0.elapsed().as_millis() as u64,
                                            None,
                                            qk.as_deref(),
                                        );
                                        let mut payload = serde_json::json!({
                                            "ok": true,
                                            "provider": "auto",
                                            "backend_provider": "searxng",
                                            "query": query.clone(),
                                            "query_key": Self::query_key(&query),
                                            "max_results": max_results,
                                            "request": { "provider": "auto", "auto_mode": auto_mode, "query": q.query, "query_key": Self::query_key(&q.query), "max_results": max_results, "language": q.language, "country": q.country },
                                            "selection": { "requested_provider": "auto", "selected_provider": "searxng", "auto_mode": auto_mode, "mab": { "candidates": debug_rows0, "frontier": frontier0, "routing_context_used": routing_context_used, "routing_query_key": qk, "attempted_chain": attempted_chain } },
                                            "providers": attempts,
                                            "cost_units": r.cost_units,
                                            "timings_ms": { "total": t0.elapsed().as_millis() },
                                            "results": r.results,
                                        });
                                        if let Some(i) = arm_idx {
                                            payload["selection"]["selected_arm"] =
                                                serde_json::json!(format!("searxng#{i}"));
                                        }
                                        if payload["providers"]
                                            .as_array()
                                            .map(|a| a.len())
                                            .unwrap_or(0)
                                            >= 2
                                        {
                                            payload["warnings"] =
                                                serde_json::json!(["provider_failover"]);
                                            let codes =
                                                warning_codes_from(&["provider_failover"]);
                                            payload["warning_codes"] =
                                                serde_json::json!(codes.clone());
                                            payload["warning_hints"] =
                                                warning_hints_from(&codes);
                                        }
                                        add_envelope_fields(
                                            &mut payload,
                                            "web_search",
                                            t0.elapsed().as_millis(),
                                        );
                                        return Ok(tool_result(payload));
                                    }
                                    Err(e) => {
                                        let msg = e.to_string();
                                        let elapsed_ms = pt0.elapsed().as_millis() as u64;
                                        let http_429 = is_http_status(&msg, 429);
                                        let mut entry = serde_json::json!({"name":"searxng","ok":false,"error":msg.clone(),"elapsed_ms":elapsed_ms});
                                        if let Some(i) = arm_idx {
                                            entry["arm"] = serde_json::json!(format!("searxng#{i}"));
                                        }
                                        attempts.push(entry);
                                        self.stats_record_search_provider_qk(
                                            &stats_key,
                                            false,
                                            0,
                                            elapsed_ms,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                        let s = summaries_local.entry(stats_key).or_default();
                                        s.calls = s.calls.saturating_add(1);
                                        s.http_429 = s.http_429.saturating_add(http_429 as u64);
                                        s.elapsed_ms_sum =
                                            s.elapsed_ms_sum.saturating_add(elapsed_ms);
                                    }
                                }
                            }
                            "tavily" => {
                                match webpipe_local::search::TavilySearchProvider::from_env(
                                    client.clone(),
                                ) {
                                    Ok(p) => match p.search(&q).await {
                                        Ok(r) => {
                                            attempts.push(serde_json::json!({"name":"tavily","ok":true,"cost_units":r.cost_units,"elapsed_ms":pt0.elapsed().as_millis()}));
                                            self.stats_record_search_provider_qk(
                                                "tavily",
                                                true,
                                                r.cost_units,
                                                pt0.elapsed().as_millis() as u64,
                                                None,
                                                qk.as_deref(),
                                            );
                                            let mut ws: Vec<&'static str> = Vec::new();
                                            if attempts.len() >= 2 {
                                                ws.push("provider_failover");
                                            }
                                            ws.push("tavily_used");
                                            let mut payload = serde_json::json!({
                                                "ok": true,
                                                "provider": "auto",
                                                "backend_provider": "tavily",
                                                "query": query.clone(),
                                                "query_key": Self::query_key(&query),
                                                "max_results": max_results,
                                                "request": { "provider": "auto", "auto_mode": auto_mode, "query": q.query, "query_key": Self::query_key(&q.query), "max_results": max_results, "language": q.language, "country": q.country },
                                                "selection": { "requested_provider": "auto", "selected_provider": "tavily", "auto_mode": auto_mode, "mab": { "candidates": debug_rows0, "frontier": frontier0, "routing_context_used": routing_context_used, "routing_query_key": qk, "attempted_chain": attempted_chain } },
                                                "providers": attempts,
                                                "warnings": ws,
                                                "cost_units": r.cost_units,
                                                "timings_ms": { "total": t0.elapsed().as_millis() },
                                                "results": r.results,
                                            });
                                            let codes = warning_codes_from(&ws);
                                            payload["warning_codes"] =
                                                serde_json::json!(codes.clone());
                                            payload["warning_hints"] = warning_hints_from(&codes);
                                            add_envelope_fields(
                                                &mut payload,
                                                "web_search",
                                                t0.elapsed().as_millis(),
                                            );
                                            return Ok(tool_result(payload));
                                        }
                                        Err(e) => {
                                            let msg = e.to_string();
                                            let elapsed_ms = pt0.elapsed().as_millis() as u64;
                                            let http_429 = is_http_status(&msg, 429);
                                            attempts.push(serde_json::json!({"name":"tavily","ok":false,"error":msg.clone(),"elapsed_ms":elapsed_ms}));
                                            self.stats_record_search_provider_qk(
                                                "tavily",
                                                false,
                                                0,
                                                elapsed_ms,
                                                Some(&msg),
                                                qk.as_deref(),
                                            );
                                            let s = summaries_local
                                                .entry("tavily".to_string())
                                                .or_default();
                                            s.calls = s.calls.saturating_add(1);
                                            s.http_429 = s.http_429.saturating_add(http_429 as u64);
                                            s.elapsed_ms_sum =
                                                s.elapsed_ms_sum.saturating_add(elapsed_ms);
                                        }
                                    },
                                    Err(WebpipeError::NotConfigured(msg)) => {
                                        let elapsed_ms = pt0.elapsed().as_millis() as u64;
                                        let http_429 = is_http_status(&msg, 429);
                                        attempts.push(serde_json::json!({"name":"tavily","ok":false,"error":msg.clone(),"elapsed_ms":elapsed_ms}));
                                        self.stats_record_search_provider_qk(
                                            "tavily",
                                            false,
                                            0,
                                            elapsed_ms,
                                            Some(&msg),
                                            qk.as_deref(),
                                        );
                                        let s = summaries_local
                                            .entry("tavily".to_string())
                                            .or_default();
                                        s.calls = s.calls.saturating_add(1);
                                        s.http_429 = s.http_429.saturating_add(http_429 as u64);
                                        s.elapsed_ms_sum =
                                            s.elapsed_ms_sum.saturating_add(elapsed_ms);
                                    }
                                    Err(e) => {
                                        return Err(McpError::internal_error(e.to_string(), None))
                                    }
                                }
                            }
                            other => {
                                attempts.push(serde_json::json!({"name": other, "ok": false, "error": "unknown provider"}));
                            }
                        }

                        // Never retry the same provider within one call.
                        remaining.retain(|x| x != &chosen);
                    }

                    let hint = if attempts.iter().any(|a| {
                        a.get("error")
                            .and_then(|v| v.as_str())
                            .is_some_and(|s| is_http_status(s, 429))
                    }) {
                        "At least one provider is rate-limiting (HTTP 429). Retry later; reduce max_results; or use urls=[...] to skip search."
                    } else {
                        "All configured providers failed. Inspect `providers` for per-provider errors; retry later or switch provider."
                    };

                    let mut payload = serde_json::json!({
                        "ok": false,
                        "provider": "auto",
                        "backend_provider": "fallback",
                        "query": query.clone(),
                        "query_key": Self::query_key(&query),
                        "max_results": max_results,
                        "request": { "provider": "auto", "auto_mode": auto_mode, "query": q.query, "query_key": Self::query_key(&q.query), "max_results": max_results, "language": q.language, "country": q.country },
                        "selection": { "requested_provider": "auto", "auto_mode": auto_mode, "selected_provider": "none", "mab": { "candidates": debug_rows0, "frontier": frontier0, "routing_context_used": routing_context_used, "routing_query_key": qk, "attempted_chain": attempted_chain } },
                        "providers": attempts,
                        "error": error_obj(
                            ErrorCode::SearchFailed,
                            "all configured providers failed (auto fallback)",
                            hint
                        )
                    });
                    add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                    return Ok(tool_result(payload));
                }
                "brave" => {
                    let pt0 = std::time::Instant::now();
                    let provider = match webpipe_local::search::BraveSearchProvider::from_env(
                        client.clone(),
                    ) {
                        Ok(p) => p,
                        Err(WebpipeError::NotConfigured(msg)) => {
                            self.stats_record_search_provider_qk(
                                "brave",
                                false,
                                0,
                                pt0.elapsed().as_millis() as u64,
                                Some(&msg),
                                qk.as_deref(),
                            );
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "brave",
                                "query": query.clone(),
                                "max_results": max_results,
                                "request": { "provider": "brave", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::NotConfigured,
                                    msg,
                                    "Set WEBPIPE_BRAVE_API_KEY (or BRAVE_SEARCH_API_KEY) in the webpipe MCP server environment."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                        Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
                    };
                    match provider.search(&q).await {
                        Ok(r) => {
                            self.stats_record_search_provider_qk(
                                "brave",
                                true,
                                r.cost_units,
                                pt0.elapsed().as_millis() as u64,
                                None,
                                qk.as_deref(),
                            );
                            r
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            self.stats_record_search_provider_qk(
                                "brave",
                                false,
                                0,
                                pt0.elapsed().as_millis() as u64,
                                Some(&msg),
                                qk.as_deref(),
                            );
                            let hint = search_failed_hint(
                                "brave",
                                &msg,
                                "Brave search failed. If this is a transient HTTP error, retry with smaller max_results; otherwise verify WEBPIPE_BRAVE_API_KEY.",
                            );
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "brave",
                                "query": query.clone(),
                                "max_results": max_results,
                                "request": { "provider": "brave", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::SearchFailed,
                                    msg,
                                    hint
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                    }
                }
                "tavily" => {
                    let pt0 = std::time::Instant::now();
                    let provider = match webpipe_local::search::TavilySearchProvider::from_env(
                        client.clone(),
                    ) {
                        Ok(p) => p,
                        Err(WebpipeError::NotConfigured(msg)) => {
                            self.stats_record_search_provider_qk(
                                "tavily",
                                false,
                                0,
                                pt0.elapsed().as_millis() as u64,
                                Some(&msg),
                                qk.as_deref(),
                            );
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "tavily",
                                "query": query.clone(),
                                "max_results": max_results,
                                "request": { "provider": "tavily", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::NotConfigured,
                                    msg,
                                    "Set WEBPIPE_TAVILY_API_KEY (or TAVILY_API_KEY) in the webpipe MCP server environment."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                        Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
                    };
                    match provider.search(&q).await {
                        Ok(r) => {
                            self.stats_record_search_provider_qk(
                                "tavily",
                                true,
                                r.cost_units,
                                pt0.elapsed().as_millis() as u64,
                                None,
                                qk.as_deref(),
                            );
                            r
                        }
                        Err(e) => {
                            // In this environment, Tavily can return HTTP 433 even when configured.
                            // Prefer a bounded fallback to Brave if available.
                            let msg = e.to_string();
                            self.stats_record_search_provider_qk(
                                "tavily",
                                false,
                                0,
                                pt0.elapsed().as_millis() as u64,
                                Some(&msg),
                                qk.as_deref(),
                            );
                            if msg.contains("HTTP 433") {
                                if let Ok(brave) =
                                    webpipe_local::search::BraveSearchProvider::from_env(
                                        client.clone(),
                                    )
                                {
                                    let r = match brave.search(&q).await {
                                        Ok(r) => {
                                            self.stats_record_search_provider_qk(
                                                "brave",
                                                true,
                                                r.cost_units,
                                                pt0.elapsed().as_millis() as u64,
                                                None,
                                                qk.as_deref(),
                                            );
                                            r
                                        }
                                        Err(e2) => {
                                            let msg2 = e2.to_string();
                                            self.stats_record_search_provider_qk(
                                                "brave",
                                                false,
                                                0,
                                                pt0.elapsed().as_millis() as u64,
                                                Some(&msg2),
                                                qk.as_deref(),
                                            );
                                            let mut payload = serde_json::json!({
                                                "ok": false,
                                                "provider": "brave",
                                                "query": query.clone(),
                                                "max_results": max_results,
                                                "request": { "provider": "tavily", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                                "fallback": {
                                                    "requested_provider": "tavily",
                                                    "reason": "tavily_http_433",
                                                    "tavily_error": msg
                                                },
                                                "error": error_obj(
                                                    ErrorCode::SearchFailed,
                                                    msg2,
                                                    "Fallback to Brave failed. Verify WEBPIPE_BRAVE_API_KEY or switch provider."
                                                )
                                            });
                                            add_envelope_fields(
                                                &mut payload,
                                                "web_search",
                                                t0.elapsed().as_millis(),
                                            );
                                            return Ok(tool_result(payload));
                                        }
                                    };
                                    // Carry evidence about the fallback.
                                    // (We don't change the top-level `provider` field; it indicates the backend used.)
                                    let mut payload = serde_json::json!({
                                        "ok": true,
                                        "provider": "tavily",
                                        "backend_provider": r.provider,
                                        "query": query.clone(),
                                        "max_results": max_results,
                                        "request": { "provider": "tavily", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                        "fallback": {
                                            "requested_provider": "tavily",
                                            "reason": "tavily_http_433",
                                            "tavily_error": msg
                                        },
                                        "cost_units": r.cost_units,
                                        "timings_ms": r.timings_ms,
                                        "results": r.results,
                                    });
                                    add_envelope_fields(
                                        &mut payload,
                                        "web_search",
                                        t0.elapsed().as_millis(),
                                    );
                                    return Ok(tool_result(payload));
                                }
                                // If Brave isn't configured, keep the error as a *tool-level* failure (not transport).
                                let mut payload = serde_json::json!({
                                    "ok": false,
                                    "provider": "tavily",
                                    "query": query.clone(),
                                    "max_results": max_results,
                                    "request": { "provider": "tavily", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                    "error": error_obj(
                                        ErrorCode::ProviderUnavailable,
                                        msg,
                                        "Tavily returned HTTP 433 in this environment. Configure WEBPIPE_BRAVE_API_KEY to allow automatic fallback, or switch provider."
                                    )
                                });
                                add_envelope_fields(
                                    &mut payload,
                                    "web_search",
                                    t0.elapsed().as_millis(),
                                );
                                return Ok(tool_result(payload));
                            }
                            let hint = search_failed_hint(
                                "tavily",
                                &msg,
                                "Tavily search failed. Retry with smaller max_results or switch provider.",
                            );
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "tavily",
                                "query": query.clone(),
                                "max_results": max_results,
                                "request": { "provider": "tavily", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::SearchFailed,
                                    msg,
                                    hint
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                    }
                }
                "searxng" => {
                    let pt0 = std::time::Instant::now();
                    let provider = match webpipe_local::search::SearxngSearchProvider::from_env(
                        client.clone(),
                    ) {
                        Ok(p) => p,
                        Err(WebpipeError::NotConfigured(msg)) => {
                            self.stats_record_search_provider_qk(
                                "searxng",
                                false,
                                0,
                                pt0.elapsed().as_millis() as u64,
                                Some(&msg),
                                qk.as_deref(),
                            );
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "searxng",
                                "query": query.clone(),
                                "max_results": max_results,
                                "request": { "provider": "searxng", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::NotConfigured,
                                    msg,
                                    "Set WEBPIPE_SEARXNG_ENDPOINT (or WEBPIPE_SEARXNG_ENDPOINTS) in the webpipe MCP server environment."
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                        Err(e) => return Err(McpError::internal_error(e.to_string(), None)),
                    };
                    match provider.search(&q).await {
                        Ok(r) => {
                            self.stats_record_search_provider_qk(
                                "searxng",
                                true,
                                r.cost_units,
                                pt0.elapsed().as_millis() as u64,
                                None,
                                qk.as_deref(),
                            );
                            r
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            self.stats_record_search_provider_qk(
                                "searxng",
                                false,
                                0,
                                pt0.elapsed().as_millis() as u64,
                                Some(&msg),
                                qk.as_deref(),
                            );
                            let hint = search_failed_hint(
                                "searxng",
                                &msg,
                                "SearXNG search failed. Verify WEBPIPE_SEARXNG_ENDPOINT or switch provider.",
                            );
                            let mut payload = serde_json::json!({
                                "ok": false,
                                "provider": "searxng",
                                "query": query.clone(),
                                "max_results": max_results,
                                "request": { "provider": "searxng", "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                                "error": error_obj(
                                    ErrorCode::SearchFailed,
                                    msg,
                                    hint
                                )
                            });
                            add_envelope_fields(
                                &mut payload,
                                "web_search",
                                t0.elapsed().as_millis(),
                            );
                            return Ok(tool_result(payload));
                        }
                    }
                }
                other => {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "provider": other,
                        "query": query.clone(),
                        "max_results": max_results,
                        "request": { "provider": other, "auto_mode": auto_mode, "query": query.clone(), "max_results": max_results, "language": language, "country": country },
                        "error": error_obj(
                            ErrorCode::InvalidParams,
                            format!("unknown provider: {other}"),
                            "provider must be one of: auto, brave, tavily, searxng"
                        )
                    });
                    add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());
                    return Ok(tool_result(payload));
                }
            };

            let mut payload = serde_json::json!({
                "ok": true,
                "provider": provider_name,
                "backend_provider": resp.provider,
                "query": query,
                "query_key": Self::query_key(&query),
                "max_results": max_results,
                "request": { "provider": provider_name, "auto_mode": auto_mode, "query": q.query, "query_key": Self::query_key(&q.query), "max_results": max_results, "language": q.language, "country": q.country },
                "cost_units": resp.cost_units,
                "timings_ms": resp.timings_ms,
                "results": resp.results,
            });
            if provider_name.as_str() == "auto" {
                payload["selection"] = serde_json::json!({
                    "requested_provider": "auto",
                    "selected_provider": payload["backend_provider"].as_str().unwrap_or("unknown"),
                    "auto_mode": auto_mode
                });
            }
            add_envelope_fields(&mut payload, "web_search", t0.elapsed().as_millis());

            Ok(tool_result(payload))
        }

        #[tool(description = "Fetch a URL and extract readable text (HTML -> text)")]
        async fn web_extract(
            &self,
            params: Parameters<Option<WebExtractArgs>>,
        ) -> Result<CallToolResult, McpError> {
            let args = params.0.unwrap_or_default();
            self.stats_inc_tool("web_extract");
            let width = args.width.unwrap_or(100).clamp(20, 240);
            let max_chars = args.max_chars.unwrap_or(20_000).min(200_000);
            let top_chunks = args.top_chunks.unwrap_or(5).min(50);
            let max_chunk_chars = args.max_chunk_chars.unwrap_or(500).min(5_000);
            let include_links = args.include_links.unwrap_or(false);
            let max_links = args.max_links.unwrap_or(50).min(500);
            // Default behavior: return full extracted text when no query is provided (users asked for “extract”),
            // but keep it off when query is provided (callers usually want bounded chunks).
            let include_text = args.include_text.unwrap_or(args.query.is_none());
            let include_structure = args.include_structure.unwrap_or(false);
            let max_outline_items = args.max_outline_items.unwrap_or(25).min(200);
            let max_blocks = args.max_blocks.unwrap_or(40).min(200);
            let max_block_chars = args.max_block_chars.unwrap_or(400).min(2000);
            let semantic_rerank = args.semantic_rerank.unwrap_or(false);
            let semantic_top_k = args.semantic_top_k.unwrap_or(5).min(50);
            let fetch_backend = args.fetch_backend.unwrap_or_else(|| "local".to_string());
            let no_network = args.no_network.unwrap_or(false);

            let url = args.url.unwrap_or_default();
            let t0 = std::time::Instant::now();
            if url.trim().is_empty() {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "url": url,
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "url must be non-empty",
                        "Pass an absolute URL like https://example.com."
                    ),
                    "request": {
                        "fetch_backend": fetch_backend,
                        "no_network": no_network,
                        "timeout_ms": args.timeout_ms.or(Some(20_000)),
                        "max_bytes": args.max_bytes.or(Some(5_000_000)),
                        "cache": { "read": args.cache_read.unwrap_or(true), "write": args.cache_write.unwrap_or(true), "ttl_s": args.cache_ttl_s },
                        "width": width,
                        "max_chars": max_chars,
                        "query": args.query,
                        "include_text": include_text,
                        "include_links": include_links,
                        "max_links": max_links,
                        "include_structure": include_structure,
                        "max_outline_items": max_outline_items,
                        "max_blocks": max_blocks,
                        "max_block_chars": max_block_chars
                    }
                });
                add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if fetch_backend.as_str() != "local" && fetch_backend.as_str() != "firecrawl" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "url": url,
                    "error": error_obj(
                        ErrorCode::InvalidParams,
                        "unknown fetch_backend",
                        "Allowed fetch_backends: local, firecrawl"
                    ),
                    "request": { "fetch_backend": fetch_backend }
                });
                add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }

            if no_network && fetch_backend == "firecrawl" {
                let mut payload = serde_json::json!({
                    "ok": false,
                    "url": url,
                    "error": error_obj(
                        ErrorCode::NotSupported,
                        "no_network=true cannot be used with fetch_backend=\"firecrawl\"",
                        "Use fetch_backend=\"local\" with a warmed WEBPIPE_CACHE_DIR, or set no_network=false."
                    ),
                    "request": { "fetch_backend": fetch_backend, "no_network": no_network }
                });
                add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                return Ok(tool_result(payload));
            }
            if let Some(qs) = args.query.as_deref() {
                if qs.trim().is_empty() {
                    let mut payload = serde_json::json!({
                        "ok": false,
                        "url": url,
                        "error": error_obj(
                            ErrorCode::InvalidParams,
                            "query must be non-empty when provided",
                            "Either omit query, or pass a non-empty query string."
                        ),
                        "request": {
                            "fetch_backend": fetch_backend,
                            "timeout_ms": args.timeout_ms.or(Some(20_000)),
                            "max_bytes": args.max_bytes.or(Some(5_000_000)),
                            "cache": { "read": args.cache_read.unwrap_or(true), "write": args.cache_write.unwrap_or(true), "ttl_s": args.cache_ttl_s },
                            "width": width,
                            "max_chars": max_chars,
                            "query": args.query,
                            "include_text": include_text,
                            "include_links": include_links,
                            "max_links": max_links,
                            "include_structure": include_structure,
                            "max_outline_items": max_outline_items,
                            "max_blocks": max_blocks,
                            "max_block_chars": max_block_chars
                        }
                    });
                    add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                    return Ok(tool_result(payload));
                }
            }

            // Firecrawl mode: return markdown as extracted text, with limited link support.
            if fetch_backend == "firecrawl" {
                let timeout_ms = args.timeout_ms.unwrap_or(20_000);
                let max_age_ms = args.cache_ttl_s.map(|s| s.saturating_mul(1000));
                let fc = match webpipe_local::firecrawl::FirecrawlClient::from_env(
                    self.http.clone(),
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        let msg = e.to_string();
                        self.stats_record_fetch_backend(
                            "firecrawl",
                            false,
                            t0.elapsed().as_millis() as u64,
                            Some(&msg),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(
                                ErrorCode::NotConfigured,
                                msg,
                                "Set WEBPIPE_FIRECRAWL_API_KEY (or FIRECRAWL_API_KEY) to use fetch_backend=\"firecrawl\"."
                            ),
                            "request": { "fetch_backend": fetch_backend, "timeout_ms": timeout_ms }
                        });
                        add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                };

                let r = match fc.fetch_markdown(&url, timeout_ms, max_age_ms).await {
                    Ok(r) => r,
                    Err(e) => {
                        let msg = e.to_string();
                        self.stats_record_fetch_backend(
                            "firecrawl",
                            false,
                            t0.elapsed().as_millis() as u64,
                            Some(&msg),
                        );
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(
                                ErrorCode::FetchFailed,
                                msg,
                                "Firecrawl fetch failed. If this looks transient, retry later or reduce scope."
                            ),
                            "request": { "fetch_backend": fetch_backend, "timeout_ms": timeout_ms }
                        });
                        add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                };

                let md = r.markdown;
                let md_bytes = md.as_bytes().to_vec();
                let md_raw_bytes = Self::approx_bytes_len(&md);
                let extracted0 = webpipe_local::extract::ExtractedText {
                    engine: "firecrawl",
                    text: md,
                    warnings: Vec::new(),
                };
                let pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                    &md_bytes,
                    Some("text/markdown"),
                    url.as_str(),
                    extracted0,
                    webpipe_local::extract::ExtractPipelineCfg {
                        query: args.query.as_deref(),
                        width,
                        max_chars,
                        top_chunks,
                        max_chunk_chars,
                        include_structure,
                        max_outline_items,
                        max_blocks,
                        max_block_chars,
                    },
                );
                let extracted = pipeline.extracted;
                let text = extracted.text.clone();
                let n = pipeline.text_chars;
                let clipped = pipeline.text_truncated;
                let mut warnings: Vec<&'static str> = Vec::new();
                if clipped {
                    warnings.push("text_truncated_by_max_chars");
                }

                let mut payload = serde_json::json!({
                    "ok": true,
                    "fetch_backend": "firecrawl",
                    "url": url,
                    "final_url": url,
                    "status": 200,
                    "content_type": "text/markdown",
                    "bytes": md_raw_bytes,
                    "truncated": clipped,
                    "text_chars": n,
                    "text_truncated": clipped,
                    "extraction": {
                        "engine": "firecrawl",
                        "width": width,
                        "max_chars": max_chars
                    },
                    "timings_ms": {
                        "total": t0.elapsed().as_millis(),
                        "firecrawl_fetch": r.elapsed_ms
                    }
                });
                add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                payload["request"] = serde_json::json!({
                    "fetch_backend": "firecrawl",
                    "timeout_ms": timeout_ms,
                    "cache_ttl_s": args.cache_ttl_s,
                    "width": width,
                    "max_chars": max_chars,
                    "query": args.query,
                    "include_text": include_text,
                    "include_links": include_links,
                    "max_links": max_links,
                    "include_structure": include_structure,
                    "max_outline_items": max_outline_items,
                    "max_blocks": max_blocks,
                    "max_block_chars": max_block_chars
                });
                if include_text {
                    payload["text"] = serde_json::json!(text);
                }
                if include_links {
                    payload["links"] = serde_json::json!([]);
                    warnings.push("links_unavailable_for_firecrawl");
                }
                if !warnings.is_empty() {
                    payload["warnings"] = serde_json::json!(warnings);
                    let codes = warning_codes_from(&warnings);
                    payload["warning_codes"] = serde_json::json!(codes.clone());
                    payload["warning_hints"] = warning_hints_from(&codes);
                    self.stats_record_warnings(&warnings);
                }
                self.stats_record_fetch_backend(
                    "firecrawl",
                    true,
                    t0.elapsed().as_millis() as u64,
                    None,
                );
                // Unify: add `extract` object (keep existing top-level fields for back-compat).
                payload["extract"] = serde_json::json!({
                    "engine": "firecrawl",
                    "width": width,
                    "max_chars": max_chars,
                    "text_chars": n,
                    "text_truncated": clipped,
                    "top_chunks": top_chunks,
                    "max_chunk_chars": max_chunk_chars
                });
                if include_structure {
                    if let Some(s) = pipeline.structure.as_ref() {
                        payload["structure"] = serde_json::json!(s);
                        payload["extract"]["structure"] = serde_json::json!(s);
                    }
                }
                if let Some(q) = args.query.as_deref() {
                    if !q.trim().is_empty() {
                        payload["query"] = serde_json::json!(q);
                        payload["chunks"] = serde_json::json!(pipeline.chunks);
                        payload["extraction"]["top_chunks"] = serde_json::json!(top_chunks);
                        payload["extraction"]["max_chunk_chars"] =
                            serde_json::json!(max_chunk_chars);
                        payload["extract"]["chunks"] = serde_json::json!(pipeline.chunks);
                    }
                }
                if semantic_rerank
                    && args
                        .query
                        .as_deref()
                        .map(|s| !s.trim().is_empty())
                        .unwrap_or(false)
                {
                    let cands: Vec<(usize, usize, String)> = pipeline
                        .chunks
                        .iter()
                        .map(|c| (c.start_char, c.end_char, c.text.clone()))
                        .collect();
                    let q0 = args.query.clone().unwrap_or_default();
                    let sem = tokio::task::spawn_blocking(move || {
                        webpipe_local::semantic::semantic_rerank_chunks(&q0, &cands, semantic_top_k)
                    })
                    .await
                    .unwrap_or_else(|_| {
                        webpipe_local::semantic::SemanticRerankResult {
                            ok: false,
                            backend: "unknown".to_string(),
                            model_id: None,
                            cache_hits: 0,
                            cache_misses: 0,
                            chunks: Vec::new(),
                            warnings: vec!["semantic_rerank_task_failed"],
                        }
                    });
                    payload["semantic"] = serde_json::json!(sem);
                    payload["extract"]["semantic"] = serde_json::json!(sem);
                }
                if include_links {
                    payload["extract"]["links"] = serde_json::json!([]);
                    payload["extract"]["max_links"] = serde_json::json!(max_links);
                }
                return Ok(tool_result(payload));
            }

            let req = FetchRequest {
                url: url.clone(),
                timeout_ms: args.timeout_ms.or(Some(20_000)),
                max_bytes: args.max_bytes.or(Some(5_000_000)),
                headers: BTreeMap::new(),
                cache: FetchCachePolicy {
                    read: args.cache_read.unwrap_or(true) || no_network,
                    write: if no_network {
                        false
                    } else {
                        args.cache_write.unwrap_or(true)
                    },
                    ttl_s: args.cache_ttl_s,
                },
            };

            // Cache-only mode (no network): return ok=false on cache miss.
            let resp = if no_network {
                match self.fetcher.cache_get(&req) {
                    Ok(Some(r)) => r,
                    Ok(None) => {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(
                                ErrorCode::NotSupported,
                                "cache miss in no_network mode",
                                "Warm the cache first (run without no_network), or set no_network=false."
                            ),
                            "request": { "fetch_backend": "local", "no_network": true, "cache": { "read": true, "write": false, "ttl_s": req.cache.ttl_s } }
                        });
                        add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                    Err(e) => {
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(ErrorCode::CacheError, e.to_string(), "Cache read failed in no_network mode."),
                            "request": { "fetch_backend": "local", "no_network": true }
                        });
                        add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                }
            } else {
                match self.fetcher.fetch(&req).await {
                    Ok(r) => r,
                    Err(e) => {
                        let (code, hint) = match &e {
                            WebpipeError::InvalidUrl(_) => (
                                ErrorCode::InvalidUrl,
                                "Check that the URL is absolute (includes http/https) and is well-formed.",
                            ),
                            WebpipeError::Fetch(_) => (
                                ErrorCode::FetchFailed,
                                "Fetch failed. If this looks transient, retry with a smaller max_bytes and a larger timeout_ms; otherwise try a different URL.",
                            ),
                            WebpipeError::Cache(_) => (
                                ErrorCode::CacheError,
                                "Cache failed. Retry with cache_read=false/cache_write=false to isolate network vs cache issues.",
                            ),
                            WebpipeError::Search(_) => (
                                ErrorCode::UnexpectedError,
                                "Unexpected search error while fetching. This is likely a bug; include this payload when reporting.",
                            ),
                            WebpipeError::Llm(_) => (
                                ErrorCode::UnexpectedError,
                                "Unexpected LLM error while fetching. This is likely a bug; include this payload when reporting.",
                            ),
                            WebpipeError::NotConfigured(_) => (
                                ErrorCode::NotConfigured,
                                "This fetch backend should not require API keys; check your server environment and configuration.",
                            ),
                            WebpipeError::NotSupported(_) => (
                                ErrorCode::NotSupported,
                                "This operation is not supported by the current backend/config.",
                            ),
                        };
                        let mut payload = serde_json::json!({
                            "ok": false,
                            "url": url,
                            "error": error_obj(code, e.to_string(), hint),
                            "request": {
                                "fetch_backend": fetch_backend,
                                "no_network": no_network,
                                "timeout_ms": req.timeout_ms,
                                "max_bytes": req.max_bytes,
                                "cache": { "read": req.cache.read, "write": req.cache.write, "ttl_s": req.cache.ttl_s },
                                "width": width,
                                "max_chars": max_chars
                            }
                        });
                        add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());
                        return Ok(tool_result(payload));
                    }
                }
            };

            let extracted0 = webpipe_local::extract::best_effort_text_from_bytes(
                &resp.bytes,
                resp.content_type.as_deref(),
                resp.final_url.as_str(),
                width,
                500,
            );
            let cfg0 = webpipe_local::extract::ExtractPipelineCfg {
                query: args.query.as_deref(),
                width,
                max_chars,
                top_chunks,
                max_chunk_chars,
                include_structure,
                max_outline_items,
                max_blocks,
                max_block_chars,
            };

            #[cfg(feature = "vision-gemini")]
            let mut pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                &resp.bytes,
                resp.content_type.as_deref(),
                resp.final_url.as_str(),
                extracted0,
                cfg0.clone(),
            );
            #[cfg(not(feature = "vision-gemini"))]
            let pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                &resp.bytes,
                resp.content_type.as_deref(),
                resp.final_url.as_str(),
                extracted0,
                cfg0,
            );

            #[cfg(feature = "vision-gemini")]
            let mut vision_model: Option<String> = None;
            #[cfg(not(feature = "vision-gemini"))]
            let vision_model: Option<String> = None;

            // Opportunistic multimodal: if we fetched an image and local extraction produced no text,
            // optionally call Gemini Flash (feature-gated, opt-in via WEBPIPE_VISION).
            #[cfg(feature = "vision-gemini")]
            {
                let vision_mode = webpipe_local::vision_gemini::gemini_enabled_mode_from_env();
                let is_image_like = resp
                    .content_type
                    .as_deref()
                    .unwrap_or("")
                    .to_ascii_lowercase()
                    .starts_with("image/")
                    || webpipe_local::extract::bytes_look_like_image(&resp.bytes);
                if !no_network
                    && vision_mode != "off"
                    && is_image_like
                    && pipeline.text_chars == 0
                    && !resp.bytes.is_empty()
                    && webpipe_local::vision_gemini::gemini_api_key_from_env().is_some()
                {
                    let mime0 = resp
                        .content_type
                        .as_deref()
                        .unwrap_or("application/octet-stream");
                    let mime = mime0.split(';').next().unwrap_or(mime0).trim();

                    match webpipe_local::vision_gemini::gemini_image_to_text(
                        self.http.clone(),
                        &resp.bytes,
                        mime,
                    )
                    .await
                    {
                        Ok((model_id, text)) => {
                            let ex = webpipe_local::extract::ExtractedText {
                                engine: "gemini_vision",
                                text,
                                warnings: vec!["gemini_used"],
                            };
                            pipeline = webpipe_local::extract::extract_pipeline_from_extracted(
                                &resp.bytes,
                                resp.content_type.as_deref(),
                                resp.final_url.as_str(),
                                ex,
                                cfg0,
                            );
                            vision_model = Some(model_id);
                        }
                        Err(code) => {
                            if vision_mode == "strict" {
                                pipeline.extracted.warnings.push(code);
                            } else {
                                pipeline.extracted.warnings.push("gemini_failed");
                            }
                        }
                    }
                }
            }

            let extracted = pipeline.extracted;
            let text = extracted.text.clone();
            let n = pipeline.text_chars;
            let clipped = pipeline.text_truncated;
            let empty_extraction = n == 0 && !resp.bytes.is_empty();
            let is_pdf_like = Self::content_type_is_pdf(resp.content_type.as_deref())
                || Self::url_looks_like_pdf(resp.final_url.as_str())
                || webpipe_local::extract::bytes_look_like_pdf(&resp.bytes)
                || extracted.engine.starts_with("pdf-");
            let mut warnings: Vec<&'static str> = Vec::new();
            if resp.truncated {
                warnings.push("body_truncated_by_max_bytes");
            }
            if clipped {
                warnings.push("text_truncated_by_max_chars");
            }
            if empty_extraction {
                warnings.push("empty_extraction");
            }
            for w in &extracted.warnings {
                warnings.push(*w);
            }
            if no_network {
                warnings.push("cache_only");
            }
            if extracted.engine == "html_main"
                && resp.bytes.len() >= 50_000
                && pipeline
                    .structure
                    .as_ref()
                    .is_some_and(structure_looks_like_ui_shell)
            {
                warnings.push("main_content_low_signal");
            }

            let mut payload = serde_json::json!({
                "ok": true,
                "fetch_backend": "local",
                "url": resp.url,
                "final_url": resp.final_url,
                "status": resp.status,
                "content_type": resp.content_type,
                "bytes": resp.bytes.len(),
                "truncated": resp.truncated || clipped,
                "text_chars": n,
                "text_truncated": clipped,
                "extraction": {
                    "engine": extracted.engine,
                    "width": width,
                    "max_chars": max_chars
                },
                "timings_ms": {
                    "total": t0.elapsed().as_millis()
                }
            });
            add_envelope_fields(&mut payload, "web_extract", t0.elapsed().as_millis());

            payload["request"] = serde_json::json!({
                "fetch_backend": "local",
                "no_network": no_network,
                "timeout_ms": req.timeout_ms,
                "max_bytes": req.max_bytes,
                "cache": { "read": req.cache.read, "write": req.cache.write, "ttl_s": req.cache.ttl_s },
                "width": width,
                "max_chars": max_chars,
                "query": args.query,
                "include_text": include_text,
                "include_links": include_links,
                "max_links": max_links,
                "include_structure": include_structure,
                "max_outline_items": max_outline_items,
                "max_blocks": max_blocks,
                "max_block_chars": max_block_chars,
                "semantic_rerank": semantic_rerank,
                "semantic_top_k": semantic_top_k
            });

            if include_links && is_pdf_like {
                warnings.push("links_unavailable_for_pdf");
            }
            if !warnings.is_empty() {
                payload["warnings"] = serde_json::json!(warnings);
                let codes = warning_codes_from(&warnings);
                payload["warning_codes"] = serde_json::json!(codes.clone());
                payload["warning_hints"] = warning_hints_from(&codes);
            }

            if include_text {
                payload["text"] = serde_json::json!(text);
            }

            // Unify with other tools: include an `extract` object (keep existing fields for back-compat).
            payload["extract"] = serde_json::json!({
                "engine": extracted.engine,
                "width": width,
                "max_chars": max_chars,
                "text_chars": n,
                "text_truncated": clipped,
                "top_chunks": top_chunks,
                "max_chunk_chars": max_chunk_chars,
                "chunks": pipeline.chunks
            });

            if let Some(vm) = vision_model.as_ref() {
                payload["extract"]["vision_model"] = serde_json::json!(vm);
            }

            if include_structure {
                if let Some(s) = pipeline.structure.as_ref() {
                    payload["structure"] = serde_json::json!(s);
                    payload["extract"]["structure"] = serde_json::json!(s);
                }
            }

            if let Some(q) = payload["request"]["query"].as_str().map(|s| s.to_string()) {
                payload["query"] = serde_json::json!(&q);
                payload["chunks"] = serde_json::json!(pipeline.chunks);
                payload["extraction"]["top_chunks"] = serde_json::json!(top_chunks);
                payload["extraction"]["max_chunk_chars"] = serde_json::json!(max_chunk_chars);

                if semantic_rerank {
                    let cands: Vec<(usize, usize, String)> = pipeline
                        .chunks
                        .iter()
                        .map(|c| (c.start_char, c.end_char, c.text.clone()))
                        .collect();
                    let q0 = q.clone();
                    let sem = tokio::task::spawn_blocking(move || {
                        webpipe_local::semantic::semantic_rerank_chunks(&q0, &cands, semantic_top_k)
                    })
                    .await
                    .unwrap_or_else(|_| {
                        webpipe_local::semantic::SemanticRerankResult {
                            ok: false,
                            backend: "unknown".to_string(),
                            model_id: None,
                            cache_hits: 0,
                            cache_misses: 0,
                            chunks: Vec::new(),
                            warnings: vec!["semantic_rerank_task_failed"],
                        }
                    });
                    payload["semantic"] = serde_json::json!(sem);
                    payload["extract"]["semantic"] = serde_json::json!(sem);
                }
            }

            if include_links {
                if is_pdf_like {
                    payload["links"] = serde_json::json!([]);
                    payload["extraction"]["max_links"] = serde_json::json!(max_links);
                    payload["extract"]["links"] = serde_json::json!([]);
                    payload["extract"]["max_links"] = serde_json::json!(max_links);
                } else {
                    let html = resp.text_lossy();
                    let final_url = payload["final_url"].as_str().unwrap_or("");
                    let links =
                        webpipe_local::links::extract_links(&html, Some(final_url), max_links);
                    payload["links"] = serde_json::json!(links);
                    payload["extraction"]["max_links"] = serde_json::json!(max_links);
                    payload["extract"]["links"] = serde_json::json!(links);
                    payload["extract"]["max_links"] = serde_json::json!(max_links);
                }
            }

            Ok(tool_result(payload))
        }
    }

    #[tool_handler]
    impl rmcp::ServerHandler for WebpipeMcp {
        fn get_info(&self) -> ServerInfo {
            ServerInfo {
                instructions: Some(
                    "Local fetch/search plumbing. Hard mode (render/unblock) is opt-in; outputs are JSON and schema-versioned."
                        .to_string(),
                ),
                capabilities: ServerCapabilities::builder().enable_tools().build(),
                ..Default::default()
            }
        }
    }

    pub(crate) async fn serve_stdio() -> Result<(), McpError> {
        let svc = WebpipeMcp::new()?;
        let running = svc
            .serve(stdio())
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        // Keep the stdio server alive until the client closes.
        running
            .waiting()
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use proptest::prelude::*;

        fn p<T>(v: T) -> Parameters<Option<T>> {
            Parameters(Some(v))
        }

        struct EnvGuard {
            // Hold the lock for the full test (env vars are process-global).
            _lock: std::sync::MutexGuard<'static, ()>,
            saved: Vec<(String, Option<String>)>,
        }

        impl EnvGuard {
            fn new(keys: &[&str]) -> Self {
                // If a prior test panicked while holding the lock, recover the guard so we
                // don't cascade failures behind a PoisonError. (Env is process-global anyway.)
                let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                let saved: Vec<(String, Option<String>)> = keys
                    .iter()
                    .map(|k| (k.to_string(), std::env::var(k).ok()))
                    .collect();
                for (k, _) in &saved {
                    std::env::remove_var(k);
                }
                Self { _lock: lock, saved }
            }

            fn set(&self, k: &str, v: &str) {
                std::env::set_var(k, v);
            }
        }

        impl Drop for EnvGuard {
            fn drop(&mut self) {
                for (k, v) in self.saved.drain(..) {
                    match v {
                        Some(val) => std::env::set_var(k, val),
                        None => std::env::remove_var(k),
                    }
                }
            }
        }

        fn payload_from_call_tool_result(r: &CallToolResult) -> serde_json::Value {
            let s = r
                .content
                .first()
                .and_then(|c| c.as_text())
                .map(|t| t.text.clone())
                .unwrap_or_default();
            serde_json::from_str(&s).expect("tool result should be a JSON string")
        }

        // Env vars are global; serialize tests that mutate them.
        static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        const SEARCH_ENV_KEYS: [&str; 6] = [
            "WEBPIPE_BRAVE_API_KEY",
            "BRAVE_SEARCH_API_KEY",
            "WEBPIPE_TAVILY_API_KEY",
            "TAVILY_API_KEY",
            "WEBPIPE_SEARXNG_ENDPOINT",
            "WEBPIPE_ARXIV_ENDPOINT",
        ];
        const PERPLEXITY_ENV_KEYS: [&str; 3] = [
            "WEBPIPE_PERPLEXITY_API_KEY",
            "PERPLEXITY_API_KEY",
            "WEBPIPE_PERPLEXITY_ENDPOINT",
        ];

        #[test]
        fn routing_context_query_key_prefers_contextual_window() {
            let env = EnvGuard::new(&[
                "WEBPIPE_ROUTING_CONTEXT",
                "WEBPIPE_ROUTING_MAX_CONTEXTS",
                "WEBPIPE_ROUTING_WINDOW",
            ]);
            env.set("WEBPIPE_ROUTING_CONTEXT", "query_key");
            env.set("WEBPIPE_ROUTING_MAX_CONTEXTS", "10");
            env.set("WEBPIPE_ROUTING_WINDOW", "10");

            let mcp = WebpipeMcp::new().expect("mcp new");

            // Seed contextual stats: brave is junky, tavily is clean.
            mcp.stats_record_search_provider_qk("brave", true, 1, 10, None, Some("qk1"));
            mcp.stats_set_last_search_outcome_junk_level_qk("brave", true, false, Some("qk1"));

            mcp.stats_record_search_provider_qk("tavily", true, 1, 10, None, Some("qk1"));
            mcp.stats_set_last_search_outcome_junk_level_qk("tavily", false, false, Some("qk1"));

            let (summaries, used) = mcp.snapshot_search_summaries_for_query_key(Some("qk1"));
            assert_eq!(used, "query_key");

            let arms = vec!["brave".to_string(), "tavily".to_string()];
            let cfg = muxer::MabConfig {
                exploration_c: 0.0,
                ..muxer::MabConfig::default()
            };
            let sel = muxer::select_mab(&arms, &summaries, &cfg);
            assert_eq!(sel.chosen, "tavily");
        }

        #[test]
        fn routing_context_query_key_is_bounded_by_max_contexts() {
            let env = EnvGuard::new(&[
                "WEBPIPE_ROUTING_CONTEXT",
                "WEBPIPE_ROUTING_MAX_CONTEXTS",
                "WEBPIPE_ROUTING_WINDOW",
            ]);
            env.set("WEBPIPE_ROUTING_CONTEXT", "query_key");
            env.set("WEBPIPE_ROUTING_MAX_CONTEXTS", "1");
            env.set("WEBPIPE_ROUTING_WINDOW", "10");

            let mcp = WebpipeMcp::new().expect("mcp new");

            mcp.stats_record_search_provider_qk("brave", true, 1, 10, None, Some("qk_a"));
            mcp.stats_record_search_provider_qk("brave", true, 1, 10, None, Some("qk_b"));

            let s = mcp.stats_lock();
            assert!(s.search_windows_by_query_key.len() <= 1);
        }

        #[test]
        fn compute_search_junk_label_hard_always_true() {
            let env = EnvGuard::new(&[
                "WEBPIPE_ROUTING_SOFT_JUNK_MIN",
                "WEBPIPE_ROUTING_SOFT_JUNK_FRAC",
            ]);
            env.set("WEBPIPE_ROUTING_SOFT_JUNK_MIN", "2");
            env.set("WEBPIPE_ROUTING_SOFT_JUNK_FRAC", "0.5");
            assert!(WebpipeMcp::compute_search_junk_label(1, 0, 3));
        }

        #[test]
        fn compute_search_junk_label_soft_respects_min_and_frac() {
            let env = EnvGuard::new(&[
                "WEBPIPE_ROUTING_SOFT_JUNK_MIN",
                "WEBPIPE_ROUTING_SOFT_JUNK_FRAC",
            ]);
            env.set("WEBPIPE_ROUTING_SOFT_JUNK_MIN", "2");
            env.set("WEBPIPE_ROUTING_SOFT_JUNK_FRAC", "0.5");
            // Below min_soft => false.
            assert!(!WebpipeMcp::compute_search_junk_label(0, 1, 3));
            // Meets min_soft but below frac => false (2/5 < 0.5).
            assert!(!WebpipeMcp::compute_search_junk_label(0, 2, 5));
            // Meets min_soft and frac => true (2/4 >= 0.5).
            assert!(WebpipeMcp::compute_search_junk_label(0, 2, 4));
        }

        #[test]
        fn hard_junk_sets_hard_flag_in_routing_window() {
            let env = EnvGuard::new(&[
                "WEBPIPE_ROUTING_CONTEXT",
                "WEBPIPE_ROUTING_MAX_CONTEXTS",
                "WEBPIPE_ROUTING_WINDOW",
            ]);
            env.set("WEBPIPE_ROUTING_CONTEXT", "query_key");
            env.set("WEBPIPE_ROUTING_MAX_CONTEXTS", "10");
            env.set("WEBPIPE_ROUTING_WINDOW", "10");

            let mcp = WebpipeMcp::new().expect("mcp new");
            mcp.stats_record_search_provider_qk("brave", true, 1, 10, None, Some("qk1"));
            mcp.stats_set_last_search_outcome_junk_level_qk("brave", true, true, Some("qk1"));

            let s = mcp.stats_lock();
            let per = s.search_windows_by_query_key.get("qk1").expect("qk1");
            let w = per.get("brave").expect("brave");
            let sum = w.summary();
            assert_eq!(sum.junk, 1);
            assert_eq!(sum.hard_junk, 1);
        }

        #[test]
        fn url_is_probably_auth_wall_is_reasonable() {
            assert!(url_looks_like_auth_or_challenge(
                "https://medium.com/m/signin?operation=login"
            ));
            assert!(url_looks_like_auth_or_challenge(
                "https://accounts.google.com/o/oauth2/v2/auth"
            ));
            assert!(!url_looks_like_auth_or_challenge(
                "https://example.com/docs/getting-started"
            ));
        }

        #[test]
        fn filter_low_signal_chunks_drops_nextjs_bundle_gunk() {
            let chunks = vec![
                webpipe_local::extract::ScoredChunk {
                    start_char: 0,
                    end_char: 10,
                    score: 9,
                    text: "(self.__next_s=self.__next_s||[]).push([0,{\"suppressHydrationWarning\":true,\"children\":\"(function(a,b,c,d){var e;let f\"".to_string(),
                },
                webpipe_local::extract::ScoredChunk {
                    start_char: 11,
                    end_char: 42,
                    score: 3,
                    text: "Tool results may contain structured content.".to_string(),
                },
            ];
            let (out, filtered) = WebpipeMcp::filter_low_signal_chunks(chunks);
            assert!(filtered);
            assert_eq!(out.len(), 1);
            assert!(out[0].text.contains("structured content"));
        }

        #[test]
        fn truncate_to_chars_is_utf8_safe_and_consistent() {
            let s = "aé🙂中";
            let (out, n, clipped) = WebpipeMcp::truncate_to_chars(s, 3);
            assert_eq!(n, out.chars().count());
            assert_eq!(n, 3);
            assert_eq!(out, "aé🙂");
            assert!(clipped);

            let (out2, n2, clipped2) = WebpipeMcp::truncate_to_chars(s, 10);
            assert_eq!(n2, out2.chars().count());
            assert_eq!(out2, s);
            assert!(!clipped2);
        }

        proptest! {
            #[test]
            fn query_key_never_panics_for_arbitrary_unicode(s in any::<String>()) {
                let _ = WebpipeMcp::query_key(&s);
            }

            #[test]
            fn query_key_scrub_output_is_ascii_normalized(s in any::<String>()) {
                if let Some(k) = WebpipeMcp::query_key(&s) {
                    // scrub() is documented as ASCII-lowercase + whitespace-normalized.
                    prop_assert!(k.is_ascii());
                    prop_assert!(!k.starts_with(' '));
                    prop_assert!(!k.ends_with(' '));
                    prop_assert!(!k.contains("  "));
                    for ch in k.chars() {
                        prop_assert!(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == ' ');
                    }
                }
            }

            #[test]
            fn pareto_selection_is_bounded_and_deterministic(
                urls in prop::collection::vec(any::<String>(), 0..30),
                scores in prop::collection::vec(any::<u64>(), 0..30),
                texts in prop::collection::vec(any::<String>(), 0..30),
                warnings in prop::collection::vec(0usize..6, 0..30),
                cache_hits in prop::collection::vec(any::<bool>(), 0..30),
                top_k in 1usize..10,
            ) {
                let n = urls.len().min(scores.len()).min(texts.len()).min(warnings.len()).min(cache_hits.len());
                let mut cands = Vec::new();
                for i in 0..n {
                    let start = i;
                    let end = i.saturating_add(1);
                    cands.push(ChunkCandidate{
                        url: urls[i].clone(),
                        score: scores[i],
                        start_char: start,
                        end_char: end,
                        text: texts[i].clone(),
                        warnings_count: warnings[i],
                        cache_hit: cache_hits[i],
                    });
                }

                let a = WebpipeMcp::select_top_chunks(cands.clone(), top_k, "pareto");
                let b = WebpipeMcp::select_top_chunks(cands.clone(), top_k, "pareto");
                prop_assert!(a.len() <= top_k);
                prop_assert_eq!(a.len(), b.len());

                let sig = |c: &ChunkCandidate| (c.url.clone(), c.start_char, c.end_char, WebpipeMcp::score_key(c.score));
                let sa: Vec<_> = a.iter().map(sig).collect();
                let sb: Vec<_> = b.iter().map(sig).collect();
                prop_assert_eq!(sa, sb);
            }

            #[test]
            fn score_selection_is_sorted_and_deterministic(
                urls in prop::collection::vec(any::<String>(), 0..30),
                scores in prop::collection::vec(any::<u64>(), 0..30),
                texts in prop::collection::vec(any::<String>(), 0..30),
                warnings in prop::collection::vec(0usize..6, 0..30),
                cache_hits in prop::collection::vec(any::<bool>(), 0..30),
                top_k in 1usize..10,
            ) {
                let n = urls.len().min(scores.len()).min(texts.len()).min(warnings.len()).min(cache_hits.len());
                let mut cands = Vec::new();
                for i in 0..n {
                    let start = i;
                    let end = i.saturating_add(1);
                    cands.push(ChunkCandidate{
                        url: urls[i].clone(),
                        score: scores[i],
                        start_char: start,
                        end_char: end,
                        text: texts[i].clone(),
                        warnings_count: warnings[i],
                        cache_hit: cache_hits[i],
                    });
                }

                let a = WebpipeMcp::select_top_chunks(cands.clone(), top_k, "score");
                let b = WebpipeMcp::select_top_chunks(cands.clone(), top_k, "score");

                prop_assert!(a.len() <= top_k);
                prop_assert_eq!(a.len(), b.len());

                // Deterministic signature.
                let sig = |c: &ChunkCandidate| (c.url.clone(), c.start_char, c.end_char, WebpipeMcp::score_key(c.score));
                let sa: Vec<_> = a.iter().map(sig).collect();
                let sb: Vec<_> = b.iter().map(sig).collect();
                prop_assert_eq!(sa, sb);

                // Sorted by (score desc, url asc, start_char asc) per implementation.
                for w in a.windows(2) {
                    let x = &w[0];
                    let y = &w[1];
                    let kx = (-(WebpipeMcp::score_key(x.score)), x.url.clone(), x.start_char);
                    let ky = (-(WebpipeMcp::score_key(y.score)), y.url.clone(), y.start_char);
                    prop_assert!(kx <= ky);
                }
            }

            #[test]
            fn error_obj_has_stable_shape(msg in any::<String>(), hint in any::<String>()) {
                let codes = [
                    ErrorCode::InvalidParams,
                    ErrorCode::InvalidUrl,
                    ErrorCode::NotConfigured,
                    ErrorCode::NotSupported,
                    ErrorCode::ProviderUnavailable,
                    ErrorCode::FetchFailed,
                    ErrorCode::SearchFailed,
                    ErrorCode::CacheError,
                    ErrorCode::UnexpectedError,
                ];

                for code in codes {
                    let v = error_obj(code, &msg, &hint);
                    prop_assert!(v.is_object());
                    prop_assert_eq!(v.get("code").and_then(|x| x.as_str()), Some(code.as_str()));
                    prop_assert!(v.get("message").and_then(|x| x.as_str()).is_some());
                    prop_assert!(v.get("hint").and_then(|x| x.as_str()).is_some());
                    prop_assert!(v.get("retryable").and_then(|x| x.as_bool()).is_some());
                }
            }
        }

        #[tokio::test]
        async fn web_search_auto_not_configured_has_stable_shape_and_selection() {
            let _env = EnvGuard::new(&SEARCH_ENV_KEYS);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search(p(WebSearchArgs {
                    query: Some("q".to_string()),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    max_results: Some(3),
                    language: None,
                    country: None,
                }))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["schema_version"].as_u64(), Some(1));
            assert_eq!(v["kind"].as_str(), Some("web_search"));
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(v["provider"].as_str(), Some("auto"));
            assert_eq!(v["request"]["provider"].as_str(), Some("auto"));
            assert_eq!(v["query"].as_str(), Some("q"));
            assert_eq!(v["max_results"].as_u64(), Some(3));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::NotConfigured.as_str())
            );
            assert_eq!(v["selection"]["requested_provider"].as_str(), Some("auto"));
            assert_eq!(v["selection"]["selected_provider"].as_str(), Some("none"));
        }

        #[tokio::test]
        async fn web_search_explicit_provider_not_configured_echoes_provider_and_query() {
            let _env = EnvGuard::new(&SEARCH_ENV_KEYS);

            let svc = WebpipeMcp::new().expect("new");

            // brave (not configured)
            let r1 = svc
                .web_search(p(WebSearchArgs {
                    query: Some("q1".to_string()),
                    provider: Some("brave".to_string()),
                    auto_mode: None,
                    max_results: Some(1),
                    language: None,
                    country: None,
                }))
                .await
                .expect("call brave");
            let v1 = payload_from_call_tool_result(&r1);
            assert_eq!(v1["ok"].as_bool(), Some(false));
            assert_eq!(v1["provider"].as_str(), Some("brave"));
            assert_eq!(v1["request"]["provider"].as_str(), Some("brave"));
            assert_eq!(v1["query"].as_str(), Some("q1"));
            assert_eq!(
                v1["error"]["code"].as_str(),
                Some(ErrorCode::NotConfigured.as_str())
            );

            // tavily (not configured)
            let r2 = svc
                .web_search(p(WebSearchArgs {
                    query: Some("q2".to_string()),
                    provider: Some("tavily".to_string()),
                    auto_mode: None,
                    max_results: Some(1),
                    language: None,
                    country: None,
                }))
                .await
                .expect("call tavily");
            let v2 = payload_from_call_tool_result(&r2);
            assert_eq!(v2["ok"].as_bool(), Some(false));
            assert_eq!(v2["provider"].as_str(), Some("tavily"));
            assert_eq!(v2["request"]["provider"].as_str(), Some("tavily"));
            assert_eq!(v2["query"].as_str(), Some("q2"));
            assert_eq!(
                v2["error"]["code"].as_str(),
                Some(ErrorCode::NotConfigured.as_str())
            );
        }

        #[tokio::test]
        async fn web_deep_research_not_configured_is_stable_and_offline_friendly() {
            // Remove Perplexity key(s), and also remove search keys to prove we can run offline mode.
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            let _env = EnvGuard::new(&keys);

            // Local fixture server for deterministic fetch/extract.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [("content-type", "text/html")],
                        "<html><body><h1>Hello</h1><p>world</p></body></html>",
                    )
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "test question".to_string(),
                    audit: None,
                    synthesize: None,
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: None,
                    exploration: None,
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    max_results: Some(3),
                    max_urls: Some(1),
                    timeout_ms: Some(5_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(5_000),
                    top_chunks: Some(3),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: None,
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: Some("medium".to_string()),
                    max_tokens: Some(200),
                    temperature: Some(0.0),
                    top_p: None,
                    max_answer_chars: Some(2_000),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: None,
                })))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["schema_version"].as_u64(), Some(1));
            assert_eq!(v["kind"].as_str(), Some("web_deep_research"));
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::NotConfigured.as_str())
            );
            // Prove evidence exists and is successful, so failure is clearly "LLM not configured".
            assert_eq!(v["evidence"]["ok"].as_bool(), Some(true));
            assert_eq!(v["evidence"]["kind"].as_str(), Some("web_search_extract"));
        }

        #[tokio::test]
        async fn web_deep_research_ollama_local_succeeds_with_no_network_if_cache_is_warmed() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            keys.extend_from_slice(&[
                "WEBPIPE_CACHE_DIR",
                "WEBPIPE_OLLAMA_ENABLE",
                "WEBPIPE_OLLAMA_BASE_URL",
                "WEBPIPE_OLLAMA_MODEL",
            ]);
            let env = EnvGuard::new(&keys);

            // Pre-warm cache (strictly offline): write a cached fetch response that matches
            // the FetchRequest knobs used by web_search_extract inside web_deep_research.
            let tmp = tempfile::tempdir().expect("tempdir");
            env.set("WEBPIPE_CACHE_DIR", tmp.path().to_str().unwrap());

            let url = "http://example.invalid/fixture".to_string();
            let html = "<html><body><h1>Hello</h1><p>cached world</p></body></html>";
            let cache = webpipe_local::FsCache::new(tmp.path().to_path_buf());
            let req = FetchRequest {
                url: url.clone(),
                timeout_ms: Some(2_000),
                max_bytes: Some(200_000),
                headers: BTreeMap::new(),
                cache: FetchCachePolicy {
                    read: true,
                    write: true,
                    ttl_s: Some(60),
                },
            };
            cache
                .put(
                    &req,
                    &webpipe_core::FetchResponse {
                        url: url.clone(),
                        final_url: url.clone(),
                        status: 200,
                        content_type: Some("text/html".to_string()),
                        headers: BTreeMap::new(),
                        bytes: html.as_bytes().to_vec(),
                        truncated: false,
                        source: FetchSource::Network,
                        timings_ms: BTreeMap::new(),
                    },
                )
                .expect("cache put");

            // Local Ollama stub server (localhost-only) for deterministic synthesis.
            use axum::{routing::post, Json, Router};
            use std::net::SocketAddr;
            let app = Router::new().route(
                "/api/chat",
                post(|_body: Json<serde_json::Value>| async move {
                    Json(serde_json::json!({
                        "message": { "role": "assistant", "content": "Local synthesis ok." }
                    }))
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            env.set("WEBPIPE_OLLAMA_ENABLE", "true");
            env.set("WEBPIPE_OLLAMA_BASE_URL", &format!("http://{addr}"));
            env.set("WEBPIPE_OLLAMA_MODEL", "test-model");

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "test question".to_string(),
                    audit: None,
                    synthesize: None,
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: None,
                    exploration: None,
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(5_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(5_000),
                    top_chunks: Some(2),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: None,
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: Some("medium".to_string()),
                    max_tokens: Some(200),
                    temperature: Some(0.0),
                    top_p: None,
                    max_answer_chars: Some(2_000),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("ollama".to_string()),
                })))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["schema_version"].as_u64(), Some(1));
            assert_eq!(v["kind"].as_str(), Some("web_deep_research"));
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["provider"].as_str(), Some("ollama"));
            assert_eq!(v["request"]["no_network"].as_bool(), Some(true));
            assert_eq!(v["evidence"]["ok"].as_bool(), Some(true));
            assert_eq!(v["evidence"]["kind"].as_str(), Some("web_search_extract"));
            assert!(v["warnings"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|x| x.as_str() == Some("llm_ollama_used")));
            assert!(v["answer"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("Local synthesis ok"));
        }

        #[tokio::test]
        async fn web_deep_research_openai_compat_local_succeeds_with_no_network_if_cache_is_warmed()
        {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            keys.extend_from_slice(&[
                "WEBPIPE_CACHE_DIR",
                "WEBPIPE_OPENAI_COMPAT_BASE_URL",
                "WEBPIPE_OPENAI_COMPAT_API_KEY",
                "WEBPIPE_OPENAI_COMPAT_MODEL",
            ]);
            let env = EnvGuard::new(&keys);

            let tmp = tempfile::tempdir().expect("tempdir");
            env.set("WEBPIPE_CACHE_DIR", tmp.path().to_str().unwrap());

            let url = "http://example.invalid/fixture".to_string();
            let html = "<html><body><h1>Hello</h1><p>cached world</p></body></html>";
            let cache = webpipe_local::FsCache::new(tmp.path().to_path_buf());
            let req = FetchRequest {
                url: url.clone(),
                timeout_ms: Some(2_000),
                max_bytes: Some(200_000),
                headers: BTreeMap::new(),
                cache: FetchCachePolicy {
                    read: true,
                    write: true,
                    ttl_s: Some(60),
                },
            };
            cache
                .put(
                    &req,
                    &webpipe_core::FetchResponse {
                        url: url.clone(),
                        final_url: url.clone(),
                        status: 200,
                        content_type: Some("text/html".to_string()),
                        headers: BTreeMap::new(),
                        bytes: html.as_bytes().to_vec(),
                        truncated: false,
                        source: FetchSource::Network,
                        timings_ms: BTreeMap::new(),
                    },
                )
                .expect("cache put");

            // OpenAI-compatible local stub.
            use axum::{routing::post, Json, Router};
            use std::net::SocketAddr;
            let app = Router::new().route(
                "/v1/chat/completions",
                post(|_body: Json<serde_json::Value>| async move {
                    Json(serde_json::json!({
                        "choices": [
                            { "message": { "role": "assistant", "content": "OpenAI-compat synthesis ok." } }
                        ]
                    }))
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            env.set("WEBPIPE_OPENAI_COMPAT_BASE_URL", &format!("http://{addr}"));
            env.set("WEBPIPE_OPENAI_COMPAT_MODEL", "local/test");

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "test question".to_string(),
                    audit: None,
                    synthesize: None,
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: None,
                    exploration: None,
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(5_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(5_000),
                    top_chunks: Some(2),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: None,
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: Some("local/test".to_string()),
                    search_mode: None,
                    reasoning_effort: Some("medium".to_string()),
                    max_tokens: Some(200),
                    temperature: Some(0.0),
                    top_p: None,
                    max_answer_chars: Some(2_000),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("openai_compat".to_string()),
                })))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["provider"].as_str(), Some("openai_compat"));
            assert!(v.get("evidence_pack").is_some());
            assert!(v["answer"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("OpenAI-compat synthesis ok"));
        }

        #[tokio::test]
        async fn web_deep_research_openai_compat_rejects_non_localhost_in_no_network_mode() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            keys.extend_from_slice(&[
                "WEBPIPE_OPENAI_COMPAT_BASE_URL",
                "WEBPIPE_OPENAI_COMPAT_MODEL",
            ]);
            let env = EnvGuard::new(&keys);
            env.set("WEBPIPE_OPENAI_COMPAT_BASE_URL", "https://example.com");
            env.set("WEBPIPE_OPENAI_COMPAT_MODEL", "x");

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "q".to_string(),
                    audit: None,
                    synthesize: None,
                    urls: Some(vec!["http://example.invalid/fixture".to_string()]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: None,
                    exploration: None,
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(1),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: None,
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: Some("x".to_string()),
                    search_mode: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(false),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("openai_compat".to_string()),
                })))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::NotSupported.as_str())
            );
        }

        #[tokio::test]
        async fn web_deep_research_audit_rejects_search_mode() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            let _env = EnvGuard::new(&keys);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "q".to_string(),
                    audit: Some(true),
                    synthesize: Some(false),
                    urls: Some(vec!["http://example.invalid/fixture".to_string()]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    exploration: Some("balanced".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(1),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: Some("off".to_string()),
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: Some("on".to_string()),
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("auto".to_string()),
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::NotSupported.as_str())
            );
        }

        #[tokio::test]
        async fn web_deep_research_audit_requires_include_evidence() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            let _env = EnvGuard::new(&keys);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "q".to_string(),
                    audit: Some(true),
                    synthesize: Some(false),
                    urls: Some(vec!["http://example.invalid/fixture".to_string()]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    exploration: Some("balanced".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(1),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: Some("off".to_string()),
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(false),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("auto".to_string()),
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
        }

        #[tokio::test]
        async fn web_deep_research_audit_fails_closed_when_perplexity_rejects_off_mode() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            let env = EnvGuard::new(&keys);

            // Local HTML evidence server (so evidence gathering succeeds).
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let html_app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Evidence</h1><p>Some text.</p></body></html>",
                    )
                }),
            );
            let html_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let html_addr: SocketAddr = html_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(html_listener, html_app)
                    .await
                    .expect("axum serve");
            });
            let url = format!("http://{}/", html_addr);

            // Local Perplexity stub:
            // - if search_mode == "off": return HTTP 400 (forcing the "off rejected" path)
            // - otherwise: return a minimal successful shape
            use axum::{routing::post, Json};
            let pplx_app = Router::new().route(
                "/chat/completions",
                post(|body: Json<serde_json::Value>| async move {
                    let sm = body
                        .get("search_mode")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if sm == "off" {
                        return (
                            axum::http::StatusCode::BAD_REQUEST,
                            "search_mode off not supported",
                        )
                            .into_response();
                    }
                    Json(serde_json::json!({
                        "choices": [
                            { "message": { "role": "assistant", "content": "ok" } }
                        ],
                        "citations": []
                    }))
                    .into_response()
                }),
            );
            use axum::response::IntoResponse;
            let pplx_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let pplx_addr: SocketAddr = pplx_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(pplx_listener, pplx_app)
                    .await
                    .expect("axum serve");
            });

            env.set("WEBPIPE_PERPLEXITY_API_KEY", "dummy");
            env.set(
                "WEBPIPE_PERPLEXITY_ENDPOINT",
                &format!("http://{pplx_addr}/chat/completions"),
            );

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "q".to_string(),
                    audit: Some(true),
                    synthesize: Some(true),
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    exploration: Some("balanced".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(1),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: Some("off".to_string()),
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("perplexity".to_string()),
                })))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::NotSupported.as_str())
            );
            assert!(v.get("evidence_pack").is_some());
        }

        #[tokio::test]
        async fn web_search_auto_fallback_uses_tavily_when_brave_is_rate_limited() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&["WEBPIPE_BRAVE_ENDPOINT", "WEBPIPE_TAVILY_ENDPOINT"]);
            let env = EnvGuard::new(&keys);

            // Local stub search endpoints.
            use axum::{routing::get, routing::post, Json, Router};
            use std::net::SocketAddr;
            let app = Router::new()
                // Brave returns 429.
                .route(
                    "/brave",
                    get(|| async {
                        (
                            axum::http::StatusCode::TOO_MANY_REQUESTS,
                            "Too Many Requests",
                        )
                    }),
                )
                // Tavily returns a minimal success shape.
                .route(
                    "/tavily",
                    post(|_body: Json<serde_json::Value>| async move {
                        Json(serde_json::json!({
                            "results": [
                                {"url":"https://example.com","title":"Example","content":"Hello"}
                            ],
                            "usage": { "credits": 1 }
                        }))
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });

            env.set("WEBPIPE_BRAVE_API_KEY", "dummy");
            env.set("WEBPIPE_TAVILY_API_KEY", "dummy");
            env.set("WEBPIPE_BRAVE_ENDPOINT", &format!("http://{addr}/brave"));
            env.set("WEBPIPE_TAVILY_ENDPOINT", &format!("http://{addr}/tavily"));

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search(p(WebSearchArgs {
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    query: Some("q".to_string()),
                    country: None,
                    language: None,
                    max_results: Some(1),
                }))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["provider"].as_str(), Some("auto"));
            assert_eq!(v["backend_provider"].as_str(), Some("tavily"));
            assert!(v["warnings"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|x| x.as_str() == Some("provider_failover")));
            assert!(v["providers"].is_array());
        }

        #[tokio::test]
        async fn web_search_auto_fallback_can_choose_tavily_first_when_brave_is_unhealthy() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&["WEBPIPE_BRAVE_ENDPOINT", "WEBPIPE_TAVILY_ENDPOINT"]);
            keys.extend_from_slice(&["WEBPIPE_ROUTING_CONTEXT", "WEBPIPE_ROUTING_WINDOW"]);
            let env = EnvGuard::new(&keys);
            env.set("WEBPIPE_ROUTING_CONTEXT", "global");
            env.set("WEBPIPE_ROUTING_WINDOW", "50");

            // Local stub search endpoints: both succeed.
            use axum::{routing::get, routing::post, Json, Router};
            use std::net::SocketAddr;
            let app = Router::new()
                .route(
                    "/brave",
                    get(|| async {
                        (
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            r#"{"web":{"results":[{"url":"https://example.com","title":"Example","description":"Hello"}]}}"#,
                        )
                    }),
                )
                .route(
                    "/tavily",
                    post(|_body: Json<serde_json::Value>| async move {
                        Json(serde_json::json!({
                            "results": [
                                {"url":"https://example.com","title":"Example","content":"Hello"}
                            ],
                            "usage": { "credits": 1 }
                        }))
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });

            env.set("WEBPIPE_BRAVE_API_KEY", "dummy");
            env.set("WEBPIPE_TAVILY_API_KEY", "dummy");
            env.set("WEBPIPE_BRAVE_ENDPOINT", &format!("http://{addr}/brave"));
            env.set("WEBPIPE_TAVILY_ENDPOINT", &format!("http://{addr}/tavily"));

            let svc = WebpipeMcp::new().expect("new");

            // Seed global stats: Brave is currently rate-limiting; Tavily is healthy.
            for _ in 0..10 {
                svc.stats_record_search_provider_qk("brave", false, 0, 10, Some("HTTP 429"), None);
            }
            for _ in 0..10 {
                svc.stats_record_search_provider_qk("tavily", true, 1, 10, None, None);
            }

            let r = svc
                .web_search(p(WebSearchArgs {
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    query: Some("q".to_string()),
                    country: None,
                    language: None,
                    max_results: Some(1),
                }))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["provider"].as_str(), Some("auto"));
            assert_eq!(v["backend_provider"].as_str(), Some("tavily"));
            // We should not need to fail over if Tavily is chosen first.
            assert!(!v["warnings"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|x| x.as_str() == Some("provider_failover")));
        }

        #[tokio::test]
        async fn web_deep_research_can_include_arxiv_evidence_without_synthesis() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&["WEBPIPE_ARXIV_ENDPOINT"]);
            let env = EnvGuard::new(&keys);

            // Local stub Atom feed.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let atom = r#"
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <opensearch:totalResults>1</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/0805.3415v1</id>
    <updated>2008-05-22T00:00:00Z</updated>
    <published>2008-05-22T00:00:00Z</published>
    <title>Test Paper</title>
    <summary>This is a test abstract.</summary>
    <author><name>Alice</name></author>
    <category term="cs.CL" />
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.CL" />
    <link rel="related" type="application/pdf" href="https://arxiv.org/pdf/0805.3415.pdf" />
  </entry>
</feed>
"#;
            let atom = atom.to_string();
            let app = Router::new().route(
                "/api/query",
                get(move || {
                    let atom = atom.clone();
                    async move {
                        (
                            [(axum::http::header::CONTENT_TYPE, "application/atom+xml")],
                            atom,
                        )
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            env.set(
                "WEBPIPE_ARXIV_ENDPOINT",
                &format!("http://{addr}/api/query"),
            );

            // Also provide urls to avoid needing web_search_extract network calls.
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "paper about something".to_string(),
                    audit: None,
                    synthesize: Some(false),
                    urls: Some(vec!["http://example.invalid/fixture".to_string()]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    exploration: Some("balanced".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(1),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: Some("on".to_string()),
                    arxiv_max_papers: Some(1),
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: Some(2_000),
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("auto".to_string()),
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["answer"]["skipped"].as_bool(), Some(true));
            let ev = &v["evidence"];
            assert!(ev.get("arxiv").is_some());
            // In no_network mode, ArXiv should be reported as skipped.
            assert_eq!(ev["arxiv"]["ok"].as_bool(), Some(false));
        }

        #[tokio::test]
        async fn web_deep_research_includes_arxiv_papers_when_enabled() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&["WEBPIPE_ARXIV_ENDPOINT", "WEBPIPE_CACHE_DIR"]);
            let env = EnvGuard::new(&keys);

            // Local HTML evidence server.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let html_app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Evidence</h1><p>Some text.</p></body></html>",
                    )
                }),
            );
            let html_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let html_addr: SocketAddr = html_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(html_listener, html_app)
                    .await
                    .expect("axum serve");
            });
            let url = format!("http://{}/", html_addr);

            // Local ArXiv Atom endpoint.
            let atom = r#"
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <opensearch:totalResults>1</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/0805.3415v1</id>
    <updated>2008-05-22T00:00:00Z</updated>
    <published>2008-05-22T00:00:00Z</published>
    <title>Test Paper</title>
    <summary>This is a test abstract.</summary>
    <author><name>Alice</name></author>
    <category term="cs.CL" />
    <arxiv:primary_category xmlns:arxiv="http://arxiv.org/schemas/atom" term="cs.CL" />
    <link rel="related" type="application/pdf" href="https://arxiv.org/pdf/0805.3415.pdf" />
  </entry>
</feed>
"#;
            let atom = atom.to_string();
            let arxiv_app = Router::new().route(
                "/api/query",
                get(move || {
                    let atom = atom.clone();
                    async move {
                        (
                            [(axum::http::header::CONTENT_TYPE, "application/atom+xml")],
                            atom,
                        )
                    }
                }),
            );
            let arxiv_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let arxiv_addr: SocketAddr = arxiv_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(arxiv_listener, arxiv_app)
                    .await
                    .expect("axum serve");
            });
            env.set(
                "WEBPIPE_ARXIV_ENDPOINT",
                &format!("http://{arxiv_addr}/api/query"),
            );

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "paper about something".to_string(),
                    audit: None,
                    synthesize: Some(false),
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    exploration: Some("balanced".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(1),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: Some("on".to_string()),
                    arxiv_max_papers: Some(1),
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: Some(2_000),
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("auto".to_string()),
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            let ev = &v["evidence"];
            assert_eq!(ev["arxiv"]["ok"].as_bool(), Some(true));
            assert!(ev["arxiv"]["papers"]
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(false));
        }

        #[tokio::test]
        async fn web_deep_research_success_includes_evidence_pack_when_include_evidence_true() {
            let mut keys = Vec::new();
            keys.extend_from_slice(&SEARCH_ENV_KEYS);
            keys.extend_from_slice(&PERPLEXITY_ENV_KEYS);
            keys.extend_from_slice(&[
                "WEBPIPE_OLLAMA_ENABLE",
                "WEBPIPE_OLLAMA_BASE_URL",
                "WEBPIPE_OLLAMA_MODEL",
            ]);
            let env = EnvGuard::new(&keys);

            // Local HTML evidence server.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let html_app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><h1>Evidence</h1><p>Some text.</p></body></html>",
                    )
                }),
            );
            let html_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let html_addr: SocketAddr = html_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(html_listener, html_app)
                    .await
                    .expect("axum serve");
            });
            let url = format!("http://{}/", html_addr);

            // Local Ollama stub.
            use axum::{routing::post, Json};
            let ollama_app = Router::new().route(
                "/api/chat",
                post(|_body: Json<serde_json::Value>| async move {
                    Json(serde_json::json!({
                        "message": { "role": "assistant", "content": "Local synthesis ok." }
                    }))
                }),
            );
            let ollama_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let ollama_addr: SocketAddr = ollama_listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(ollama_listener, ollama_app)
                    .await
                    .expect("axum serve");
            });
            env.set("WEBPIPE_OLLAMA_ENABLE", "true");
            env.set("WEBPIPE_OLLAMA_BASE_URL", &format!("http://{ollama_addr}"));
            env.set("WEBPIPE_OLLAMA_MODEL", "test-model");

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_deep_research(Parameters(Some(WebDeepResearchArgs {
                    query: "paper about something".to_string(),
                    audit: None,
                    synthesize: Some(true),
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    exploration: Some("balanced".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(5_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(2),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    max_links: Some(10),
                    arxiv_mode: Some("off".to_string()),
                    arxiv_max_papers: None,
                    arxiv_categories: None,
                    arxiv_years: None,
                    arxiv_timeout_ms: None,
                    model: Some("sonar-deep-research".to_string()),
                    llm_model: None,
                    search_mode: None,
                    reasoning_effort: None,
                    max_tokens: None,
                    temperature: None,
                    top_p: None,
                    max_answer_chars: Some(500),
                    include_evidence: Some(true),
                    now_epoch_s: Some(1700000000),
                    llm_backend: Some("ollama".to_string()),
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert!(v.get("evidence").is_some());
            assert!(v.get("evidence_pack").is_some());
            assert_eq!(
                v["evidence_pack"]["kind"].as_str(),
                Some("webpipe_evidence_pack")
            );
            // Evidence pack should not contain raw extracted text.
            if let Some(r0) = v["evidence_pack"]["results"]
                .as_array()
                .and_then(|a| a.first())
            {
                assert!(r0.get("text").is_none());
                assert!(r0.get("extract").and_then(|e| e.get("text")).is_none());
            }
        }

        #[tokio::test]
        async fn web_search_extract_rejects_unknown_fetch_backend() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // Local fixture server.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html>hi</html>",
                    )
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("q".to_string()),
                    urls: Some(vec![url]),
                    url_selection_mode: None,
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: None,
                    fetch_backend: Some("nope".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(3),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    include_structure: None,
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(10),
                    include_text: Some(false),
                    cache_read: Some(true),
                    cache_write: Some(true),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(false),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: None,
                    agentic_frontier_max: None,
                    planner_max_calls: None,
                    retry_on_truncation: None,
                    truncation_retry_max_bytes: None,
                    compact: None,
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
        }

        #[tokio::test]
        async fn web_search_extract_include_links_extracts_links_for_html_main_pages() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // Local fixture server: include a nav with many links and an article with a few key links.
            // This shape tends to trigger html_main extraction, but regardless, link extraction should
            // come from raw HTML.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let html = r#"
<html>
  <head><title>Test</title></head>
  <body>
    <nav>
      <a href="/a">A</a><a href="/b">B</a><a href="/c">C</a><a href="/d">D</a>
      <a href="/e">E</a><a href="/f">F</a><a href="/g">G</a><a href="/h">H</a>
    </nav>
    <article>
      <h1>Main content</h1>
      <p>See <a href="https://example.com/x">X</a> and <a href="/y">Y</a>.</p>
    </article>
  </body>
</html>
"#;
            let html = html.to_string();
            let app = Router::new().route(
                "/",
                get(move || {
                    let html = html.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], html) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("main content".to_string()),
                    urls: Some(vec![url]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    url_selection_mode: Some("auto".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(2),
                    max_chunk_chars: Some(200),
                    include_links: Some(true),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(5),
                    include_text: Some(false),
                    cache_read: Some(true),
                    cache_write: Some(true),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(false),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: None,
                    agentic_frontier_max: None,
                    planner_max_calls: None,
                    retry_on_truncation: None,
                    truncation_retry_max_bytes: None,
                    compact: Some(true),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            let one = v["results"].as_array().unwrap()[0].clone();
            let links = one["extract"]["links"].as_array().unwrap();
            assert!(!links.is_empty());
            // Bounded.
            assert!(links.len() <= 5);
        }

        #[tokio::test]
        async fn web_search_extract_agentic_compact_summary_includes_stuck_and_frontier_stats() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // Two pages: one is "stuck" (empty body) and links to the other.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let page_a =
                r#"<html><head><title>Login</title></head><body>Just a moment...</body></html>"#
                    .to_string();
            let page_b = r#"<html><body><article><h1>Install</h1><p>Install instructions.</p></article></body></html>"#.to_string();
            let app = Router::new()
                .route(
                    "/a",
                    get(move || {
                        let page_a = page_a.clone();
                        async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], page_a) }
                    }),
                )
                .route(
                    "/b",
                    get(move || {
                        let page_b = page_b.clone();
                        async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], page_b) }
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url_a = format!("http://{addr}/a");

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("install".to_string()),
                    urls: Some(vec![url_a]),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    url_selection_mode: Some("auto".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(2),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(2),
                    max_chunk_chars: Some(200),
                    include_links: Some(false),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(10),
                    include_text: Some(false),
                    cache_read: Some(true),
                    cache_write: Some(true),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(true),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: Some(2),
                    agentic_frontier_max: Some(50),
                    planner_max_calls: None,
                    retry_on_truncation: None,
                    truncation_retry_max_bytes: None,
                    compact: Some(true),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["agentic"]["enabled"].as_bool(), Some(true));
            assert!(v["agentic"].get("search_rounds").is_some());
            assert!(v["agentic"].get("stuck_events").is_some());
            assert!(v["agentic"].get("frontier_added_total").is_some());
            assert!(v["agentic"].get("frontier_len_final").is_some());
            assert!(v["agentic"].get("urls_fetched").is_some());
        }

        #[tokio::test]
        async fn web_search_extract_retry_on_truncation_can_recover_tail_content() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // Local fixture: a large HTML document with the key sentence near the end.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let prefix = "<html><body><article><h1>Docs</h1><p>Intro.</p>";
            let filler = "x".repeat(20_000);
            let tail = "<p>TAIL_SENTINEL: Route Handlers let you create custom request handlers.</p></article></body></html>";
            let body = format!("{prefix}{filler}{tail}");
            let app = Router::new().route(
                "/",
                get(move || {
                    let body = body.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("route handlers".to_string()),
                    urls: Some(vec![url]),
                    url_selection_mode: Some("auto".to_string()),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    // Force truncation on first attempt.
                    max_bytes: Some(2_000),
                    retry_on_truncation: Some(true),
                    truncation_retry_max_bytes: Some(50_000),
                    width: Some(80),
                    // Keep enough extracted text to include tail content once truncation retry succeeds.
                    max_chars: Some(60_000),
                    top_chunks: Some(3),
                    max_chunk_chars: Some(300),
                    include_links: Some(false),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(10),
                    include_text: Some(false),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(false),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: None,
                    agentic_frontier_max: None,
                    planner_max_calls: None,
                    // This test asserts per-URL diagnostics ("attempts") which are omitted in compact mode.
                    compact: Some(false),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            let one = v["results"].as_array().unwrap()[0].clone();
            assert!(one.get("attempts").is_some());
            assert!(one["attempts"].get("local_retry").is_some());
            let found_tail = v["top_chunks"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|c| {
                    c.get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .contains("TAIL_SENTINEL")
                });
            assert!(found_tail);
        }

        #[tokio::test]
        async fn web_search_extract_urls_mode_without_query_still_returns_top_chunks() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><body><main><h1>Title</h1>\
                         <p>This paragraph is long enough to be selected as a default chunk even without a query.</p>\
                         <p>Second paragraph is also long enough to be useful as evidence for URL-only mode.</p>\
                         </main></body></html>",
                    )
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: None,
                    urls: Some(vec![url]),
                    url_selection_mode: Some("auto".to_string()),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    retry_on_truncation: Some(false),
                    truncation_retry_max_bytes: None,
                    width: Some(80),
                    max_chars: Some(5_000),
                    top_chunks: Some(2),
                    max_chunk_chars: Some(300),
                    include_links: Some(false),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(10),
                    include_text: Some(false),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(false),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: None,
                    agentic_frontier_max: None,
                    planner_max_calls: None,
                    compact: Some(true),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            let tc = v["top_chunks"].as_array().expect("top_chunks array");
            assert!(!tc.is_empty());
            assert!(tc[0]
                .get("text")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .contains("long enough"));
        }

        #[tokio::test]
        async fn web_search_extract_agentic_hops_to_query_relevant_link_when_max_urls_ge_2() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // Local fixture: a docs landing page with multiple links, only one of which
            // contains both query tokens ("route" and "handlers") in the URL and anchor text.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let landing_html = r#"
<html><body>
  <main>
    <h1>Docs</h1>
    <p>Welcome to the docs landing page.</p>
    <nav>
      <a href="/docs/app/getting-started/routing">Routing</a>
      <a href="/docs/app/getting-started/route-handlers">Route Handlers</a>
      <a href="/docs/app/getting-started/installation">Installation</a>
    </nav>
  </main>
</body></html>
"#;
            let routing_html = r#"
<html><body><main>
  <h1>Routing</h1>
  <p>This page is about routing but does not talk about handlers.</p>
</main></body></html>
"#;
            let handlers_html = r#"
<html><body><main>
  <h1>Route Handlers</h1>
  <p>Route Handlers let you create custom request handlers for a given route.</p>
</main></body></html>
"#;

            let app = Router::new()
                .route(
                    "/docs",
                    get({
                        let landing_html = landing_html.to_string();
                        move || {
                            let landing_html = landing_html.clone();
                            async move {
                                (
                                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                                    landing_html,
                                )
                            }
                        }
                    }),
                )
                .route(
                    "/docs/app/getting-started/routing",
                    get({
                        let routing_html = routing_html.to_string();
                        move || {
                            let routing_html = routing_html.clone();
                            async move {
                                (
                                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                                    routing_html,
                                )
                            }
                        }
                    }),
                )
                .route(
                    "/docs/app/getting-started/route-handlers",
                    get({
                        let handlers_html = handlers_html.to_string();
                        move || {
                            let handlers_html = handlers_html.clone();
                            async move {
                                (
                                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                                    handlers_html,
                                )
                            }
                        }
                    }),
                );

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let landing_url = format!("http://{}/docs", addr);
            let handlers_url = format!("http://{}/docs/app/getting-started/route-handlers", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("route handlers".to_string()),
                    urls: Some(vec![landing_url]),
                    url_selection_mode: Some("auto".to_string()),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(2),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    retry_on_truncation: Some(false),
                    truncation_retry_max_bytes: None,
                    width: Some(80),
                    max_chars: Some(10_000),
                    top_chunks: Some(5),
                    max_chunk_chars: Some(300),
                    include_links: Some(false),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(50),
                    include_text: Some(false),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(true),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: Some(1),
                    agentic_frontier_max: Some(200),
                    planner_max_calls: Some(0),
                    compact: Some(true),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));

            // We should have fetched two pages: landing + route-handlers.
            let results = v["results"].as_array().expect("results array");
            assert_eq!(results.len(), 2);
            assert!(results.iter().any(|x| {
                x.get("final_url")
                    .and_then(|u| u.as_str())
                    .map(|u| u == handlers_url)
                    .unwrap_or(false)
            }));

            // And the best chunk should come from the route-handlers page.
            let top_chunks = v["top_chunks"].as_array().expect("top_chunks array");
            assert!(top_chunks.iter().any(|c| {
                c.get("url")
                    .and_then(|u| u.as_str())
                    .map(|u| u == handlers_url)
                    .unwrap_or(false)
                    && c.get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains("custom request handlers")
            }));
        }

        #[tokio::test]
        async fn web_search_extract_agentic_can_hop_via_anchor_text_even_when_parent_has_no_hits() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // The landing page does NOT contain the query tokens in its text (so parent_relevance==0),
            // but it links out with informative anchor text. Agentic selection should still hop.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;

            let landing_html = r#"
<html><body>
  <main>
    <h1>Docs</h1>
    <p>Welcome. This page talks about installation and getting started only.</p>
    <nav>
      <a href="/x1">Route Handlers</a>
      <a href="/route-handlers">Click</a>
    </nav>
  </main>
</body></html>
"#;
            let x1_html = r#"
<html><body><main>
  <h1>Route Handlers</h1>
  <p>TAIL_SENTINEL: Route Handlers let you create custom request handlers for a given route.</p>
</main></body></html>
"#;
            let route_handlers_html = r#"
<html><body><main>
  <h1>Misc</h1>
  <p>This page URL looks relevant but the content is not.</p>
</main></body></html>
"#;

            let app =
                Router::new()
                    .route(
                        "/docs",
                        get({
                            let landing_html = landing_html.to_string();
                            move || {
                                let landing_html = landing_html.clone();
                                async move {
                                    (
                                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                                        landing_html,
                                    )
                                }
                            }
                        }),
                    )
                    .route(
                        "/x1",
                        get({
                            let x1_html = x1_html.to_string();
                            move || {
                                let x1_html = x1_html.clone();
                                async move {
                                    ([(axum::http::header::CONTENT_TYPE, "text/html")], x1_html)
                                }
                            }
                        }),
                    )
                    .route(
                        "/route-handlers",
                        get({
                            let route_handlers_html = route_handlers_html.to_string();
                            move || {
                                let route_handlers_html = route_handlers_html.clone();
                                async move {
                                    (
                                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                                        route_handlers_html,
                                    )
                                }
                            }
                        }),
                    );

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let landing_url = format!("http://{}/docs", addr);
            let x1_url = format!("http://{}/x1", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("route handlers".to_string()),
                    urls: Some(vec![landing_url]),
                    url_selection_mode: Some("auto".to_string()),
                    provider: Some("auto".to_string()),
                    auto_mode: Some("fallback".to_string()),
                    selection_mode: Some("score".to_string()),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(false),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(2),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    retry_on_truncation: Some(false),
                    truncation_retry_max_bytes: None,
                    width: Some(80),
                    max_chars: Some(10_000),
                    top_chunks: Some(5),
                    max_chunk_chars: Some(300),
                    include_links: Some(false),
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(50),
                    include_text: Some(false),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(true),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: Some(1),
                    agentic_frontier_max: Some(200),
                    planner_max_calls: Some(0),
                    compact: Some(true),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            let results = v["results"].as_array().expect("results array");
            assert_eq!(results.len(), 2);
            assert!(results.iter().any(|x| {
                x.get("final_url")
                    .and_then(|u| u.as_str())
                    .map(|u| u == x1_url)
                    .unwrap_or(false)
            }));
            let top_chunks = v["top_chunks"].as_array().expect("top_chunks array");
            assert!(top_chunks.iter().any(|c| {
                c.get("url")
                    .and_then(|u| u.as_str())
                    .map(|u| u == x1_url)
                    .unwrap_or(false)
                    && c.get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .contains("TAIL_SENTINEL")
            }));
        }

        #[tokio::test]
        async fn web_extract_warns_on_empty_extraction_for_html() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // HTML that is likely to extract to empty text.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let app = Router::new().route(
                "/",
                get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "text/html")],
                        "<html><head><script>var x = 1;</script></head><body></body></html>",
                    )
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_extract(p(WebExtractArgs {
                    url: Some(url),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    width: Some(80),
                    max_chars: Some(2_000),
                    query: None,
                    top_chunks: Some(3),
                    max_chunk_chars: Some(200),
                    include_text: Some(false),
                    include_links: Some(false),
                    max_links: Some(10),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    cache_read: Some(true),
                    cache_write: Some(true),
                    cache_ttl_s: None,
                    include_structure: None,
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["kind"].as_str(), Some("web_extract"));
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["text_chars"].as_u64(), Some(0));
            assert_eq!(
                v["warnings"]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(|x| x.as_str()),
                Some("empty_extraction")
            );
        }

        #[tokio::test]
        async fn web_fetch_warns_on_body_truncation() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);

            // Large HTML body to force truncation by max_bytes.
            use axum::{routing::get, Router};
            use std::net::SocketAddr;
            let big = "x".repeat(10_000);
            let app = Router::new().route(
                "/",
                get(move || {
                    let body = big.clone();
                    async move { ([(axum::http::header::CONTENT_TYPE, "text/html")], body) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });
            let url = format!("http://{}/", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_fetch(p(WebFetchArgs {
                    url: Some(url),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200),
                    max_text_chars: Some(1_000),
                    headers: None,
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    include_text: Some(false),
                    include_headers: Some(false),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["kind"].as_str(), Some("web_fetch"));
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["truncated"].as_bool(), Some(true));
            assert!(v["warnings"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|x| x.as_str() == Some("body_truncated_by_max_bytes")));
            assert!(v["warning_codes"]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .any(|x| x.as_str() == Some("body_truncated_by_max_bytes")));
        }

        #[tokio::test]
        async fn web_search_extract_firecrawl_fallback_on_empty_extraction_is_bounded() {
            // This is a fully offline test: we stand up one local server that:
            // - serves an HTML page that extracts to empty text
            // - acts as a fake Firecrawl v2 scrape endpoint returning markdown
            //
            // Then we run web_search_extract with fetch_backend="local" and
            // firecrawl_fallback_on_empty_extraction=true, and assert it uses the fallback
            // for that URL only (no global behavior change).
            let _env = EnvGuard::new(&[
                "WEBPIPE_CACHE_DIR",
                "WEBPIPE_FIRECRAWL_API_KEY",
                "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
            ]);
            _env.set("WEBPIPE_FIRECRAWL_API_KEY", "test-key");

            use axum::{routing::get, routing::post, Json, Router};
            use serde_json::json;
            use std::net::SocketAddr;

            let app = Router::new()
                .route(
                    "/page",
                    get(|| async {
                        (
                            [(axum::http::header::CONTENT_TYPE, "text/html")],
                            "<html><head><script>var x = 1;</script></head><body></body></html>",
                        )
                    }),
                )
                .route(
                    "/v2/scrape",
                    post(|_body: Json<serde_json::Value>| async move {
                        Json(json!({ "success": true, "data": { "markdown": "# Hi\n\nHello world" } }))
                    }),
                );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });

            _env.set(
                "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
                &format!("http://{}/v2/scrape", addr),
            );

            let url = format!("http://{}/page", addr);

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search_extract(p(WebSearchExtractArgs {
                    query: Some("hello".to_string()),
                    urls: Some(vec![url.clone()]),
                    url_selection_mode: None,
                    provider: Some("auto".to_string()),
                    auto_mode: Some("merge".to_string()),
                    selection_mode: None,
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    firecrawl_fallback_on_empty_extraction: Some(true),
                    firecrawl_fallback_on_low_signal: Some(false),
                    max_results: Some(1),
                    max_urls: Some(1),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    width: Some(80),
                    max_chars: Some(2_000),
                    top_chunks: Some(3),
                    max_chunk_chars: Some(200),
                    include_links: Some(true),
                    include_structure: None,
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                    max_links: Some(10),
                    include_text: Some(true),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    exploration: None,
                    agentic: Some(false),
                    agentic_selector: Some("lexical".to_string()),
                    agentic_max_search_rounds: None,
                    agentic_frontier_max: None,
                    planner_max_calls: None,
                    retry_on_truncation: None,
                    truncation_retry_max_bytes: None,
                    // This test asserts debug-ish per-URL fields that are omitted in compact mode.
                    compact: Some(false),
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["kind"].as_str(), Some("web_search_extract"));
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(
                v["request"]["firecrawl_fallback_on_empty_extraction"].as_bool(),
                Some(true)
            );

            let results = v["results"].as_array().expect("results array");
            assert_eq!(results.len(), 1);
            let one = &results[0];
            assert_eq!(one["ok"].as_bool(), Some(true));
            // Per-URL fetch_backend reflects the *used* backend after fallback.
            assert_eq!(one["fetch_backend"].as_str(), Some("firecrawl"));
            assert!(one["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x.as_str() == Some("firecrawl_fallback_on_empty_extraction")));
            assert!(one["warning_codes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x.as_str() == Some("firecrawl_fallback_on_empty_extraction")));
            assert!(one["extract"]["text"]
                .as_str()
                .unwrap_or("")
                .contains("Hello world"));
            // Link extraction should be disabled in fallback mode.
            assert_eq!(one["extract"]["links"].as_array().unwrap().len(), 0);
            assert!(one.get("attempts").is_some());
        }

        #[tokio::test]
        async fn web_extract_firecrawl_bytes_are_raw_markdown_bytes() {
            // Offline: fake Firecrawl v2 endpoint returning a fixed markdown payload.
            let _env = EnvGuard::new(&[
                "WEBPIPE_CACHE_DIR",
                "WEBPIPE_FIRECRAWL_API_KEY",
                "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
            ]);
            _env.set("WEBPIPE_FIRECRAWL_API_KEY", "test-key");

            use axum::{routing::post, Json, Router};
            use serde_json::json;
            use std::net::SocketAddr;

            let md = "# Hi\n\nHello world";
            let expected_bytes = md.len() as u64;

            let app = Router::new().route(
                "/v2/scrape",
                post(move |_body: Json<serde_json::Value>| {
                    let md0 = md.to_string();
                    async move { Json(json!({ "success": true, "data": { "markdown": md0 } })) }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });

            _env.set(
                "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
                &format!("http://{}/v2/scrape", addr),
            );

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_extract(p(WebExtractArgs {
                    url: Some("https://example.com/".to_string()),
                    fetch_backend: Some("firecrawl".to_string()),
                    no_network: Some(false),
                    width: Some(80),
                    max_chars: Some(2_000),
                    query: None,
                    top_chunks: Some(3),
                    max_chunk_chars: Some(200),
                    include_text: Some(true),
                    include_links: Some(true),
                    max_links: Some(10),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["kind"].as_str(), Some("web_extract"));
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["fetch_backend"].as_str(), Some("firecrawl"));
            assert_eq!(v["bytes"].as_u64(), Some(expected_bytes));
            assert_eq!(v["text_chars"].as_u64(), Some(expected_bytes));
        }

        #[tokio::test]
        async fn warning_codes_normalize_links_unavailable_for_firecrawl() {
            // Offline: Firecrawl path always has links unavailable (we return []), so it emits
            // legacy warning `links_unavailable_for_firecrawl` and canonical code `links_unavailable`.
            let _env = EnvGuard::new(&[
                "WEBPIPE_CACHE_DIR",
                "WEBPIPE_FIRECRAWL_API_KEY",
                "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
            ]);
            _env.set("WEBPIPE_FIRECRAWL_API_KEY", "test-key");

            use axum::{routing::post, Json, Router};
            use serde_json::json;
            use std::net::SocketAddr;

            let app = Router::new().route(
                "/v2/scrape",
                post(|_body: Json<serde_json::Value>| async move {
                    Json(json!({ "success": true, "data": { "markdown": "# Hi\n\nHello world" } }))
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr: SocketAddr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.expect("axum serve");
            });

            _env.set(
                "WEBPIPE_FIRECRAWL_ENDPOINT_V2",
                &format!("http://{}/v2/scrape", addr),
            );

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_extract(p(WebExtractArgs {
                    url: Some("https://example.com/".to_string()),
                    fetch_backend: Some("firecrawl".to_string()),
                    no_network: Some(false),
                    width: Some(80),
                    max_chars: Some(2_000),
                    query: None,
                    top_chunks: Some(3),
                    max_chunk_chars: Some(200),
                    include_text: Some(true),
                    include_links: Some(true),
                    max_links: Some(10),
                    timeout_ms: Some(2_000),
                    max_bytes: Some(200_000),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    include_structure: Some(false),
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                }))
                .await
                .expect("call");

            let v = payload_from_call_tool_result(&r);
            let warns = v["warnings"].as_array().unwrap();
            let codes = v["warning_codes"].as_array().unwrap();
            assert!(warns
                .iter()
                .any(|x| x.as_str() == Some("links_unavailable_for_firecrawl")));
            assert!(codes
                .iter()
                .any(|x| x.as_str() == Some("links_unavailable")));
        }

        #[tokio::test]
        async fn webpipe_meta_provider_auto_order_matrix() {
            // This test encodes the brave-first policy in executable form.
            let _env = EnvGuard::new(&[
                "WEBPIPE_BRAVE_API_KEY",
                "BRAVE_SEARCH_API_KEY",
                "WEBPIPE_TAVILY_API_KEY",
                "TAVILY_API_KEY",
                "WEBPIPE_FIRECRAWL_API_KEY",
                "FIRECRAWL_API_KEY",
                "WEBPIPE_CACHE_DIR",
            ]);

            let svc = WebpipeMcp::new().expect("new");

            // none configured
            let v0 = payload_from_call_tool_result(&svc.webpipe_meta().await.expect("meta"));
            assert_eq!(v0["ok"].as_bool(), Some(true));
            assert_eq!(
                v0["defaults"]["web_search"]["provider"].as_str(),
                Some("brave")
            );
            assert_eq!(
                v0["defaults"]["web_search"]["provider_auto_order"]
                    .as_array()
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(
                v0["configured"]["providers"]["brave"].as_bool(),
                Some(false)
            );
            assert_eq!(
                v0["configured"]["providers"]["tavily"].as_bool(),
                Some(false)
            );
            // Opportunistic tooling capability surface should exist (even when all keys are missing).
            assert!(v0["local_tools"].as_object().is_some());
            assert!(v0["local_tools"]["yt_dlp"].is_boolean());
            assert!(v0["local_tools"]["pandoc"].is_boolean());
            assert!(v0["local_tools"]["ffmpeg"].is_boolean());
            assert!(v0["local_tools"]["tesseract"].is_boolean());
            assert!(v0["local_tools"]["pdftotext"].is_boolean());
            assert!(v0["local_tools"]["mutool"].is_boolean());
            assert!(v0["configured"]["vision"]["gemini"].is_boolean());
            // Knob list is names-only and stable.
            assert!(v0["supported"]["knobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x.as_str() == Some("WEBPIPE_PDF_SHELLOUT")));
            assert!(v0["supported"]["knobs"]
                .as_array()
                .unwrap()
                .iter()
                .any(|x| x.as_str() == Some("WEBPIPE_VISION")));

            // brave only
            _env.set("WEBPIPE_BRAVE_API_KEY", "x");
            let v1 = payload_from_call_tool_result(&svc.webpipe_meta().await.expect("meta"));
            assert_eq!(
                v1["defaults"]["web_search"]["provider_auto_order"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>(),
                vec!["brave"]
            );
            assert_eq!(v1["configured"]["providers"]["brave"].as_bool(), Some(true));
            assert_eq!(
                v1["configured"]["providers"]["tavily"].as_bool(),
                Some(false)
            );

            // tavily only (clear brave, set tavily)
            std::env::remove_var("WEBPIPE_BRAVE_API_KEY");
            _env.set("WEBPIPE_TAVILY_API_KEY", "x");
            let v2 = payload_from_call_tool_result(&svc.webpipe_meta().await.expect("meta"));
            assert_eq!(
                v2["defaults"]["web_search"]["provider_auto_order"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>(),
                vec!["tavily"]
            );
            assert_eq!(
                v2["configured"]["providers"]["brave"].as_bool(),
                Some(false)
            );
            assert_eq!(
                v2["configured"]["providers"]["tavily"].as_bool(),
                Some(true)
            );

            // both configured (brave-first ordering)
            _env.set("WEBPIPE_BRAVE_API_KEY", "x");
            let v3 = payload_from_call_tool_result(&svc.webpipe_meta().await.expect("meta"));
            assert_eq!(
                v3["defaults"]["web_search"]["provider_auto_order"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>(),
                vec!["brave", "tavily"]
            );
            assert_eq!(v3["configured"]["providers"]["brave"].as_bool(), Some(true));
            assert_eq!(
                v3["configured"]["providers"]["tavily"].as_bool(),
                Some(true)
            );
        }

        #[tokio::test]
        async fn web_seed_urls_contract_is_stable_and_bounded() {
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_seed_urls(p(WebSeedUrlsArgs {
                    max: Some(2),
                    include_ids: Some(true),
                    ids_only: Some(false),
                }))
                .await
                .expect("seed urls");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["kind"].as_str(), Some("web_seed_urls"));
            assert_eq!(v["schema_version"].as_u64(), Some(1));
            assert_eq!(v["max"].as_u64(), Some(2));
            assert_eq!(v["ids"].as_array().map(|a| a.len()), Some(2));
            let seeds = v["seeds"].as_array().expect("seeds array");
            assert_eq!(seeds.len(), 2);
            assert!(seeds[0].get("url").and_then(|x| x.as_str()).is_some());
        }

        #[tokio::test]
        async fn web_seed_search_extract_works_on_local_urls() {
            use axum::{http::header, routing::get, Router};
            use std::net::SocketAddr;

            async fn serve(app: Router) -> SocketAddr {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr: SocketAddr = listener.local_addr().unwrap();
                tokio::spawn(async move {
                    axum::serve(listener, app).await.unwrap();
                });
                addr
            }

            // Local “seed docs” with distinctive tokens.
            let app = Router::new()
                .route(
                    "/a",
                    get(|| async {
                        (
                            [(header::CONTENT_TYPE, "text/markdown")],
                            "# A\n\nseedtoken_a unique_a",
                        )
                    }),
                )
                .route(
                    "/b",
                    get(|| async {
                        (
                            [(header::CONTENT_TYPE, "text/markdown")],
                            "# B\n\nseedtoken_b unique_b",
                        )
                    }),
                );
            let addr = serve(app).await;

            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_seed_search_extract(p(WebSeedSearchExtractArgs {
                    query: "unique_b".to_string(),
                    urls: Some(vec![format!("http://{addr}/a"), format!("http://{addr}/b")]),
                    seed_ids: None,
                    max_urls: Some(2),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(false),
                    width: Some(80),
                    max_chars: Some(10_000),
                    max_bytes: Some(200_000),
                    top_chunks: Some(3),
                    merged_max_per_seed: None,
                    max_chunk_chars: Some(300),
                    cache_read: Some(false),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    include_text: Some(false),
                    compact: Some(true),
                }))
                .await
                .expect("seed search extract");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["kind"].as_str(), Some("web_seed_search_extract"));
            let merged = v["results"]["merged_chunks"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(!merged.is_empty());
            let any = merged.iter().any(|c| {
                c.get("text")
                    .and_then(|x| x.as_str())
                    .is_some_and(|t| t.contains("unique_b"))
            });
            assert!(any, "expected to find query token in merged chunks");
        }

        #[test]
        fn seed_registry_ids_are_unique_and_nonempty() {
            let mut seen = std::collections::BTreeSet::<&'static str>::new();
            for s in seed_registry() {
                assert!(!s.id.trim().is_empty());
                assert!(!s.url.trim().is_empty());
                assert!(seen.insert(s.id), "duplicate seed id: {}", s.id);
            }
        }

        #[test]
        fn select_seed_urls_defaults_to_registry_order() {
            let (picked, unknown) = select_seed_urls(seed_registry(), None, None, 3);
            assert!(unknown.is_empty());
            assert_eq!(picked.len(), 3);
            assert_eq!(picked[0].0, "public-apis");
            assert_eq!(picked[1].0, "awesome-selfhosted");
            assert_eq!(picked[2].0, "awesome");
        }

        #[test]
        fn select_seed_urls_respects_seed_ids_order_and_reports_unknowns() {
            let (picked, unknown) = select_seed_urls(
                seed_registry(),
                None,
                Some(vec![
                    "awesome-python".to_string(),
                    "nope".to_string(),
                    "public-apis".to_string(),
                    "awesome-python".to_string(), // duplicate should be ignored
                ]),
                10,
            );
            assert_eq!(picked.len(), 2);
            assert_eq!(picked[0].0, "awesome-python");
            assert_eq!(picked[1].0, "public-apis");
            assert_eq!(unknown, vec!["nope".to_string()]);
        }

        #[test]
        fn web_seed_urls_ids_only_includes_ids_array() {
            let svc = WebpipeMcp::new().expect("new");
            // We don't need async: the tool is async, so use a tokio runtime for a minimal call.
            let rt = tokio::runtime::Runtime::new().expect("rt");
            let r = rt
                .block_on(svc.web_seed_urls(p(WebSeedUrlsArgs {
                    max: Some(5),
                    include_ids: Some(true),
                    ids_only: Some(true),
                })))
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(true));
            assert_eq!(v["kind"].as_str(), Some("web_seed_urls"));
            assert_eq!(v["ids"].as_array().map(|a| a.len()), Some(5));
            // ids_only should omit kind/note in seeds entries.
            let seeds = v["seeds"].as_array().unwrap();
            assert_eq!(seeds.len(), 5);
            assert!(seeds[0].get("id").is_some());
            assert!(seeds[0].get("url").is_some());
            assert!(seeds[0].get("kind").is_none());
            assert!(seeds[0].get("note").is_none());
        }

        #[tokio::test]
        async fn web_seed_search_extract_rejects_all_unknown_seed_ids() {
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_seed_search_extract(Parameters(Some(WebSeedSearchExtractArgs {
                    query: "anything".to_string(),
                    urls: None,
                    seed_ids: Some(vec!["nope1".to_string(), "nope2".to_string()]),
                    max_urls: Some(5),
                    fetch_backend: Some("local".to_string()),
                    no_network: Some(true),
                    width: Some(80),
                    max_chars: Some(2000),
                    max_bytes: Some(200_000),
                    top_chunks: Some(3),
                    merged_max_per_seed: None,
                    max_chunk_chars: Some(200),
                    cache_read: Some(true),
                    cache_write: Some(false),
                    cache_ttl_s: None,
                    include_text: Some(false),
                    compact: Some(true),
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
            assert_eq!(
                v["unknown_seed_ids"].as_array().map(|a| a.len()),
                Some(2)
            );
        }

        #[tokio::test]
        async fn web_search_rejects_empty_query() {
            let _env = EnvGuard::new(&SEARCH_ENV_KEYS);
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search(Parameters(Some(WebSearchArgs {
                    query: Some("   ".to_string()),
                    provider: Some("brave".to_string()),
                    auto_mode: None,
                    max_results: Some(5),
                    language: None,
                    country: None,
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(v["provider"].as_str(), Some("brave"));
            assert_eq!(v["request"]["provider"].as_str(), Some("brave"));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
        }

        #[tokio::test]
        async fn web_search_rejects_unknown_provider() {
            let _env = EnvGuard::new(&SEARCH_ENV_KEYS);
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_search(Parameters(Some(WebSearchArgs {
                    query: Some("q".to_string()),
                    provider: Some("nope".to_string()),
                    auto_mode: None,
                    max_results: Some(1),
                    language: None,
                    country: None,
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(v["provider"].as_str(), Some("nope"));
            assert_eq!(v["request"]["provider"].as_str(), Some("nope"));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
        }

        #[tokio::test]
        async fn web_fetch_rejects_empty_url() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_fetch(Parameters(Some(WebFetchArgs {
                    url: Some("   ".to_string()),
                    fetch_backend: None,
                    no_network: None,
                    timeout_ms: None,
                    max_bytes: None,
                    max_text_chars: None,
                    headers: None,
                    cache_read: None,
                    cache_write: None,
                    cache_ttl_s: None,
                    include_headers: None,
                    include_text: None,
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
        }

        #[tokio::test]
        async fn web_extract_rejects_empty_url() {
            let _env = EnvGuard::new(&["WEBPIPE_CACHE_DIR"]);
            let svc = WebpipeMcp::new().expect("new");
            let r = svc
                .web_extract(Parameters(Some(WebExtractArgs {
                    url: Some("   ".to_string()),
                    fetch_backend: None,
                    no_network: None,
                    width: None,
                    max_chars: None,
                    query: None,
                    top_chunks: None,
                    max_chunk_chars: None,
                    include_text: None,
                    include_links: None,
                    max_links: None,
                    timeout_ms: None,
                    max_bytes: None,
                    cache_read: None,
                    cache_write: None,
                    cache_ttl_s: None,
                    include_structure: None,
                    max_outline_items: None,
                    max_blocks: None,
                    max_block_chars: None,
                    semantic_rerank: None,
                    semantic_top_k: None,
                })))
                .await
                .expect("call");
            let v = payload_from_call_tool_result(&r);
            assert_eq!(v["ok"].as_bool(), Some(false));
            assert_eq!(
                v["error"]["code"].as_str(),
                Some(ErrorCode::InvalidParams.as_str())
            );
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Optional env-file loader (opt-in).
    //
    // Rationale: Cursor/MCP server environments often aren't interactive shells, so users
    // want a single place to keep keys without exporting them manually.
    //
    // Safety:
    // - opt-in only (WEBPIPE_ENV_FILE)
    // - sets vars only if not already set in the process environment
    // - does not log values
    if let Ok(p) = std::env::var("WEBPIPE_ENV_FILE") {
        let p = p.trim();
        if !p.is_empty() {
            if let Ok(txt) = std::fs::read_to_string(p) {
                for raw in txt.lines() {
                    let s = raw.trim();
                    if s.is_empty() || s.starts_with('#') {
                        continue;
                    }
                    let Some((k, v)) = s.split_once('=') else {
                        continue;
                    };
                    let k = k.trim();
                    let v = v.trim();
                    if k.is_empty() {
                        continue;
                    }
                    // Don't override explicit process env.
                    if std::env::var_os(k).is_none() {
                        std::env::set_var(k, v);
                    }
                }
            }
        }
    }

    let cli = Cli::parse();

    match cli.command {
        #[cfg(feature = "stdio")]
        Commands::McpStdio => {
            mcp::serve_stdio()
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Commands::EvalSearch(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(".generated/webpipe-eval-search-{now}.json"))
            });
            let providers = args
                .providers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            let queries = eval::load_queries(&args.queries_file, &args.query)?;
            let spec = eval::EvalSearchSpec {
                queries,
                providers,
                max_results: args.max_results,
                language: args.language,
                country: args.country,
                out,
                now_epoch_s: Some(now),
            };
            let p = eval::eval_search(spec).await?;
            println!("{}", p.display());
        }
        Commands::EvalFetch(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(".generated/webpipe-eval-fetch-{now}.json"))
            });
            let fetchers = args
                .fetchers
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>();
            let urls = eval::load_urls(&args.urls_file, &args.url)?;
            let spec = eval::EvalFetchSpec {
                urls,
                fetchers,
                timeout_ms: args.timeout_ms,
                max_bytes: args.max_bytes,
                max_text_chars: args.max_text_chars,
                extract: args.extract,
                extract_width: args.extract_width,
                out,
                now_epoch_s: Some(now),
            };
            let p = eval::eval_fetch(spec).await?;
            println!("{}", p.display());
        }
        #[cfg(feature = "stdio")]
        Commands::EvalSearchExtract(args) => {
            use rmcp::handler::server::wrapper::Parameters;
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(
                    ".generated/webpipe-eval-search-extract-{now}.json"
                ))
            });

            std::fs::create_dir_all(
                out.parent()
                    .unwrap_or_else(|| std::path::Path::new(".generated")),
            )?;

            let mut query_items: Vec<serde_json::Value> = Vec::new();
            for p in &args.queries_json {
                let raw = std::fs::read_to_string(p)?;
                let v: serde_json::Value = serde_json::from_str(&raw)?;
                if v.get("schema_version").and_then(|x| x.as_u64()) != Some(1)
                    || v.get("kind").and_then(|x| x.as_str()) != Some("webpipe_seed_queries")
                {
                    anyhow::bail!("unexpected queries_json kind/schema_version");
                }
                if let Some(qs) = v.get("queries").and_then(|x| x.as_array()) {
                    for q in qs {
                        if let (Some(id), Some(text)) = (
                            q.get("query_id").and_then(|x| x.as_str()),
                            q.get("query").and_then(|x| x.as_str()),
                        ) {
                            query_items.push(serde_json::json!({ "query_id": id, "query": text }));
                        }
                    }
                }
            }
            let queries_plain = eval::load_queries(&args.queries_file, &args.query)?;
            for q in queries_plain {
                query_items.push(serde_json::json!({ "query": q }));
            }
            let query_count = query_items.len();
            let urls = eval::load_urls(&args.urls_file, &args.url)?;
            let url_count = urls.len();

            // Reuse the MCP implementation directly (no transport), so evaluation stays aligned
            // with the actual tool behavior and bounds.
            let svc = mcp::WebpipeMcp::new().map_err(|e| anyhow::anyhow!(e.to_string()))?;

            let mut runs: Vec<serde_json::Value> = Vec::new();

            let selection_modes: Vec<String> =
                if let Some(s) = args.compare_selection_modes.as_ref() {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                } else {
                    vec![args.selection_mode.clone()]
                };
            if selection_modes.len() > 2 {
                anyhow::bail!(
                    "compare_selection_modes currently supports at most 2 modes (got {}). Hint: use \"score,pareto\".",
                    selection_modes.len()
                );
            }
            for m in &selection_modes {
                if m.as_str() != "score" && m.as_str() != "pareto" {
                    anyhow::bail!("unknown selection_mode in compare_selection_modes: {m} (allowed: score, pareto)");
                }
            }

            if !urls.is_empty() {
                // Dataset-style URL mode: run all provided queries against the same URL seed list.
                // If no queries are provided, run once with an empty query (extraction-only profiling).
                let qs: Vec<serde_json::Value> = if query_items.is_empty() {
                    vec![serde_json::json!({ "query": "" })]
                } else {
                    query_items.clone()
                };

                for qi in qs {
                    let q0 = qi
                        .get("query")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let qid = qi
                        .get("query_id")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    if args.compare_selection_modes.is_some() && q0.trim().is_empty() {
                        anyhow::bail!(
                            "URL mode with compare_selection_modes requires a non-empty query. Pass --query \"...\" or --queries-file <file>."
                        );
                    }

                    let mut per_mode: Vec<serde_json::Value> = Vec::new();
                    for m in &selection_modes {
                        let r = svc
                            .web_search_extract(Parameters(Some(mcp::WebSearchExtractArgs {
                                query: if q0.trim().is_empty() {
                                    None
                                } else {
                                    Some(q0.clone())
                                },
                                urls: Some(urls.clone()),
                                url_selection_mode: Some(args.url_selection_mode.clone()),
                                provider: Some(args.provider.clone()),
                                auto_mode: Some(args.auto_mode.clone()),
                                selection_mode: Some(m.clone()),
                                fetch_backend: Some(args.fetch_backend.clone()),
                                no_network: Some(false),
                                firecrawl_fallback_on_empty_extraction: Some(
                                    args.firecrawl_fallback_on_empty_extraction,
                                ),
                                firecrawl_fallback_on_low_signal: Some(false),
                                max_results: args.max_results,
                                max_urls: args.max_urls,
                                timeout_ms: Some(args.timeout_ms),
                                max_bytes: Some(args.max_bytes),
                                retry_on_truncation: Some(args.retry_on_truncation),
                                truncation_retry_max_bytes: args.truncation_retry_max_bytes,
                                width: Some(args.width),
                                max_chars: args.max_chars,
                                top_chunks: args.top_chunks,
                                max_chunk_chars: args.max_chunk_chars,
                                include_links: Some(args.include_links),
                                include_structure: None,
                                max_outline_items: None,
                                max_blocks: None,
                                max_block_chars: None,
                                semantic_rerank: None,
                                semantic_top_k: None,
                                max_links: Some(args.max_links),
                                include_text: Some(args.include_text),
                                cache_read: Some(true),
                                cache_write: Some(true),
                                cache_ttl_s: None,
                                exploration: Some(args.exploration.clone()),
                                agentic: Some(args.agentic),
                                agentic_selector: Some(args.agentic_selector.clone()),
                                agentic_max_search_rounds: args.agentic_max_search_rounds,
                                agentic_frontier_max: args.agentic_frontier_max,
                                planner_max_calls: args.planner_max_calls,
                                compact: None,
                            })))
                            .await
                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

                        let s = r
                            .content
                            .first()
                            .and_then(|c| c.as_text())
                            .map(|t| t.text.clone())
                            .unwrap_or_default();
                        let v: serde_json::Value = serde_json::from_str(&s)?;
                        per_mode.push(serde_json::json!({ "selection_mode": m, "result": v }));
                    }

                    // Stable, tiny comparison: overlap of top_chunks by (url,start,end).
                    let comparison = if per_mode.len() >= 2 {
                        let mut sigs: Vec<std::collections::BTreeSet<(String, usize, usize)>> =
                            Vec::new();
                        for pm in &per_mode {
                            let mut s = std::collections::BTreeSet::new();
                            if let Some(chunks) = pm["result"]["top_chunks"].as_array() {
                                for c in chunks {
                                    let url = c
                                        .get("url")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let sc =
                                        c.get("start_char").and_then(|x| x.as_u64()).unwrap_or(0)
                                            as usize;
                                    let ec = c.get("end_char").and_then(|x| x.as_u64()).unwrap_or(0)
                                        as usize;
                                    if !url.is_empty() {
                                        s.insert((url, sc, ec));
                                    }
                                }
                            }
                            sigs.push(s);
                        }
                        let a = &sigs[0];
                        let b = &sigs[1];
                        let overlap = a.intersection(b).count();
                        serde_json::json!({
                            "modes": [per_mode[0]["selection_mode"].clone(), per_mode[1]["selection_mode"].clone()],
                            "top_chunks_overlap_count": overlap,
                            "top_chunks_count_a": a.len(),
                            "top_chunks_count_b": b.len()
                        })
                    } else {
                        serde_json::Value::Null
                    };

                    runs.push(serde_json::json!({
                        "mode": "urls",
                        "query_id": qid,
                        "query": q0,
                        "urls_count": urls.len(),
                        "per_selection_mode": per_mode,
                        "comparison": comparison
                    }));
                }
            } else {
                for qi in &query_items {
                    let q = qi
                        .get("query")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string();
                    let qid = qi
                        .get("query_id")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                    let mut per_mode: Vec<serde_json::Value> = Vec::new();
                    for m in &selection_modes {
                        let r = svc
                            .web_search_extract(Parameters(Some(mcp::WebSearchExtractArgs {
                                query: Some(q.clone()),
                                urls: None,
                                provider: Some(args.provider.clone()),
                                auto_mode: Some(args.auto_mode.clone()),
                                selection_mode: Some(m.clone()),
                                fetch_backend: Some(args.fetch_backend.clone()),
                                no_network: Some(false),
                                firecrawl_fallback_on_empty_extraction: Some(
                                    args.firecrawl_fallback_on_empty_extraction,
                                ),
                                firecrawl_fallback_on_low_signal: Some(false),
                                max_results: args.max_results,
                                max_urls: args.max_urls,
                                timeout_ms: Some(args.timeout_ms),
                                max_bytes: Some(args.max_bytes),
                                retry_on_truncation: Some(args.retry_on_truncation),
                                truncation_retry_max_bytes: args.truncation_retry_max_bytes,
                                width: Some(args.width),
                                max_chars: args.max_chars,
                                top_chunks: args.top_chunks,
                                max_chunk_chars: args.max_chunk_chars,
                                include_links: Some(args.include_links),
                                include_structure: None,
                                max_outline_items: None,
                                max_blocks: None,
                                max_block_chars: None,
                                semantic_rerank: None,
                                semantic_top_k: None,
                                max_links: Some(args.max_links),
                                include_text: Some(args.include_text),
                                cache_read: Some(true),
                                cache_write: Some(true),
                                cache_ttl_s: None,
                                url_selection_mode: Some(args.url_selection_mode.clone()),
                                exploration: Some(args.exploration.clone()),
                                agentic: Some(args.agentic),
                                agentic_selector: Some(args.agentic_selector.clone()),
                                agentic_max_search_rounds: args.agentic_max_search_rounds,
                                agentic_frontier_max: args.agentic_frontier_max,
                                planner_max_calls: args.planner_max_calls,
                                compact: None,
                            })))
                            .await
                            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

                        let s = r
                            .content
                            .first()
                            .and_then(|c| c.as_text())
                            .map(|t| t.text.clone())
                            .unwrap_or_default();
                        let v: serde_json::Value = serde_json::from_str(&s)?;
                        per_mode.push(serde_json::json!({ "selection_mode": m, "result": v }));
                    }

                    let comparison = if per_mode.len() >= 2 {
                        let mut sigs: Vec<std::collections::BTreeSet<(String, usize, usize)>> =
                            Vec::new();
                        for pm in &per_mode {
                            let mut s = std::collections::BTreeSet::new();
                            if let Some(chunks) = pm["result"]["top_chunks"].as_array() {
                                for c in chunks {
                                    let url = c
                                        .get("url")
                                        .and_then(|x| x.as_str())
                                        .unwrap_or("")
                                        .to_string();
                                    let sc =
                                        c.get("start_char").and_then(|x| x.as_u64()).unwrap_or(0)
                                            as usize;
                                    let ec = c.get("end_char").and_then(|x| x.as_u64()).unwrap_or(0)
                                        as usize;
                                    if !url.is_empty() {
                                        s.insert((url, sc, ec));
                                    }
                                }
                            }
                            sigs.push(s);
                        }
                        let a = &sigs[0];
                        let b = &sigs[1];
                        let overlap = a.intersection(b).count();
                        serde_json::json!({
                            "modes": [per_mode[0]["selection_mode"].clone(), per_mode[1]["selection_mode"].clone()],
                            "top_chunks_overlap_count": overlap,
                            "top_chunks_count_a": a.len(),
                            "top_chunks_count_b": b.len()
                        })
                    } else {
                        serde_json::Value::Null
                    };

                    runs.push(serde_json::json!({
                        "mode": "query",
                        "query_id": qid,
                        "query": q,
                        "per_selection_mode": per_mode,
                        "comparison": comparison
                    }));
                }
            }

            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "eval_search_extract",
                "generated_at_epoch_s": now,
                "inputs": {
                    "provider": args.provider,
                    "auto_mode": args.auto_mode,
                    "selection_mode": args.selection_mode,
                    "compare_selection_modes": args.compare_selection_modes,
                    "fetch_backend": args.fetch_backend,
                    "firecrawl_fallback_on_empty_extraction": args.firecrawl_fallback_on_empty_extraction,
                    "url_selection_mode": args.url_selection_mode,
                    "max_results": args.max_results,
                    "max_urls": args.max_urls,
                    "timeout_ms": args.timeout_ms,
                    "max_bytes": args.max_bytes,
                    "width": args.width,
                    "max_chars": args.max_chars,
                    "top_chunks": args.top_chunks,
                    "max_chunk_chars": args.max_chunk_chars,
                    "include_links": args.include_links,
                    "max_links": args.max_links,
                    "include_text": args.include_text,
                    "query_count": query_count,
                    "url_count": url_count
                },
                "runs": runs
            });

            std::fs::write(&out, serde_json::to_string_pretty(&payload)? + "\n")?;
            println!("{}", out.display());
        }
        #[cfg(feature = "stdio")]
        Commands::EvalMatrix(args) => {
            use rmcp::handler::server::wrapper::Parameters;

            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(".generated/webpipe-eval-matrix-{now}.jsonl"))
            });
            std::fs::create_dir_all(
                out.parent()
                    .unwrap_or_else(|| std::path::Path::new(".generated")),
            )?;

            let base_url = args.base_url.trim_end_matches('/').to_string();
            let e2e = eval::load_e2e_queries_v1(&args.queries_json)?;

            let svc = mcp::WebpipeMcp::new().map_err(|e| anyhow::anyhow!(e.to_string()))?;

            let mut f = std::io::BufWriter::new(std::fs::File::create(&out)?);

            let max_results = args.max_results.clamp(1, 20);
            let max_urls = args.max_urls.clamp(1, 10);
            let top_chunks = args.top_chunks.min(50).max(1);
            let max_chunk_chars = args.max_chunk_chars.min(5_000).max(20);

            let call_json = |r: rmcp::model::CallToolResult| -> Result<serde_json::Value> {
                let s = r
                    .content
                    .first()
                    .and_then(|c| c.as_text())
                    .map(|t| t.text.clone())
                    .unwrap_or_default();
                Ok(serde_json::from_str(&s)?)
            };

            for q in e2e.queries {
                let urls: Vec<String> = q
                    .url_paths
                    .iter()
                    .map(|p| format!("{base_url}/{}", p.trim_start_matches('/')))
                    .collect();

                // Case A: search leg (uses configured providers/endpoints via env)
                let t0 = std::time::Instant::now();
                let res_a = svc
                    .web_search_extract(Parameters(Some(mcp::WebSearchExtractArgs {
                        query: Some(q.query.clone()),
                        urls: None,
                        provider: Some(args.provider.clone()),
                        auto_mode: Some(args.auto_mode.clone()),
                        selection_mode: Some(args.selection_mode.clone()),
                        fetch_backend: Some(args.fetch_backend.clone()),
                        no_network: Some(false),
                        max_results: Some(max_results),
                        max_urls: Some(max_urls),
                        timeout_ms: Some(args.timeout_ms),
                        max_bytes: Some(args.max_bytes),
                        top_chunks: Some(top_chunks),
                        max_chunk_chars: Some(max_chunk_chars),
                        include_links: Some(false),
                        include_text: Some(false),
                        agentic: Some(false),
                        url_selection_mode: Some("preserve".to_string()),
                        cache_read: Some(true),
                        cache_write: Some(true),
                        ..Default::default()
                    })))
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;

                let row_a = serde_json::json!({
                    "schema_version": 1,
                    "kind": "webpipe_eval_matrix_row",
                    "generated_at_epoch_s": now,
                    "case": "search",
                    "query_id": q.query_id,
                    "query": q.query,
                    "tags": q.tags,
                    "urls": urls,
                    "elapsed_ms": t0.elapsed().as_millis(),
                    "result": call_json(res_a)?
                });
                use std::io::Write;
                writeln!(f, "{}", serde_json::to_string(&row_a)?)?;

                // Case B: warm cache via urls-mode (bounded)
                let t1 = std::time::Instant::now();
                let res_b = svc
                    .web_search_extract(Parameters(Some(mcp::WebSearchExtractArgs {
                        query: row_a.get("query").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        urls: Some(
                            row_a
                                .get("urls")
                                .and_then(|v| v.as_array())
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect(),
                        ),
                        url_selection_mode: Some("preserve".to_string()),
                        provider: Some("auto".to_string()),
                        auto_mode: Some("fallback".to_string()),
                        selection_mode: Some(args.selection_mode.clone()),
                        fetch_backend: Some(args.fetch_backend.clone()),
                        no_network: Some(false),
                        max_urls: Some(max_urls),
                        timeout_ms: Some(args.timeout_ms),
                        max_bytes: Some(args.max_bytes),
                        top_chunks: Some(top_chunks),
                        max_chunk_chars: Some(max_chunk_chars),
                        include_links: Some(false),
                        include_text: Some(false),
                        agentic: Some(false),
                        cache_read: Some(true),
                        cache_write: Some(true),
                        ..Default::default()
                    })))
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let row_b = serde_json::json!({
                    "schema_version": 1,
                    "kind": "webpipe_eval_matrix_row",
                    "generated_at_epoch_s": now,
                    "case": "warm_urls",
                    "query_id": row_a["query_id"].clone(),
                    "query": row_a["query"].clone(),
                    "tags": row_a["tags"].clone(),
                    "urls": row_a["urls"].clone(),
                    "elapsed_ms": t1.elapsed().as_millis(),
                    "result": call_json(res_b)?
                });
                writeln!(f, "{}", serde_json::to_string(&row_b)?)?;

                // Case C: offline replay (urls-mode + no_network=true)
                let t2 = std::time::Instant::now();
                let res_c = svc
                    .web_search_extract(Parameters(Some(mcp::WebSearchExtractArgs {
                        query: row_a.get("query").and_then(|v| v.as_str()).map(|s| s.to_string()),
                        urls: Some(
                            row_a
                                .get("urls")
                                .and_then(|v| v.as_array())
                                .into_iter()
                                .flatten()
                                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                                .collect(),
                        ),
                        url_selection_mode: Some("preserve".to_string()),
                        provider: Some("auto".to_string()),
                        auto_mode: Some("fallback".to_string()),
                        selection_mode: Some(args.selection_mode.clone()),
                        fetch_backend: Some(args.fetch_backend.clone()),
                        no_network: Some(true),
                        max_urls: Some(max_urls),
                        timeout_ms: Some(args.timeout_ms),
                        max_bytes: Some(args.max_bytes),
                        top_chunks: Some(top_chunks),
                        max_chunk_chars: Some(max_chunk_chars),
                        include_links: Some(false),
                        include_text: Some(false),
                        agentic: Some(false),
                        cache_read: Some(true),
                        cache_write: Some(false),
                        ..Default::default()
                    })))
                    .await
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                let row_c = serde_json::json!({
                    "schema_version": 1,
                    "kind": "webpipe_eval_matrix_row",
                    "generated_at_epoch_s": now,
                    "case": "offline_urls",
                    "query_id": row_a["query_id"].clone(),
                    "query": row_a["query"].clone(),
                    "tags": row_a["tags"].clone(),
                    "urls": row_a["urls"].clone(),
                    "elapsed_ms": t2.elapsed().as_millis(),
                    "result": call_json(res_c)?
                });
                writeln!(f, "{}", serde_json::to_string(&row_c)?)?;
            }

            println!("{}", out.display());
        }
        Commands::EvalMatrixScore(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(
                    ".generated/webpipe-eval-matrix-score-{now}.json"
                ))
            });
            std::fs::create_dir_all(
                out.parent()
                    .unwrap_or_else(|| std::path::Path::new(".generated")),
            )?;

            let qrels = eval::load_e2e_qrels_v1(&args.qrels)?;

            // Read + parse JSONL rows.
            let raw = std::fs::read_to_string(&args.matrix_artifact)?;
            let mut rows: Vec<serde_json::Value> = Vec::new();
            for (i, line) in raw.lines().enumerate() {
                let s = line.trim();
                if s.is_empty() {
                    continue;
                }
                let v: serde_json::Value = serde_json::from_str(s).map_err(|e| {
                    anyhow::anyhow!("matrix_artifact line {}: invalid json: {}", i + 1, e)
                })?;
                rows.push(v);
            }

            // Index: query_id -> case -> row
            let mut by_q: std::collections::BTreeMap<String, std::collections::BTreeMap<String, serde_json::Value>> =
                std::collections::BTreeMap::new();
            for r in rows {
                let qid = r.get("query_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let case = r.get("case").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if qid.is_empty() || case.is_empty() {
                    continue;
                }
                by_q.entry(qid).or_default().insert(case, r);
            }

            fn extract_urls_from_result(res: &serde_json::Value) -> std::collections::BTreeSet<String> {
                let mut out = std::collections::BTreeSet::new();
                if let Some(top) = res.get("top_chunks").and_then(|v| v.as_array()) {
                    for c in top {
                        if let Some(u) = c.get("url").and_then(|v| v.as_str()) {
                            out.insert(u.to_string());
                        }
                    }
                }
                if let Some(per_url) = res.get("results").and_then(|v| v.as_array()) {
                    for r in per_url {
                        if let Some(u) = r.get("url").and_then(|v| v.as_str()) {
                            out.insert(u.to_string());
                        }
                        if let Some(u) = r.get("final_url").and_then(|v| v.as_str()) {
                            out.insert(u.to_string());
                        }
                    }
                }
                out
            }

            let expected_cases = ["search", "warm_urls", "offline_urls"];
            let mut per_query: Vec<serde_json::Value> = Vec::new();
            let mut totals = serde_json::json!({
                "queries": qrels.qrels.len(),
                "cases": { "search": {"ok": 0, "hit": 0}, "warm_urls": {"ok": 0, "hit": 0}, "offline_urls": {"ok": 0, "hit": 0} }
            });

            for q in &qrels.qrels {
                let mut per_case = Vec::new();
                for case in expected_cases {
                    let row = by_q
                        .get(&q.query_id)
                        .and_then(|m| m.get(case))
                        .cloned();

                    if let Some(row) = row {
                        let res = row.get("result").cloned().unwrap_or(serde_json::Value::Null);
                        let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                        let urls = extract_urls_from_result(&res);
                        let hit = ok
                            && q.expected_url_substrings.iter().any(|needle| {
                                let needle = needle.as_str();
                                urls.iter().any(|u| u.contains(needle))
                            });

                        if ok {
                            totals["cases"][case]["ok"] =
                                serde_json::json!(totals["cases"][case]["ok"].as_u64().unwrap_or(0) + 1);
                        }
                        if hit {
                            totals["cases"][case]["hit"] =
                                serde_json::json!(totals["cases"][case]["hit"].as_u64().unwrap_or(0) + 1);
                        }

                        per_case.push(serde_json::json!({
                            "case": case,
                            "row_present": true,
                            "ok": ok,
                            "hit": hit,
                            "expected_url_substrings": q.expected_url_substrings,
                            "observed_url_count": urls.len(),
                        }));
                    } else {
                        per_case.push(serde_json::json!({
                            "case": case,
                            "row_present": false,
                            "ok": false,
                            "hit": false,
                            "expected_url_substrings": q.expected_url_substrings,
                            "observed_url_count": 0,
                        }));
                    }
                }
                per_query.push(serde_json::json!({
                    "query_id": q.query_id,
                    "cases": per_case
                }));
            }

            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "webpipe_eval_matrix_score",
                "generated_at_epoch_s": now,
                "inputs": {
                    "matrix_artifact": args.matrix_artifact,
                    "qrels": args.qrels
                },
                "totals": totals,
                "per_query": per_query
            });

            std::fs::write(&out, serde_json::to_string_pretty(&payload)? + "\n")?;
            println!("{}", out.display());
        }
        Commands::EvalMatrixExport(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(
                    ".generated/webpipe-eval-matrix-export-{now}.jsonl"
                ))
            });
            std::fs::create_dir_all(
                out.parent()
                    .unwrap_or_else(|| std::path::Path::new(".generated")),
            )?;

            let qrels = if let Some(p) = args.qrels.as_ref() {
                Some(eval::load_e2e_qrels_v1(p)?)
            } else {
                None
            };
            let mut qrels_by_qid: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            if let Some(ref qrels) = qrels {
                for q in &qrels.qrels {
                    qrels_by_qid.insert(q.query_id.clone(), q.expected_url_substrings.clone());
                }
            }

            fn truncate_chars(s: &str, max_chars: usize) -> (String, bool) {
                if max_chars == 0 {
                    return ("".to_string(), !s.is_empty());
                }
                let mut out = String::new();
                for (n, ch) in s.chars().enumerate() {
                    if n >= max_chars {
                        return (out, true);
                    }
                    out.push(ch);
                }
                (out, false)
            }

            fn extract_urls(res: &serde_json::Value) -> std::collections::BTreeSet<String> {
                let mut out = std::collections::BTreeSet::new();
                if let Some(top) = res.get("top_chunks").and_then(|v| v.as_array()) {
                    for c in top {
                        if let Some(u) = c.get("url").and_then(|v| v.as_str()) {
                            out.insert(u.to_string());
                        }
                    }
                }
                if let Some(per_url) = res.get("results").and_then(|v| v.as_array()) {
                    for r in per_url {
                        if let Some(u) = r.get("url").and_then(|v| v.as_str()) {
                            out.insert(u.to_string());
                        }
                        if let Some(u) = r.get("final_url").and_then(|v| v.as_str()) {
                            out.insert(u.to_string());
                        }
                    }
                }
                out
            }

            // Read + parse JSONL rows.
            let raw = std::fs::read_to_string(&args.matrix_artifact)?;
            let mut out_rows: Vec<serde_json::Value> = Vec::new();
            for (i, line) in raw.lines().enumerate() {
                let s = line.trim();
                if s.is_empty() {
                    continue;
                }
                let row: serde_json::Value = serde_json::from_str(s).map_err(|e| {
                    anyhow::anyhow!("matrix_artifact line {}: invalid json: {}", i + 1, e)
                })?;

                let query_id = row.get("query_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let case = row.get("case").and_then(|v| v.as_str()).unwrap_or("").to_string();
                let query = row.get("query").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if query_id.is_empty() || case.is_empty() {
                    continue;
                }

                let tags: Vec<String> = row
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();

                let res = row.get("result").cloned().unwrap_or(serde_json::Value::Null);
                let ok = res.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                let backend_provider = res.get("backend_provider").and_then(|v| v.as_str()).map(|s| s.to_string());
                let urls = extract_urls(&res);

                let expected = qrels_by_qid.get(&query_id).cloned().unwrap_or_default();
                let matched = if ok {
                    expected.iter().find(|needle| urls.iter().any(|u| u.contains(needle.as_str()))).cloned()
                } else {
                    None
                };
                let hit_expected_url = matched.is_some();

                // Build a compact judge text: concatenate top_chunks.text (already bounded by tool),
                // then truncate here as a safety net.
                let mut joined = String::new();
                if let Some(top) = res.get("top_chunks").and_then(|v| v.as_array()) {
                    for (idx, c) in top.iter().enumerate() {
                        if let Some(t) = c.get("text").and_then(|v| v.as_str()) {
                            if idx > 0 {
                                joined.push('\n');
                                joined.push('\n');
                            }
                            joined.push_str(t);
                        }
                    }
                }
                let (judge_text, judge_text_truncated) = truncate_chars(&joined, args.max_text_chars);

                out_rows.push(serde_json::json!({
                    "schema_version": 1,
                    "kind": "webpipe_eval_matrix_example",
                    "generated_at_epoch_s": now,
                    "query_id": query_id,
                    "case": case,
                    "query": query,
                    "tags": tags,
                    "expected_url_substrings": expected,
                    "hit_expected_url": hit_expected_url,
                    "matched_substring": matched,
                    "ok": ok,
                    "backend_provider": backend_provider,
                    "observed_url_count": urls.len(),
                    "judge_text": judge_text,
                    "judge_text_truncated": judge_text_truncated
                }));
            }

            let mut f = std::io::BufWriter::new(std::fs::File::create(&out)?);
            use std::io::Write;
            for r in out_rows {
                writeln!(f, "{}", serde_json::to_string(&r)?)?;
            }

            println!("{}", out.display());
        }
        Commands::EvalMatrixJudge(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(
                    ".generated/webpipe-eval-matrix-judge-{now}.json"
                ))
            });
            std::fs::create_dir_all(
                out.parent()
                    .unwrap_or_else(|| std::path::Path::new(".generated")),
            )?;

            let raw = std::fs::read_to_string(&args.examples_artifact)?;

            fn tokenize_ascii(s: &str) -> Vec<String> {
                // Deterministic, cheap tokenization for overlap checks.
                let mut out = Vec::new();
                let mut cur = String::new();
                for ch in s.chars() {
                    let c = ch.to_ascii_lowercase();
                    if c.is_ascii_alphanumeric() {
                        cur.push(c);
                    } else {
                        if cur.len() >= 3 {
                            out.push(cur.clone());
                        }
                        cur.clear();
                    }
                }
                if cur.len() >= 3 {
                    out.push(cur);
                }
                out
            }

            fn is_low_signal_text(t: &str) -> bool {
                let s = t.to_ascii_lowercase();
                // Cheap heuristics: known failure modes for JS apps/auth walls.
                s.contains("enable javascript")
                    || s.contains("javascript required")
                    || s.contains("access denied")
                    || s.contains("sign in")
                    || s.contains("captcha")
                    || s.contains("cloudflare")
                    || s.contains("loading...")
            }

            let mut per_example = Vec::new();
            let mut totals = serde_json::json!({
                "examples": 0u64,
                "by_case": {},
                "metrics": {
                    "ok": 0u64,
                    "hit_expected_url": 0u64,
                    "nonempty_text": 0u64,
                    "query_overlap": 0u64,
                    "low_signal": 0u64
                }
            });

            for (i, line) in raw.lines().enumerate() {
                let s = line.trim();
                if s.is_empty() {
                    continue;
                }
                let ex: serde_json::Value = serde_json::from_str(s).map_err(|e| {
                    anyhow::anyhow!("examples_artifact line {}: invalid json: {}", i + 1, e)
                })?;

                let case = ex.get("case").and_then(|v| v.as_str()).unwrap_or("unknown");
                let ok = ex.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                let hit = ex
                    .get("hit_expected_url")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let query = ex.get("query").and_then(|v| v.as_str()).unwrap_or("");
                let text = ex.get("judge_text").and_then(|v| v.as_str()).unwrap_or("");

                let nonempty_text = !text.trim().is_empty();
                let low_signal = nonempty_text && is_low_signal_text(text);

                let q_tokens = tokenize_ascii(query);
                let t_tokens = tokenize_ascii(text);
                let mut overlap = 0usize;
                if !q_tokens.is_empty() && !t_tokens.is_empty() {
                    let tset: std::collections::BTreeSet<&str> =
                        t_tokens.iter().map(|s| s.as_str()).collect();
                    for qt in &q_tokens {
                        if tset.contains(qt.as_str()) {
                            overlap += 1;
                        }
                    }
                }
                let query_overlap = overlap >= 1;

                // Baseline pass/fail: must be ok + not low-signal + (hit OR overlap).
                let pass = ok && nonempty_text && !low_signal && (hit || query_overlap);

                totals["examples"] =
                    serde_json::json!(totals["examples"].as_u64().unwrap_or(0) + 1);
                if ok {
                    totals["metrics"]["ok"] =
                        serde_json::json!(totals["metrics"]["ok"].as_u64().unwrap_or(0) + 1);
                }
                if hit {
                    totals["metrics"]["hit_expected_url"] = serde_json::json!(
                        totals["metrics"]["hit_expected_url"].as_u64().unwrap_or(0) + 1
                    );
                }
                if nonempty_text {
                    totals["metrics"]["nonempty_text"] = serde_json::json!(
                        totals["metrics"]["nonempty_text"].as_u64().unwrap_or(0) + 1
                    );
                }
                if query_overlap {
                    totals["metrics"]["query_overlap"] = serde_json::json!(
                        totals["metrics"]["query_overlap"].as_u64().unwrap_or(0) + 1
                    );
                }
                if low_signal {
                    totals["metrics"]["low_signal"] = serde_json::json!(
                        totals["metrics"]["low_signal"].as_u64().unwrap_or(0) + 1
                    );
                }

                // Per-case counters
                if totals["by_case"][case].is_null() {
                    totals["by_case"][case] = serde_json::json!({
                        "examples": 0u64,
                        "pass": 0u64
                    });
                }
                totals["by_case"][case]["examples"] = serde_json::json!(
                    totals["by_case"][case]["examples"].as_u64().unwrap_or(0) + 1
                );
                if pass {
                    totals["by_case"][case]["pass"] = serde_json::json!(
                        totals["by_case"][case]["pass"].as_u64().unwrap_or(0) + 1
                    );
                }

                per_example.push(serde_json::json!({
                    "query_id": ex.get("query_id").cloned().unwrap_or(serde_json::Value::Null),
                    "case": case,
                    "ok": ok,
                    "hit_expected_url": hit,
                    "nonempty_text": nonempty_text,
                    "low_signal": low_signal,
                    "query_overlap": query_overlap,
                    "pass": pass
                }));
            }

            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "webpipe_eval_matrix_judge",
                "generated_at_epoch_s": now,
                "inputs": {
                    "examples_artifact": args.examples_artifact
                },
                "totals": totals,
                "per_example": per_example
            });

            std::fs::write(&out, serde_json::to_string_pretty(&payload)? + "\n")?;
            println!("{}", out.display());
        }
        Commands::EvalMatrixRun(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let now_s = now.to_string();
            let out_dir = args
                .out_dir
                .unwrap_or_else(|| std::path::PathBuf::from(".generated"));
            std::fs::create_dir_all(&out_dir)?;

            let matrix_out = out_dir.join(format!("webpipe-eval-matrix-run-{now}.jsonl"));
            let score_out = out_dir.join(format!("webpipe-eval-matrix-score-run-{now}.json"));
            let export_out = out_dir.join(format!("webpipe-eval-matrix-export-run-{now}.jsonl"));
            let judge_out = out_dir.join(format!("webpipe-eval-matrix-judge-run-{now}.json"));
            let manifest_out = out_dir.join(format!("webpipe-eval-matrix-manifest-run-{now}.json"));

            let exe = std::env::current_exe()?;
            let queries_json = args.queries_json.to_string_lossy().to_string();
            let qrels = args.qrels.to_string_lossy().to_string();
            let matrix_out_s = matrix_out.to_string_lossy().to_string();
            let score_out_s = score_out.to_string_lossy().to_string();
            let export_out_s = export_out.to_string_lossy().to_string();
            let judge_out_s = judge_out.to_string_lossy().to_string();
            let max_text_chars_s = args.max_text_chars.to_string();

            fn run(mut cmd: std::process::Command) -> Result<()> {
                let out = cmd.output()?;
                if !out.status.success() {
                    let code = out.status.code().unwrap_or(-1);
                    anyhow::bail!(
                        "command failed (exit={code}):\nstdout:\n{}\nstderr:\n{}",
                        String::from_utf8_lossy(&out.stdout),
                        String::from_utf8_lossy(&out.stderr)
                    );
                }
                Ok(())
            }

            fn best_effort_git_sha() -> Option<String> {
                let out = std::process::Command::new("git")
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .ok()?;
                if !out.status.success() {
                    return None;
                }
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.is_empty() {
                    None
                } else {
                    Some(s)
                }
            }

            // 1) eval-matrix
            let mut c = std::process::Command::new(&exe);
            c.args([
                "eval-matrix",
                "--queries-json",
                &queries_json,
                "--base-url",
                &args.base_url,
                "--provider",
                &args.provider,
                "--auto-mode",
                &args.auto_mode,
                "--selection-mode",
                &args.selection_mode,
                "--fetch-backend",
                &args.fetch_backend,
                "--out",
                &matrix_out_s,
                "--now-epoch-s",
                &now_s,
            ]);
            run(c)?;

            // 2) eval-matrix-score
            let mut c = std::process::Command::new(&exe);
            c.args([
                "eval-matrix-score",
                "--matrix-artifact",
                &matrix_out_s,
                "--qrels",
                &qrels,
                "--out",
                &score_out_s,
                "--now-epoch-s",
                &now_s,
            ]);
            run(c)?;

            // 3) eval-matrix-export
            let mut c = std::process::Command::new(&exe);
            c.args([
                "eval-matrix-export",
                "--matrix-artifact",
                &matrix_out_s,
                "--qrels",
                &qrels,
                "--max-text-chars",
                &max_text_chars_s,
                "--out",
                &export_out_s,
                "--now-epoch-s",
                &now_s,
            ]);
            run(c)?;

            // 4) eval-matrix-judge
            let mut c = std::process::Command::new(&exe);
            c.args([
                "eval-matrix-judge",
                "--examples-artifact",
                &export_out_s,
                "--out",
                &judge_out_s,
                "--now-epoch-s",
                &now_s,
            ]);
            run(c)?;

            let git_sha = args.git_sha.clone().or_else(best_effort_git_sha);
            let manifest = serde_json::json!({
                "schema_version": 1,
                "kind": "webpipe_eval_matrix_run_manifest",
                "generated_at_epoch_s": now,
                "inputs": {
                    "queries_json": args.queries_json,
                    "qrels": args.qrels,
                    "base_url": args.base_url,
                    "provider": args.provider,
                    "auto_mode": args.auto_mode,
                    "selection_mode": args.selection_mode,
                    "fetch_backend": args.fetch_backend,
                    "max_text_chars": args.max_text_chars
                },
                "git": {
                    "sha": git_sha
                },
                "artifacts": {
                    "matrix": matrix_out,
                    "score": score_out,
                    "export": export_out,
                    "judge": judge_out
                }
            });
            std::fs::write(&manifest_out, serde_json::to_string_pretty(&manifest)? + "\n")?;
            match args.output.to_ascii_lowercase().as_str() {
                "json" => println!("{}", serde_json::to_string(&manifest)?),
                _ => println!("{}", judge_out.display()),
            }
        }
        Commands::Doctor(args) => {
            fn has_env(k: &str) -> bool {
                std::env::var(k).ok().is_some_and(|v| !v.trim().is_empty())
            }

            let t0 = std::time::Instant::now();

            // Env presence (booleans only; never print values).
            let brave_configured =
                has_env("WEBPIPE_BRAVE_API_KEY") || has_env("BRAVE_SEARCH_API_KEY");
            let tavily_configured = has_env("WEBPIPE_TAVILY_API_KEY") || has_env("TAVILY_API_KEY");
            let searxng_configured =
                has_env("WEBPIPE_SEARXNG_ENDPOINT") || has_env("WEBPIPE_SEARXNG_ENDPOINTS");
            let firecrawl_configured =
                has_env("WEBPIPE_FIRECRAWL_API_KEY") || has_env("FIRECRAWL_API_KEY");
            let perplexity_configured =
                has_env("WEBPIPE_PERPLEXITY_API_KEY") || has_env("PERPLEXITY_API_KEY");

            // Cache dir is relevant both for the MCP server and eval harnesses.
            let cache_dir = std::env::var("WEBPIPE_CACHE_DIR")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .map(std::path::PathBuf::from)
                .unwrap_or_else(mcp::default_cache_dir);

            let mut checks: Vec<serde_json::Value> = Vec::new();

            // Check: cache dir is creatable + writable.
            let cache_ok = (|| -> anyhow::Result<()> {
                std::fs::create_dir_all(&cache_dir)?;
                let probe = cache_dir.join(format!(
                    "webpipe-doctor-{}.probe",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                ));
                std::fs::write(&probe, b"ok")?;
                let _ = std::fs::remove_file(&probe);
                Ok(())
            })()
            .is_ok();
            checks.push(serde_json::json!({
                "name": "cache_dir_writable",
                "ok": cache_ok,
                "message": if cache_ok { "cache dir is writable" } else { "cache dir is not writable" },
                "hint": if cache_ok { "" } else { "Set WEBPIPE_CACHE_DIR to a writable directory." },
            }));

            // Check: stdio MCP handshake (optional).
            let mut stdio_ok: Option<bool> = None;
            let mut stdio_tool_count: Option<usize> = None;
            let mut stdio_error: Option<serde_json::Value> = None;
            let mut stdio_elapsed_ms: Option<u128> = None;

            #[cfg(feature = "stdio")]
            if args.check_stdio {
                use rmcp::service::ServiceExt;
                use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
                use tokio::process::Command;

                let exe =
                    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("webpipe"));
                let child = TokioChildProcess::new(Command::new(exe).configure(|cmd| {
                    cmd.args(["mcp-stdio"]);
                    cmd.env("WEBPIPE_CACHE_DIR", &cache_dir);
                    // Avoid accidentally inheriting provider keys for this probe.
                    cmd.env_remove("WEBPIPE_BRAVE_API_KEY");
                    cmd.env_remove("BRAVE_SEARCH_API_KEY");
                    cmd.env_remove("WEBPIPE_TAVILY_API_KEY");
                    cmd.env_remove("TAVILY_API_KEY");
                    cmd.env_remove("WEBPIPE_SEARXNG_ENDPOINT");
                    cmd.env_remove("WEBPIPE_FIRECRAWL_API_KEY");
                    cmd.env_remove("FIRECRAWL_API_KEY");
                    cmd.env_remove("WEBPIPE_PERPLEXITY_API_KEY");
                    cmd.env_remove("PERPLEXITY_API_KEY");
                    // Keep stderr quiet-ish for this probe unless explicitly enabled.
                    cmd.env("RUST_LOG", "error");
                }))?;

                let service = ().serve(child).await?;
                let check_t0 = std::time::Instant::now();
                let res = tokio::time::timeout(
                    std::time::Duration::from_millis(args.timeout_ms),
                    service.list_tools(Default::default()),
                )
                .await;
                stdio_elapsed_ms = Some(check_t0.elapsed().as_millis());

                match res {
                    Ok(Ok(tools)) => {
                        stdio_ok = Some(true);
                        stdio_tool_count = Some(tools.tools.len());
                    }
                    Ok(Err(e)) => {
                        stdio_ok = Some(false);
                        let msg = e.to_string();
                        let hint = if msg.contains("ConnectionClosed")
                            || msg.contains("initialized request")
                            || msg.contains("TransportClosed")
                        {
                            "The child process closed the stdio transport early. Common causes: stdout contamination (printing logs to stdout), wrong args (not running mcp-stdio), or a crash on startup. Reinstall `webpipe` and check it prints nothing to stdout in mcp-stdio mode."
                        } else {
                            "Stdio MCP handshake failed. Reinstall `webpipe` and verify your Cursor mcp.json points at the correct command and uses args: [\"mcp-stdio\"]."
                        };
                        stdio_error = Some(serde_json::json!({
                            "code": "handshake_failed",
                            "message": msg,
                            "hint": hint
                        }));
                    }
                    Err(_elapsed) => {
                        stdio_ok = Some(false);
                        stdio_error = Some(serde_json::json!({
                            "code": "timeout",
                            "message": format!("stdio handshake timed out after {}ms", args.timeout_ms),
                            "hint": "The child did not respond to list_tools in time. Check for a stuck startup (deadlock, slow disk, or a prompt). Also verify WEBPIPE_CACHE_DIR is writable."
                        }));
                    }
                }

                let _ = service.cancel().await;
            }

            #[cfg(not(feature = "stdio"))]
            if args.check_stdio {
                stdio_ok = Some(false);
            }

            checks.push(serde_json::json!({
                "name": "mcp_stdio_handshake",
                "ok": if args.check_stdio { stdio_ok.unwrap_or(false) } else { true },
                "skipped": !args.check_stdio,
                "message": if !args.check_stdio {
                    "stdio MCP handshake skipped"
                } else if stdio_ok.unwrap_or(false) {
                    "stdio MCP handshake succeeded"
                } else {
                    "stdio MCP handshake failed"
                },
                "hint": if !args.check_stdio || stdio_ok.unwrap_or(false) {
                    ""
                } else if cfg!(feature = "stdio") {
                    "Check that Cursor is pointing at the correct `webpipe` binary, and that `WEBPIPE_CACHE_DIR` is writable. If needed, reinstall: `cargo install --path <repo>/webpipe/crates/webpipe-mcp --bin webpipe --force`."
                } else {
                    "`mcp-stdio` requires building with feature `stdio`."
                },
                "tool_count": stdio_tool_count,
                "elapsed_ms": stdio_elapsed_ms,
                "error": stdio_error,
            }));

            let ok = checks.iter().all(|c| c["ok"].as_bool().unwrap_or(false));
            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "doctor",
                "ok": ok,
                "name": "webpipe",
                "version": env!("CARGO_PKG_VERSION"),
                "platform": {
                    "os": std::env::consts::OS,
                    "arch": std::env::consts::ARCH,
                },
                "features": {
                    "stdio": cfg!(feature = "stdio"),
                    "semantic": false,
                    "embeddings_openai": false,
                    "embeddings_tei": false,
                },
                "elapsed_ms": t0.elapsed().as_millis(),
                "configured": {
                    "providers": {
                        "brave": brave_configured,
                        "tavily": tavily_configured,
                        "searxng": searxng_configured,
                    },
                    "remote_fetch": {
                        "firecrawl": firecrawl_configured,
                    },
                    "llm": {
                        "perplexity": perplexity_configured,
                        "openrouter": has_env("WEBPIPE_OPENROUTER_API_KEY") || has_env("OPENROUTER_API_KEY"),
                        "openrouter_model": if has_env("WEBPIPE_OPENROUTER_API_KEY") || has_env("OPENROUTER_API_KEY") {
                            Some(mcp::WebpipeMcp::openrouter_model_from_env())
                        } else {
                            None
                        },
                        "openai": has_env("WEBPIPE_OPENAI_API_KEY") || has_env("OPENAI_API_KEY"),
                        "openai_model": if has_env("WEBPIPE_OPENAI_API_KEY") || has_env("OPENAI_API_KEY") {
                            Some(mcp::WebpipeMcp::openai_model_from_env())
                        } else {
                            None
                        },
                        "groq": has_env("WEBPIPE_GROQ_API_KEY") || has_env("GROQ_API_KEY"),
                        "groq_model": if has_env("WEBPIPE_GROQ_API_KEY") || has_env("GROQ_API_KEY") {
                            Some(mcp::WebpipeMcp::groq_model_from_env())
                        } else {
                            None
                        },
                    },
                    "cache_dir": cache_dir.to_string_lossy().to_string(),
                },
                "checks": checks,
            });
            match args.output.to_ascii_lowercase().as_str() {
                "text" => {
                    println!("webpipe {} (ok={})", env!("CARGO_PKG_VERSION"), ok);
                    println!(
                        "cache_dir: {}",
                        payload["configured"]["cache_dir"].as_str().unwrap_or("")
                    );
                    println!(
                        "providers: brave={} tavily={}",
                        payload["configured"]["providers"]["brave"]
                            .as_bool()
                            .unwrap_or(false),
                        payload["configured"]["providers"]["tavily"]
                            .as_bool()
                            .unwrap_or(false),
                    );
                    println!(
                        "remote_fetch: firecrawl={}",
                        payload["configured"]["remote_fetch"]["firecrawl"]
                            .as_bool()
                            .unwrap_or(false),
                    );
                    println!(
                        "llm: openrouter={} openai={} groq={} perplexity={}",
                        payload["configured"]["llm"]["openrouter"]
                            .as_bool()
                            .unwrap_or(false),
                        payload["configured"]["llm"]["openai"]
                            .as_bool()
                            .unwrap_or(false),
                        payload["configured"]["llm"]["groq"]
                            .as_bool()
                            .unwrap_or(false),
                        payload["configured"]["llm"]["perplexity"]
                            .as_bool()
                            .unwrap_or(false),
                    );
                    println!("checks:");
                    if let Some(arr) = payload["checks"].as_array() {
                        for c in arr {
                            let name = c["name"].as_str().unwrap_or("?");
                            let ok = c["ok"].as_bool().unwrap_or(false);
                            let skipped = c["skipped"].as_bool().unwrap_or(false);
                            if skipped {
                                println!("- {}: skipped", name);
                            } else {
                                println!("- {}: {}", name, if ok { "ok" } else { "fail" });
                            }
                        }
                    }
                }
                _ => println!("{payload}"),
            }
        }
        Commands::Version(args) => {
            let v = serde_json::json!({
                "schema_version": 1,
                "kind": "version",
                "ok": true,
                "name": "webpipe",
                "version": env!("CARGO_PKG_VERSION"),
            });
            match args.output.to_ascii_lowercase().as_str() {
                "text" => println!("webpipe {}", env!("CARGO_PKG_VERSION")),
                _ => println!("{}", v),
            }
        }
        Commands::EvalQrels(args) => {
            let now = args.now_epoch_s.unwrap_or_else(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
            });
            let out = args.out.unwrap_or_else(|| {
                std::path::PathBuf::from(format!(".generated/webpipe-eval-qrels-{now}.json"))
            });

            #[derive(serde::Deserialize)]
            struct Qrel {
                query_id: String,
                url: String,
                grade: u64,
            }
            #[derive(serde::Deserialize)]
            struct QrelsFile {
                schema_version: u64,
                kind: String,
                qrels: Vec<Qrel>,
            }

            let qrels_raw = std::fs::read_to_string(&args.qrels)?;
            let qrels_file: QrelsFile = serde_json::from_str(&qrels_raw)?;
            if qrels_file.schema_version != 1 || qrels_file.kind != "webpipe_seed_qrels" {
                anyhow::bail!("unexpected qrels file kind/schema_version");
            }

            let eval_raw = std::fs::read_to_string(&args.eval_artifact)?;
            let eval_v: serde_json::Value = serde_json::from_str(&eval_raw)?;
            if eval_v.get("kind").and_then(|v| v.as_str()) != Some("eval_search_extract") {
                anyhow::bail!("eval_artifact.kind must be eval_search_extract");
            }

            fn canonicalize_eval_url(u: &str) -> Option<String> {
                let s = u.trim();
                if s.is_empty() {
                    return None;
                }
                // For scoring, treat fragment URLs as the same document.
                // This matches our extraction/link behavior which often drops fragments for stability.
                if let Ok(mut url) = reqwest::Url::parse(s) {
                    url.set_fragment(None);
                    Some(url.to_string())
                } else {
                    None
                }
            }

            let mut by_q: std::collections::BTreeMap<String, Vec<&Qrel>> =
                std::collections::BTreeMap::new();
            for q in &qrels_file.qrels {
                by_q.entry(q.query_id.clone()).or_default().push(q);
            }

            let mut per_query = Vec::new();
            let mut total_expected = 0usize;
            let mut total_found = 0usize;

            let runs = eval_v
                .get("runs")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for run in runs {
                let q = run
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let qid = run
                    .get("query_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| q.clone());

                let expected = by_q.get(&qid).cloned().unwrap_or_default();
                if expected.is_empty() {
                    continue;
                }

                let mut got_urls = std::collections::BTreeSet::<String>::new();
                if let Some(per_mode) = run.get("per_selection_mode").and_then(|v| v.as_array()) {
                    for pm in per_mode {
                        if let Some(results) = pm
                            .get("result")
                            .and_then(|r| r.get("results"))
                            .and_then(|v| v.as_array())
                        {
                            for r in results {
                                // Score against both the original URL and the final (redirected) URL.
                                // This makes scoring robust to redirects while keeping the tool output unchanged.
                                if let Some(u) = r.get("url").and_then(|x| x.as_str()) {
                                    if let Some(c) = canonicalize_eval_url(u) {
                                        got_urls.insert(c);
                                    }
                                }
                                if let Some(u) = r.get("final_url").and_then(|x| x.as_str()) {
                                    if let Some(c) = canonicalize_eval_url(u) {
                                        got_urls.insert(c);
                                    }
                                }
                            }
                        }
                    }
                }

                let mut expected_urls = std::collections::BTreeSet::<String>::new();
                for e in &expected {
                    if e.grade > 0 {
                        if let Some(c) = canonicalize_eval_url(&e.url) {
                            expected_urls.insert(c);
                        }
                    }
                }

                let found = expected_urls.intersection(&got_urls).count();
                let exp = expected_urls.len();
                total_expected += exp;
                total_found += found;

                per_query.push(serde_json::json!({
                    "query_id": qid,
                    "expected_relevant_url_count": exp,
                    "found_relevant_url_count": found,
                    "missed_urls": expected_urls.difference(&got_urls).cloned().collect::<Vec<_>>(),
                }));
            }

            let payload = serde_json::json!({
                "schema_version": 1,
                "kind": "eval_qrels",
                "generated_at_epoch_s": now,
                "inputs": {
                    "eval_artifact": args.eval_artifact.display().to_string(),
                    "qrels": args.qrels.display().to_string(),
                },
                "summary": {
                    "expected_relevant_urls_total": total_expected,
                    "found_relevant_urls_total": total_found,
                    "recall": if total_expected == 0 { 0.0 } else { (total_found as f64) / (total_expected as f64) }
                },
                "per_query": per_query
            });

            if let Some(p) = out.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&out, serde_json::to_string_pretty(&payload)? + "\n")?;
            println!("{}", out.display());
        }
        #[cfg(not(feature = "stdio"))]
        Commands::McpStdio => {
            anyhow::bail!("mcp-stdio requires feature `stdio` (rebuild with: --features stdio)");
        }
    }

    Ok(())
}
