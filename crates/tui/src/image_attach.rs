//! Turning a local image file into a wire-ready image content block.
//!
//! CodeWhale's message model has been multimodal for a long time —
//! [`ContentBlock::ImageUrl`] round-trips
//! through session persistence, compaction and all three wire builders. What it
//! never had was a *faucet*: nothing outside `#[cfg(test)]` ever constructed
//! one, so a user who attached a screenshot got the literal text
//! `[Attached image: /path/to/shot.png]` and a model that correctly concluded it
//! could not see the picture.
//!
//! This module is that faucet. It reads a file, proves it is an image the
//! providers actually accept, holds it to a size budget, and emits a
//! `data:` URL.
//!
//! # Two stages, because the two failure kinds differ
//!
//! [`expand_attachment_blocks`] runs when the message is built and decides the
//! *permanent* questions: does this file exist, is it really an image, is it
//! small enough. Those answers cannot change, so they are baked into history.
//!
//! [`strip_images_when_unsupported`] runs per outbound request and decides the
//! one *contingent* question: can the model this request is going to actually
//! see images. Routes change mid-session, so answering that at build time
//! would mean attaching a screenshot under a text-only model and losing it
//! permanently, even after switching to a vision model. History keeps the
//! image; each request is normalized against its own route.
//!
//! # Provider neutrality
//!
//! There is exactly one internal representation — a `data:<media-type>;base64,…`
//! URL on `ContentBlock::ImageUrl` — and each wire builder projects it:
//!
//! | wire format | shape |
//! |---|---|
//! | Chat Completions | `{"type":"image_url","image_url":{"url":"data:…"}}` |
//! | Responses | `{"type":"input_image","image_url":"data:…"}` |
//! | Anthropic Messages | `{"type":"image","source":{"type":"base64","media_type":…,"data":…}}` |
//!
//! The Anthropic split lives in [`parse_data_url`], which
//! `client::anthropic` calls. Anthropic is the reason the accepted-format list
//! below is not simply "whatever the `image` crate can decode": it accepts only
//! PNG/JPEG/GIF/WebP, and so, therefore, do we. Refusing a BMP here with a
//! readable message beats letting one through to a provider-side 400.

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use crate::model_profile::SupportState;
use crate::models::{ContentBlock, ImageUrlContent};

/// Largest source image accepted, in bytes, before base64 expansion.
///
/// Base64 inflates by 4/3, so this admits roughly 6.7 MB of request body per
/// image. The number is Anthropic's documented per-image ceiling; keeping the
/// tightest provider limit as the shared limit is what makes a "CodeWhale
/// accepted it" verdict portable across routes.
pub const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;

/// Why a file could not be attached as an image.
///
/// Every variant renders to a sentence naming the file and the reason. These
/// strings reach both the user (as a command error) and the model (as an
/// in-band notice), so they say what to do next rather than only what failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageAttachError {
    /// The file could not be read at all.
    Unreadable { path: String, reason: String },
    /// The file is zero bytes.
    Empty { path: String },
    /// Over [`MAX_IMAGE_BYTES`].
    TooLarge { path: String, bytes: usize },
    /// Magic bytes identify a format no provider in the set accepts.
    UnsupportedFormat { path: String, detected: String },
    /// Magic bytes match nothing we recognize as an image.
    NotAnImage { path: String },
}

impl std::fmt::Display for ImageAttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { path, reason } => {
                write!(f, "Cannot attach {path}: {reason}")
            }
            Self::Empty { path } => {
                write!(f, "Cannot attach {path}: the file is empty")
            }
            Self::TooLarge { path, bytes } => write!(
                f,
                "Cannot attach {path}: {} exceeds the {} per-image limit. \
                 Downscale or crop it first.",
                human_bytes(*bytes),
                human_bytes(MAX_IMAGE_BYTES),
            ),
            Self::UnsupportedFormat { path, detected } => write!(
                f,
                "Cannot attach {path}: {detected} is not accepted by vision \
                 models. Convert it to PNG, JPEG, GIF or WebP.",
            ),
            Self::NotAnImage { path } => write!(
                f,
                "Cannot attach {path}: the file is not a PNG, JPEG, GIF or \
                 WebP image (its contents do not match any of those formats).",
            ),
        }
    }
}

impl std::error::Error for ImageAttachError {}

fn human_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} bytes")
    }
}

/// An image that is ready to go on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedImage {
    /// e.g. `"image/png"`.
    pub media_type: &'static str,
    /// `data:<media_type>;base64,<payload>`.
    pub data_url: String,
    /// Size of the source file, before base64.
    pub source_bytes: usize,
}

/// Validated image content returned by lowercase `read`.
pub struct PreparedToolImage {
    pub block: Option<codewhale_tools::ToolResultContentBlock>,
    pub note: String,
}

/// Prepare one provider-neutral tool-result image using Codewhale's existing
/// cross-provider format and size policy. Failure is a visible text receipt,
/// not a failed file read.
#[must_use]
pub fn prepare_tool_image_bytes(bytes: &[u8], mime_type: &str) -> PreparedToolImage {
    let mime_type = mime_type.split(';').next().unwrap_or(mime_type).trim();
    let valid = sniff_media_type(bytes) == Some(mime_type) && bytes.len() <= MAX_IMAGE_BYTES;
    if !valid {
        return PreparedToolImage {
            block: None,
            note: format!(
                "Read image file [{mime_type}]\n[Image omitted: unsupported, invalid, or above the 5 MiB inline limit.]"
            ),
        };
    }
    PreparedToolImage {
        block: Some(codewhale_tools::ToolResultContentBlock::Image {
            mime_type: mime_type.to_string(),
            data: STANDARD.encode(bytes),
        }),
        note: format!("Read image file [{mime_type}]"),
    }
}

fn valid_tool_image(mime_type: &str, data: &str) -> bool {
    matches!(
        mime_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp"
    ) && data.len() <= MAX_IMAGE_BYTES.div_ceil(3) * 4
        && STANDARD.decode(data).is_ok()
}

/// Enforce the same one-image limit at the tool execution boundary so plugin
/// or future rich tools cannot create unbounded history.
#[must_use]
pub(crate) fn bound_rich_tool_result(
    mut rich: crate::tools::spec::RichToolResult,
) -> crate::tools::spec::RichToolResult {
    let mut kept = Vec::with_capacity(1);
    let mut omitted = 0usize;
    for block in rich.content_blocks.drain(..) {
        let codewhale_tools::ToolResultContentBlock::Image { mime_type, data } = &block;
        if kept.is_empty() && valid_tool_image(mime_type, data) {
            kept.push(block);
        } else {
            omitted += 1;
        }
    }
    rich.result.content = tool_result_text_with_omission(&rich.result.content, omitted);
    rich.content_blocks = kept;
    rich
}

/// Borrow the first valid inline image and count everything omitted.
#[must_use]
pub(crate) fn provider_tool_result_image_refs(
    blocks: Option<&[serde_json::Value]>,
) -> (Option<(&str, &str)>, usize) {
    let mut image = None;
    let mut omitted = 0usize;
    for block in blocks.unwrap_or_default() {
        let fields = block
            .get("type")
            .and_then(serde_json::Value::as_str)
            .filter(|kind| *kind == "image")
            .and_then(|_| {
                block
                    .get("mime_type")
                    .and_then(serde_json::Value::as_str)
                    .zip(block.get("data").and_then(serde_json::Value::as_str))
            });
        if image.is_none()
            && let Some((mime_type, data)) = fields
            && valid_tool_image(mime_type, data)
        {
            image = Some((mime_type, data));
        } else {
            omitted += 1;
        }
    }
    (image, omitted)
}

#[must_use]
pub(crate) fn tool_result_text_with_omission(content: &str, omitted: usize) -> String {
    if omitted == 0 {
        return content.to_string();
    }
    format!(
        "{content}\n[{omitted} tool-result image block(s) omitted: invalid, unsupported, oversized, or additional.]"
    )
}

/// Copy/export projection that retains metadata but never inline base64.
#[must_use]
pub(crate) fn safe_tool_result_content_blocks(
    blocks: Option<&[serde_json::Value]>,
) -> Option<Vec<serde_json::Value>> {
    blocks.map(|blocks| {
        blocks
            .iter()
            .map(|block| {
                if block.get("type").and_then(serde_json::Value::as_str) == Some("image") {
                    serde_json::json!({
                        "type": "image",
                        "mime_type": block.get("mime_type").and_then(serde_json::Value::as_str).unwrap_or("application/octet-stream"),
                        "omission_code": "inline_or_local_image_payload",
                        "omitted_base64_bytes": block.get("data").and_then(serde_json::Value::as_str).map_or(0, str::len),
                    })
                } else {
                    block.clone()
                }
            })
            .collect()
    })
}

#[must_use]
pub(crate) fn safe_tool_result_message_projection(
    messages: &[crate::models::Message],
) -> Vec<crate::models::Message> {
    let mut projected = messages.to_vec();
    for message in &mut projected {
        for block in &mut message.content {
            if let ContentBlock::ToolResult { content_blocks, .. } = block {
                *content_blocks = safe_tool_result_content_blocks(content_blocks.as_deref());
            }
        }
    }
    projected
}

impl AttachedImage {
    /// The content block this image becomes in a message.
    #[must_use]
    pub fn content_block(&self) -> ContentBlock {
        ContentBlock::ImageUrl {
            image_url: ImageUrlContent {
                url: self.data_url.clone(),
            },
        }
    }
}

/// Identify an image format from its leading bytes.
///
/// Extension sniffing is not enough here: the extension is attacker- and
/// typo-controlled, while what the provider validates is the payload. A
/// `.png` holding JPEG bytes must be declared `image/jpeg` or the request is
/// rejected with a media-type mismatch that reads like a CodeWhale bug.
///
/// Returns `None` for anything that is not one of the four accepted formats;
/// [`detect_rejected_format`] names the near-misses so the error can be
/// specific.
#[must_use]
pub fn sniff_media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// Name a format we can recognize but deliberately refuse.
///
/// These are real images, so `NotAnImage` would be a lie and would send the
/// user looking for a corrupt file. Naming the format points at the fix
/// (convert it) instead.
#[must_use]
pub fn detect_rejected_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"BM") {
        return Some("BMP");
    }
    if bytes.starts_with(b"II\x2a\x00") || bytes.starts_with(b"MM\x00\x2a") {
        return Some("TIFF");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"\0\0\0") && &bytes[4..8] == b"ftyp" {
        return Some("HEIC/AVIF");
    }
    if bytes.starts_with(b"<svg") || bytes.starts_with(b"<?xml") {
        return Some("SVG");
    }
    if bytes.starts_with(b"%PDF") {
        return Some("PDF");
    }
    None
}

/// Validate and encode raw bytes that were read from `path`.
///
/// Split from [`attach_image_from_path`] so the whole policy — order of
/// checks, limits, format verdicts — is testable without touching a
/// filesystem.
pub fn encode_image_bytes(bytes: &[u8], path: &str) -> Result<AttachedImage, ImageAttachError> {
    if bytes.is_empty() {
        return Err(ImageAttachError::Empty {
            path: path.to_string(),
        });
    }
    // Size is checked before format so a huge file is rejected on the cheap
    // fact rather than after we have decided what it is.
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(ImageAttachError::TooLarge {
            path: path.to_string(),
            bytes: bytes.len(),
        });
    }
    let Some(media_type) = sniff_media_type(bytes) else {
        return Err(match detect_rejected_format(bytes) {
            Some(detected) => ImageAttachError::UnsupportedFormat {
                path: path.to_string(),
                detected: detected.to_string(),
            },
            None => ImageAttachError::NotAnImage {
                path: path.to_string(),
            },
        });
    };
    let payload = STANDARD.encode(bytes);
    Ok(AttachedImage {
        media_type,
        data_url: format!("data:{media_type};base64,{payload}"),
        source_bytes: bytes.len(),
    })
}

/// Read, validate and encode an image file.
pub fn attach_image_from_path(path: &Path) -> Result<AttachedImage, ImageAttachError> {
    let display = path.display().to_string();
    // Check the size from metadata first so a multi-gigabyte file is refused
    // without being read into memory.
    if let Ok(meta) = std::fs::metadata(path) {
        let len = meta.len();
        if len > MAX_IMAGE_BYTES as u64 {
            return Err(ImageAttachError::TooLarge {
                path: display,
                bytes: usize::try_from(len).unwrap_or(usize::MAX),
            });
        }
    }
    let bytes = std::fs::read(path).map_err(|error| ImageAttachError::Unreadable {
        path: display.clone(),
        reason: error.to_string(),
    })?;
    encode_image_bytes(&bytes, &display)
}

/// Split a `data:<media-type>;base64,<payload>` URL.
///
/// Anthropic's Messages API models an image as a tagged `source` rather than a
/// URL, so the native route has to take the data URL back apart. Returns
/// `None` for `http(s)` URLs and for anything malformed, which the caller
/// renders as a remote source or a visible degradation respectively.
#[must_use]
pub fn parse_data_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("data:")?;
    let (header, payload) = rest.split_once(',')?;
    let media_type = header.strip_suffix(";base64")?;
    if media_type.is_empty() || payload.is_empty() {
        return None;
    }
    Some((media_type, payload))
}

/// Whether a URL is one a provider can fetch for itself.
#[must_use]
pub fn is_remote_image_url(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

/// The outcome of expanding a user turn's attachment placeholders.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExpandedAttachments {
    /// Image blocks to append to the user message, in placeholder order.
    pub blocks: Vec<ContentBlock>,
    /// One line per attachment that could not be sent. These are shown to the
    /// user and also handed to the model, because a model that is not told an
    /// image was dropped will confidently discuss it from the filename.
    pub notices: Vec<String>,
}

/// Build the image blocks for a user turn from its `[Attached image: …]` lines.
///
/// This is the ingest half of the composer's placeholder design: the buffer
/// holds a path-bearing text line (which survives editing, history and session
/// reload for free), and the bytes are read here, once, as the message is
/// built.
///
/// Only *permanent* failures are decided here — a file that is missing,
/// oversized, or not an image will still be all of those things next turn, so
/// baking the verdict into history costs nothing. Whether the *model* can see
/// images is deliberately not decided here; that is contingent on the active
/// route and is re-decided per request by
/// [`strip_images_when_unsupported`].
///
/// Each image is bracketed by text tags naming its path. Without them a turn
/// carrying three screenshots gives the model three anonymous images in a row
/// and no way to say which is which.
///
/// Never returns an error: a turn with one bad attachment should still be
/// sent, with the failure stated in-band rather than swallowed.
#[must_use]
pub fn expand_attachment_blocks(text: &str) -> ExpandedAttachments {
    let references = crate::tui::file_mention::media_attachment_references(text);
    let mut out = ExpandedAttachments::default();
    for reference in references {
        if reference.kind != "image" {
            // Video and any future kind: left as the text reference it
            // already was. Silently ignoring it here is not a drop — the
            // path is still in the prompt, exactly as before this module
            // existed.
            continue;
        }
        match attach_image_from_path(Path::new(&reference.path)) {
            Ok(image) => {
                out.blocks
                    .push(tag_block(&format!("<image path=\"{}\">", reference.path)));
                out.blocks.push(image.content_block());
                out.blocks.push(tag_block("</image>"));
            }
            Err(error) => out.notices.push(error.to_string()),
        }
    }
    out
}

fn tag_block(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
        cache_control: None,
    }
}

/// Replace every image in a request with text when the route cannot see them.
///
/// Capability is a property of the *route*, not of the attachment, and the
/// route changes freely mid-session. Deciding this when the message is built
/// would burn the answer into history: attach a screenshot while a text-only
/// model is selected, switch to a vision model, and the image would be gone
/// for good. So history always keeps the real image and each outbound request
/// is normalized against the model it is actually going to.
///
/// Only a known `Unsupported` strips. Most routes report `Unknown` because
/// models.dev has no modality data for them, and treating unknown as "no"
/// would make the feature dead on arrival for exactly the self-hosted and
/// custom routes that most need it — so `Unknown` sends the image and lets the
/// provider be the authority.
///
/// The image is replaced in place rather than removed, so the model is told
/// why it is looking at a gap instead of being left to invent one.
pub fn strip_images_when_unsupported(
    messages: &mut [crate::models::Message],
    vision: SupportState,
    model: &str,
) -> usize {
    if vision != SupportState::Unsupported {
        return 0;
    }
    let mut stripped = 0;
    for message in messages.iter_mut() {
        for block in &mut message.content {
            match block {
                ContentBlock::ImageUrl { .. } => {
                    *block = ContentBlock::Text {
                        text: format!(
                            "[image content omitted: the active model ({model}) does \
                             not accept image input. Switch to a vision-capable \
                             model with /model to see it.]"
                        ),
                        cache_control: None,
                    };
                    stripped += 1;
                }
                ContentBlock::ToolResult {
                    content,
                    content_blocks,
                    ..
                } => {
                    let count = content_blocks.as_ref().map_or(0, Vec::len);
                    if count > 0 {
                        *content_blocks = None;
                        *content = format!(
                            "{content}\n[{count} image block(s) omitted: the active model ({model}) does not accept image input.]"
                        );
                        stripped += count;
                    }
                }
                _ => {}
            }
        }
    }
    stripped
}

/// Image extensions worth sniffing. Matches the four formats
/// [`sniff_media_type`] accepts; other raster types are left as path text.
const ATTACHABLE_IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];
const MAX_AUTO_ATTACHED_IMAGES: usize = 8;
const IMAGE_PATH_WINDOW: usize = 12;

fn looks_like_windows_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn looks_like_unc(s: &str) -> bool {
    s.starts_with("\\\\")
}

/// Absolute, home, file-URL, or Windows-rooted. A bare `foo.png` in prose is
/// not an attachment — drag-and-drop and Finder/Explorer paste emit anchors.
fn looks_like_path_anchor(s: &str) -> bool {
    let s = s.trim();
    s.starts_with('/')
        || s.starts_with("~/")
        || s.starts_with("file://")
        || looks_like_windows_drive(s)
        || looks_like_unc(s)
}

fn strip_matching_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn shell_unescape(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('\\') || looks_like_windows_drive(s) || looks_like_unc(s) {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(next) => result.push(next),
                None => result.push(c),
            }
        } else {
            result.push(c);
        }
    }
    std::borrow::Cow::Owned(result)
}

fn percent_decode_to_string(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(value) = u8::from_str_radix(hex, 16)
        {
            out.push(value);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn decode_file_url(s: &str) -> Option<PathBuf> {
    let rest = s.strip_prefix("file://")?;
    let rest = rest
        .strip_prefix("localhost")
        .or_else(|| rest.strip_prefix("LOCALHOST"))
        .unwrap_or(rest);
    let decoded = percent_decode_to_string(rest);
    if decoded.is_empty() || decoded == "/" {
        return None;
    }
    Some(crate::config::expand_path(&decoded))
}

fn token_to_path(token: &str) -> Option<PathBuf> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let unquoted = strip_matching_quotes(token);
    if unquoted.starts_with("file://") {
        return decode_file_url(unquoted);
    }
    if !looks_like_path_anchor(unquoted) {
        return None;
    }
    let unescaped = shell_unescape(unquoted);
    Some(crate::config::expand_path(unescaped.as_ref()))
}

fn has_attachable_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ATTACHABLE_IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

fn path_sniffs_as_image(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut buf = [0u8; 16];
    let Ok(n) = file.read(&mut buf) else {
        return false;
    };
    sniff_media_type(&buf[..n]).is_some()
}

fn is_attachable_image_path(path: &Path) -> bool {
    has_attachable_image_extension(path) && path_sniffs_as_image(path)
}

fn push_unique_image(path: PathBuf, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    if !is_attachable_image_path(&path) {
        return;
    }
    let display = path.to_string_lossy().into_owned();
    if seen.insert(display.clone()) {
        out.push(display);
    }
}

fn markdown_image_paths(text: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    for (bang, _) in text.match_indices("![") {
        let after_alt = bang + 2;
        let Some(rel_close) = text[after_alt..].find("](") else {
            continue;
        };
        let path_start = after_alt + rel_close + 2;
        let Some(rel_end) = text[path_start..].find(')') else {
            continue;
        };
        let raw = text[path_start..path_start + rel_end].trim();
        if let Some(path) = token_to_path(raw) {
            push_unique_image(path, out, seen);
        }
    }
}

fn quoted_and_mention_paths(text: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '@' => {
                if let Some(quote @ ('"' | '\'')) = chars.get(i + 1).copied()
                    && let Some(rel) = chars[i + 2..].iter().position(|&ch| ch == quote)
                {
                    let raw: String = chars[i + 2..i + 2 + rel].iter().collect();
                    if let Some(path) = token_to_path(&raw) {
                        push_unique_image(path, out, seen);
                    }
                    i += 3 + rel;
                    continue;
                }
                let mut end = i + 1;
                while end < chars.len() && !chars[end].is_whitespace() {
                    end += 1;
                }
                let raw: String = chars[i + 1..end].iter().collect();
                if let Some(path) = token_to_path(raw.trim_end_matches(|ch: char| {
                    matches!(ch, ',' | '.' | ';' | ':' | ')' | ']' | '}')
                })) {
                    push_unique_image(path, out, seen);
                }
                i = end;
            }
            quote @ ('"' | '\'') => {
                if let Some(rel) = chars[i + 1..]
                    .iter()
                    .take_while(|&&ch| ch != '\n')
                    .position(|&ch| ch == quote)
                {
                    let raw: String = chars[i + 1..i + 1 + rel].iter().collect();
                    if let Some(path) = token_to_path(&raw) {
                        push_unique_image(path, out, seen);
                    }
                    i += 2 + rel;
                    continue;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
}

fn image_extension_token_windows(text: &str, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens: Vec<(usize, usize)> = Vec::new();
    let mut start = None;
    for (idx, ch) in chars.iter().enumerate() {
        match (ch.is_whitespace(), start) {
            (true, Some(s)) => {
                tokens.push((s, idx));
                start = None;
            }
            (false, None) => start = Some(idx),
            _ => {}
        }
    }
    if let Some(s) = start {
        tokens.push((s, chars.len()));
    }
    for (seed, &(token_start, token_end)) in tokens.iter().enumerate() {
        let token: String = chars[token_start..token_end].iter().collect();
        let trimmed = token.trim_end_matches(|ch: char| {
            matches!(ch, ',' | '.' | ';' | ':' | ')' | ']' | '}' | '!' | '?')
        });
        let ext = Path::new(trimmed)
            .extension()
            .and_then(|ext| ext.to_str())
            .map(str::to_ascii_lowercase);
        if !ext.is_some_and(|ext| ATTACHABLE_IMAGE_EXTENSIONS.contains(&ext.as_str())) {
            continue;
        }
        let max_span = tokens.len().min(IMAGE_PATH_WINDOW);
        let mut found = None;
        for left in 0..=seed.min(max_span) {
            let (ws, we) = (tokens[seed - left].0, token_end);
            let raw: String = chars[ws..we].iter().collect();
            let joined = raw
                .trim_start_matches(|ch: char| {
                    matches!(ch, '(' | '[' | '{' | '<' | '"' | '\'' | '@')
                })
                .trim_end_matches(|ch: char| {
                    matches!(ch, ',' | '.' | ';' | ':' | ')' | ']' | '}' | '!' | '?')
                })
                .to_string();
            let Some(path) = token_to_path(&joined) else {
                continue;
            };
            if is_attachable_image_path(&path) {
                found = Some(path);
                break;
            }
        }
        if let Some(path) = found {
            push_unique_image(path, out, seen);
        }
    }
}

/// Existing image files named in user text, in encounter order.
///
/// Anchored paths only (`/`, `~/`, `file://`, Windows drive/UNC). Bytes must
/// sniff as PNG/JPEG/GIF/WebP. Spaced names are recovered by expanding left
/// from an extension token until a real file is found.
pub(crate) fn discover_image_paths(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    markdown_image_paths(text, &mut out, &mut seen);
    quoted_and_mention_paths(text, &mut out, &mut seen);
    image_extension_token_windows(text, &mut out, &mut seen);
    out
}

/// Append `[Attached image: …]` markers for every existing image path named
/// in `text` that is not already marked. Clipboard `/attach` lines are left
/// alone. Never errors: a path that is not an image stays ordinary text.
#[must_use]
pub fn ensure_image_attachment_markers(text: &str) -> String {
    let already: HashSet<String> = crate::tui::file_mention::media_attachment_references(text)
        .into_iter()
        .filter(|reference| reference.kind == "image")
        .map(|reference| reference.path)
        .collect();
    let mut extra = Vec::new();
    let mut seen = already;
    for path in discover_image_paths(text) {
        if extra.len() >= MAX_AUTO_ATTACHED_IMAGES {
            break;
        }
        if seen.insert(path.clone()) {
            extra.push(path);
        }
    }
    if extra.is_empty() {
        return text.to_string();
    }
    let mut out = text.to_string();
    if !out.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    for path in extra {
        out.push_str("[Attached image: ");
        out.push_str(&path);
        out.push_str("]\n");
    }
    out
}

/// Render dropped-attachment notices as a block the model will read.
///
/// Wrapped in a tag rather than appended as bare prose so the model can tell
/// the difference between the user saying something and the harness reporting
/// on itself.
#[must_use]
pub fn notice_block(notices: &[String]) -> Option<ContentBlock> {
    if notices.is_empty() {
        return None;
    }
    let body = notices.join("\n");
    Some(ContentBlock::Text {
        text: format!(
            "<attachment_notice>\n{body}\nDo not describe these images from \
             memory or from their filenames; ask the user to re-share them.\n\
             </attachment_notice>"
        ),
        cache_control: None,
    })
}

#[cfg(test)]
#[path = "image_attach/tests.rs"]
pub(crate) mod tests;
