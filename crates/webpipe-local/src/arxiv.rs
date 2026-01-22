//! Minimal ArXiv client (Atom feed) with bounded results.
//!
//! Notes:
//! - ArXiv exposes an Atom API at `https://export.arxiv.org/api/query`.
//! - This module keeps parsing deliberately minimal and resilient.
//! - "Semantic search" is not provided by ArXiv itself; callers can optionally rerank results.

use crate::Error;
use crate::Result;

fn arxiv_api_endpoint() -> Result<reqwest::Url> {
    let s = std::env::var("WEBPIPE_ARXIV_ENDPOINT")
        .ok()
        .unwrap_or_else(|| "https://export.arxiv.org/api/query".to_string());
    let url = reqwest::Url::parse(s.trim()).map_err(|e| Error::Fetch(e.to_string()))?;
    Ok(url)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArxivPaper {
    pub arxiv_id: String,
    pub url: String,
    pub pdf_url: Option<String>,
    pub title: String,
    pub summary: String,
    pub published: Option<String>,
    pub updated: Option<String>,
    pub authors: Vec<String>,
    pub categories: Vec<String>,
    pub primary_category: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ArxivSearchResponse {
    pub ok: bool,
    pub query: String,
    pub page: usize,
    pub per_page: usize,
    pub total_results: Option<u64>,
    pub papers: Vec<ArxivPaper>,
    pub warnings: Vec<&'static str>,
}

fn normalize_ws(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

fn arxiv_id_from_url(url: &str) -> Option<String> {
    // Examples:
    // - https://arxiv.org/abs/0805.3415
    // - http://arxiv.org/abs/cs/9901001v1
    let u = url.trim();
    let i = u.rfind("/abs/")?;
    let tail = &u[i + "/abs/".len()..];
    let id = tail.trim_matches('/').trim();
    (!id.is_empty()).then_some(id.to_string())
}

pub fn arxiv_abs_url(id: &str) -> String {
    format!("https://arxiv.org/abs/{}", id.trim())
}

pub fn arxiv_pdf_url(id: &str) -> String {
    format!("https://arxiv.org/pdf/{}.pdf", id.trim())
}

fn build_search_query(query: &str, categories: &[String]) -> String {
    // ArXiv query syntax:
    // - all:term
    // - cat:cs.LG
    //
    // We'll approximate phrase search by quoting when the query has spaces.
    let q = query.trim();
    let q_part = if q.contains(' ') {
        format!("all:\"{}\"", q.replace('"', ""))
    } else {
        format!("all:{}", q)
    };
    if categories.is_empty() {
        return q_part;
    }
    let cats = categories
        .iter()
        .map(|c| c.trim())
        .filter(|c| !c.is_empty())
        .map(|c| format!("cat:{}", c))
        .collect::<Vec<_>>();
    if cats.is_empty() {
        return q_part;
    }
    format!("{q_part} AND ({})", cats.join(" OR "))
}

fn year_from_rfc3339(s: &str) -> Option<u32> {
    // ArXiv returns RFC3339-ish timestamps (e.g. 2024-09-08T00:00:00Z).
    let y = s.get(0..4)?.parse::<u32>().ok()?;
    Some(y)
}

fn parse_atom(body: &str) -> (Option<u64>, Vec<ArxivPaper>, Vec<&'static str>) {
    let mut warnings: Vec<&'static str> = Vec::new();
    let mut total_results: Option<u64> = None;
    let mut papers: Vec<ArxivPaper> = Vec::new();

    // We use quick-xml because Atom namespaces make regex parsing brittle.
    let mut reader = quick_xml::Reader::from_str(body);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();

    #[derive(Default)]
    struct Cur {
        id_url: String,
        title: String,
        summary: String,
        published: Option<String>,
        updated: Option<String>,
        authors: Vec<String>,
        categories: Vec<String>,
        primary_category: Option<String>,
        pdf_url: Option<String>,
        in_entry: bool,
        in_author: bool,
        cur_text: String,
        cur_tag: String,
    }

    let mut cur = Cur::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(quick_xml::events::Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                cur.cur_tag = name.clone();
                if name.ends_with("entry") {
                    cur = Cur::default();
                    cur.in_entry = true;
                }
                if cur.in_entry && name.ends_with("author") {
                    cur.in_author = true;
                }
                if cur.in_entry && name.ends_with("category") {
                    for a in e.attributes().flatten() {
                        let k = String::from_utf8_lossy(a.key.as_ref());
                        if k == "term" {
                            let v = a
                                .unescape_value()
                                .map(|v| v.to_string())
                                .unwrap_or_default();
                            if !v.trim().is_empty() {
                                cur.categories.push(v);
                            }
                        }
                    }
                }
                if cur.in_entry && name.ends_with("primary_category") {
                    for a in e.attributes().flatten() {
                        let k = String::from_utf8_lossy(a.key.as_ref());
                        if k == "term" {
                            let v = a
                                .unescape_value()
                                .map(|v| v.to_string())
                                .unwrap_or_default();
                            cur.primary_category = (!v.trim().is_empty()).then_some(v);
                        }
                    }
                }
                if cur.in_entry && name.ends_with("link") {
                    let mut rel = None;
                    let mut ty = None;
                    let mut href = None;
                    for a in e.attributes().flatten() {
                        let k = String::from_utf8_lossy(a.key.as_ref()).to_string();
                        let v = a
                            .unescape_value()
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        match k.as_str() {
                            "rel" => rel = Some(v),
                            "type" => ty = Some(v),
                            "href" => href = Some(v),
                            _ => {}
                        }
                    }
                    if rel.as_deref() == Some("related") && ty.as_deref() == Some("application/pdf")
                    {
                        cur.pdf_url = href;
                    }
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if cur.in_entry && name.ends_with("category") {
                    for a in e.attributes().flatten() {
                        let k = String::from_utf8_lossy(a.key.as_ref());
                        if k == "term" {
                            let v = a
                                .unescape_value()
                                .map(|v| v.to_string())
                                .unwrap_or_default();
                            if !v.trim().is_empty() {
                                cur.categories.push(v);
                            }
                        }
                    }
                }
                if cur.in_entry && name.ends_with("primary_category") {
                    for a in e.attributes().flatten() {
                        let k = String::from_utf8_lossy(a.key.as_ref());
                        if k == "term" {
                            let v = a
                                .unescape_value()
                                .map(|v| v.to_string())
                                .unwrap_or_default();
                            cur.primary_category = (!v.trim().is_empty()).then_some(v);
                        }
                    }
                }
                if cur.in_entry && name.ends_with("link") {
                    let mut rel = None;
                    let mut ty = None;
                    let mut href = None;
                    for a in e.attributes().flatten() {
                        let k = String::from_utf8_lossy(a.key.as_ref()).to_string();
                        let v = a
                            .unescape_value()
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        match k.as_str() {
                            "rel" => rel = Some(v),
                            "type" => ty = Some(v),
                            "href" => href = Some(v),
                            _ => {}
                        }
                    }
                    if rel.as_deref() == Some("related") && ty.as_deref() == Some("application/pdf")
                    {
                        cur.pdf_url = href;
                    }
                }
            }
            Ok(quick_xml::events::Event::Text(t)) => {
                let txt = t.unescape().map(|t| t.to_string()).unwrap_or_default();
                if cur.in_entry {
                    cur.cur_text.push_str(&txt);
                } else if cur.cur_tag.ends_with("totalResults") {
                    if let Ok(n) = txt.trim().parse::<u64>() {
                        total_results = Some(n);
                    }
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                if cur.in_entry {
                    let txt = normalize_ws(&cur.cur_text);
                    if name.ends_with("id") {
                        cur.id_url = txt;
                    } else if name.ends_with("title") {
                        cur.title = txt;
                    } else if name.ends_with("summary") {
                        cur.summary = txt;
                    } else if name.ends_with("published") {
                        cur.published = (!txt.is_empty()).then_some(txt);
                    } else if name.ends_with("updated") {
                        cur.updated = (!txt.is_empty()).then_some(txt);
                    } else if cur.in_author && name.ends_with("name") && !txt.is_empty() {
                        cur.authors.push(txt);
                    }
                    cur.cur_text.clear();

                    if name.ends_with("author") {
                        cur.in_author = false;
                    }
                    if name.ends_with("entry") {
                        cur.in_entry = false;
                        let url = cur.id_url.clone();
                        let arxiv_id = arxiv_id_from_url(&url).unwrap_or_else(|| url.clone());
                        let pdf_url = cur
                            .pdf_url
                            .clone()
                            .or_else(|| Some(arxiv_pdf_url(&arxiv_id)));
                        papers.push(ArxivPaper {
                            arxiv_id,
                            url: url.clone(),
                            pdf_url,
                            title: cur.title.clone(),
                            summary: cur.summary.clone(),
                            published: cur.published.clone(),
                            updated: cur.updated.clone(),
                            authors: cur.authors.clone(),
                            categories: cur.categories.clone(),
                            primary_category: cur.primary_category.clone(),
                        });
                    }
                }
                cur.cur_tag.clear();
            }
            Err(_) => {
                warnings.push("arxiv_xml_parse_failed_partial");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    (total_results, papers, warnings)
}

pub async fn arxiv_search(
    http: reqwest::Client,
    query: String,
    categories: Vec<String>,
    years: Vec<u32>,
    page: usize,
    per_page: usize,
    timeout_ms: u64,
) -> Result<ArxivSearchResponse> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, 50);
    let start = (page - 1) * per_page;

    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(Error::InvalidUrl("query must be non-empty".to_string()));
    }

    let search_query = build_search_query(&q, &categories);
    let mut url = arxiv_api_endpoint()?;
    url.query_pairs_mut()
        .append_pair("search_query", &search_query)
        .append_pair("start", &start.to_string())
        .append_pair("max_results", &per_page.to_string());

    // Keep ordering deterministic.
    url.query_pairs_mut().append_pair("sortBy", "submittedDate");
    url.query_pairs_mut().append_pair("sortOrder", "descending");

    let resp = http
        .get(url)
        .timeout(std::time::Duration::from_millis(timeout_ms.max(1000)))
        .send()
        .await
        .map_err(|e| Error::Fetch(e.to_string()))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(Error::Fetch(format!("arxiv query failed: HTTP {status}")));
    }
    let body = resp.text().await.map_err(|e| Error::Fetch(e.to_string()))?;
    let (mut total_results, mut papers, warnings) = parse_atom(&body);

    if !years.is_empty() {
        let ys: std::collections::HashSet<u32> = years.iter().copied().collect();
        papers.retain(|p| {
            p.published
                .as_deref()
                .and_then(year_from_rfc3339)
                .map(|y| ys.contains(&y))
                .unwrap_or(false)
        });
        // total_results is no longer exact after filtering.
        total_results = None;
    }

    Ok(ArxivSearchResponse {
        ok: true,
        query: q,
        page,
        per_page,
        total_results,
        papers,
        warnings,
    })
}

pub async fn arxiv_lookup_by_id(
    http: reqwest::Client,
    id: String,
    timeout_ms: u64,
) -> Result<Option<ArxivPaper>> {
    let id = id.trim().to_string();
    if id.is_empty() {
        return Err(Error::InvalidUrl("id must be non-empty".to_string()));
    }

    let mut url = arxiv_api_endpoint()?;
    url.query_pairs_mut()
        .append_pair("id_list", &id)
        .append_pair("max_results", "5");

    let resp = http
        .get(url)
        .timeout(std::time::Duration::from_millis(timeout_ms.max(1000)))
        .send()
        .await
        .map_err(|e| Error::Fetch(e.to_string()))?;
    let status = resp.status().as_u16();
    if !resp.status().is_success() {
        return Err(Error::Fetch(format!(
            "arxiv id_list query failed: HTTP {status}"
        )));
    }
    let body = resp.text().await.map_err(|e| Error::Fetch(e.to_string()))?;
    let (_total, papers, _warnings) = parse_atom(&body);
    Ok(papers.into_iter().find(|p| p.url.contains("/abs/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_atom_extracts_two_entries_and_total_results() {
        let xml = r#"
<feed xmlns="http://www.w3.org/2005/Atom"
      xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
  <opensearch:totalResults>2</opensearch:totalResults>
  <entry>
    <id>http://arxiv.org/abs/0805.3415v1</id>
    <updated>2008-05-22T00:00:00Z</updated>
    <published>2008-05-22T00:00:00Z</published>
    <title> On Upper-Confidence Bound Policies for Non-Stationary Bandit Problems </title>
    <summary>  Some abstract here.  </summary>
    <author><name>A. Author</name></author>
    <author><name>B. Author</name></author>
    <category term="cs.LG" />
    <category term="stat.ML" />
    <link rel="related" type="application/pdf" href="http://arxiv.org/pdf/0805.3415v1"/>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/1305.2545v2</id>
    <updated>2013-05-11T00:00:00Z</updated>
    <published>2013-05-11T00:00:00Z</published>
    <title>Bandits with Knapsacks</title>
    <summary>Abstract two.</summary>
    <author><name>C. Author</name></author>
    <category term="cs.DS" />
  </entry>
</feed>
"#;
        let (total, papers, warnings) = parse_atom(xml);
        assert_eq!(total, Some(2));
        assert!(warnings.is_empty());
        assert_eq!(papers.len(), 2);
        assert_eq!(papers[0].arxiv_id, "0805.3415v1");
        assert!(papers[0]
            .pdf_url
            .as_deref()
            .unwrap_or("")
            .contains("0805.3415v1"));
        assert_eq!(papers[0].authors.len(), 2);
        assert!(papers[0].categories.iter().any(|c| c == "cs.LG"));
    }
}
