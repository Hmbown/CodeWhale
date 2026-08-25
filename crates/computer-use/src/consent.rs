//! Per-app consent for computer use (Codex-style, enforced server-side).
//!
//! Three verdicts shape every app-targeted call: hard exclusions always win
//! (`Excluded`), the `[apps] deny` list beats the allow list (`Denied`), the
//! allow list grants access (`Allowed`), and anything unmatched asks the
//! user (`NeedsApproval`). MCP tool approval still stands in front of every
//! call; this is the additional, durable, per-target gate.

/// Apps the agent may never target, not configurable. Security surfaces and
/// Codewhale's own processes; the terminal that hosts Codewhale is added at
/// runtime (see [`Policy::with_host_terminal`]).
pub const HARD_EXCLUDED_BUNDLE_IDS: &[&str] = &[
    "com.apple.securityagent",
    "com.apple.loginwindow",
    "com.apple.systempreferences",
    "com.apple.systemuiserver",
    "com.apple.terminal",
];

pub const HARD_EXCLUDED_PROCESS_NAMES: &[&str] = &[
    "SecurityAgent",
    "SecurityAgentService",
    "loginwindow",
    "System Preferences",
    "System Settings",
    "codewhale",
    "codew",
    "codewhale-computer-use",
];

/// One app as the consent model sees it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppIdentity {
    pub pid: u32,
    /// Localized display name, e.g. "Notes".
    pub name: String,
    /// Reverse-DNS bundle id ("com.apple.Notes"), Android package, or
    /// HarmonyOS bundle name. Empty when the platform has none.
    pub bundle_id: String,
    /// Host process name (macOS/Linux/Windows); empty on devices.
    pub process_name: String,
}

impl AppIdentity {
    /// All names this app answers to, for policy matching and messages.
    pub fn aliases(&self) -> Vec<String> {
        let mut out = Vec::new();
        for alias in [&self.bundle_id, &self.name, &self.process_name] {
            let trimmed = alias.trim();
            if !trimmed.is_empty()
                && !out
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(trimmed))
            {
                out.push(trimmed.to_string());
            }
        }
        out
    }

    /// The best human label for messages.
    pub fn label(&self) -> String {
        if !self.name.trim().is_empty() {
            self.name.trim().to_string()
        } else if !self.process_name.trim().is_empty() {
            self.process_name.trim().to_string()
        } else if !self.bundle_id.trim().is_empty() {
            self.bundle_id.trim().to_string()
        } else {
            format!("pid {}", self.pid)
        }
    }
}

/// The `[apps]` policy from `computer-use.toml`, plus the detected host
/// terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policy {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
    /// Process name / bundle id of the terminal app hosting Codewhale.
    pub host_terminal: Option<String>,
    /// Pid of the terminal app hosting Codewhale, detected at runtime.
    pub host_terminal_pid: Option<u32>,
}

impl Policy {
    /// True when the identity names a hard-excluded app or the host
    /// terminal.
    pub fn is_excluded(&self, app: &AppIdentity) -> bool {
        if let Some(pid) = self.host_terminal_pid
            && app.pid == pid
        {
            return true;
        }
        let aliases = app.aliases();
        let matches = |candidate: &str| {
            aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(candidate))
        };
        if HARD_EXCLUDED_BUNDLE_IDS.iter().any(|id| matches(id)) {
            return true;
        }
        if HARD_EXCLUDED_PROCESS_NAMES.iter().any(|name| matches(name)) {
            return true;
        }
        if let Some(terminal) = &self.host_terminal
            && !terminal.trim().is_empty()
            && matches(terminal)
        {
            return true;
        }
        false
    }

    pub fn verdict(&self, app: &AppIdentity) -> Verdict {
        if self.is_excluded(app) {
            return Verdict::Excluded;
        }
        let aliases = app.aliases();
        let listed = |list: &[String]| {
            list.iter().any(|entry| {
                let entry = entry.trim();
                !entry.is_empty() && aliases.iter().any(|a| a.eq_ignore_ascii_case(entry))
            })
        };
        if listed(&self.deny) {
            return Verdict::Denied;
        }
        if listed(&self.allow) {
            Verdict::Allowed
        } else {
            Verdict::NeedsApproval
        }
    }

    /// The `[apps]` entries currently granted for status listings.
    pub fn allow_line(&self) -> String {
        toml_list(&self.allow)
    }

    pub fn deny_line(&self) -> String {
        toml_list(&self.deny)
    }
}

/// The outcome of a consent check for one app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    NeedsApproval,
    Denied,
    Excluded,
}

/// The structured approval error the `/computer` command turns into a
/// question to the user.
pub fn needs_approval_error(app: &AppIdentity, path_hint: &str) -> String {
    let entry = if app.bundle_id.trim().is_empty() {
        app.label()
    } else {
        app.bundle_id.trim().to_string()
    };
    format!(
        "needs_app_approval: `{}` is not allowed for computer use yet.\nAsk the user; if they agree, add this line to {} under [apps]:\n  allow = [\"{}\"]\n(or deny it with deny = [\"{}\"]). Computer-use actions stay disabled for this app until then.",
        app.label(),
        path_hint,
        entry,
        entry
    )
}

pub fn excluded_error(app: &AppIdentity) -> String {
    format!(
        "`{}` can never be targeted: security, login, and system-settings surfaces and Codewhale's own processes are hard-excluded from computer use",
        app.label()
    )
}

pub fn denied_error(app: &AppIdentity) -> String {
    format!(
        "`{}` is on the deny list in [apps]; remove it from deny to target this app",
        app.label()
    )
}

fn toml_list(items: &[String]) -> String {
    if items.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            items
                .iter()
                .map(|item| format!("\"{}\"", item.trim()))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, bundle: &str) -> AppIdentity {
        AppIdentity {
            pid: 100,
            name: name.into(),
            bundle_id: bundle.into(),
            process_name: name.into(),
        }
    }

    #[test]
    fn unlisted_apps_need_approval() {
        let policy = Policy::default();
        assert_eq!(
            policy.verdict(&app("Notes", "com.apple.Notes")),
            Verdict::NeedsApproval
        );
        let message = needs_approval_error(
            &app("Notes", "com.apple.Notes"),
            "~/.codewhale/computer-use.toml",
        );
        assert!(message.starts_with("needs_app_approval:"));
        assert!(message.contains("allow = [\"com.apple.Notes\"]"));
    }

    #[test]
    fn allow_matches_bundle_name_or_process_case_insensitively() {
        let policy = Policy {
            allow: vec!["com.apple.notes".into()],
            ..Default::default()
        };
        assert_eq!(
            policy.verdict(&app("Notes", "com.apple.Notes")),
            Verdict::Allowed
        );
        let policy = Policy {
            allow: vec!["CALCULATOR".into()],
            ..Default::default()
        };
        assert_eq!(
            policy.verdict(&app("Calculator", "com.apple.calculator")),
            Verdict::Allowed
        );
    }

    #[test]
    fn deny_beats_allow() {
        let policy = Policy {
            allow: vec!["Notes".into()],
            deny: vec!["com.apple.Notes".into()],
            ..Default::default()
        };
        assert_eq!(
            policy.verdict(&app("Notes", "com.apple.Notes")),
            Verdict::Denied
        );
    }

    #[test]
    fn hard_exclusions_win_over_allow() {
        let policy = Policy {
            allow: vec!["com.apple.securityagent".into(), "System Settings".into()],
            ..Default::default()
        };
        assert_eq!(
            policy.verdict(&app("SecurityAgent", "com.apple.securityagent")),
            Verdict::Excluded
        );
        assert_eq!(
            policy.verdict(&app("System Settings", "com.apple.systemsettings")),
            Verdict::Excluded
        );
        assert_eq!(
            policy.verdict(&app("Codewhale", "com.codewhale.cli")),
            Verdict::Excluded
        );
    }

    #[test]
    fn host_terminal_is_excluded() {
        let policy = Policy {
            host_terminal: Some("WezTerm".into()),
            allow: vec!["WezTerm".into()],
            ..Default::default()
        };
        assert_eq!(
            policy.verdict(&app("WezTerm", "com.github.wez.wezterm")),
            Verdict::Excluded
        );
        assert_eq!(
            policy.verdict(&app("Notes", "com.apple.Notes")),
            Verdict::NeedsApproval
        );
    }

    #[test]
    fn host_terminal_pid_wins_even_when_renamed() {
        let policy = Policy {
            host_terminal_pid: Some(100),
            allow: vec!["Anything".into()],
            ..Default::default()
        };
        assert_eq!(
            policy.verdict(&app("Anything", "com.x.y")),
            Verdict::Excluded
        );
    }

    #[test]
    fn aliases_and_labels() {
        let app = app("Notes", "com.apple.Notes");
        assert_eq!(app.aliases().len(), 2);
        assert_eq!(app.label(), "Notes");
        let pid_only = AppIdentity {
            pid: 7,
            ..Default::default()
        };
        assert_eq!(pid_only.label(), "pid 7");
        assert!(pid_only.aliases().is_empty());
    }
}
