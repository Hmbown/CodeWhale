//! Durable, append-only receipts for the DSH integration.
//!
//! One JSON document at `$CODEWHALE_HOME/integrations/dsh/receipt.json`
//! holds the current connection record (or `null`) and an append-only history
//! of every install/update/disable/enable/remove event. Receipts follow
//! `docs/RECEIPTS.md`: read-only summaries that never carry credentials.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::identity::MappedIdentity;

pub(crate) const RECEIPT_SCHEMA_VERSION: u32 = 1;
pub(crate) const MAX_HISTORY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DshReceiptEvent {
    Connect,
    Update,
    Disable,
    Enable,
    Remove,
    InstallBundle,
    RemoveBundle,
}

impl DshReceiptEvent {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Update => "update",
            Self::Disable => "disable",
            Self::Enable => "enable",
            Self::Remove => "remove",
            Self::InstallBundle => "install_bundle",
            Self::RemoveBundle => "remove_bundle",
        }
    }
}

/// The live connection record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DshConnectionRecord {
    pub(crate) connected_at: String,
    pub(crate) updated_at: String,
    pub(crate) dsh_version: Option<String>,
    pub(crate) dsh_binary: Option<PathBuf>,
    pub(crate) dsh_home: PathBuf,
    /// `web` or `headless`.
    pub(crate) profile: String,
    pub(crate) overlay_path: PathBuf,
    pub(crate) overlay_sha256: String,
    pub(crate) skin_enabled: bool,
    pub(crate) skin_path: Option<PathBuf>,
    pub(crate) skin_sha256: Option<String>,
    pub(crate) disabled: bool,
    /// Documented plugin path, when installed into the dedicated profile.
    #[serde(default)]
    pub(crate) bundle: Option<super::bundle::DshBundleRecord>,
    pub(crate) identity: MappedIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DshReceiptEntry {
    pub(crate) event: DshReceiptEvent,
    pub(crate) at: String,
    pub(crate) codewhale_version: String,
    pub(crate) dsh_version: Option<String>,
    pub(crate) dsh_home: PathBuf,
    pub(crate) overlay_sha256: Option<String>,
    pub(crate) skin_sha256: Option<String>,
    /// `provider/model` the overlay pinned, when applicable.
    pub(crate) identity_summary: Option<String>,
    pub(crate) permission_mode: Option<String>,
    pub(crate) note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DshReceiptDocument {
    pub(crate) schema_version: u32,
    pub(crate) current: Option<DshConnectionRecord>,
    #[serde(default)]
    pub(crate) history: Vec<DshReceiptEntry>,
}

impl Default for DshReceiptDocument {
    fn default() -> Self {
        Self {
            schema_version: RECEIPT_SCHEMA_VERSION,
            current: None,
            history: Vec::new(),
        }
    }
}

impl DshReceiptDocument {
    pub(crate) fn load(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let doc: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse DSH receipt {}", path.display()))?;
                if doc.schema_version > RECEIPT_SCHEMA_VERSION {
                    anyhow::bail!(
                        "DSH receipt {} has schema_version {} (this build understands {})",
                        path.display(),
                        doc.schema_version,
                        RECEIPT_SCHEMA_VERSION
                    );
                }
                Ok(doc)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("read DSH receipt {}", path.display()))
            }
        }
    }

    pub(crate) fn push(&mut self, entry: DshReceiptEntry) {
        self.history.push(entry);
        if self.history.len() > MAX_HISTORY {
            let excess = self.history.len() - MAX_HISTORY;
            self.history.drain(0..excess);
        }
    }

    pub(crate) fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("receipt path has no parent"))?;
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        let json = serde_json::to_vec_pretty(self)?;
        write_atomic(path, &json)
    }
}

/// Write via a sibling temp file + rename so a crash never leaves a torn
/// receipt or overlay behind.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("path has no parent"))?;
    let tmp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".to_string()),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

pub(crate) fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}
