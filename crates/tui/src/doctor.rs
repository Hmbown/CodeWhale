//! Structural, offline-by-default diagnostics shared by doctor renderers.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;

/// Canonical user-scoped paths reported by both human and JSON doctor output.
///
/// Resolution is read-only: this type does not construct managers, create
/// directories, or trigger legacy migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DoctorPathReport {
    pub(crate) home: PathBuf,
    pub(crate) config: PathBuf,
    pub(crate) settings: PathBuf,
    pub(crate) sessions: PathBuf,
    pub(crate) logs: PathBuf,
    pub(crate) automations: PathBuf,
    pub(crate) task_manager_root: PathBuf,
    pub(crate) task_manager_tasks: PathBuf,
    pub(crate) task_manager_artifacts: PathBuf,
    pub(crate) runtime_store: PathBuf,
    pub(crate) runtime_events: PathBuf,
    pub(crate) personal_fleet_definitions: PathBuf,
    pub(crate) personal_fleet_agents: PathBuf,
    pub(crate) secrets: PathBuf,
}

impl DoctorPathReport {
    pub(crate) fn resolve(config_override: Option<&Path>) -> Result<Self> {
        let home = codewhale_paths::codewhale_home()
            .map_err(anyhow::Error::new)?
            .context("could not resolve the canonical Codewhale state root")?;
        let config = match config_override {
            Some(path) => codewhale_config::resolve_config_path(Some(path.to_path_buf()))
                .context("could not normalize the explicit config path")?,
            None => codewhale_config::resolve_config_path(None)
                .unwrap_or_else(|_| home.join(codewhale_config::CONFIG_FILE_NAME)),
        };
        let settings = crate::settings::Settings::path()
            .context("could not resolve the canonical settings path")?;
        let sessions = codewhale_config::resolve_state_dir("sessions")
            .context("could not resolve the sessions path")?;
        let logs = crate::runtime_log::log_directory()
            .context("could not resolve the runtime log directory")?;
        let automations = crate::automation_manager::default_automations_dir();
        let task_manager_root = crate::task_manager::default_tasks_dir();
        let task_manager_tasks = task_manager_root.join("tasks");
        let task_manager_artifacts = task_manager_root.join("artifacts");
        let runtime_config = crate::runtime_threads::RuntimeThreadManagerConfig::from_task_data_dir(
            task_manager_root.clone(),
        );
        let runtime_store = runtime_config.data_dir;
        let runtime_events = runtime_store.join("events");
        let personal_fleet_definitions = crate::fleet::exact::personal_fleet_definitions_dir()
            .context("could not resolve the personal Fleet definitions directory")?;
        let personal_fleet_agents = crate::fleet::profile::personal_agent_profile_dir()
            .context("could not resolve the personal Fleet agent directory")?;
        let (secrets, _) = codewhale_secrets::FileKeyringStore::default_paths_read_only()
            .context("could not resolve the file secret backend path")?;
        Ok(Self {
            home,
            config,
            settings,
            sessions,
            logs,
            automations,
            task_manager_root,
            task_manager_tasks,
            task_manager_artifacts,
            runtime_store,
            runtime_events,
            personal_fleet_definitions,
            personal_fleet_agents,
            secrets,
        })
    }

    pub(crate) fn entries(&self) -> [(&'static str, &Path); 14] {
        [
            ("home", self.home.as_path()),
            ("config", self.config.as_path()),
            ("settings", self.settings.as_path()),
            ("sessions", self.sessions.as_path()),
            ("logs", self.logs.as_path()),
            ("automations", self.automations.as_path()),
            ("task_manager_root", self.task_manager_root.as_path()),
            ("task_manager_tasks", self.task_manager_tasks.as_path()),
            (
                "task_manager_artifacts",
                self.task_manager_artifacts.as_path(),
            ),
            ("runtime_store", self.runtime_store.as_path()),
            ("runtime_events", self.runtime_events.as_path()),
            (
                "personal_fleet_definitions",
                self.personal_fleet_definitions.as_path(),
            ),
            (
                "personal_fleet_agents",
                self.personal_fleet_agents.as_path(),
            ),
            ("secrets", self.secrets.as_path()),
        ]
    }
}

/// Render structural secret-backend facts for the human doctor report.
///
/// The input type cannot carry secret values, keeping this renderer safe by
/// construction.
pub(crate) fn secret_backend_human_lines(
    diagnostic: &codewhale_secrets::SecretBackendDiagnostic,
) -> Vec<String> {
    use codewhale_secrets::{
        SecretBackendDiagnosticKind, SecretBackendInspection, SecretBackendPresence,
    };

    let presence = |value| match value {
        SecretBackendPresence::Present => "present",
        SecretBackendPresence::Absent => "absent",
        SecretBackendPresence::Unknown => "unknown",
    };
    let inspection = match diagnostic.inspection {
        SecretBackendInspection::MetadataOnly => "metadata_only",
        SecretBackendInspection::NotProbed => "not_probed",
    };
    let mut lines = match diagnostic.backend {
        SecretBackendDiagnosticKind::File => vec![
            "backend: file".to_string(),
            format!("presence: {} ({inspection})", presence(diagnostic.presence)),
        ],
        SecretBackendDiagnosticKind::System => vec![
            "backend: system".to_string(),
            "status: unknown (not_probed)".to_string(),
        ],
        SecretBackendDiagnosticKind::Unknown => vec![
            "backend: unknown".to_string(),
            "status: unknown (not_probed; unsupported configuration)".to_string(),
        ],
    };
    if let Some(path) = diagnostic.path.as_deref() {
        lines.push(format!("path: {}", path.display()));
    }
    if let Some(path) = diagnostic.legacy_path.as_deref() {
        lines.push(format!(
            "legacy_path: {} ({}, {inspection})",
            path.display(),
            presence(diagnostic.legacy_presence)
        ));
    }
    lines.push("No credential-store values were read or printed by this check.".to_string());
    lines
}

/// Report key names — never values — for config entries whose value is
/// shaped like a bearer credential. `config.toml` is plain text, not a
/// secret store; doctor warns so tokens migrate to the secret backend
/// (morning-report issue: a plaintext OAuth token sat beside a `[redacted]`
/// sibling entry).
pub(crate) fn config_credential_shaped_keys(raw: &str) -> Vec<String> {
    fn strong_shape(value: &str) -> bool {
        const PREFIXES: [&str; 9] = [
            "sk-",
            "sk_",
            "xai-",
            "ghp_",
            "gho_",
            "github_pat_",
            "xoxb-",
            "xoxp-",
            "eyJ",
        ];
        value.len() >= 20 && PREFIXES.iter().any(|prefix| value.starts_with(prefix))
    }
    fn suspect_key(key: &str) -> bool {
        let key = key.to_ascii_lowercase();
        [
            "token",
            "secret",
            "password",
            "credential",
            "api_key",
            "apikey",
            "access_key",
        ]
        .iter()
        .any(|needle| key.contains(needle))
    }
    fn random_shape(value: &str) -> bool {
        value.len() >= 24
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            && value.chars().any(|ch| ch.is_ascii_digit())
            && value.chars().any(|ch| ch.is_ascii_alphabetic())
    }

    let mut flagged: Vec<String> = Vec::new();
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().trim_matches('"');
        let value = value.trim();
        let Some(value) = value
            .strip_prefix('"')
            .and_then(|value| value.strip_suffix('"'))
        else {
            continue;
        };
        if value.is_empty() || value.eq_ignore_ascii_case("[redacted]") {
            continue;
        }
        if (strong_shape(value) || (suspect_key(key) && random_shape(value)))
            && !flagged.iter().any(|existing| existing == key)
        {
            flagged.push(key.to_string());
        }
    }
    flagged
}

/// Return only the non-secret network authority of a configured URL.
///
/// Userinfo, path, query keys and values, and fragments are all omitted because
/// every one of those components can carry credentials. Parse failures also
/// omit the original input rather than echoing an attacker-controlled value.
pub(crate) fn structural_url_authority(url: &str) -> String {
    let Some(parsed) = reqwest::Url::parse(url).ok() else {
        return "unparseable (configured value omitted)".to_string();
    };
    let Some(host) = parsed.host_str() else {
        return "unparseable (configured value omitted)".to_string();
    };
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let mut authority = format!("{}://{host}", parsed.scheme());
    if let Some(port) = parsed.port() {
        authority.push(':');
        authority.push_str(&port.to_string());
    }
    authority
}

/// Explicit live operations requested for one doctor invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DoctorProbeRequest {
    pub(crate) check_updates: bool,
    pub(crate) probe_api: bool,
    pub(crate) probe_local: bool,
    pub(crate) probe_mcp: bool,
}

impl DoctorProbeRequest {
    /// Whether a release service may be contacted.
    pub(crate) fn should_check_updates(self) -> bool {
        self.check_updates
    }

    /// Whether the configured provider endpoint may be contacted.
    ///
    /// Hosted and local endpoints have separate opt-ins because a local probe
    /// can wake a desktop-managed daemon.
    pub(crate) fn should_probe_api(self, endpoint_is_local: bool) -> bool {
        if endpoint_is_local {
            self.probe_local
        } else {
            self.probe_api
        }
    }

    /// Whether configured MCP processes may be started and contacted.
    pub(crate) fn should_probe_mcp(self) -> bool {
        self.probe_mcp
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum DoctorUpdateReport {
    NotChecked,
    UpdateAvailable { latest: String },
    UpToDate { latest: String },
    CurrentNewer { latest: String },
    ReleaseMetadataInvalid,
    ReleaseCheckFailed,
}

fn doctor_update_report<E>(
    current_version: &str,
    latest_result: Result<String, E>,
) -> DoctorUpdateReport {
    let Ok(raw_latest) = latest_result else {
        return DoctorUpdateReport::ReleaseCheckFailed;
    };
    let Some(latest) = doctor_safe_release_tag(&raw_latest) else {
        return DoctorUpdateReport::ReleaseMetadataInvalid;
    };
    match codewhale_release::compare_release_versions(current_version, &latest) {
        Ok(std::cmp::Ordering::Less) => DoctorUpdateReport::UpdateAvailable { latest },
        Ok(std::cmp::Ordering::Equal) => DoctorUpdateReport::UpToDate { latest },
        Ok(std::cmp::Ordering::Greater) => DoctorUpdateReport::CurrentNewer { latest },
        Err(_) => DoctorUpdateReport::ReleaseMetadataInvalid,
    }
}

/// Canonicalize a release tag before any doctor renderer sees it. A release
/// server response is untrusted input: it may not become an error echo or a
/// terminal control sequence merely because the user opted into an update
/// check.
fn doctor_safe_release_tag(raw: &str) -> Option<String> {
    let version = raw.trim().strip_prefix('v').unwrap_or(raw.trim());
    semver::Version::parse(version)
        .ok()
        .map(|version| format!("v{version}"))
}

fn doctor_update_report_lines(report: &DoctorUpdateReport) -> Vec<String> {
    match report {
        DoctorUpdateReport::NotChecked => vec![
            "latest: unknown (not checked; offline default)".to_string(),
            "Run `codewhale doctor --check-updates` to opt in.".to_string(),
        ],
        DoctorUpdateReport::UpdateAvailable { latest } => vec![
            format!("latest: {latest}"),
            "Update available. Run `codewhale update` to install.".to_string(),
        ],
        DoctorUpdateReport::UpToDate { latest } => {
            vec![
                format!("latest: {latest}"),
                "Already up to date.".to_string(),
            ]
        }
        DoctorUpdateReport::CurrentNewer { latest } => vec![
            format!("latest: {latest}"),
            "Current build is newer than the latest published release.".to_string(),
        ],
        DoctorUpdateReport::ReleaseMetadataInvalid => vec![
            "latest: unknown (release metadata invalid; details omitted)".to_string(),
            "Run `codewhale update --check` to retry.".to_string(),
        ],
        DoctorUpdateReport::ReleaseCheckFailed => vec![
            "latest: unknown (release check failed; details omitted)".to_string(),
            "Run `codewhale update --check` to retry.".to_string(),
        ],
    }
}

/// Print the update portion of the human doctor report.
///
/// The release service is contacted only when `--check-updates` populated the
/// explicit request bit. The default branch returns before constructing an
/// HTTP request. Failure details are deliberately typed and generic because
/// transport errors and release metadata are untrusted strings.
pub(crate) async fn print_update_report(probes: DoctorProbeRequest) {
    let current_version = env!("CARGO_PKG_VERSION");
    println!("  · current: v{current_version}");
    let report = if probes.should_check_updates() {
        doctor_update_report(
            current_version,
            codewhale_release::latest_release_tag_async(codewhale_release::ReleaseChannel::Stable)
                .await,
        )
    } else {
        DoctorUpdateReport::NotChecked
    };
    for (index, line) in doctor_update_report_lines(&report).into_iter().enumerate() {
        let indent = if index == 0 { "  ·" } else { "   " };
        println!("{indent} {line}");
    }
}

#[cfg(test)]
#[path = "doctor/tests.rs"]
mod tests;
