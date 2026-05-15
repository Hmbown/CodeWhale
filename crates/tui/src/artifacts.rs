//! Session-scoped artifact metadata.
//!
//! Large tool outputs are written under the owning session directory and saved
//! sessions keep a durable metadata index for resume/listing flows.

use std::io;
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

fn artifact_sessions_root() -> Option<PathBuf> {
    #[cfg(test)]
    if let Some(root) = TEST_ARTIFACT_SESSIONS_ROOT
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .clone()
    {
        return Some(root);
    }

    Some(dirs::home_dir()?.join(".deepseek").join("sessions"))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptArtifactRef {
    pub artifact_id: String,
    pub tool_name: String,
    pub tool_call_id: String,
    pub byte_size: u64,
    pub storage_path: PathBuf,
    pub preview: String,
}

impl From<&ArtifactRecord> for TranscriptArtifactRef {
    fn from(record: &ArtifactRecord) -> Self {
        Self {
            artifact_id: record.id.clone(),
            tool_name: record.tool_name.clone(),
            tool_call_id: record.tool_call_id.clone(),
            byte_size: record.byte_size,
            storage_path: record.storage_path.clone(),
            preview: record.preview.clone(),
        }
    }
}

#[must_use]
pub fn render_transcript_artifact_ref(reference: &TranscriptArtifactRef) -> String {
    // The model sees several identifiers in this block. Keep a literal
    // retrieve command next to them so it does not have to infer which
    // field is accepted by `retrieve_tool_result`.
    format!(
        "[artifact: {tool}]\n\
         id:           {id}\n\
         tool:         {tool}\n\
         tool_call_id: {tool_call_id}\n\
         size:         {size}\n\
         path:         {path}\n\
         preview:      {preview}\n\
         retrieve:     retrieve_tool_result ref={id}",
        tool = reference.tool_name,
        id = reference.artifact_id,
        tool_call_id = reference.tool_call_id,
        size = format_byte_size(reference.byte_size),
        path = format_artifact_relative_path(&reference.storage_path),
        preview = reference.preview.replace('\n', " "),
    )
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
    fn transcript_ref_renders_relative_paths_with_forward_slashes() {
        let reference = TranscriptArtifactRef {
            artifact_id: "art_call-big".to_string(),
            tool_name: "exec_shell".to_string(),
            tool_call_id: "call-big".to_string(),
            byte_size: 1024,
            storage_path: PathBuf::from(r"artifacts\art_call-big.txt"),
            preview: "checking crate".to_string(),
        };

        let rendered = render_transcript_artifact_ref(&reference);

        assert!(rendered.contains("path:         artifacts/art_call-big.txt"));
        assert!(
            rendered.contains("retrieve:     retrieve_tool_result ref=art_call-big"),
            "rendered block must embed the literal retrieve command: {rendered}"
        );
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
}
