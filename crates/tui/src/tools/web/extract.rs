//! Content-type routing and readable-document extraction for web tools.
//!
//! Networking deliberately lives elsewhere. This module accepts already
//! fetched bytes and turns them into one normalized document so `fetch_url`
//! and `web.run` cannot disagree about HTML, Markdown, PDF, or media handling.

use std::sync::OnceLock;

#[cfg(feature = "pdf")]
use std::fmt::Display;

use encoding_rs::Encoding;
use regex::Regex;

use crate::tools::spec::ToolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocumentKind {
    Html,
    Markdown,
    Text,
    Pdf,
    Media,
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractedDocument {
    pub(crate) kind: DocumentKind,
    pub(crate) title: Option<String>,
    pub(crate) text: String,
    pub(crate) markdown: String,
    /// Readability-cleaned HTML. `web.run` consumes this to retain clickable
    /// links while avoiding page chrome and consent-banner noise.
    pub(crate) cleaned_html: Option<String>,
    pub(crate) pdf_pages: Option<Vec<Vec<String>>>,
    /// Validated extension for image/audio/video artifacts.
    pub(crate) media_extension: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MediaSignature {
    extension: &'static str,
    family: MediaFamily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaFamily {
    Image,
    Audio,
    Video,
}

static TITLE_RE: OnceLock<Regex> = OnceLock::new();
static FALLBACK_RE: OnceLock<Vec<Regex>> = OnceLock::new();
static PAGE_CHROME_RE: OnceLock<Regex> = OnceLock::new();
static TAG_RE: OnceLock<Regex> = OnceLock::new();
static WHITESPACE_RE: OnceLock<Regex> = OnceLock::new();
static CHARSET_RE: OnceLock<Regex> = OnceLock::new();
static META_CHARSET_RE: OnceLock<Regex> = OnceLock::new();

pub(crate) fn extract_document(
    url: &str,
    content_type: Option<&str>,
    bytes: &[u8],
) -> Result<ExtractedDocument, ToolError> {
    let declared = normalized_content_type(content_type);
    let declared = declared.as_deref();

    if bytes.is_empty() {
        return Ok(ExtractedDocument {
            kind: DocumentKind::Text,
            title: None,
            text: String::new(),
            markdown: String::new(),
            cleaned_html: None,
            pdf_pages: None,
            media_extension: None,
        });
    }

    if looks_like_pdf(bytes) || declared == Some("application/pdf") || url_is_pdf(url) {
        if looks_like_pdf(bytes) && declared_media_family(declared).is_some() {
            return Err(ToolError::execution_failed(format!(
                "Response media type `{}` did not match its PDF bytes",
                declared.unwrap_or("unknown")
            )));
        }
        if !looks_like_pdf(bytes) {
            return Err(ToolError::execution_failed(
                "Response claimed to be a PDF, but its bytes did not contain a PDF signature",
            ));
        }
        return extract_pdf(bytes);
    }

    if let Some(signature) = sniff_media(bytes) {
        if let Some(declared_family) = declared_media_family(declared)
            && declared_family != signature.family
        {
            return Err(ToolError::execution_failed(format!(
                "Response media type `{}` did not match its bytes",
                declared.unwrap_or("unknown")
            )));
        }
        return Ok(ExtractedDocument {
            kind: DocumentKind::Media,
            title: None,
            text: String::new(),
            markdown: String::new(),
            cleaned_html: None,
            pdf_pages: None,
            media_extension: Some(signature.extension),
        });
    }

    if declared_media_family(declared).is_some() {
        return Err(ToolError::execution_failed(format!(
            "Response claimed media type `{}`, but its bytes did not match a supported media signature",
            declared.unwrap_or("unknown")
        )));
    }

    let body = decode_text(bytes, content_type)?;
    if is_html(declared, url, &body) {
        return extract_html(url, &body);
    }
    if is_markdown(declared, url) {
        return Ok(ExtractedDocument {
            kind: DocumentKind::Markdown,
            title: markdown_title(&body),
            text: body.clone(),
            markdown: body,
            cleaned_html: None,
            pdf_pages: None,
            media_extension: None,
        });
    }
    if is_textual(declared, url) {
        return Ok(ExtractedDocument {
            kind: DocumentKind::Text,
            title: None,
            text: body.clone(),
            markdown: body,
            cleaned_html: None,
            pdf_pages: None,
            media_extension: None,
        });
    }

    Err(ToolError::execution_failed(format!(
        "Unsupported binary response type `{}`; use a dedicated download tool",
        declared.unwrap_or("unknown")
    )))
}

fn extract_html(url: &str, html: &str) -> Result<ExtractedDocument, ToolError> {
    let parsed_url = reqwest::Url::parse(url)
        .map_err(|err| ToolError::invalid_input(format!("invalid URL: {err}")))?;
    let original_title = html_title(html);
    let mut input = html.as_bytes();
    let readable = readability::extractor::extract(&mut input, &parsed_url).ok();

    let readable_html = readable
        .as_ref()
        .map(|product| product.content.trim())
        .filter(|content| meaningful_html(content))
        .map(ToOwned::to_owned);
    let cleaned_html = readable_html
        .or_else(|| fallback_main_html(html))
        .ok_or_else(|| js_required_error(url))?;
    let markdown = htmd::convert(&cleaned_html).map_err(|err| {
        ToolError::execution_failed(format!(
            "Failed to convert readable HTML to Markdown: {err}"
        ))
    })?;
    let text = readable
        .as_ref()
        .map(|product| normalize_text(&product.text))
        .filter(|content| meaningful_text(content))
        .unwrap_or_else(|| html_to_plain_text(&cleaned_html));

    if !meaningful_text(&text) && !meaningful_text(&markdown) {
        return Err(js_required_error(url));
    }

    let title = readable
        .map(|product| normalize_text(&product.title))
        .filter(|value| !value.is_empty())
        .or(original_title);

    Ok(ExtractedDocument {
        kind: DocumentKind::Html,
        title,
        text,
        markdown,
        cleaned_html: Some(cleaned_html),
        pdf_pages: None,
        media_extension: None,
    })
}

fn fallback_main_html(html: &str) -> Option<String> {
    let page_chrome = PAGE_CHROME_RE.get_or_init(|| {
        Regex::new(concat!(
            r"(?is)(?:<script(?:\s[^>]*)?>.*?</script\s*>",
            r"|<style(?:\s[^>]*)?>.*?</style\s*>",
            r"|<noscript(?:\s[^>]*)?>.*?</noscript\s*>",
            r"|<nav(?:\s[^>]*)?>.*?</nav\s*>",
            r"|<header(?:\s[^>]*)?>.*?</header\s*>",
            r"|<footer(?:\s[^>]*)?>.*?</footer\s*>",
            r"|<aside(?:\s[^>]*)?>.*?</aside\s*>",
            r"|<form(?:\s[^>]*)?>.*?</form\s*>)",
        ))
        .expect("page chrome regex")
    });
    for re in FALLBACK_RE.get_or_init(|| {
        ["article", "main", "body"]
            .into_iter()
            .map(|tag| {
                Regex::new(&format!(r"(?is)<{tag}(?:\s[^>]*)?>(.*?)</{tag}\s*>"))
                    .expect("fallback element regex")
            })
            .collect()
    }) {
        let Some(capture) = re.captures(html) else {
            continue;
        };
        let Some(content) = capture.get(1) else {
            continue;
        };
        let without_chrome = page_chrome.replace_all(content.as_str(), "");
        if meaningful_html(&without_chrome) {
            return Some(without_chrome.into_owned());
        }
    }
    None
}

fn meaningful_html(html: &str) -> bool {
    meaningful_text(&html_to_plain_text(html))
}

fn meaningful_text(text: &str) -> bool {
    text.chars().filter(|ch| !ch.is_whitespace()).count() >= 32
        && text.split_whitespace().count() >= 5
}

fn html_to_plain_text(html: &str) -> String {
    let without_tags = TAG_RE
        .get_or_init(|| Regex::new(r"(?s)<[^>]+>").expect("tag regex"))
        .replace_all(html, " ");
    normalize_text(&decode_common_entities(&without_tags))
}

fn normalize_text(text: &str) -> String {
    WHITESPACE_RE
        .get_or_init(|| Regex::new(r"\s+").expect("whitespace regex"))
        .replace_all(text.trim(), " ")
        .into_owned()
}

fn decode_common_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn html_title(html: &str) -> Option<String> {
    let capture = TITLE_RE
        .get_or_init(|| {
            Regex::new(r"(?is)<title(?:\s[^>]*)?>(.*?)</title\s*>").expect("title regex")
        })
        .captures(html)?;
    let title = normalize_text(&decode_common_entities(capture.get(1)?.as_str()));
    (!title.is_empty()).then_some(title)
}

fn markdown_title(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let title = line.trim().strip_prefix("# ")?.trim();
        (!title.is_empty()).then(|| title.to_string())
    })
}

fn js_required_error(url: &str) -> ToolError {
    ToolError::execution_failed(format!(
        "No readable page content was found at {url}; the page may require JavaScript. Recovery: use browser automation for this URL."
    ))
}

fn decode_text(bytes: &[u8], content_type: Option<&str>) -> Result<String, ToolError> {
    if bytes.iter().take(8_192).any(|byte| *byte == 0) {
        return Err(ToolError::execution_failed(
            "Unsupported binary response contained NUL bytes",
        ));
    }
    Ok(decode_response_body(bytes, content_type))
}

pub(crate) fn decode_response_body(bytes: &[u8], content_type: Option<&str>) -> String {
    let declared_encoding = content_type
        .and_then(charset_label)
        .or_else(|| html_meta_charset_label(bytes));
    if let Some(encoding) =
        declared_encoding.and_then(|label| Encoding::for_label(label.as_bytes()))
    {
        let (decoded, _, _) = encoding.decode(bytes);
        return decoded.into_owned();
    }

    String::from_utf8_lossy(bytes).into_owned()
}

fn charset_label(value: &str) -> Option<String> {
    CHARSET_RE
        .get_or_init(|| {
            Regex::new(r#"(?i)\bcharset\s*=\s*["']?\s*([a-z0-9._-]+)"#).expect("charset regex")
        })
        .captures(value)
        .and_then(|captures| captures.get(1))
        .map(|label| label.as_str().to_ascii_lowercase())
}

fn html_meta_charset_label(bytes: &[u8]) -> Option<String> {
    let sniff_len = bytes.len().min(8 * 1024);
    let prefix = String::from_utf8_lossy(&bytes[..sniff_len]);
    META_CHARSET_RE
        .get_or_init(|| {
            Regex::new(r#"(?is)<meta\b[^>]*\bcharset\s*=\s*["']?\s*([a-z0-9._-]+)"#)
                .expect("meta charset regex")
        })
        .captures(&prefix)
        .and_then(|captures| captures.get(1))
        .map(|label| label.as_str().to_ascii_lowercase())
}

fn normalized_content_type(content_type: Option<&str>) -> Option<String> {
    content_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn is_html(content_type: Option<&str>, url: &str, body: &str) -> bool {
    matches!(content_type, Some("text/html" | "application/xhtml+xml"))
        || url_path_ends_with(url, &[".html", ".htm"])
        || {
            let prefix = body.trim_start().chars().take(64).collect::<String>();
            let prefix = prefix.to_ascii_lowercase();
            prefix.contains("<!doctype html") || prefix.contains("<html")
        }
}

fn is_markdown(content_type: Option<&str>, url: &str) -> bool {
    matches!(
        content_type,
        Some("text/markdown" | "text/x-markdown" | "application/markdown")
    ) || url_path_ends_with(url, &[".md", ".markdown"])
}

fn is_textual(content_type: Option<&str>, url: &str) -> bool {
    content_type.is_some_and(|value| {
        value.starts_with("text/")
            || value.contains("json")
            || value.contains("xml")
            || value.contains("yaml")
            || value.contains("javascript")
            || value == "application/sql"
    }) || url_path_ends_with(
        url,
        &[
            ".txt", ".json", ".jsonl", ".xml", ".yaml", ".yml", ".csv", ".tsv", ".rs", ".py",
            ".js", ".ts", ".toml",
        ],
    )
}

fn url_is_pdf(url: &str) -> bool {
    url_path_ends_with(url, &[".pdf"])
}

fn url_path_ends_with(url: &str, extensions: &[&str]) -> bool {
    reqwest::Url::parse(url)
        .ok()
        .map(|parsed| parsed.path().to_ascii_lowercase())
        .is_some_and(|path| extensions.iter().any(|extension| path.ends_with(extension)))
}

fn looks_like_pdf(bytes: &[u8]) -> bool {
    bytes.starts_with(b"%PDF-")
}

fn declared_media_family(content_type: Option<&str>) -> Option<MediaFamily> {
    let content_type = content_type?;
    if content_type.starts_with("image/") {
        Some(MediaFamily::Image)
    } else if content_type.starts_with("audio/") {
        Some(MediaFamily::Audio)
    } else if content_type.starts_with("video/") {
        Some(MediaFamily::Video)
    } else {
        None
    }
}

fn sniff_media(bytes: &[u8]) -> Option<MediaSignature> {
    let trimmed = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .map(|start| &bytes[start..])
        .unwrap_or(bytes);
    let signature = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        MediaSignature {
            extension: "png",
            family: MediaFamily::Image,
        }
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        MediaSignature {
            extension: "jpg",
            family: MediaFamily::Image,
        }
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        MediaSignature {
            extension: "gif",
            family: MediaFamily::Image,
        }
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        MediaSignature {
            extension: "webp",
            family: MediaFamily::Image,
        }
    } else if bytes.starts_with(b"ID3") || bytes.starts_with(b"\xff\xfb") {
        MediaSignature {
            extension: "mp3",
            family: MediaFamily::Audio,
        }
    } else if bytes.starts_with(b"fLaC") {
        MediaSignature {
            extension: "flac",
            family: MediaFamily::Audio,
        }
    } else if bytes.starts_with(b"OggS") {
        MediaSignature {
            extension: "ogg",
            family: MediaFamily::Audio,
        }
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        MediaSignature {
            extension: "wav",
            family: MediaFamily::Audio,
        }
    } else if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        MediaSignature {
            extension: "mp4",
            family: MediaFamily::Video,
        }
    } else if bytes.starts_with(b"\x1aE\xdf\xa3") {
        MediaSignature {
            extension: "webm",
            family: MediaFamily::Video,
        }
    } else if trimmed.starts_with(b"<svg")
        || (trimmed.starts_with(b"<?xml")
            && trimmed
                .windows(4)
                .take(1_024)
                .any(|window| window.eq_ignore_ascii_case(b"<svg")))
    {
        MediaSignature {
            extension: "svg",
            family: MediaFamily::Image,
        }
    } else {
        return None;
    };
    Some(signature)
}

#[cfg(feature = "pdf")]
fn extract_pdf(bytes: &[u8]) -> Result<ExtractedDocument, ToolError> {
    let text = guard_pdf_extract(|| pdf_extract::extract_text_from_mem(bytes))
        .map_err(|err| ToolError::execution_failed(format!("PDF extract failed: {err}")))?;
    let pages = split_pdf_pages(&text);
    let text = pages
        .iter()
        .map(|page| page.join("\n"))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(ExtractedDocument {
        kind: DocumentKind::Pdf,
        title: Some("PDF Document".to_string()),
        markdown: text.clone(),
        text,
        cleaned_html: None,
        pdf_pages: Some(pages),
        media_extension: None,
    })
}

#[cfg(not(feature = "pdf"))]
fn extract_pdf(_bytes: &[u8]) -> Result<ExtractedDocument, ToolError> {
    Err(ToolError::execution_failed(
        "PDF extraction is unavailable in this build",
    ))
}

#[cfg(feature = "pdf")]
fn guard_pdf_extract<T, E, F>(extract: F) -> Result<T, String>
where
    E: Display,
    F: FnOnce() -> Result<T, E>,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(extract)) {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(format!(
            "extractor panicked: {}",
            panic_payload_message(payload.as_ref())
        )),
    }
}

#[cfg(feature = "pdf")]
fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(feature = "pdf")]
fn split_pdf_pages(text: &str) -> Vec<Vec<String>> {
    text.split('\x0C')
        .map(|page| {
            page.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_becomes_readable_markdown_without_page_chrome() {
        let html = br#"<!doctype html><html><head><title>Whale &amp; Signal</title></head><body>
            <nav>Products Pricing Log in Cookies</nav>
            <article><h1>Fetch once</h1><p>This is the important article body with enough words to be useful.</p>
            <a href="/proof">Read the proof</a></article>
            <footer>Privacy Cookies Terms</footer></body></html>"#;
        let document = extract_document("https://example.com/post", Some("text/html"), html)
            .expect("extract html");

        assert_eq!(document.kind, DocumentKind::Html);
        assert_eq!(document.title.as_deref(), Some("Whale & Signal"));
        assert!(document.markdown.contains("Fetch once") || document.title.is_some());
        assert!(
            document
                .markdown
                .contains("[Read the proof](https://example.com/proof)")
        );
        assert!(!document.markdown.contains("Products Pricing"));
        assert!(!document.markdown.contains("Privacy Cookies"));
    }

    #[test]
    fn sparse_document_uses_article_fallback() {
        let html = br#"<html><head><title>Fallback</title></head><body><nav>cookie banner</nav>
            <article><h2>Small source</h2><p>Five useful words survive this compact article fallback path.</p></article>
            </body></html>"#;
        let document = extract_document("https://example.com/short", Some("text/html"), html)
            .expect("extract fallback");

        assert!(document.markdown.contains("## Small source"));
        assert!(!document.markdown.contains("cookie banner"));
    }

    #[test]
    fn javascript_shell_returns_actionable_error() {
        let error = extract_document(
            "https://example.com/app",
            Some("text/html"),
            b"<html><body><div id='root'></div><script>boot()</script></body></html>",
        )
        .expect_err("empty app shell must fail");

        let message = error.to_string();
        assert!(message.contains("may require JavaScript"));
        assert!(message.contains("browser automation"));
    }

    #[test]
    fn markdown_passes_through_unchanged() {
        let body = b"# Release note\n\nA complete markdown response remains intact.\n";
        let document = extract_document(
            "https://example.com/release.md",
            Some("text/markdown; charset=utf-8"),
            body,
        )
        .expect("extract markdown");

        assert_eq!(document.kind, DocumentKind::Markdown);
        assert_eq!(document.markdown.as_bytes(), body);
        assert_eq!(document.title.as_deref(), Some("Release note"));
    }

    #[test]
    fn media_requires_matching_magic_bytes() {
        let error = extract_document(
            "https://example.com/not-image.png",
            Some("image/png"),
            b"<html>not really an image</html>",
        )
        .expect_err("spoofed media must fail");
        assert!(error.to_string().contains("did not match"));

        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(b"fake test payload");
        let document = extract_document(
            "https://example.com/image",
            Some("application/octet-stream"),
            &png,
        )
        .expect("sniff png");
        assert_eq!(document.kind, DocumentKind::Media);
        assert_eq!(document.media_extension, Some("png"));
    }

    #[test]
    fn arbitrary_binary_is_rejected() {
        let error = extract_document(
            "https://example.com/archive.bin",
            Some("application/octet-stream"),
            b"PK\x03\x04archive bytes",
        )
        .expect_err("archive must be rejected");
        assert!(error.to_string().contains("Unsupported binary response"));
    }

    #[test]
    fn empty_success_body_is_valid_text() {
        let document = extract_document(
            "https://example.com/no-content",
            Some("application/octet-stream"),
            b"",
        )
        .expect("empty body");
        assert_eq!(document.kind, DocumentKind::Text);
        assert!(document.text.is_empty());
    }

    #[test]
    fn content_type_matching_is_case_insensitive() {
        let document = extract_document(
            "https://example.com/document",
            Some("Application/JSON; Charset=UTF-8"),
            br#"{"status":"ok"}"#,
        )
        .expect("mixed-case JSON content type");

        assert_eq!(document.kind, DocumentKind::Text);
        assert_eq!(document.text, r#"{"status":"ok"}"#);
    }

    #[test]
    fn declared_and_sniffed_non_utf8_charsets_decode_before_extraction() {
        let html = r#"<html><head><title>央广网要闻</title><meta charset="gb2312"></head>
            <body><article><h1>央广网要闻</h1>
            <p>这是 一段 包含 足够 中文 词语 的 新闻 内容 用于 验证 网页 解码。</p>
            </article></body></html>"#;
        let (gbk_bytes, _, _) = encoding_rs::GBK.encode(html);

        let from_http_header = decode_response_body(&gbk_bytes, Some("text/html; charset=gb2312"));
        assert!(from_http_header.contains("央广网要闻"));
        assert!(!from_http_header.contains('\u{fffd}'));

        let from_html_meta = decode_response_body(&gbk_bytes, Some("text/html"));
        assert!(from_html_meta.contains("央广网要闻"));
        assert!(!from_html_meta.contains('\u{fffd}'));

        let (gbk_plain_bytes, _, _) = encoding_rs::GBK.encode("要闻_央广网");
        let plain_text = extract_document(
            "https://example.com/news.txt",
            Some("text/plain; charset=gbk"),
            &gbk_plain_bytes,
        )
        .expect("extract declared non-utf8 text");
        assert!(plain_text.text.contains("要闻_央广网"));

        let utf8 = decode_response_body("保持 UTF-8".as_bytes(), Some("text/plain"));
        assert_eq!(utf8, "保持 UTF-8");
    }

    #[test]
    fn svg_requires_and_accepts_svg_markup_signature() {
        let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg"></svg>"#;
        let document = extract_document("https://example.com/diagram", Some("image/svg+xml"), svg)
            .expect("sniff svg");
        assert_eq!(document.kind, DocumentKind::Media);
        assert_eq!(document.media_extension, Some("svg"));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_panic_is_contained() {
        let error = guard_pdf_extract::<(), &str, _>(|| panic!("malformed pdf"))
            .expect_err("panic must be captured");
        assert!(error.contains("extractor panicked: malformed pdf"));
    }
}
