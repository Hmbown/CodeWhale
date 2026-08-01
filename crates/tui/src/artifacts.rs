//! Session-scoped artifact metadata.
//!
//! Large tool outputs are written under the owning session directory and saved
//! sessions keep a durable metadata index for resume/listing flows.

use std::io;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const ARTIFACTS_DIR_NAME: &str = "artifacts";

#[cfg(test)]
static TEST_ARTIFACT_SESSIONS_ROOT: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub(crate) static TEST_ARTIFACT_SESSIONS_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    ToolOutput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactRecord {
    pub id: String,
    pub kind: ArtifactKind,
    #[serde(default)]
    pub session_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub created_at: DateTime<Utc>,
    pub byte_size: u64,
    pub preview: String,
    pub storage_path: PathBuf,
}

fn sanitize_id_component(input: &str) -> String {
    input
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn is_valid_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[must_use]
pub fn artifact_id_for_tool_call(tool_call_id: &str) -> String {
    format!("art_{}", sanitize_id_component(tool_call_id))
}

#[must_use]
pub fn session_artifact_relative_path(artifact_id: &str) -> PathBuf {
    PathBuf::from(ARTIFACTS_DIR_NAME).join(format!("{artifact_id}.txt"))
}

fn session_artifact_relative_path_with_extension(
    artifact_id: &str,
    extension: &str,
) -> io::Result<PathBuf> {
    let artifact_id = sanitize_id_component(artifact_id);
    let extension = extension.trim_start_matches('.').to_ascii_lowercase();
    if artifact_id.is_empty()
        || extension.is_empty()
        || !extension
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "artifact id and extension must contain safe ASCII characters",
        ));
    }
    Ok(PathBuf::from(ARTIFACTS_DIR_NAME).join(format!("{artifact_id}.{extension}")))
}

fn artifact_sessions_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_ARTIFACT_SESSIONS_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
    {
        return Some(root);
    }

    // Honor explicit HOME/USERPROFILE isolation before consulting the host
    // known-folder API. On Windows, `crate::config::effective_home_dir()` can ignore subprocess
    // environment redirection and leak artifacts into the runner profile.
    let home = crate::config::effective_home_dir()?;
    let primary = home.join(".codewhale").join("sessions");
    let legacy = home.join(".deepseek").join("sessions");
    if primary.exists() || !legacy.exists() {
        return Some(primary);
    }
    Some(legacy)
}

#[cfg(test)]
pub(crate) fn set_test_artifact_sessions_root(root: Option<PathBuf>) -> Option<PathBuf> {
    let mut guard = TEST_ARTIFACT_SESSIONS_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    std::mem::replace(&mut *guard, root)
}

#[must_use]
pub fn session_artifact_absolute_path(session_id: &str, relative_path: &Path) -> Option<PathBuf> {
    if !is_valid_session_id(session_id) {
        return None;
    }
    if relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return None;
    }
    Some(
        artifact_sessions_root()?
            .join(session_id)
            .join(relative_path),
    )
}

pub fn write_session_artifact(
    session_id: &str,
    artifact_id: &str,
    content: &str,
) -> io::Result<(PathBuf, PathBuf)> {
    let relative_path = session_artifact_relative_path(artifact_id);
    let absolute_path =
        session_artifact_absolute_path(session_id, &relative_path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve session artifact path (missing home directory)",
            )
        })?;
    if let Some(parent) = absolute_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::utils::write_atomic(&absolute_path, content.as_bytes())?;
    Ok((absolute_path, relative_path))
}

/// Publish immutable session-owned bytes without replacing an earlier handle.
/// A duplicate replay with identical bytes is idempotent; a different payload
/// for the same relative path fails closed.
pub fn write_session_relative_immutable(
    session_id: &str,
    relative_path: &Path,
    content: &[u8],
) -> io::Result<PathBuf> {
    let absolute_path =
        session_artifact_absolute_path(session_id, relative_path).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid session artifact path")
        })?;
    if let Some(parent) = absolute_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if absolute_path.exists() {
        return if std::fs::read(&absolute_path)? == content {
            Ok(absolute_path)
        } else {
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "immutable artifact handle already contains different bytes",
            ))
        };
    }
    let file_name = absolute_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("artifact");
    let temp_path = absolute_path.with_file_name(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let publish = (|| -> io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)?;
        file.write_all(content)?;
        file.sync_all()?;
        match std::fs::hard_link(&temp_path, &absolute_path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                if std::fs::read(&absolute_path)? == content {
                    Ok(())
                } else {
                    Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        "immutable artifact handle raced with different bytes",
                    ))
                }
            }
            Err(err) => Err(err),
        }
    })();
    let _ = std::fs::remove_file(&temp_path);
    publish?;
    Ok(absolute_path)
}

pub fn write_session_artifact_immutable(
    session_id: &str,
    artifact_id: &str,
    content: &[u8],
) -> io::Result<(PathBuf, PathBuf)> {
    let relative_path = session_artifact_relative_path(artifact_id);
    let absolute_path = write_session_relative_immutable(session_id, &relative_path, content)?;
    Ok((absolute_path, relative_path))
}

/// Write arbitrary fetched bytes into a session artifact with a validated
/// extension. Media fetches use this after magic-byte validation.
pub fn write_session_artifact_bytes(
    session_id: &str,
    artifact_id: &str,
    extension: &str,
    content: &[u8],
) -> io::Result<(PathBuf, PathBuf)> {
    let relative_path = session_artifact_relative_path_with_extension(artifact_id, extension)?;
    let absolute_path =
        session_artifact_absolute_path(session_id, &relative_path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "could not resolve session artifact path (missing home directory)",
            )
        })?;
    if let Some(parent) = absolute_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::utils::write_atomic(&absolute_path, content)?;
    Ok((absolute_path, relative_path))
}

fn preview_text(content: &str, max_chars: usize) -> String {
    let mut preview: String = content.chars().take(max_chars).collect();
    if content.chars().count() > max_chars {
        preview.push_str("...");
    }
    preview
}

pub fn record_tool_output_artifact(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    storage_path: impl Into<PathBuf>,
    content: &str,
) -> ArtifactRecord {
    let storage_path = storage_path.into();
    let byte_size = std::fs::metadata(&storage_path)
        .map(|metadata| metadata.len())
        .unwrap_or_else(|_| content.len() as u64);
    record_tool_output_artifact_with_size(
        session_id,
        tool_call_id,
        tool_name,
        storage_path,
        byte_size,
        &preview_text(content, 200),
    )
}

pub fn record_tool_output_artifact_with_size(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    storage_path: impl Into<PathBuf>,
    byte_size: u64,
    preview: &str,
) -> ArtifactRecord {
    ArtifactRecord {
        id: artifact_id_for_tool_call(tool_call_id),
        kind: ArtifactKind::ToolOutput,
        session_id: session_id.to_string(),
        tool_call_id: tool_call_id.to_string(),
        tool_name: tool_name.to_string(),
        created_at: Utc::now(),
        byte_size,
        preview: preview_text(preview, 200),
        storage_path: storage_path.into(),
    }
}

#[must_use]
pub fn format_artifact_relative_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

#[must_use]
pub fn format_byte_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    if bytes >= MIB {
        format!("{} MB", bytes.div_ceil(MIB))
    } else if bytes >= KIB {
        format!("{} KB", bytes.div_ceil(KIB))
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestArtifactSessionsRoot {
        prior: Option<PathBuf>,
    }

    impl Drop for TestArtifactSessionsRoot {
        fn drop(&mut self) {
            set_test_artifact_sessions_root(self.prior.take());
        }
    }

    fn set_test_sessions_root(root: PathBuf) -> TestArtifactSessionsRoot {
        TestArtifactSessionsRoot {
            prior: set_test_artifact_sessions_root(Some(root)),
        }
    }

    #[test]
    fn session_artifact_absolute_path_uses_test_sessions_root() {
        let _guard = TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let _root = set_test_sessions_root(tmp.path().join("sessions"));

        let path = session_artifact_absolute_path(
            "session-123",
            &PathBuf::from("artifacts").join("art_call-big.txt"),
        )
        .expect("path");

        assert_eq!(
            path,
            tmp.path()
                .join("sessions")
                .join("session-123")
                .join("artifacts")
                .join("art_call-big.txt")
        );
    }

    #[test]
    fn binary_session_artifact_uses_validated_extension_and_exact_bytes() {
        let _guard = TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let _root = set_test_sessions_root(tmp.path().join("sessions"));
        let bytes = b"\x89PNG\r\n\x1a\nfixture";

        let (absolute, relative) =
            write_session_artifact_bytes("session-123", "web/media", ".PNG", bytes)
                .expect("write binary artifact");

        assert_eq!(relative, PathBuf::from("artifacts/web_media.png"));
        assert_eq!(std::fs::read(absolute).unwrap(), bytes);
        assert!(write_session_artifact_bytes("session-123", "bad", "../png", bytes).is_err());
    }

    #[test]
    fn adaptive_evidence_publication_is_immutable_and_replay_safe() {
        let _guard = TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let _root = set_test_sessions_root(tmp.path().join("sessions"));
        let relative = PathBuf::from("artifacts/art_call.txt");
        let bytes = b"first exact payload";

        let first = write_session_relative_immutable("session-a", &relative, bytes).unwrap();
        let replay = write_session_relative_immutable("session-a", &relative, bytes).unwrap();
        assert_eq!(first, replay);
        let err = write_session_relative_immutable("session-a", &relative, b"different")
            .expect_err("handle aliasing must fail");
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(first).unwrap(), bytes);
    }

    #[test]
    fn adaptive_evidence_failed_publication_creates_no_handle() {
        let _guard = TEST_ARTIFACT_SESSIONS_GUARD
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let _root = set_test_sessions_root(sessions.clone());
        std::fs::create_dir_all(sessions.join("session-a")).unwrap();
        std::fs::write(sessions.join("session-a/artifacts"), b"block directory").unwrap();
        let relative = PathBuf::from("artifacts/art_failed.txt");

        assert!(write_session_relative_immutable("session-a", &relative, b"payload").is_err());
        assert!(!sessions.join("session-a/artifacts/art_failed.txt").exists());
    }
}
