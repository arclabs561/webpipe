use std::time::Duration;
use tokio::io::AsyncWriteExt;
use webpipe_core::{Error, Result};

#[derive(Debug, Clone)]
pub struct RenderedPage {
    pub final_url: String,
    pub status: Option<u16>,
    pub html: String,
    pub elapsed_ms: u64,
    pub console_error_count: u64,
    pub mode: String,
}

fn env_truthy(k: &str) -> bool {
    matches!(
        std::env::var(k)
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn privacy_mode_from_env() -> String {
    std::env::var("WEBPIPE_PRIVACY_MODE")
        .unwrap_or_else(|_| "normal".to_string())
        .trim()
        .to_ascii_lowercase()
}

fn node_path_candidates() -> Vec<String> {
    // Best-effort Node global module roots across common setups.
    //
    // We avoid shelling out to `npm root -g` here to keep this deterministic and
    // usable in minimal environments; if the user wants an explicit override,
    // they can set NODE_PATH or WEBPIPE_NODE_PATH.
    let mut out: Vec<String> = Vec::new();

    if let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) {
        // Common “custom npm prefix” layout.
        out.push(
            home.join(".npm-global")
                .join("lib")
                .join("node_modules")
                .to_string_lossy()
                .to_string(),
        );
        // NVM-style global installs (varies, but this catches some setups).
        out.push(
            home.join(".nvm")
                .join("versions")
                .join("node")
                .to_string_lossy()
                .to_string(),
        );
    }

    // macOS/Homebrew defaults.
    out.push("/opt/homebrew/lib/node_modules".to_string());
    // macOS/Intel/Homebrew or manual installs.
    out.push("/usr/local/lib/node_modules".to_string());
    // Linux-ish defaults.
    out.push("/usr/lib/node_modules".to_string());

    out
}

fn detect_node_path_for_playwright() -> Option<String> {
    fn node_path_has_playwright(np: &str) -> bool {
        let s = np.trim();
        if s.is_empty() {
            return false;
        }
        for part in s.split(':') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let p = std::path::PathBuf::from(part).join("playwright");
            if p.is_dir() {
                return true;
            }
        }
        false
    }

    fn npm_root_g() -> Option<String> {
        let out = std::process::Command::new("npm")
            .args(["root", "-g"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() {
            return None;
        }
        let p = std::path::PathBuf::from(s.trim()).join("playwright");
        if p.is_dir() {
            Some(s)
        } else {
            None
        }
    }

    // Explicit override for webpipe (lets users keep NODE_PATH clean globally).
    if let Ok(v) = std::env::var("WEBPIPE_NODE_PATH") {
        let v = v.trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }

    // If NODE_PATH is already set and already contains Playwright, do nothing.
    let existing = std::env::var("NODE_PATH").ok().unwrap_or_default();
    if node_path_has_playwright(&existing) {
        return None;
    }

    // Prefer `npm root -g` when available.
    let found = npm_root_g().or_else(|| {
        for root in node_path_candidates() {
            if root.trim().is_empty() {
                continue;
            }
            let p = std::path::PathBuf::from(root.trim()).join("playwright");
            if p.is_dir() {
                return Some(root);
            }
        }
        None
    })?;

    if existing.trim().is_empty() {
        Some(found)
    } else {
        Some(format!("{existing}:{found}"))
    }
}

fn is_localhost_url(url: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(url) else {
        return false;
    };
    let Some(host) = u.host_str() else {
        return false;
    };
    let h = host.to_ascii_lowercase();
    if h == "localhost" {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return ip.is_loopback();
    }
    false
}

pub async fn render_html_playwright(
    url: &str,
    timeout_ms: u64,
    proxy: Option<&str>,
) -> Result<RenderedPage> {
    // Deterministic escape hatch (tests and “no local tooling” environments).
    if env_truthy("WEBPIPE_RENDER_DISABLE") {
        return Err(Error::NotConfigured(
            "render backend disabled (WEBPIPE_RENDER_DISABLE)".to_string(),
        ));
    }

    // Advanced modes (opt-in via env; validated *before* spawning Node/Playwright so we can be
    // deterministic in environments without node/playwright installed).
    let cdp_endpoint = std::env::var("WEBPIPE_RENDER_CDP_ENDPOINT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let user_data_dir = std::env::var("WEBPIPE_RENDER_USER_DATA_DIR")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if cdp_endpoint.is_some() && user_data_dir.is_some() {
        return Err(Error::NotSupported(
            "WEBPIPE_RENDER_CDP_ENDPOINT and WEBPIPE_RENDER_USER_DATA_DIR are mutually exclusive"
                .to_string(),
        ));
    }

    if cdp_endpoint.is_some() && proxy.is_some() {
        return Err(Error::NotSupported(
            "proxy is not supported with WEBPIPE_RENDER_CDP_ENDPOINT mode".to_string(),
        ));
    }

    // Fail-closed: “use my browser” / “reuse my profile” is incompatible with anonymous mode for
    // non-localhost URLs (it leaks identity via cookies/session/fingerprint).
    let pm = privacy_mode_from_env();
    if pm == "anonymous"
        && (cdp_endpoint.is_some() || user_data_dir.is_some())
        && !is_localhost_url(url)
    {
        return Err(Error::NotSupported(
            "render cdp/persistent modes are not supported in privacy_mode=anonymous for non-localhost URLs".to_string(),
        ));
    }

    // NOTE: We intentionally do not auto-install Playwright at runtime.
    // Auto-install would be slow, non-deterministic, and could violate “offline/anonymous” intent.
    //
    // Expected setup:
    // - Node.js present
    // - `playwright` npm package available to Node (global, or via NODE_PATH / local project)
    // - Browsers installed (e.g. `npx playwright install chromium`)
    //
    // We keep the script small and ensure stdout is JSON-only.
    const JS: &str = r#"
const fs = require('fs');

function ok(obj) { process.stdout.write(JSON.stringify(obj)); }
function bad(code, message, hint) { ok({ ok: false, error: { code, message, hint } }); }

async function main() {
  // Prefer stdin for passing args to avoid argv quoting/encoding issues.
  let arg = '';
  try { arg = fs.readFileSync(0, 'utf8'); } catch (_) {}
  if (!arg || !String(arg).trim()) arg = process.argv[2] || '';
  let req;
  try { req = JSON.parse(arg); } catch (e) { return bad('invalid_params', 'bad JSON args', 'Internal error: could not parse render args.'); }

  let pw;
  try { pw = require('playwright'); } catch (e) {
    return bad('not_configured',
      'Playwright is not installed for Node.js (require(\"playwright\") failed)',
      'Install Playwright (Node): `npm i -g playwright` and then `npx playwright install chromium` (or install it in your project so Node can require it).');
  }

  const url = String(req.url || '').trim();
  if (!url) return bad('invalid_params', 'url must be non-empty', 'Pass an absolute URL like https://example.com.');

  const timeoutMs = Number(req.timeout_ms || 20000);
  const proxy = (req.proxy || '').trim();
  const cdpEndpoint = (req.cdp_endpoint || '').trim();
  const userDataDir = (req.user_data_dir || '').trim();
  const blockResources = (req.block_resources === undefined) ? true : !!req.block_resources;

  let consoleErrorCount = 0;
  const t0 = Date.now();
  let browser;
  let context;
  try {
    let mode = 'launch';
    const contextOpts = { serviceWorkers: 'block' };

    if (cdpEndpoint && userDataDir) {
      return bad('invalid_params', 'cdp_endpoint and user_data_dir are mutually exclusive', 'Choose one advanced mode: connectOverCDP (cdp_endpoint) or launchPersistentContext (user_data_dir).');
    }
    if (cdpEndpoint && proxy) {
      return bad('not_supported', 'proxy is not supported with cdp_endpoint mode', 'Configure proxy at the browser level, or use launch/persistent mode where Playwright can set proxy.');
    }

    if (cdpEndpoint) {
      // Connect to an existing Chrome instance (user-owned browser). Requires Chrome started with remote debugging.
      mode = 'cdp';
      browser = await pw.chromium.connectOverCDP(cdpEndpoint);
      // Reuse existing context if present; else create one.
      const contexts = browser.contexts ? browser.contexts() : [];
      context = (contexts && contexts.length > 0) ? contexts[0] : await browser.newContext(contextOpts);
    } else if (userDataDir) {
      // Persistent profile directory (cookie/session reuse). This is still a Playwright-managed browser process.
      mode = 'persistent';
      const launchOpts = { headless: true };
      if (proxy) launchOpts.proxy = { server: proxy };
      context = await pw.chromium.launchPersistentContext(userDataDir, { ...launchOpts, ...contextOpts });
      browser = context.browser();
    } else {
      const launchOpts = { headless: true };
      if (proxy) launchOpts.proxy = { server: proxy };
      browser = await pw.chromium.launch(launchOpts);
      context = await browser.newContext(contextOpts);
    }

    const page = await context.newPage();
    page.on('console', (msg) => { if (msg.type && msg.type() === 'error') consoleErrorCount += 1; });
    // Reduce tail latency + bandwidth for “render to text”: images/media/fonts rarely help extraction.
    if (blockResources && page.route) {
      try {
        await page.route('**/*', (route) => {
          const req = route.request();
          const rt = req && req.resourceType ? req.resourceType() : '';
          if (rt === 'image' || rt === 'media' || rt === 'font') return route.abort();
          return route.continue();
        });
      } catch (_) {}
    }

    const resp = await page.goto(url, { waitUntil: 'domcontentloaded', timeout: timeoutMs });
    // Best-effort settle: don't block forever on long-polling.
    try { await page.waitForLoadState('networkidle', { timeout: Math.min(5000, timeoutMs) }); } catch (_) {}
    try { await page.waitForTimeout(250); } catch (_) {}

    const html = await page.content();
    const finalUrl = page.url();
    const status = resp ? resp.status() : null;
    const elapsedMs = Date.now() - t0;
    ok({ ok: true, mode, final_url: finalUrl, status, html, elapsed_ms: elapsedMs, console_error_count: consoleErrorCount });
  } catch (e) {
    const elapsedMs = Date.now() - t0;
    bad('fetch_failed', String(e && e.message ? e.message : e), 'Playwright render failed. Try a longer timeout_ms, or a different URL.');
  } finally {
    try { if (browser) await browser.close(); } catch (_) {}
  }
}

main().catch((e) => bad('fetch_failed', String(e && e.message ? e.message : e), 'Playwright render failed.'));
"#;

    let t0 = std::time::Instant::now();
    let proxy_s = proxy.unwrap_or("").trim().to_string();
    let block_resources = std::env::var("WEBPIPE_RENDER_BLOCK_RESOURCES")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .map(|s| !(s == "0" || s == "false" || s == "no" || s == "off"))
        .unwrap_or(true);
    let args_json = serde_json::json!({
        "url": url,
        "timeout_ms": timeout_ms,
        "proxy": proxy_s,
        "cdp_endpoint": cdp_endpoint,
        "user_data_dir": user_data_dir,
        "block_resources": block_resources,
    })
    .to_string();

    // Hard wall-clock timeout for the entire Node+Playwright operation.
    //
    // Important: this must be enforced with `tokio::time::timeout` around the child wait;
    // checking elapsed *after* completion does not prevent hangs.
    let hard_timeout_ms = std::env::var("WEBPIPE_RENDER_HARD_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(timeout_ms.saturating_add(10_000));

    // Prefer the user's Node binary if they set it; otherwise use PATH.
    let node_bin = std::env::var("WEBPIPE_NODE").unwrap_or_else(|_| "node".to_string());

    let mut cmd = tokio::process::Command::new(node_bin);
    // Make Playwright discoverable when installed globally, without requiring users to set NODE_PATH.
    if let Some(node_path) = detect_node_path_for_playwright() {
        cmd.env("NODE_PATH", node_path);
    }
    let mut child = cmd
        .arg("-e")
        .arg(JS)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            Error::NotConfigured(format!(
                "Playwright render requires Node.js (`node`) and the Playwright npm package: {e}"
            ))
        })?;

    if let Some(mut stdin) = child.stdin.take() {
        // Best-effort: if stdin write fails, the child will produce a deterministic JSON error
        // (or the outer wait will fail).
        let _ = stdin.write_all(args_json.as_bytes()).await;
        // Ensure EOF so the script's readFileSync(0, ...) completes deterministically.
        let _ = stdin.shutdown().await;
    }

    // `wait_with_output` consumes the child, which prevents killing it on timeout.
    // Instead, read stdout/stderr concurrently and `wait()` with a hard timeout.
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Fetch("Playwright render: missing stdout pipe".to_string()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Fetch("Playwright render: missing stderr pipe".to_string()))?;

    let stdout_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stdout, &mut buf).await;
        buf
    });
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut buf).await;
        buf
    });

    let status =
        match tokio::time::timeout(Duration::from_millis(hard_timeout_ms), child.wait()).await {
            Ok(r) => r.map_err(|e| {
                Error::NotConfigured(format!(
                "Playwright render requires Node.js (`node`) and the Playwright npm package: {e}"
            ))
            })?,
            Err(_) => {
                let _ = child.kill().await;
                // Avoid leaving zombie processes around; wait best-effort.
                let _ = child.wait().await;
                // Don't leak background tasks if the child never produced output.
                stdout_task.abort();
                stderr_task.abort();
                return Err(Error::Fetch(format!(
                    "Playwright render hard timeout after {hard_timeout_ms}ms"
                )));
            }
        };

    let out_stdout = stdout_task.await.unwrap_or_default();
    let out_stderr = stderr_task.await.unwrap_or_default();

    let out = std::process::Output {
        status,
        stdout: out_stdout,
        stderr: out_stderr,
    };

    // If node exits non-zero we still try to parse stdout (we force JSON on stdout).
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let v: serde_json::Value = serde_json::from_str(&stdout).map_err(|e| {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        if stderr.is_empty() {
            Error::Fetch(format!("Playwright render returned invalid JSON: {e}"))
        } else {
            Error::Fetch(format!(
                "Playwright render returned invalid JSON: {e}. stderr: {stderr}"
            ))
        }
    })?;

    if v.get("ok").and_then(|x| x.as_bool()) != Some(true) {
        let code = v
            .pointer("/error/code")
            .and_then(|x| x.as_str())
            .unwrap_or("fetch_failed");
        let message = v
            .pointer("/error/message")
            .and_then(|x| x.as_str())
            .unwrap_or("Playwright render failed");
        let hint = v
            .pointer("/error/hint")
            .and_then(|x| x.as_str())
            .unwrap_or("");

        let err = match code {
            "not_configured" => Error::NotConfigured(message.to_string()),
            "invalid_params" => Error::InvalidUrl(message.to_string()),
            "not_supported" => Error::NotSupported(message.to_string()),
            _ => Error::Fetch(message.to_string()),
        };

        if !hint.trim().is_empty() {
            // Preserve the actionable hint in the error message; callers can also show their own hint.
            return Err(match err {
                Error::NotConfigured(m) => Error::NotConfigured(format!("{m}. {hint}")),
                Error::InvalidUrl(m) => Error::InvalidUrl(format!("{m}. {hint}")),
                Error::Fetch(m) => Error::Fetch(format!("{m}. {hint}")),
                other => other,
            });
        }
        return Err(err);
    }

    let final_url = v
        .get("final_url")
        .and_then(|x| x.as_str())
        .unwrap_or(url)
        .to_string();
    let status = v.get("status").and_then(|x| x.as_u64()).map(|n| n as u16);
    let html = v
        .get("html")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let elapsed_ms = v
        .get("elapsed_ms")
        .and_then(|x| x.as_u64())
        .unwrap_or(t0.elapsed().as_millis() as u64);
    let console_error_count = v
        .get("console_error_count")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let mode = v
        .get("mode")
        .and_then(|x| x.as_str())
        .unwrap_or("launch")
        .to_string();

    // Safety: avoid pathological empty results looking like success.
    if html.trim().is_empty() {
        return Err(Error::Fetch(
            "Playwright render returned empty HTML".to_string(),
        ));
    }

    // Enforce an upper bound similar to fetch maxes, so we don't accidentally move 100MB through MCP.
    let max_html_chars = std::env::var("WEBPIPE_RENDER_MAX_HTML_CHARS")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .unwrap_or(2_000_000);
    if html.len() > max_html_chars {
        return Err(Error::Fetch(format!(
            "Playwright render HTML too large ({} chars > WEBPIPE_RENDER_MAX_HTML_CHARS={})",
            html.len(),
            max_html_chars
        )));
    }

    Ok(RenderedPage {
        final_url,
        status,
        html,
        elapsed_ms,
        console_error_count,
        mode,
    })
}
