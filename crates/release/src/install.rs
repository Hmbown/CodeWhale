//! How *this* binary was installed, and therefore which command updates it.
//!
//! `codewhale update` replaces the running executable in place. That is the
//! right thing for a binary the user downloaded from GitHub Releases, and the
//! wrong thing for one a package manager owns: overwriting Homebrew's Cellar
//! binary or npm's `node_modules` payload leaves the manager's metadata
//! describing a version that is no longer on disk, and the next
//! `brew upgrade` / `npm install -g` silently reverts the user.
//!
//! So before we tell anyone to run anything, we work out who owns the file.
//! Detection is path-based where managers use distinct prefixes. Omarchy's
//! AUR packages live in the ordinary `/usr/bin` prefix, so that case also
//! consults the local pacman ownership database. No network access is needed.

use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

/// Environment variable that overrides install-method detection.
///
/// Accepts `npm`, `homebrew` (or `brew`), `cargo`, `omarchy`, and `binary`.
/// Anything else is ignored and detection falls back to automatic detection.
/// Packagers who relocate the binary somewhere the heuristics cannot read —
/// and users debugging a wrong guess — set this.
pub const INSTALL_METHOD_ENV: &str = "CODEWHALE_INSTALL_METHOD";

/// The package manager (if any) that owns the running executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InstallMethod {
    /// Global npm install — the `codewhale` package under `node_modules`.
    Npm,
    /// Homebrew — a binary under a `Cellar` or `linuxbrew` prefix.
    Homebrew,
    /// `cargo install` — a binary under `~/.cargo/bin`.
    Cargo,
    /// An AUR/pacman package installed on Omarchy.
    Omarchy,
    /// A release binary the user placed on disk themselves. The default, and
    /// the only case where in-place self-update is correct.
    Binary,
}

impl InstallMethod {
    /// Detect from an executable path, honouring [`INSTALL_METHOD_ENV`].
    ///
    /// Pass the *resolved* path — `std::env::current_exe()` already follows
    /// symlinks on the platforms we ship, which is what puts a globally
    /// npm-installed binary inside `node_modules` and a Homebrew one inside
    /// `Cellar` rather than in the manager's flat `bin` shim directory.
    #[must_use]
    pub fn detect(exe: &Path) -> Self {
        if let Some(forced) = std::env::var(INSTALL_METHOD_ENV)
            .ok()
            .and_then(|raw| Self::from_token(&raw))
        {
            return forced;
        }
        Self::detect_with_omarchy_probe(exe, omarchy_package_owns)
    }

    fn detect_with_omarchy_probe(exe: &Path, owns_package: impl FnOnce(&Path) -> bool) -> Self {
        let path_method = Self::from_path(exe);
        if path_method != Self::Binary {
            return path_method;
        }
        if owns_package(exe) {
            Self::Omarchy
        } else {
            Self::Binary
        }
    }

    /// Path-only detection, with no environment lookup. Split out from
    /// [`detect`](Self::detect) so tests can exercise the heuristics without
    /// mutating process-global state.
    #[must_use]
    pub fn from_path(exe: &Path) -> Self {
        let components: Vec<String> = exe
            .components()
            .filter_map(|c| c.as_os_str().to_str())
            .map(str::to_ascii_lowercase)
            .collect();

        let has = |name: &str| components.iter().any(|c| c == name);

        // npm is checked first: a `node_modules` install *inside* a Homebrew
        // or Termux prefix is still npm's to update.
        if has("node_modules") {
            return Self::Npm;
        }
        if has("cellar") || has(".linuxbrew") || has("linuxbrew") {
            return Self::Homebrew;
        }
        // `.cargo/bin/codewhale` — require the pair so an unrelated `bin`
        // directory does not read as a Cargo install.
        if components
            .windows(2)
            .any(|pair| pair[0] == ".cargo" && pair[1] == "bin")
        {
            return Self::Cargo;
        }
        Self::Binary
    }

    fn from_token(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "npm" => Some(Self::Npm),
            "homebrew" | "brew" => Some(Self::Homebrew),
            "cargo" => Some(Self::Cargo),
            "omarchy" => Some(Self::Omarchy),
            "binary" | "release" => Some(Self::Binary),
            _ => None,
        }
    }

    /// The exact shell command that updates this install.
    ///
    /// Homebrew's primary formula is `codewhale`. Existing Cellar paths
    /// under the legacy `deepseek-tui` name still detect as Homebrew; those
    /// installs can keep using `brew upgrade deepseek-tui` during the
    /// overlap window, but new notices name the Codewhale formula.
    #[must_use]
    pub fn update_command(self) -> &'static str {
        match self {
            Self::Npm => "npm install -g codewhale@latest",
            Self::Homebrew => "brew upgrade codewhale",
            Self::Cargo => "cargo install codewhale-cli --locked --force",
            Self::Omarchy => "omarchy update",
            Self::Binary => "codewhale update",
        }
    }

    /// Whether `codewhale update` may replace this binary in place.
    ///
    /// False for every package-managed install: see the module docs for why
    /// overwriting a managed binary is worse than doing nothing.
    #[must_use]
    pub fn supports_self_update(self) -> bool {
        matches!(self, Self::Binary)
    }

    /// Short human label, for messages that name the owner of the install.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Homebrew => "Homebrew",
            Self::Cargo => "cargo",
            Self::Omarchy => "Omarchy",
            Self::Binary => "release binary",
        }
    }
}

#[cfg(target_os = "linux")]
fn omarchy_package_owns(exe: &Path) -> bool {
    if !Path::new("/usr/share/omarchy/version").is_file() {
        return false;
    }

    Command::new("pacman")
        .args(["-Qo"])
        .arg(exe)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(target_os = "linux"))]
fn omarchy_package_owns(_exe: &Path) -> bool {
    false
}

/// Detect the install method for the currently running executable.
///
/// Returns [`InstallMethod::Binary`] when the executable path cannot be
/// resolved — the conservative answer, because it is the one that tells the
/// user to run our own updater rather than a package manager command that may
/// not apply to them.
#[must_use]
pub fn current_install_method() -> InstallMethod {
    match std::env::current_exe() {
        Ok(exe) => InstallMethod::detect(&exe),
        Err(_) => InstallMethod::Binary,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn npm_global_install_is_detected_from_node_modules() {
        let exe = PathBuf::from("/usr/local/lib/node_modules/codewhale/bin/codewhale");
        assert_eq!(InstallMethod::from_path(&exe), InstallMethod::Npm);
        assert_eq!(
            InstallMethod::Npm.update_command(),
            "npm install -g codewhale@latest"
        );
        assert!(!InstallMethod::Npm.supports_self_update());
    }

    #[test]
    fn homebrew_install_is_detected_from_cellar_on_both_prefixes() {
        for exe in [
            "/opt/homebrew/Cellar/codewhale/0.9.8/bin/codewhale",
            "/usr/local/Cellar/codewhale/0.9.8/bin/codewhale",
            "/home/linuxbrew/.linuxbrew/Cellar/codewhale/0.9.8/bin/codewhale",
            "/opt/homebrew/Cellar/deepseek-tui/0.9.4/bin/codewhale",
            "/usr/local/Cellar/deepseek-tui/0.9.4/bin/codewhale",
            "/home/linuxbrew/.linuxbrew/Cellar/deepseek-tui/0.9.4/bin/codewhale",
        ] {
            assert_eq!(
                InstallMethod::from_path(&PathBuf::from(exe)),
                InstallMethod::Homebrew,
                "{exe} should read as Homebrew"
            );
        }
        assert_eq!(
            InstallMethod::Homebrew.update_command(),
            "brew upgrade codewhale"
        );
        assert!(!InstallMethod::Homebrew.supports_self_update());
    }

    #[test]
    fn cargo_install_requires_the_cargo_bin_pair() {
        assert_eq!(
            InstallMethod::from_path(&PathBuf::from("/home/u/.cargo/bin/codewhale")),
            InstallMethod::Cargo
        );
        // A bare `bin` directory is not a Cargo install.
        assert_eq!(
            InstallMethod::from_path(&PathBuf::from("/home/u/bin/codewhale")),
            InstallMethod::Binary
        );
        assert!(!InstallMethod::Cargo.supports_self_update());
    }

    #[test]
    fn npm_wins_over_an_enclosing_manager_prefix() {
        // npm installed under a Homebrew-managed node prefix is still npm's.
        let exe = PathBuf::from("/opt/homebrew/lib/node_modules/codewhale/bin/codewhale");
        assert_eq!(InstallMethod::from_path(&exe), InstallMethod::Npm);
    }

    #[test]
    fn omarchy_probe_only_claims_package_owned_plain_paths() {
        let managed = PathBuf::from("/usr/bin/codewhale");
        assert_eq!(
            InstallMethod::detect_with_omarchy_probe(&managed, |_| true),
            InstallMethod::Omarchy
        );
        assert_eq!(
            InstallMethod::detect_with_omarchy_probe(&managed, |_| false),
            InstallMethod::Binary
        );

        let npm = PathBuf::from("/usr/lib/node_modules/codewhale/bin/codewhale");
        assert_eq!(
            InstallMethod::detect_with_omarchy_probe(&npm, |_| {
                panic!("a path-owned install must not query pacman")
            }),
            InstallMethod::Npm
        );
    }

    #[test]
    fn termux_and_plain_release_binaries_self_update() {
        for exe in [
            "/data/data/com.termux/files/usr/bin/codewhale",
            "/usr/local/bin/codewhale",
            "/home/u/Downloads/codewhale",
        ] {
            let method = InstallMethod::from_path(&PathBuf::from(exe));
            assert_eq!(method, InstallMethod::Binary, "{exe} should self-update");
            assert!(method.supports_self_update());
            assert_eq!(method.update_command(), "codewhale update");
        }
    }

    #[test]
    fn env_tokens_map_to_methods_and_junk_is_ignored() {
        assert_eq!(InstallMethod::from_token("npm"), Some(InstallMethod::Npm));
        assert_eq!(
            InstallMethod::from_token("  BREW "),
            Some(InstallMethod::Homebrew)
        );
        assert_eq!(
            InstallMethod::from_token("homebrew"),
            Some(InstallMethod::Homebrew)
        );
        assert_eq!(
            InstallMethod::from_token("cargo"),
            Some(InstallMethod::Cargo)
        );
        assert_eq!(
            InstallMethod::from_token("omarchy"),
            Some(InstallMethod::Omarchy)
        );
        assert_eq!(
            InstallMethod::from_token("binary"),
            Some(InstallMethod::Binary)
        );
        assert_eq!(InstallMethod::from_token("apt"), None);
    }

    #[test]
    fn omarchy_install_is_package_managed() {
        assert_eq!(InstallMethod::Omarchy.update_command(), "omarchy update");
        assert_eq!(InstallMethod::Omarchy.label(), "Omarchy");
        assert!(!InstallMethod::Omarchy.supports_self_update());
    }
}
