//! Read-only detection of an installed official DeepSeek Harness (`dsh`).
//!
//! Detection never writes: it locates the `dsh` launcher on `PATH`, asks it
//! for `--version` and the launcher `--help` (neither initializes a profile),
//! resolves `$DSH_HOME` the way `dsh-home-paths` does, and inventories the
//! profile names, `settings.yaml` namespaces, and the *presence* of the
//! managed credentials file. Credential values are never read.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// The exact `dsh` release this integration was verified against.
pub(crate) const VERIFIED_DSH_VERSION: &str = "0.1.0-rc.6";

/// Parsed `MAJOR.MINOR.PATCH[-rc.N]` version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DshVersion {
    pub(crate) major: u64,
    pub(crate) minor: u64,
    pub(crate) patch: u64,
    /// `None` = a final release (sorts after every rc of the same base).
    pub(crate) rc: Option<u64>,
}

impl DshVersion {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim().trim_start_matches('v');
        let (base, pre) = match raw.split_once('-') {
            Some((base, pre)) => (base, Some(pre)),
            None => (raw, None),
        };
        let mut parts = base.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        let rc = match pre {
            None => None,
            Some(pre) => {
                let n = pre.strip_prefix("rc.")?.parse().ok()?;
                Some(n)
            }
        };
        Some(Self {
            major,
            minor,
            patch,
            rc,
        })
    }

    fn base(self) -> (u64, u64, u64) {
        (self.major, self.minor, self.patch)
    }

    /// Prerelease ordering: any rc sorts *before* the final release of the
    /// same base, so `Option<u64>` derives the wrong order and is compared
    /// here explicitly.
    fn cmp_semver(self, other: Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match self.base().cmp(&other.base()) {
            Ordering::Equal => match (self.rc, other.rc) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(&b),
            },
            ordering => ordering,
        }
    }
}

impl std::fmt::Display for DshVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(rc) = self.rc {
            write!(f, "-rc.{rc}")?;
        }
        Ok(())
    }
}

/// How the installed `dsh` relates to the verified release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DshCompatibility {
    /// Exactly the verified release.
    Verified,
    /// Newer than the verified release: launchable, but unverified.
    NewerUnverified { verified: String },
    /// Older than the verified release, or missing the `--patch` seam.
    Incompatible { reason: String },
    /// The launcher exists but could not be run or did not report a version.
    Offline { reason: String },
    /// Version text that does not parse.
    Unparsed { raw: String },
}

impl DshCompatibility {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NewerUnverified { .. } => "newer-unverified",
            Self::Incompatible { .. } => "incompatible",
            Self::Offline { .. } => "offline",
            Self::Unparsed { .. } => "unparsed",
        }
    }
}

/// Classify a version string against [`VERIFIED_DSH_VERSION`] and whether the
/// launcher advertises `--patch`.
pub(crate) fn classify_version(raw: &str, supports_patch: bool) -> DshCompatibility {
    let Some(version) = DshVersion::parse(raw) else {
        return DshCompatibility::Unparsed {
            raw: raw.trim().to_string(),
        };
    };
    let verified = DshVersion::parse(VERIFIED_DSH_VERSION).expect("verified version parses");
    match version.cmp_semver(verified) {
        std::cmp::Ordering::Less => DshCompatibility::Incompatible {
            reason: format!("{version} is older than the verified {verified}"),
        },
        _ if !supports_patch => DshCompatibility::Incompatible {
            reason: "launcher does not advertise --patch overlays".to_string(),
        },
        std::cmp::Ordering::Equal => DshCompatibility::Verified,
        std::cmp::Ordering::Greater => DshCompatibility::NewerUnverified {
            verified: verified.to_string(),
        },
    }
}

/// Raw facts about one `dsh` installation. Every field is derived without
/// writing to disk or reading a credential value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DshDetection {
    /// Resolved launcher path when one is on `PATH`.
    pub(crate) binary: Option<PathBuf>,
    /// `dsh --version` output when it ran.
    pub(crate) version: Option<String>,
    pub(crate) compatibility: DshCompatibility,
    /// Whether the launcher help text advertises `--patch`.
    pub(crate) supports_patch: bool,
    /// `$DSH_HOME` (or `~/.dsh`), whether or not it exists yet.
    pub(crate) dsh_home: PathBuf,
    pub(crate) dsh_home_exists: bool,
    pub(crate) dsh_home_from_env: bool,
    /// Profile directory names under `$DSH_HOME/profiles`.
    pub(crate) profiles: Vec<String>,
    /// Top-level namespaces present in `$DSH_HOME/settings.yaml`.
    pub(crate) settings_namespaces: Vec<String>,
    /// `$DSH_HOME/.credentials.yaml` exists (values are never read).
    pub(crate) credentials_present: bool,
    /// Whether the credentials file is `0600` (POSIX only; `None` elsewhere).
    pub(crate) credentials_mode_ok: Option<bool>,
}

impl DshDetection {
    pub(crate) fn installed(&self) -> bool {
        self.binary.is_some()
    }
}

/// Environment facts detection reads. Injected so tests never depend on the
/// machine's real `PATH`, `HOME`, or `DSH_HOME`.
#[derive(Debug, Clone)]
pub(crate) struct DetectEnv {
    pub(crate) path: Option<OsString>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) dsh_home: Option<OsString>,
}

impl DetectEnv {
    pub(crate) fn from_process() -> Self {
        Self {
            path: std::env::var_os("PATH"),
            home: dirs::home_dir(),
            dsh_home: std::env::var_os("DSH_HOME"),
        }
    }
}

/// Runs the launcher. Injected so tests can stub `--version`/`--help`.
pub(crate) trait DshRunner {
    /// Returns `(exit_success, stdout+stderr)`.
    fn run(&self, binary: &Path, args: &[&str]) -> std::io::Result<(bool, String)>;
}

pub(crate) struct ProcessRunner;

impl DshRunner for ProcessRunner {
    fn run(&self, binary: &Path, args: &[&str]) -> std::io::Result<(bool, String)> {
        let output = Command::new(binary)
            .args(args)
            // Belt and braces: the launcher help/version paths do not send
            // telemetry, but the harness honors this as a hard opt-out.
            .env("DSH_TELEMETRY_DISABLED", "1")
            .stdin(std::process::Stdio::null())
            .output()?;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((output.status.success(), text))
    }
}

/// Resolve `$DSH_HOME` like `dsh-home-paths`: a non-blank env value (with
/// `~` expansion), else `~/.dsh`.
pub(crate) fn resolve_dsh_home(env: &DetectEnv) -> (PathBuf, bool) {
    if let Some(raw) = env.dsh_home.as_ref() {
        let text = raw.to_string_lossy();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            if let Some(rest) = trimmed.strip_prefix("~/") {
                if let Some(home) = env.home.as_ref() {
                    return (home.join(rest), true);
                }
            } else if trimmed != "~" {
                return (PathBuf::from(trimmed), true);
            }
        }
    }
    let home = env.home.clone().unwrap_or_else(|| PathBuf::from("."));
    (home.join(".dsh"), false)
}

fn find_on_path(path: Option<&OsString>) -> Option<PathBuf> {
    let path = path?;
    for dir in std::env::split_paths(path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for name in ["dsh", "dsh.cmd", "dsh.exe"] {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Top-level YAML mapping keys of a settings document, without a YAML parser:
/// a key is a line with no leading whitespace ending in `:` (optionally with
/// an inline value). Bounded to the first 64 KiB.
pub(crate) fn settings_namespaces(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.lines().take(4096) {
        if raw.starts_with([' ', '\t', '#', '-']) || raw.trim().is_empty() {
            continue;
        }
        let Some((key, _)) = raw.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.starts_with('"') || key.starts_with('\'') {
            continue;
        }
        if !out.iter().any(|k| k == key) {
            out.push(key.to_string());
        }
    }
    out
}

pub(crate) fn detect(env: &DetectEnv, runner: &dyn DshRunner) -> DshDetection {
    let (dsh_home, dsh_home_from_env) = resolve_dsh_home(env);
    let dsh_home_exists = dsh_home.is_dir();
    let mut profiles = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dsh_home.join("profiles")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "node_modules" || name.starts_with('.') {
                continue;
            }
            if entry.path().is_dir() {
                profiles.push(name);
            }
        }
    }
    profiles.sort();
    let settings_namespaces = std::fs::read(dsh_home.join("settings.yaml"))
        .ok()
        .map(|bytes| {
            let bytes = &bytes[..bytes.len().min(64 * 1024)];
            settings_namespaces(&String::from_utf8_lossy(bytes))
        })
        .unwrap_or_default();
    let credentials_path = dsh_home.join(".credentials.yaml");
    let credentials_present = credentials_path.is_file();
    let credentials_mode_ok = credentials_mode_ok(&credentials_path);

    let binary = find_on_path(env.path.as_ref());
    let Some(binary) = binary else {
        return DshDetection {
            binary: None,
            version: None,
            compatibility: DshCompatibility::Offline {
                reason: "dsh is not on PATH".to_string(),
            },
            supports_patch: false,
            dsh_home,
            dsh_home_exists,
            dsh_home_from_env,
            profiles,
            settings_namespaces,
            credentials_present,
            credentials_mode_ok,
        };
    };

    let (version, supports_patch, compatibility) = match runner.run(&binary, &["--version"]) {
        Ok((true, text)) => {
            let version = text.trim().lines().last().unwrap_or("").trim().to_string();
            let supports_patch = match runner.run(&binary, &["--help"]) {
                Ok((_, help)) => help.contains("--patch"),
                Err(_) => false,
            };
            let compatibility = classify_version(&version, supports_patch);
            (Some(version), supports_patch, compatibility)
        }
        Ok((false, text)) => (
            None,
            false,
            DshCompatibility::Offline {
                reason: format!(
                    "dsh --version exited non-zero: {}",
                    text.trim()
                        .lines()
                        .next()
                        .unwrap_or("")
                        .chars()
                        .take(120)
                        .collect::<String>()
                ),
            },
        ),
        Err(error) => (
            None,
            false,
            DshCompatibility::Offline {
                reason: format!("dsh could not be run: {error}"),
            },
        ),
    };

    DshDetection {
        binary: Some(binary),
        version,
        compatibility,
        supports_patch,
        dsh_home,
        dsh_home_exists,
        dsh_home_from_env,
        profiles,
        settings_namespaces,
        credentials_present,
        credentials_mode_ok,
    }
}

#[cfg(unix)]
fn credentials_mode_ok(path: &Path) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::metadata(path).ok()?;
    Some(meta.permissions().mode() & 0o077 == 0)
}

#[cfg(not(unix))]
fn credentials_mode_ok(_path: &Path) -> Option<bool> {
    None
}
