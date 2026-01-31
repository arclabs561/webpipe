//! URL rewriting helpers (bounded, deterministic).
//!
//! These are conservative: they only rewrite when the target is very likely a low-signal HTML shell
//! and a higher-signal canonical text artifact is available.

fn env_csv(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .unwrap_or_default()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn host_matches(host: &str, pat: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    let pat = pat.trim().to_ascii_lowercase();
    if host == pat {
        return true;
    }
    host.ends_with(&format!(".{pat}"))
}

fn github_rewrite_hosts() -> Vec<String> {
    let v = env_csv("WEBPIPE_GITHUB_REWRITE_HOSTS");
    if v.is_empty() {
        vec!["github.com".to_string(), "www.github.com".to_string()]
    } else {
        v
    }
}

fn github_raw_host() -> String {
    std::env::var("WEBPIPE_GITHUB_RAW_HOST")
        .ok()
        .unwrap_or_else(|| "raw.githubusercontent.com".to_string())
        .trim()
        .to_string()
}

fn host_with_port(u: &reqwest::Url) -> String {
    let host = u.host_str().unwrap_or_default();
    if let Some(port) = u.port() {
        format!("{host}:{port}")
    } else {
        host.to_string()
    }
}

fn github_rewrite_branches() -> Vec<String> {
    let v = env_csv("WEBPIPE_GITHUB_REWRITE_BRANCHES");
    if v.is_empty() {
        vec!["main".to_string(), "master".to_string()]
    } else {
        v
    }
}

fn github_readme_paths() -> Vec<&'static str> {
    // Keep bounded and pragmatic.
    [
        "README.md",
        "README.rst",
        "README.txt",
        "Readme.md",
        "readme.md",
    ]
    .to_vec()
}

fn arxiv_rewrite_hosts() -> Vec<String> {
    let v = env_csv("WEBPIPE_ARXIV_REWRITE_HOSTS");
    if v.is_empty() {
        vec!["arxiv.org".to_string(), "www.arxiv.org".to_string()]
    } else {
        v
    }
}

/// If `url` looks like an arXiv abstract page (`arxiv.org/abs/<id>`), return a PDF candidate URL.
///
/// This is bounded/deterministic and does not perform network IO.
pub fn arxiv_abs_pdf_candidates(url: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !arxiv_rewrite_hosts().iter().any(|h| host_matches(&host, h)) {
        return None;
    }
    let path = u.path().trim_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    if parts.len() < 2 {
        return None;
    }
    if parts[0] != "abs" {
        return None;
    }
    let id = parts[1].trim();
    if id.is_empty() {
        return None;
    }
    // Preserve version suffixes like 1234.5678v2 by copying the whole id segment.
    let scheme = u.scheme();
    let mut pdf_host = u.host_str().unwrap_or("arxiv.org").to_string();
    if let Some(port) = u.port() {
        pdf_host = format!("{pdf_host}:{port}");
    }
    let mut out = Vec::new();
    out.push(format!("{scheme}://{pdf_host}/pdf/{id}.pdf"));
    Some(out)
}

/// If `url` looks like a GitHub file view (`github.com/<owner>/<repo>/blob/<ref>/<path...>`),
/// return a raw file candidate URL (`raw.githubusercontent.com/<owner>/<repo>/<ref>/<path...>`).
///
/// This is bounded/deterministic and does not perform network IO.
pub fn github_blob_raw_candidates(url: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !github_rewrite_hosts()
        .iter()
        .any(|h| host_matches(&host, h))
    {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    // owner/repo/blob/<ref>/<path...>
    if parts.len() < 5 {
        return None;
    }
    if parts[2] != "blob" {
        return None;
    }
    let owner = parts[0].trim();
    let repo = parts[1].trim();
    let rf = parts[3].trim();
    if owner.is_empty() || repo.is_empty() || rf.is_empty() {
        return None;
    }
    let rel_path = parts[4..].join("/");
    if rel_path.trim().is_empty() {
        return None;
    }
    let scheme = u.scheme();
    let raw_host = github_raw_host();
    Some(vec![format!(
        "{scheme}://{raw_host}/{owner}/{repo}/{rf}/{rel_path}"
    )])
}

/// If `url` looks like a GitHub PR page (`.../pull/<n>`), return patch/diff candidate URLs.
///
/// This avoids extracting GitHub UI shells for PRs when the `.patch` (or `.diff`) artifact is available.
pub fn github_pr_patch_candidates(url: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !github_rewrite_hosts()
        .iter()
        .any(|h| host_matches(&host, h))
    {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[2] != "pull" {
        return None;
    }
    let owner = parts[0].trim();
    let repo = parts[1].trim();
    let n = parts[3].trim();
    if owner.is_empty() || repo.is_empty() || n.is_empty() {
        return None;
    }
    if !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let scheme = u.scheme();
    let hostp = host_with_port(&u);
    Some(vec![
        format!("{scheme}://{hostp}/{owner}/{repo}/pull/{n}.patch"),
        format!("{scheme}://{hostp}/{owner}/{repo}/pull/{n}.diff"),
    ])
}

/// If `url` looks like a GitHub commit page (`.../commit/<sha>`), return a patch candidate URL.
pub fn github_commit_patch_candidates(url: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !github_rewrite_hosts()
        .iter()
        .any(|h| host_matches(&host, h))
    {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[2] != "commit" {
        return None;
    }
    let owner = parts[0].trim();
    let repo = parts[1].trim();
    let sha = parts[3].trim();
    if owner.is_empty() || repo.is_empty() || sha.is_empty() {
        return None;
    }
    let scheme = u.scheme();
    let hostp = host_with_port(&u);
    Some(vec![format!(
        "{scheme}://{hostp}/{owner}/{repo}/commit/{sha}.patch"
    )])
}

fn gist_rewrite_hosts() -> Vec<String> {
    let v = env_csv("WEBPIPE_GIST_REWRITE_HOSTS");
    if v.is_empty() {
        vec![
            "gist.github.com".to_string(),
            "www.gist.github.com".to_string(),
        ]
    } else {
        v
    }
}

fn gist_raw_host() -> String {
    std::env::var("WEBPIPE_GIST_RAW_HOST")
        .ok()
        .unwrap_or_else(|| "gist.githubusercontent.com".to_string())
        .trim()
        .to_string()
}

/// If `url` looks like a GitHub Gist page (`gist.github.com/<user>/<gistid>`),
/// return a raw candidate (`gist.githubusercontent.com/<user>/<gistid>/raw`).
pub fn gist_raw_candidates(url: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !gist_rewrite_hosts().iter().any(|h| host_matches(&host, h)) {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let user = parts[0].trim();
    let gist_id = parts[1].trim();
    if user.is_empty() || gist_id.is_empty() {
        return None;
    }
    let scheme = u.scheme();
    let raw_host = gist_raw_host();
    Some(vec![format!("{scheme}://{raw_host}/{user}/{gist_id}/raw")])
}

/// If `url` looks like a GitHub issue page (`github.com/<owner>/<repo>/issues/<n>`),
/// return a GitHub API issue endpoint candidate (`api.github.com/repos/<owner>/<repo>/issues/<n>`).
///
/// This is bounded/deterministic and does not perform network IO.
pub fn github_issue_api_candidates(url: &str, api_base: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !github_rewrite_hosts()
        .iter()
        .any(|h| host_matches(&host, h))
    {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    if parts.len() != 4 {
        return None;
    }
    if parts[2] != "issues" {
        return None;
    }
    let owner = parts[0].trim();
    let repo = parts[1].trim();
    let n = parts[3].trim();
    if owner.is_empty() || repo.is_empty() || n.is_empty() {
        return None;
    }
    if !n.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let base = api_base.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    Some(vec![format!("{base}/repos/{owner}/{repo}/issues/{n}")])
}

/// If `url` looks like a GitHub release page:
/// - `.../releases/latest`
/// - `.../releases/tag/<tag>`
/// return GitHub API candidates:
/// - `/repos/<owner>/<repo>/releases/latest`
/// - `/repos/<owner>/<repo>/releases/tags/<tag>`
pub fn github_release_api_candidates(url: &str, api_base: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !github_rewrite_hosts()
        .iter()
        .any(|h| host_matches(&host, h))
    {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    if parts.len() < 4 {
        return None;
    }
    let owner = parts[0].trim();
    let repo = parts[1].trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    if parts[2] != "releases" {
        return None;
    }
    let base = api_base.trim().trim_end_matches('/');
    if base.is_empty() {
        return None;
    }
    if parts[3] == "latest" {
        return Some(vec![format!("{base}/repos/{owner}/{repo}/releases/latest")]);
    }
    if parts[3] == "tag" && parts.len() >= 5 {
        let tag = parts[4].trim();
        if tag.is_empty() {
            return None;
        }
        return Some(vec![format!(
            "{base}/repos/{owner}/{repo}/releases/tags/{tag}"
        )]);
    }
    None
}

/// If `url` looks like a GitHub repo root (`github.com/<owner>/<repo>`), return raw README candidate URLs.
///
/// This avoids extracting the GitHub HTML app shell (often low-signal for evidence gathering).
///
/// Candidates are bounded and deterministic. This function does not perform any network IO.
pub fn github_repo_raw_readme_candidates(url: &str) -> Option<Vec<String>> {
    let u = reqwest::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_string();
    if !github_rewrite_hosts()
        .iter()
        .any(|h| host_matches(&host, h))
    {
        return None;
    }
    let parts: Vec<&str> = u.path().trim_matches('/').split('/').collect();
    if parts.len() != 2 {
        return None;
    }
    let owner = parts[0].trim();
    let repo = parts[1].trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }

    let scheme = u.scheme();
    let raw_host = github_raw_host();
    let branches = github_rewrite_branches();
    let readmes = github_readme_paths();
    let mut out = Vec::new();
    for br in branches {
        for p in &readmes {
            out.push(format!("{scheme}://{raw_host}/{owner}/{repo}/{br}/{p}"));
        }
    }
    Some(out)
}
