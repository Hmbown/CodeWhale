//! Shared DuckDuckGo / Bing HTML SERP scrapers and spam filter.
//!
//! Used by both `web_search` and `web_run` so parser behavior (including the
//! #964 single-domain spam heuristic) cannot drift between the two paths.

use base64::{Engine as _, engine::general_purpose};
use regex::Regex;
use std::sync::OnceLock;

/// One parsed search hit: title, absolute URL, optional snippet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrapedSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
}

// Cached regex patterns for HTML parsing
static TITLE_RE: OnceLock<Regex> = OnceLock::new();
static SNIPPET_RE: OnceLock<Regex> = OnceLock::new();
static TAG_RE: OnceLock<Regex> = OnceLock::new();
static BING_RESULT_RE: OnceLock<Regex> = OnceLock::new();
static BING_TITLE_RE: OnceLock<Regex> = OnceLock::new();
static BING_SNIPPET_RE: OnceLock<Regex> = OnceLock::new();

fn get_title_re() -> &'static Regex {
    TITLE_RE.get_or_init(|| {
        Regex::new(r#"<a[^>]*class=\"result__a\"[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("title regex pattern is valid")
    })
}

fn get_snippet_re() -> &'static Regex {
    SNIPPET_RE.get_or_init(|| {
        Regex::new(
            r#"<a[^>]*class=\"result__snippet\"[^>]*>(.*?)</a>|<div[^>]*class=\"result__snippet\"[^>]*>(.*?)</div>"#,
        )
        .expect("snippet regex pattern is valid")
    })
}

fn get_tag_re() -> &'static Regex {
    TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").expect("tag regex pattern is valid"))
}

fn get_bing_result_re() -> &'static Regex {
    BING_RESULT_RE.get_or_init(|| {
        Regex::new(r#"(?is)<li[^>]*class=\"[^\"]*\bb_algo\b[^\"]*\"[^>]*>(.*?)</li>"#)
            .expect("bing result regex pattern is valid")
    })
}

fn get_bing_title_re() -> &'static Regex {
    BING_TITLE_RE.get_or_init(|| {
        Regex::new(r#"(?is)<h2[^>]*>.*?<a[^>]*href=\"([^\"]+)\"[^>]*>(.*?)</a>"#)
            .expect("bing title regex pattern is valid")
    })
}

fn get_bing_snippet_re() -> &'static Regex {
    BING_SNIPPET_RE.get_or_init(|| {
        Regex::new(r#"(?is)<div[^>]*class=\"[^\"]*\bb_caption\b[^\"]*\"[^>]*>.*?<p[^>]*>(.*?)</p>"#)
            .expect("bing snippet regex pattern is valid")
    })
}

/// Parse DuckDuckGo HTML SERP results. Applies the spam filter unconditionally:
/// a single-domain-dominated result set is returned as empty.
pub fn parse_duckduckgo_results(html: &str, max_results: usize) -> Vec<ScrapedSearchResult> {
    let title_re = get_title_re();
    let snippet_re = get_snippet_re();
    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .filter_map(|cap| cap.get(1).or_else(|| cap.get(2)))
        .map(|m| normalize_text(m.as_str()))
        .collect();

    let mut results = Vec::new();
    for (idx, cap) in title_re.captures_iter(html).enumerate() {
        if results.len() >= max_results {
            break;
        }
        let href = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_text(title_raw);
        if title.is_empty() {
            continue;
        }
        let url = normalize_duckduckgo_url(href);
        let snippet = snippets
            .get(idx)
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty());

        results.push(ScrapedSearchResult {
            title,
            url,
            snippet,
        });
    }

    if is_likely_spam_results(&results) {
        // Same defence as the Bing path (#964): a DDG fallback page can
        // also serve a single-domain stuffed result set when the upstream
        // is degraded. Drop rather than mislead the model.
        return Vec::new();
    }
    results
}

/// Parse Bing HTML SERP results. Applies the spam filter unconditionally.
pub fn parse_bing_results(html: &str, max_results: usize) -> Vec<ScrapedSearchResult> {
    let mut results = Vec::new();
    for cap in get_bing_result_re().captures_iter(html) {
        if results.len() >= max_results {
            break;
        }
        let Some(block) = cap.get(1).map(|m| m.as_str()) else {
            continue;
        };
        let Some(title_cap) = get_bing_title_re().captures(block) else {
            continue;
        };
        let href = title_cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let title_raw = title_cap.get(2).map(|m| m.as_str()).unwrap_or("");
        let title = normalize_text(title_raw);
        if title.is_empty() {
            continue;
        }
        let snippet = get_bing_snippet_re()
            .captures(block)
            .and_then(|snippet_cap| snippet_cap.get(1))
            .map(|m| normalize_text(m.as_str()))
            .filter(|s| !s.is_empty());

        results.push(ScrapedSearchResult {
            title,
            url: normalize_bing_url(href),
            snippet,
        });
    }

    if is_likely_spam_results(&results) {
        // Bing's scraping endpoint occasionally serves a stuffed page
        // where the same low-quality domain owns most of the b_algo
        // entries — #964 reported eight in a row from
        // `astralia.forumgratuit.org` for unrelated queries. Treat the
        // batch as "no results" so the caller surfaces a clean failure
        // message instead of routing the model toward junk.
        return Vec::new();
    }
    results
}

/// Detect DuckDuckGo bot-challenge interstitial HTML.
pub fn is_duckduckgo_challenge(html: &str) -> bool {
    html.contains("anomaly-modal") || html.contains("Unfortunately, bots use DuckDuckGo too")
}

/// Heuristic spam detector for scraped SERP HTML (#964).
///
/// Returns `true` when one root domain owns at least 60% of the result
/// set and there are at least three results. A real-world top-five page
/// from Google/Bing/DDG mixes domains; a result page dominated by one
/// host is almost always SEO spam or a bot-detection-stuffed substitute.
pub fn is_likely_spam_results(results: &[ScrapedSearchResult]) -> bool {
    if results.len() < 3 {
        return false;
    }
    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for r in results {
        if let Some(host) = root_domain(&r.url) {
            *counts.entry(host).or_insert(0) += 1;
        }
    }
    let Some(&max) = counts.values().max() else {
        return false;
    };
    // 60% threshold: 3-of-5, 4-of-6, 5-of-8 all trip; 2-of-5 doesn't.
    max * 5 >= results.len() * 3
}

/// Extract the registrable root domain (eTLD+1 approximation) from a URL
/// so spam detection groups `astralia.forumgratuit.org` with
/// `russia.forumgratuit.org`. Returns lowercase host minus the leftmost
/// label, or the bare host when there are only two labels.
pub fn root_domain(url: &str) -> Option<String> {
    let after_scheme = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = after_scheme.split(['/', '?', '#']).next()?;
    let host = host.split('@').next_back()?;
    let host = host.split(':').next()?.to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let labels: Vec<&str> = host.split('.').filter(|s| !s.is_empty()).collect();
    if labels.len() <= 2 {
        return Some(host);
    }
    Some(labels[labels.len().saturating_sub(2)..].join("."))
}

fn normalize_duckduckgo_url(href: &str) -> String {
    if let Some(uddg) = extract_query_param(href, "uddg") {
        let decoded = percent_decode(&uddg);
        if !decoded.is_empty() {
            return decoded;
        }
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with('/') {
        return format!("https://duckduckgo.com{href}");
    }
    href.to_string()
}

/// Normalize a Bing SERP result href, unwrapping `/ck/a?...&u=<base64>` redirects.
pub fn normalize_bing_url(href: &str) -> String {
    // Bing wraps every SERP result URL in a `/ck/a?...&u=<base64>` click-tracking
    // redirect, and in the raw HTML the separators are `&amp;` entities. Without
    // decoding entities first, `extract_query_param` looks for `u` but the actual
    // key is `amp;u`, so the real URL is never recovered: every result collapses to
    // a `bing.com` root domain, which the spam heuristic then rejects — yielding
    // zero results for the default Bing backend. Decode entities before parsing.
    let href = decode_html_entities(href);
    let href = href.as_str();
    if let Some(encoded) = extract_query_param(href, "u") {
        let decoded = percent_decode(&encoded);
        let token = decoded.strip_prefix("a1").unwrap_or(&decoded);
        let mut padded = token.replace('-', "+").replace('_', "/");
        while !padded.len().is_multiple_of(4) {
            padded.push('=');
        }
        if let Ok(bytes) = general_purpose::STANDARD.decode(padded)
            && let Ok(url) = String::from_utf8(bytes)
            && (url.starts_with("http://") || url.starts_with("https://"))
        {
            return url;
        }
    }
    if href.starts_with("//") {
        return format!("https:{href}");
    }
    if href.starts_with('/') {
        return format!("https://www.bing.com{href}");
    }
    href.to_string()
}

fn normalize_text(text: &str) -> String {
    let stripped = strip_html_tags(text);
    let decoded = decode_html_entities(&stripped);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_html_tags(text: &str) -> String {
    get_tag_re().replace_all(text, "").to_string()
}

/// Decode common HTML named and numeric character references.
pub fn decode_html_entities(text: &str) -> String {
    static ENTITY_RE: OnceLock<Regex> = OnceLock::new();
    let re = ENTITY_RE.get_or_init(|| {
        Regex::new(r"&(?:#(\d+)|#x([0-9A-Fa-f]+)|([a-zA-Z]+));").expect("HTML entity regex")
    });

    re.replace_all(text, |caps: &regex::Captures| {
        if let Some(dec) = caps.get(1) {
            return dec
                .as_str()
                .parse::<u32>()
                .ok()
                .and_then(std::char::from_u32)
                .unwrap_or('\u{FFFD}')
                .to_string();
        }
        if let Some(hex) = caps.get(2) {
            return u32::from_str_radix(hex.as_str(), 16)
                .ok()
                .and_then(std::char::from_u32)
                .unwrap_or('\u{FFFD}')
                .to_string();
        }
        let named = caps.get(3).map(|m| m.as_str());
        match named {
            Some("amp") => "&",
            Some("lt") => "<",
            Some("gt") => ">",
            Some("quot") => "\"",
            Some("apos") => "'",
            Some("nbsp") => " ",
            Some("copy") => "\u{00A9}",
            Some("reg") => "\u{00AE}",
            Some("mdash") => "\u{2014}",
            Some("ndash") => "\u{2013}",
            Some("lsquo") => "\u{2018}",
            Some("rsquo") => "\u{2019}",
            Some("ldquo") => "\u{201C}",
            Some("rdquo") => "\u{201D}",
            Some("hellip") => "\u{2026}",
            _ => return caps.get(0).map(|m| m.as_str()).unwrap_or("").to_string(),
        }
        .to_string()
    })
    .to_string()
}

/// Percent-decode a URL component. `+` becomes space (query-string convention).
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = &input[i + 1..i + 3];
                if let Ok(val) = u8::from_str_radix(hex, 16) {
                    out.push(val);
                    i += 3;
                    continue;
                }
                out.push(bytes[i]);
            }
            b'+' => out.push(b' '),
            _ => out.push(bytes[i]),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    let query = url.split_once('?')?.1;
    for part in query.split('&') {
        let mut iter = part.splitn(2, '=');
        let name = iter.next().unwrap_or("");
        if name == key {
            return iter.next().map(str::to_string);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> ScrapedSearchResult {
        ScrapedSearchResult {
            title: "x".into(),
            url: url.into(),
            snippet: None,
        }
    }

    // Regression guard: Bing /ck/a redirect hrefs are HTML-entity-encoded
    // (`&amp;`). normalize_bing_url must decode entities before extracting the
    // `u=` base64 payload, otherwise the real URL is never recovered and the
    // result's root domain collapses to bing.com (then dropped as spam → 0
    // results for the default Bing backend).
    #[test]
    fn bing_ckurl_with_html_entities_decodes_real_url() {
        let href = "https://www.bing.com/ck/a?!&amp;&amp;p=abc&amp;u=a1aHR0cHM6Ly9ydXN0LWxhbmcub3JnLw&amp;ntb=1";
        assert_eq!(normalize_bing_url(href), "https://rust-lang.org/");
    }

    #[test]
    fn parses_bing_results_and_decodes_redirect_url() {
        let html = r#"
            <ol>
              <li class="b_algo">
                <h2><a href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9wYXRoP3E9MQ">Example &amp; Result</a></h2>
                <div class="b_caption"><p>A <strong>useful</strong> snippet.</p></div>
              </li>
            </ol>
        "#;

        let results = parse_bing_results(html, 5);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example & Result");
        assert_eq!(results[0].url, "https://example.com/path?q=1");
        assert_eq!(results[0].snippet.as_deref(), Some("A useful snippet."));
    }

    #[test]
    fn parses_duckduckgo_results() {
        let html = r#"
            <a class="result__a" href="https://example.com/rust">Rust &amp; async</a>
            <a class="result__snippet">A <b>useful</b> snippet.</a>
            <a class="result__a" href="//docs.rs/tokio">Tokio</a>
            <div class="result__snippet">Runtime docs</div>
        "#;
        let results = parse_duckduckgo_results(html, 5);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Rust & async");
        assert_eq!(results[0].url, "https://example.com/rust");
        assert_eq!(results[0].snippet.as_deref(), Some("A useful snippet."));
        assert_eq!(results[1].url, "https://docs.rs/tokio");
    }

    #[test]
    fn root_domain_strips_subdomain_keeps_two_labels() {
        assert_eq!(
            root_domain("https://astralia.forumgratuit.org/path/page").as_deref(),
            Some("forumgratuit.org"),
        );
        assert_eq!(
            root_domain("http://www.example.com/").as_deref(),
            Some("example.com"),
        );
        assert_eq!(
            root_domain("https://example.com").as_deref(),
            Some("example.com")
        );
    }

    #[test]
    fn root_domain_handles_port_and_userinfo() {
        assert_eq!(
            root_domain("http://user:pass@blog.example.com:8080/x").as_deref(),
            Some("example.com"),
        );
    }

    #[test]
    fn root_domain_returns_none_for_garbage() {
        assert!(
            root_domain("not-a-url").as_deref().is_some(),
            "bare token is treated as host"
        );
        assert!(root_domain("https:///path").is_none());
    }

    #[test]
    fn spam_detector_flags_single_domain_dominance() {
        // #964 reproduction: 5/5 results from the same low-quality host.
        let r = vec![
            entry("https://astralia.forumgratuit.org/page1"),
            entry("https://russia.forumgratuit.org/page2"),
            entry("https://other.forumgratuit.org/page3"),
            entry("https://hello.forumgratuit.org/page4"),
            entry("https://world.forumgratuit.org/page5"),
        ];
        assert!(is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_passes_diverse_serp() {
        // A normal SERP mixes domains; nothing flagged.
        let r = vec![
            entry("https://example.com/a"),
            entry("https://wikipedia.org/b"),
            entry("https://stackoverflow.com/c"),
            entry("https://reddit.com/d"),
            entry("https://example.com/e"),
        ];
        assert!(!is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_passes_short_result_set() {
        // Two results from the same domain isn't enough signal — false
        // positives on legitimate two-link answers (docs + homepage)
        // would hurt more than letting them through.
        let r = vec![
            entry("https://example.com/a"),
            entry("https://example.com/b"),
        ];
        assert!(!is_likely_spam_results(&r));
    }

    #[test]
    fn spam_detector_threshold_is_sixty_percent() {
        // 3-of-5 same domain trips the 60% threshold.
        let r3of5 = vec![
            entry("https://spam.example.com/a"),
            entry("https://spam.example.com/b"),
            entry("https://spam.example.com/c"),
            entry("https://other.com/d"),
            entry("https://third.com/e"),
        ];
        assert!(is_likely_spam_results(&r3of5));
        // 2-of-5 does NOT trip the threshold.
        let r2of5 = vec![
            entry("https://spam.example.com/a"),
            entry("https://spam.example.com/b"),
            entry("https://other.com/c"),
            entry("https://third.com/d"),
            entry("https://fourth.com/e"),
        ];
        assert!(!is_likely_spam_results(&r2of5));
    }

    #[test]
    fn parse_duckduckgo_filters_spam_domain_dominance() {
        // Shared path used by web_run and web_search: spam SERP → empty.
        let html = r#"
            <a class="result__a" href="https://astralia.forumgratuit.org/a">A</a>
            <a class="result__snippet">s</a>
            <a class="result__a" href="https://russia.forumgratuit.org/b">B</a>
            <a class="result__snippet">s</a>
            <a class="result__a" href="https://other.forumgratuit.org/c">C</a>
            <a class="result__snippet">s</a>
            <a class="result__a" href="https://hello.forumgratuit.org/d">D</a>
            <a class="result__snippet">s</a>
            <a class="result__a" href="https://world.forumgratuit.org/e">E</a>
            <a class="result__snippet">s</a>
        "#;
        assert!(parse_duckduckgo_results(html, 10).is_empty());
    }

    #[test]
    fn parse_bing_filters_spam_domain_dominance() {
        let html = r#"
            <ol>
              <li class="b_algo">
                <h2><a href="https://astralia.forumgratuit.org/a">A</a></h2>
                <div class="b_caption"><p>s</p></div>
              </li>
              <li class="b_algo">
                <h2><a href="https://russia.forumgratuit.org/b">B</a></h2>
                <div class="b_caption"><p>s</p></div>
              </li>
              <li class="b_algo">
                <h2><a href="https://other.forumgratuit.org/c">C</a></h2>
                <div class="b_caption"><p>s</p></div>
              </li>
              <li class="b_algo">
                <h2><a href="https://hello.forumgratuit.org/d">D</a></h2>
                <div class="b_caption"><p>s</p></div>
              </li>
              <li class="b_algo">
                <h2><a href="https://world.forumgratuit.org/e">E</a></h2>
                <div class="b_caption"><p>s</p></div>
              </li>
            </ol>
        "#;
        assert!(parse_bing_results(html, 10).is_empty());
    }

    #[test]
    fn decode_html_entities_handles_named_entities() {
        assert_eq!(decode_html_entities("&amp;"), "&");
        assert_eq!(decode_html_entities("&lt;"), "<");
        assert_eq!(decode_html_entities("&gt;"), ">");
        assert_eq!(decode_html_entities("&quot;"), "\"");
        assert_eq!(decode_html_entities("&apos;"), "'");
        assert_eq!(decode_html_entities("&nbsp;"), " ");
        assert_eq!(decode_html_entities("&copy;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&mdash;"), "\u{2014}");
    }

    #[test]
    fn decode_html_entities_handles_decimal_numeric_references() {
        assert_eq!(decode_html_entities("&#65;"), "A");
        assert_eq!(decode_html_entities("&#60;"), "<");
        assert_eq!(decode_html_entities("&#8211;"), "\u{2013}");
    }

    #[test]
    fn decode_html_entities_handles_hex_numeric_references() {
        assert_eq!(decode_html_entities("&#x41;"), "A");
        assert_eq!(decode_html_entities("&#x3C;"), "<");
        assert_eq!(decode_html_entities("&#x2014;"), "\u{2014}");
    }

    #[test]
    fn decode_html_entities_passthrough_unknown() {
        assert_eq!(decode_html_entities("&unknown;"), "&unknown;");
    }

    #[test]
    fn decode_html_entities_mixed_content() {
        let input = "Hello &amp; welcome to &quot;Rust&apos;s world&quot; &mdash; enjoy!";
        let expected = "Hello & welcome to \"Rust's world\" \u{2014} enjoy!";
        assert_eq!(decode_html_entities(input), expected);
    }

    #[test]
    fn percent_decode_handles_utf8_multibyte_sequences() {
        // Percent-encoded CJK: %E4%B8%AA%E4%BA%BA = 个人 (each glyph is 3 UTF-8 bytes).
        assert_eq!(percent_decode("Hello %E4%B8%AA%E4%BA%BA"), "Hello 个人");
        assert_eq!(percent_decode("%E7%B4%A0%E6%9D%90"), "素材");
        // Percent-encoded UTF-8 inside a URL path (DuckDuckGo `uddg=` redirect shape).
        assert_eq!(
            percent_decode("https://example.com/%E9%A1%B5%E9%9D%A2"),
            "https://example.com/页面"
        );
        // Raw UTF-8 in the input passes through unchanged.
        assert_eq!(percent_decode("查询 keyword"), "查询 keyword");
        // Query-string convention: `+` becomes space; `%20` becomes space.
        assert_eq!(percent_decode("foo+bar%20baz"), "foo bar baz");
    }

    #[test]
    fn is_duckduckgo_challenge_detects_interstitial() {
        assert!(is_duckduckgo_challenge(
            "Unfortunately, bots use DuckDuckGo too."
        ));
        assert!(is_duckduckgo_challenge(
            r#"<div class="anomaly-modal">challenge</div>"#
        ));
        assert!(!is_duckduckgo_challenge(
            r#"<a class="result__a" href="https://example.com">ok</a>"#
        ));
    }
}
