//! `codewhale computer-use setup`: the one-step on-ramp.
//!
//! 1. Write the embedded bundle to the user plugins root with the same
//!    `.installed-from` provenance marker the in-session installer writes,
//!    so `/plugin uninstall` and the review flow treat it as a normal install.
//! 2. Ask the OS for the permissions the driver needs (macOS prompts).
//! 3. Seed the per-app consent lists in `computer-use.toml` so the operator
//!    knows where to grant an app before the model first asks.
//! 4. Take a test capture and report what the model would see.
//! 5. Print the exact in-session commands that finish activation. Trust is
//!    never granted here: the hash-bound review stays in the TUI.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::bundle::{self, BUNDLE_NAME, WriteOutcome};
use crate::config::Config;
use crate::session::Session;

pub const INSTALLED_FROM_MARKER: &str = ".installed-from";
/// Install spec recorded in the marker. `/plugin update` refuses non-remote
/// specs, so updates happen by re-running `codewhale computer-use setup`.
pub const INSTALL_SPEC: &str = "builtin:computer-use";

/// SHA-256 over the embedded files in path order (`path\0contents\0`).
pub fn content_digest() -> String {
    let mut hasher = Sha256::new();
    for (rel, contents) in bundle::FILES {
        hasher.update(rel.as_bytes());
        hasher.update([0]);
        hasher.update(contents.as_bytes());
        hasher.update([0]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The schema-v2 marker body (`crates/tui/src/skills/install.rs`).
pub fn marker_json() -> String {
    let digest = content_digest();
    serde_json::json!({
        "schema_version": 2,
        "spec": INSTALL_SPEC,
        "url": null,
        "source_checksum": digest,
        "content_digest": digest,
        "installed_name": BUNDLE_NAME,
        "registry_version": null,
    })
    .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReport {
    pub bundle_dir: PathBuf,
    pub outcome: WriteOutcome,
}

/// Write the bundle under `plugins_root` (creating it) with the marker.
pub fn install_bundle(plugins_root: &Path, force: bool) -> Result<SetupReport, String> {
    std::fs::create_dir_all(plugins_root)
        .map_err(|e| format!("failed to create {}: {e}", plugins_root.display()))?;
    let bundle_dir = plugins_root.join(BUNDLE_NAME);
    let marker = marker_json();
    let outcome = bundle::write(&bundle_dir, Some((INSTALLED_FROM_MARKER, &marker)), force)?;
    Ok(SetupReport {
        bundle_dir,
        outcome,
    })
}

/// The commented `[apps]` block `setup` seeds into `computer-use.toml`.
///
/// Consent is deliberately empty to start with: the first app-targeted call
/// returns a `needs_app_approval` error naming the exact line to add here, so
/// the user grants access one app at a time and on purpose.
pub fn apps_section_text(host_terminal: Option<&str>) -> String {
    let mut out = String::from(
        "\n\
# Per-app consent for the background/element tools (computer_apps,\n\
# computer_app_state, computer_element, and the `app` argument on\n\
# computer_click/type/key/scroll/drag). An app that is on neither list makes\n\
# those tools return a `needs_app_approval` error naming the line to add.\n\
# Entries match a bundle id, an app name, or a process name, case-insensitive.\n\
# `deny` beats `allow`.\n\
[apps]\n\
allow = []\n\
deny = []\n",
    );
    if let Some(terminal) = host_terminal {
        out.push_str(&format!(
            "# Always excluded, whatever these lists say: {terminal} (the terminal\n\
# hosting Codewhale), Codewhale itself, and security/login/System Settings.\n"
        ));
    }
    out
}

/// True when the file has no `[apps]` table yet.
pub fn needs_apps_section(existing: &str) -> bool {
    !existing
        .lines()
        .map(str::trim)
        .any(|line| line == "[apps]" || line.starts_with("[apps]"))
}

/// Seed `[apps]` in the config file, creating the file if needed. Returns a
/// line for the setup report.
pub fn ensure_apps_section(path: &Path, host_terminal: Option<&str>) -> Result<String, String> {
    let section = apps_section_text(host_terminal);
    if path.is_file() {
        let existing = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if !needs_apps_section(&existing) {
            return Ok(format!(
                "consent lists already configured in {}",
                path.display()
            ));
        }
        let mut updated = existing;
        if !updated.ends_with('\n') {
            updated.push('\n');
        }
        updated.push_str(&section);
        std::fs::write(path, updated)
            .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        return Ok(format!(
            "added an empty [apps] consent block to {}",
            path.display()
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    }
    let body = format!(
        "# Codewhale computer use. See docs/COMPUTER_USE.md.\n\
# target = \"auto\"   # auto | desktop | android | harmony\n\
# mode = \"act\"      # act | observe (observe blocks all input)\n{section}"
    );
    std::fs::write(path, body).map_err(|e| format!("failed to write {}: {e}", path.display()))?;
    Ok(format!(
        "wrote {} with an empty [apps] consent block",
        path.display()
    ))
}

fn check(ok: bool) -> &'static str {
    if ok { "✓" } else { "✗" }
}

/// Run the whole wizard, printing progress. Returns the exit code.
pub fn run(cfg: Config, force: bool, skip_permissions: bool) -> i32 {
    let mut failed = false;
    println!("Codewhale computer use — setup");
    println!();

    // 1. Bundle.
    match bundle::user_plugins_dir() {
        Some(root) => match install_bundle(&root, force) {
            Ok(report) => {
                let verb = match report.outcome {
                    WriteOutcome::Installed => "installed",
                    WriteOutcome::UpToDate => "already up to date",
                    WriteOutcome::Updated => "updated",
                };
                println!(
                    "{} plugin bundle {} at {} (v{})",
                    check(true),
                    verb,
                    report.bundle_dir.display(),
                    bundle::version()
                );
            }
            Err(e) => {
                failed = true;
                println!("{} plugin bundle: {e}", check(false));
            }
        },
        None => {
            failed = true;
            println!(
                "{} plugin bundle: cannot resolve the Codewhale home directory (set CODEWHALE_HOME or HOME)",
                check(false)
            );
        }
    }

    // 2. Permissions / helpers.
    #[cfg(target_os = "macos")]
    {
        let (ax, screen) = if skip_permissions {
            crate::drivers::macos::permission_status()
        } else {
            crate::drivers::macos::request_permissions()
        };
        println!(
            "{} accessibility permission{}",
            check(ax),
            if ax {
                ""
            } else {
                " — System Settings → Privacy & Security → Accessibility → enable your terminal app"
            }
        );
        println!(
            "{} screen recording permission{}",
            check(screen),
            if screen {
                ""
            } else {
                " — System Settings → Privacy & Security → Screen Recording → enable your terminal app, then restart it"
            }
        );
        if !(ax && screen) && !skip_permissions {
            println!(
                "  macOS has added your terminal app to those lists; flip the toggles, restart the terminal, and rerun `codewhale computer-use setup` to confirm."
            );
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = skip_permissions;
    }

    // 3. Per-app consent lists.
    let host_terminal = cfg.apps.host_terminal.clone();
    match crate::config::default_config_path() {
        Some(path) => match ensure_apps_section(&path, host_terminal.as_deref()) {
            Ok(line) => println!("{} {line}", check(true)),
            Err(e) => {
                failed = true;
                println!("{} consent lists: {e}", check(false));
            }
        },
        None => println!(
            "{} consent lists: cannot resolve the config path (set CODEWHALE_HOME or HOME); add [apps] allow = [\"…\"] by hand",
            check(false)
        ),
    }
    if let Some(terminal) = &host_terminal {
        println!(
            "  excluded: {terminal} (hosts Codewhale), Codewhale itself, security/login/System Settings"
        );
    }

    // 4. Test capture + diagnostics.
    match crate::drivers::select_driver(&cfg) {
        Ok(driver) => {
            let mut session = Session::new(driver, cfg);
            let info = session.call("computer_info", &serde_json::Value::Null);
            for line in info.text.lines() {
                println!("  {line}");
            }
            let shot = session.call("computer_screenshot", &serde_json::Value::Null);
            if shot.is_error {
                failed = true;
                println!(
                    "{} test screenshot: {}",
                    check(false),
                    shot.text.lines().next().unwrap_or("")
                );
            } else {
                println!(
                    "{} test screenshot: {} ({} bytes PNG)",
                    check(true),
                    shot.text.lines().next().unwrap_or(""),
                    shot.image_png.as_ref().map_or(0, Vec::len)
                );
            }
        }
        Err(e) => {
            failed = true;
            println!("{} driver: {e}", check(false));
        }
    }

    // 5. Next steps.
    println!();
    println!("Next, inside a Codewhale session:");
    println!("  /plugin reload");
    println!("  /plugin enable computer-use      # shows the review and the exact trust command");
    println!("  /plugin trust computer-use <token printed by the review>");
    println!("  /plugin enable computer-use");
    println!("  /model flash-vision              # deepseek-v4-flash-vision-exp");
    println!("  /computer open the calculator and add 2 and 2");
    println!();
    println!(
        "Config: ~/.codewhale/computer-use.toml (target = \"android\" | \"harmony\", mode = \"observe\", …)"
    );
    println!("Docs:   https://github.com/Hmbown/CodeWhale/blob/main/docs/COMPUTER_USE.md");
    if failed { 1 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_schema_v2_with_builtin_spec() {
        let marker: serde_json::Value = serde_json::from_str(&marker_json()).unwrap();
        assert_eq!(marker["schema_version"], 2);
        assert_eq!(marker["spec"], INSTALL_SPEC);
        assert_eq!(marker["installed_name"], BUNDLE_NAME);
        assert_eq!(marker["source_checksum"], marker["content_digest"]);
        assert_eq!(content_digest().len(), 64);
    }

    #[test]
    fn apps_section_is_seeded_once_and_never_grants_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("computer-use.toml");

        // No file yet: setup writes one whose consent lists are empty.
        let first = ensure_apps_section(&path, Some("WezTerm")).unwrap();
        assert!(first.contains("wrote"), "{first}");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("[apps]"));
        assert!(body.contains("allow = []"));
        assert!(body.contains("deny = []"));
        assert!(
            body.contains("WezTerm"),
            "the host terminal is named as excluded"
        );
        // It must parse, and it must grant nothing.
        let mut cfg = Config::default();
        cfg.apply_toml(&body).expect("seeded config parses");
        assert!(cfg.apps.allow.is_empty());
        assert!(cfg.apps.deny.is_empty());

        // Running setup again leaves the operator's edits alone.
        std::fs::write(
            &path,
            body.replace("allow = []", "allow = [\"com.apple.Notes\"]"),
        )
        .unwrap();
        let second = ensure_apps_section(&path, Some("WezTerm")).unwrap();
        assert!(second.contains("already configured"), "{second}");
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains("allow = [\"com.apple.Notes\"]"));
        assert_eq!(after.matches("[apps]").count(), 1);
    }

    #[test]
    fn an_existing_config_without_apps_gains_the_block() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("computer-use.toml");
        std::fs::write(&path, "target = \"android\"").unwrap();
        assert!(needs_apps_section("target = \"android\""));
        ensure_apps_section(&path, None).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        let mut cfg = Config::default();
        cfg.apply_toml(&body).expect("appended config parses");
        assert_eq!(cfg.target, crate::config::Target::Android);
        assert!(!needs_apps_section(&body));
    }

    #[test]
    fn install_bundle_writes_marker_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("plugins");
        let first = install_bundle(&root, false).unwrap();
        assert_eq!(first.outcome, WriteOutcome::Installed);
        assert!(first.bundle_dir.join("plugin.json").is_file());
        let marker = std::fs::read_to_string(first.bundle_dir.join(INSTALLED_FROM_MARKER)).unwrap();
        assert!(marker.contains("builtin:computer-use"));
        assert_eq!(
            install_bundle(&root, false).unwrap().outcome,
            WriteOutcome::UpToDate
        );
    }
}
