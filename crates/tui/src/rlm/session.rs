//! Persistent RLM session state for the v0.8.33 head/hands tool surface.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::repl::PythonRuntime;

pub type SharedRlmSessionStore = Arc<Mutex<HashMap<String, Arc<Mutex<RlmSession>>>>>;

#[must_use]
pub fn new_shared_rlm_session_store() -> SharedRlmSessionStore {
    Arc::new(Mutex::new(HashMap::new()))
}

#[derive(Debug)]
pub struct RlmSession {
    pub name: String,
    pub id: String,
    pub kernel: Option<PythonRuntime>,
    pub context_meta: ContextMeta,
    pub config: RlmSessionConfig,
    pub rpc_count: u32,
    pub total_duration: Duration,
    pub peak_var_count: usize,
    pub final_count: usize,
    pub created_at: Instant,
    pub last_used_at: Instant,
    pub context_path: PathBuf,
}

impl RlmSession {
    #[must_use]
    pub fn new(
        name: String,
        kernel: PythonRuntime,
        context_meta: ContextMeta,
        context_path: PathBuf,
    ) -> Self {
        let now = Instant::now();
        Self {
            name,
            id: format!("rlm:{}", Uuid::new_v4().simple()),
            kernel: Some(kernel),
            context_meta,
            config: RlmSessionConfig::default(),
            rpc_count: 0,
            total_duration: Duration::ZERO,
            peak_var_count: 0,
            final_count: 0,
            created_at: now,
            last_used_at: now,
            context_path,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextMeta {
    pub length: usize,
    #[serde(rename = "type")]
    pub type_name: String,
    pub preview_500: String,
    pub sha256: String,
}

impl ContextMeta {
    #[must_use]
    pub fn from_body(body: &str, type_name: impl Into<String>) -> Self {
        Self {
            length: body.chars().count(),
            type_name: type_name.into(),
            preview_500: body.chars().take(500).collect(),
            sha256: sha256_hex(body.as_bytes()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputFeedback {
    Full,
    Metadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RlmSessionConfig {
    pub output_feedback: OutputFeedback,
    pub sub_query_timeout_secs: u64,
    pub sub_rlm_max_depth: u32,
    pub share_session: bool,
}

impl Default for RlmSessionConfig {
    fn default() -> Self {
        Self {
            output_feedback: OutputFeedback::Full,
            sub_query_timeout_secs: 120,
            sub_rlm_max_depth: 1,
            share_session: false,
        }
    }
}

pub fn write_context_file(body: &str) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join("deepseek_rlm_ctx");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!(
        "session_{}_{}.txt",
        std::process::id(),
        Uuid::new_v4().simple()
    ));
    std::fs::write(&path, body)?;
    Ok(path)
}

#[must_use]
pub fn derive_session_name(source_hint: Option<&str>) -> String {
    let hint = source_hint
        .and_then(|raw| {
            Path::new(raw)
                .file_name()
                .and_then(|name| name.to_str())
                .or(Some(raw))
        })
        .unwrap_or("context");
    let mut out = String::new();
    for ch in hint.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_matches('_');
    if out.is_empty() {
        "context".to_string()
    } else {
        out.to_string()
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_session_name_slugifies_path() {
        assert_eq!(
            derive_session_name(Some("src/Big File.rs")),
            "big_file_rs".to_string()
        );
    }

    #[test]
    fn context_meta_hashes_and_previews_body() {
        let meta = ContextMeta::from_body("abcdef", "text");
        assert_eq!(meta.length, 6);
        assert_eq!(meta.preview_500, "abcdef");
        assert_eq!(
            meta.sha256,
            "bef57ec7f53a6d40beb640a780a639c83bc29ac8a9816f1fc6c5c6dcd93c4721"
        );
    }
}
