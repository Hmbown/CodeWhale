//! Image download utility shared by `image_analyze` and `/attach-url`.
//!
//! Downloads an image from a URL, validates the Content-Type, and saves
//! it to a local cache. SSRF protection reuses the same restricted-IP
//! policy as `fetch_url`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::spec::{ToolContext, ToolError};

const DEFAULT_MAX_BYTES: u64 = 20 * 1024 * 1024; // 20 MB
const DEFAULT_TIMEOUT_SECS: u64 = 30;
const MAX_REDIRECTS: usize = 3;
const USER_AGENT: &str =
    "Mozilla/5.0 (compatible; deepseek-tui/0.8; +https://github.com/Hmbown/DeepSeek-TUI)";

/// Supported image MIME types that we accept from remote URLs.
const ALLOWED_CONTENT_TYPES: &[&str] = &[
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/tiff",
];

/// Result of downloading an image from a URL.
#[derive(Debug, Clone)]
pub struct DownloadedImage {
    /// Local filesystem path where the image was saved.
    pub path: PathBuf,
    /// MIME type as reported by the server (e.g. "image/png").
    pub content_type: String,
    /// Size in bytes.
    pub size_bytes: u64,
    /// SHA-256 hex digest of the downloaded bytes.
    pub sha256: String,
}

/// Download an image from `url` and save it to `cache_dir`.
///
/// The file is named `<sha256>.<ext>` where `<ext>` is derived from the
/// Content-Type. If the file already exists (cache hit), the download is
/// skipped and the existing path is returned.
///
/// SSRF protection restricts the resolved IP address using the same policy
/// as `fetch_url` (no loopback, private, link-local, or cloud-metadata IPs).
/// Network policy is also checked via the context.
pub async fn download_image(
    url: &str,
    cache_dir: &Path,
    context: &ToolContext,
) -> Result<DownloadedImage, ToolError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| ToolError::invalid_input(format!("invalid URL: {e}")))?;

    if parsed.scheme() != "http" && parsed.scheme() != "https" {
        return Err(ToolError::invalid_input(
            "only http:// and https:// image URLs are supported",
        ));
    }

    let host = parsed
        .host_str()
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| ToolError::invalid_input("URL must include a host"))?;

    // Network policy check (same path as fetch_url)
    validate_image_fetch_policy(&host, context)?;

    // SSRF protection: resolve and reject restricted IPs
    validate_image_fetch_target(&parsed).await?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| ToolError::execution_failed(format!("failed to build HTTP client: {e}")))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| ToolError::execution_failed(format!("download failed: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "server returned HTTP {}",
            status.as_u16()
        )));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|ct| ct.split(';').next().unwrap_or(ct).trim().to_ascii_lowercase())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    if !ALLOWED_CONTENT_TYPES.contains(&content_type.as_str()) {
        // Fallback: try to guess from URL extension
        let from_ext = mime_from_url_path(&parsed);
        if let Some(ext_mime) = from_ext {
            if !ALLOWED_CONTENT_TYPES.contains(&ext_mime.as_str()) {
                return Err(ToolError::invalid_input(format!(
                    "unsupported image type: {content_type}. Supported: PNG, JPEG, GIF, WebP, BMP, TIFF"
                )));
            }
        } else {
            return Err(ToolError::invalid_input(format!(
                "unsupported image type: {content_type}. Supported: PNG, JPEG, GIF, WebP, BMP, TIFF"
            )));
        }
    }

    // Use extension from Content-Type (or URL fallback)
    let effective_mime = if ALLOWED_CONTENT_TYPES.contains(&content_type.as_str()) {
        content_type.as_str()
    } else {
        mime_from_url_path(&parsed).as_deref().unwrap_or(&content_type)
    };

    let ext = extension_for_mime(effective_mime);

    let content_length = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    if let Some(len) = content_length {
        if len > DEFAULT_MAX_BYTES {
            return Err(ToolError::execution_failed(format!(
                "image too large: {len} bytes (max {DEFAULT_MAX_BYTES})"
            )));
        }
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| ToolError::execution_failed(format!("failed to read response body: {e}")))?;

    if bytes.len() as u64 > DEFAULT_MAX_BYTES {
        return Err(ToolError::execution_failed(format!(
            "image too large: {} bytes (max {DEFAULT_MAX_BYTES})",
            bytes.len()
        )));
    }

    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let sha256_hex = format!("{digest:x}");

    let filename = format!("{sha256_hex}.{ext}");
    let file_path = cache_dir.join(&filename);

    // Cache hit: file already exists
    if file_path.exists() {
        let existing_size = tokio::fs::metadata(&file_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        return Ok(DownloadedImage {
            path: file_path,
            content_type: effective_mime.to_string(),
            size_bytes: existing_size,
            sha256: sha256_hex,
        });
    }

    // Ensure cache dir exists
    tokio::fs::create_dir_all(cache_dir)
        .await
        .map_err(|e| ToolError::execution_failed(format!("failed to create cache dir: {e}")))?;

    // Write atomically: write to temp file, then rename
    let tmp_path = cache_dir.join(format!(".tmp-{sha256_hex}"));
    {
        let mut f = std::fs::File::create(&tmp_path)
            .map_err(|e| ToolError::execution_failed(format!("failed to create temp file: {e}")))?;
        f.write_all(&bytes)
            .map_err(|e| ToolError::execution_failed(format!("failed to write image: {e}")))?;
        f.flush()
            .map_err(|e| ToolError::execution_failed(format!("failed to flush image: {e}")))?;
    }
    std::fs::rename(&tmp_path, &file_path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        ToolError::execution_failed(format!("failed to finalize image file: {e}"))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&file_path, std::fs::Permissions::from_mode(0o600));
    }

    Ok(DownloadedImage {
        path: file_path,
        content_type: effective_mime.to_string(),
        size_bytes: bytes.len() as u64,
        sha256: sha256_hex,
    })
}

/// Derive MIME type from the URL path extension.
fn mime_from_url_path(url: &reqwest::Url) -> Option<String> {
    let path = url.path();
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())?;
    match ext.as_str() {
        "png" => Some("image/png".to_string()),
        "jpg" | "jpeg" => Some("image/jpeg".to_string()),
        "gif" => Some("image/gif".to_string()),
        "webp" => Some("image/webp".to_string()),
        "bmp" => Some("image/bmp".to_string()),
        "tif" | "tiff" => Some("image/tiff".to_string()),
        _ => None,
    }
}

fn extension_for_mime(mime: &str) -> &'static str {
    match mime {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tif",
        _ => "png",
    }
}

/// Check network policy for the image host (reuses the same decider path as fetch_url).
fn validate_image_fetch_policy(host: &str, context: &ToolContext) -> Result<(), ToolError> {
    let Some(decider) = context.network_policy.as_ref() else {
        return Ok(());
    };

    match decider.evaluate(host, "fetch_url") {
        crate::network_policy::Decision::Allow => Ok(()),
        crate::network_policy::Decision::Deny => Err(ToolError::permission_denied(format!(
            "network call to '{host}' blocked by network policy"
        ))),
        crate::network_policy::Decision::Prompt => Err(ToolError::permission_denied(format!(
            "network call to '{host}' requires approval; \
             re-run after `/network allow {host}` or set network.default = \"allow\" in config"
        ))),
    }
}

/// Resolve the hostname and reject restricted IPs (SSRF protection).
async fn validate_image_fetch_target(url: &reqwest::Url) -> Result<(), ToolError> {
    let host = url
        .host_str()
        .ok_or_else(|| ToolError::invalid_input("URL must include a host"))?;

    // Fast-path for localhost
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("localhost.localdomain") {
        return Err(ToolError::permission_denied(
            "image URLs pointing to localhost are not allowed",
        ));
    }

    // Normalize bracketed IPv6 literals
    let ip_candidate = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);

    if let Ok(ip) = ip_candidate.parse::<std::net::IpAddr>() {
        if is_restricted_ip(&ip) {
            return Err(ToolError::permission_denied(format!(
                "IP {ip} is a restricted address (private/loopback/link-local)"
            )));
        }
        return Ok(());
    }

    // DNS resolution check
    if let Ok(addrs) = tokio::net::lookup_host((host, 0u16)).await {
        for addr in addrs {
            if is_restricted_ip(&addr.ip()) {
                return Err(ToolError::permission_denied(format!(
                    "resolved IP {} is a restricted address (private/loopback/link-local)",
                    addr.ip()
                )));
            }
        }
    }

    Ok(())
}

/// Same restricted-IP check as fetch_url.rs for consistent SSRF protection.
fn is_restricted_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_unspecified()
                || matches!(v4.octets(), [100, 64..=127, ..])
                || *ip == std::net::IpAddr::V4(std::net::Ipv4Addr::new(169, 254, 169, 254))
                || matches!(v4.octets(), [198, 18..=19, ..])
                || v4.octets()[0] >= 240
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_unspecified()
                || matches!(v6.octets(), [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, ..])
            {
                return true;
            }
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_restricted_ip(&std::net::IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_multicast()
                || matches!(v6.segments(), [0xfc00..=0xfdff, ..])
                || matches!(v6.segments(), [0xfe80..=0xfebf, ..])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_from_url_path_detects_common_extensions() {
        for (url, expected) in [
            ("https://example.com/photo.png", Some("image/png")),
            ("https://example.com/photo.PNG", Some("image/png")),
            ("https://example.com/photo.jpg", Some("image/jpeg")),
            ("https://example.com/photo.jpeg", Some("image/jpeg")),
            ("https://example.com/photo.gif", Some("image/gif")),
            ("https://example.com/photo.webp", Some("image/webp")),
            ("https://example.com/photo.bmp", Some("image/bmp")),
            ("https://example.com/photo.tiff", Some("image/tiff")),
            ("https://example.com/photo", None),
            ("https://example.com/photo.svg", None),
        ] {
            let parsed = reqwest::Url::parse(url).expect("valid URL");
            assert_eq!(
                mime_from_url_path(&parsed).as_deref(),
                expected,
                "mismatch for {url}"
            );
        }
    }

    #[test]
    fn extension_for_mime_covers_all_allowed_types() {
        for mime in ALLOWED_CONTENT_TYPES {
            let ext = extension_for_mime(mime);
            assert!(!ext.is_empty(), "no extension for {mime}");
            assert!(ext.len() <= 4, "suspicious extension for {mime}: {ext}");
        }
    }
}
