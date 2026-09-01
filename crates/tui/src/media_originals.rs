//! Content-addressed store for pre-compression image originals.
//!
//! When `read_media` downsamples or lossily re-encodes an image to fit the
//! delivery budget, the model only ever sees the degraded copy. Persisting
//! the pre-compression bytes — named by content hash — lets a later
//! `read_media` call with a crop region read the full-resolution source back
//! (the tool's delivery note points at the stored path), even if the
//! workspace file has moved or changed since the first read.
//!
//! Design notes:
//! - Content-addressed (sha256): repeated reads of the same image reuse one
//!   file and repeated writes are idempotent.
//! - Best effort: any filesystem failure returns `None`; media delivery never
//!   blocks on persistence.
//! - Size-capped: after each write the store is swept oldest-first (mtime)
//!   until it fits [`MAX_TOTAL_BYTES`], so long sessions cannot fill the disk.
//! - Placement: the production engine wires the store dir through
//!   `RuntimeToolServices::media_originals_dir`
//!   ([`default_store_dir`]); test and one-off contexts leave it `None`, which
//!   disables persistence so unit tests never touch the real state dir.

use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// Total size ceiling for the store (1 GiB). Each admitted source is already
/// capped at `read_media`'s 20 MiB source limit, so the store holds dozens of
/// full-resolution originals before the sweep engages.
pub const MAX_TOTAL_BYTES: u64 = 1024 * 1024 * 1024;

/// Store subdirectory under the CodeWhale home directory.
pub const MEDIA_ORIGINALS_SUBDIR: &str = "media-originals";

/// The production store location: `<codewhale home>/media-originals`.
///
/// The directory is created lazily on first persist, not here, so resolving
/// the path never has a filesystem side effect.
#[must_use]
pub fn default_store_dir() -> Option<PathBuf> {
    codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join(MEDIA_ORIGINALS_SUBDIR))
}

fn extension_for_mime(mime_type: &str) -> &'static str {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/tiff" => "tif",
        _ => "img",
    }
}

/// Persist `bytes` under `dir`, returning the content-addressed path.
///
/// Best effort: returns `None` on any filesystem failure. A same-named entry
/// with the same length is reused as-is (content addressing makes a length
/// match a content match for practical purposes).
pub fn persist_original_image(bytes: &[u8], mime_type: &str, dir: &Path) -> Option<PathBuf> {
    if bytes.is_empty() {
        return None;
    }
    let digest = Sha256::digest(bytes);
    let mut hash = String::with_capacity(32);
    for byte in digest.iter().take(16) {
        use std::fmt::Write as _;
        let _ = write!(hash, "{byte:02x}");
    }
    let path = dir.join(format!("{hash}.{}", extension_for_mime(mime_type)));
    std::fs::create_dir_all(dir).ok()?;
    let write_needed = match std::fs::metadata(&path) {
        Ok(meta) => meta.len() != bytes.len() as u64,
        Err(_) => true,
    };
    if write_needed {
        std::fs::write(&path, bytes).ok()?;
    }
    sweep_store(dir, MAX_TOTAL_BYTES);
    std::fs::metadata(&path).ok()?;
    Some(path)
}

/// Resolve `raw` as a read-back path inside the store.
///
/// Returns the canonical path only when the file exists and lives under
/// `store_dir`. `read_media` uses this as a fallback after the workspace
/// resolver rejects a path: the store only contains image bytes that already
/// passed the tool's read guards, so read-back widens nothing.
#[must_use]
pub fn resolve_stored_original(raw: &str, workspace: &Path, store_dir: &Path) -> Option<PathBuf> {
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        workspace.join(raw)
    };
    let candidate = candidate.canonicalize().ok()?;
    let store_dir = store_dir.canonicalize().ok()?;
    candidate.starts_with(store_dir).then_some(candidate)
}

/// Oldest-first (mtime) sweep until the store fits `max_total_bytes`.
/// Best effort: individual stat/unlink failures are skipped.
fn sweep_store(dir: &Path, max_total_bytes: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }
        files.push((
            entry.path(),
            meta.len(),
            meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
        ));
    }
    let mut total: u64 = files.iter().map(|(_, len, _)| *len).sum();
    if total <= max_total_bytes {
        return;
    }
    files.sort_by_key(|(_, _, mtime)| *mtime);
    for (path, len, _) in files {
        if total <= max_total_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_is_content_addressed_and_idempotent() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("media-originals");
        let bytes = b"\x89PNG fake-but-deterministic payload";

        let first = persist_original_image(bytes, "image/png", &store).expect("persist");
        assert!(first.exists());
        assert_eq!(first.parent(), Some(store.as_path()));
        assert_eq!(
            first.extension().and_then(|e| e.to_str()),
            Some("png"),
            "mime drives the extension"
        );
        let stem = first.file_stem().unwrap().to_str().unwrap().to_string();
        assert_eq!(stem.len(), 32, "sha256 prefix names the file: {stem}");

        let second = persist_original_image(bytes, "image/png", &store).expect("re-persist");
        assert_eq!(first, second, "same bytes reuse one file");
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 1);
    }

    #[test]
    fn sweep_removes_oldest_until_under_cap() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().to_path_buf();
        let mut paths = Vec::new();
        for i in 0..4u8 {
            let path = store.join(format!("img{i}.png"));
            std::fs::write(&path, vec![i; 100]).expect("write");
            paths.push(path);
        }
        // 400 bytes total; cap at 250 -> at least the two oldest go.
        sweep_store(&store, 250);
        let remaining = std::fs::read_dir(&store).unwrap().count();
        assert!(
            remaining <= 2,
            "sweep must evict oldest-first until under cap, {remaining} left"
        );
        let total: u64 = std::fs::read_dir(&store)
            .unwrap()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        assert!(total <= 250);
    }

    #[test]
    fn resolve_stored_original_admits_store_paths_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = dir.path().join("media-originals");
        std::fs::create_dir_all(&store).unwrap();
        let inside = store.join("abc123.png");
        std::fs::write(&inside, b"png").unwrap();
        let outside = dir.path().join("other.png");
        std::fs::write(&outside, b"png").unwrap();

        let resolved =
            resolve_stored_original(inside.to_str().unwrap(), dir.path(), &store).expect("inside");
        assert!(resolved.ends_with("abc123.png"));
        assert!(
            resolve_stored_original(outside.to_str().unwrap(), dir.path(), &store).is_none(),
            "paths outside the store must not resolve through the fallback"
        );
        assert!(
            resolve_stored_original("missing.png", dir.path(), &store).is_none(),
            "non-existent files do not resolve"
        );
    }
}
