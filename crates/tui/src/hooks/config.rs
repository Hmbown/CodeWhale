use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::{Path, PathBuf};

/// Project hook files are executable configuration and must not become an
/// unbounded startup allocation merely because a trusted repository supplied
/// a very large file.
const PROJECT_HOOKS_FILE_MAX_BYTES: usize = 1024 * 1024;

fn read_project_hooks_file(path: &Path) -> std::io::Result<String> {
    let file = std::fs::File::open(path)?;
    let mut contents = String::new();
    file.take((PROJECT_HOOKS_FILE_MAX_BYTES + 1) as u64)
        .read_to_string(&mut contents)?;
    if contents.len() > PROJECT_HOOKS_FILE_MAX_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "project hooks file exceeds the 1 MiB limit",
        ));
    }
    Ok(contents)
}

/// Events that can trigger hook execution
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// Triggered when a new session starts
    SessionStart,
    /// Triggered when a session ends (quit, Ctrl+C)
    SessionEnd,
    /// Triggered before a user message is sent to the LLM
    MessageSubmit,
    /// Triggered before a tool is executed
    ToolCallBefore,
    /// Triggered after a tool completes (success or failure)
    ToolCallAfter,
    /// Triggered when the user changes modes (Plan, Act, Operate)
    ModeChange,
    /// Triggered when an error occurs
    OnError,
    /// Triggered after a turn completes and post-turn state has been updated
    TurnEnd,
    /// Triggered when a sub-agent is spawned
    SubagentSpawn,
    /// Triggered when a sub-agent reaches a terminal state
    SubagentComplete,
    /// Triggered immediately before each `exec_shell` invocation. The hook's
    /// stdout is parsed as `KEY=VALUE\n` lines and merged on top of the
    /// shell command's environment — useful for ephemeral credentials,
    /// per-skill PATH adjustments, or short-lived tokens (#456). Hooks that
    /// fail or time out are logged but do *not* abort the shell call; they
    /// simply contribute no env vars.
    ShellEnv,
}

/// Every event name the runtime actually fires, in the order `/hooks events`
/// and `docs/HOOKS.md` list them. Tests assert this is exhaustive so a new
/// variant cannot ship without a documented firing point.
#[cfg(test)]
pub const ALL_HOOK_EVENTS: [HookEvent; 11] = [
    HookEvent::SessionStart,
    HookEvent::SessionEnd,
    HookEvent::TurnEnd,
    HookEvent::MessageSubmit,
    HookEvent::ToolCallBefore,
    HookEvent::ToolCallAfter,
    HookEvent::ModeChange,
    HookEvent::OnError,
    HookEvent::SubagentSpawn,
    HookEvent::SubagentComplete,
    HookEvent::ShellEnv,
];

/// How much a hook's result can change what Codewhale does next.
///
/// This is the steering allowlist. "Observer" is a statement about
/// Codewhale's control flow only — an observer hook is still an arbitrary
/// shell command and can have any external side effect it likes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookSteering {
    /// stdout/exit code can replace or block the submitted text.
    TransformsSubmittedText,
    /// stdout/exit code can allow, deny, ask, rewrite input, or add context.
    DecidesToolCall,
    /// stdout contributes `KEY=VALUE` pairs to one `exec_shell` invocation.
    ContributesShellEnv,
    /// stdout is ignored and the result cannot change Codewhale's behavior.
    Observer,
}

impl HookEvent {
    /// Get string representation for environment variable
    #[allow(dead_code)] // Used in tests and future hook dispatch
    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session_start",
            HookEvent::SessionEnd => "session_end",
            HookEvent::MessageSubmit => "message_submit",
            HookEvent::ToolCallBefore => "tool_call_before",
            HookEvent::ToolCallAfter => "tool_call_after",
            HookEvent::ModeChange => "mode_change",
            HookEvent::OnError => "on_error",
            HookEvent::TurnEnd => "turn_end",
            HookEvent::SubagentSpawn => "subagent_spawn",
            HookEvent::SubagentComplete => "subagent_complete",
            HookEvent::ShellEnv => "shell_env",
        }
    }

    /// The steering contract for this event, as implemented.
    #[must_use]
    pub fn steering(self) -> HookSteering {
        match self {
            HookEvent::MessageSubmit => HookSteering::TransformsSubmittedText,
            HookEvent::ToolCallBefore => HookSteering::DecidesToolCall,
            HookEvent::ShellEnv => HookSteering::ContributesShellEnv,
            HookEvent::SessionStart
            | HookEvent::SessionEnd
            | HookEvent::ToolCallAfter
            | HookEvent::ModeChange
            | HookEvent::OnError
            | HookEvent::TurnEnd
            | HookEvent::SubagentSpawn
            | HookEvent::SubagentComplete => HookSteering::Observer,
        }
    }

    /// Whether a hook result for this event can change Codewhale's own
    /// behavior. Never read this as "side-effect free" — see [`HookSteering`].
    #[must_use]
    pub fn can_steer(self) -> bool {
        !matches!(self.steering(), HookSteering::Observer)
    }

    /// Whether this event's context carries a tool name/arguments, so
    /// `tool_name` / `tool_category` conditions can ever match.
    ///
    /// `on_error` is included because the tool-failure path in
    /// `tui/tool_routing.rs` fires it with the tool name, call id, and result
    /// attached. An `on_error` firing that has no tool behind it (a transport
    /// or capacity error) simply does not match a tool predicate — it is
    /// skipped at dispatch, not rejected at load.
    #[must_use]
    pub fn provides_tool_identity(self) -> bool {
        matches!(
            self,
            HookEvent::ToolCallBefore
                | HookEvent::ToolCallAfter
                | HookEvent::ShellEnv
                | HookEvent::OnError
        )
    }

    /// Whether this event's context can carry a real process exit code, so
    /// `exit_code` conditions can ever match. `tool_call_after` observes every
    /// completed tool and `on_error` observes the failing ones; in both cases
    /// the code is only present when the tool actually reported one
    /// (`exec_shell` and friends).
    #[must_use]
    pub fn provides_exit_code(self) -> bool {
        matches!(self, HookEvent::ToolCallAfter | HookEvent::OnError)
    }

    /// Whether this event's context carries a mode label, so `mode`
    /// conditions can ever match. `shell_env` fires inside the `exec_shell`
    /// tool with a deliberately narrow context and has no mode.
    #[must_use]
    pub fn provides_mode(self) -> bool {
        !matches!(self, HookEvent::ShellEnv)
    }

    /// Whether `background = true` is honored as actual scheduling for this
    /// event. Events whose result is part of the contract are always run in
    /// the foreground, so declaring them background is a config error rather
    /// than a scheduling choice.
    #[must_use]
    pub fn honors_background(self) -> bool {
        !matches!(self, HookEvent::ShellEnv)
    }
}

/// Condition for when a hook should run
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[derive(Default)]
pub enum HookCondition {
    /// Always run this hook
    #[default]
    Always,
    /// Only run for specific tool names
    ToolName {
        /// Tool name to match (e.g., "`exec_shell`", "`write_file`")
        name: String,
    },
    /// Only run for specific tool categories
    ToolCategory {
        /// Category: "safe", "`file_write`", "shell"
        category: String,
    },
    /// Only run in specific modes
    Mode {
        /// Mode: "plan", "agent", "yolo"
        mode: String,
    },
    /// Only run when exit code matches (for `ToolCallAfter` / `OnError`)
    ExitCode {
        /// Exit code to match.
        ///
        /// `i64`, not `i32`: a Windows crash code such as `3221225477`
        /// (`0xC0000005`, access violation) is a real code a shell tool
        /// reports, and narrowing it would silently turn the predicate into
        /// one that can never match.
        code: i64,
    },
    /// Combine multiple conditions with AND
    All { conditions: Vec<HookCondition> },
    /// Combine multiple conditions with OR
    Any { conditions: Vec<HookCondition> },
}

/// A single hook definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hook {
    /// The event that triggers this hook
    pub event: HookEvent,

    /// Shell command to execute (platform shell: `sh -c` on Unix, `cmd /C` on Windows)
    pub command: String,

    /// Optional condition for when this hook should run
    #[serde(default)]
    pub condition: Option<HookCondition>,

    /// Timeout in seconds (default: 30)
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,

    /// Run in background (don't wait for completion)
    #[serde(default)]
    pub background: bool,

    /// Continue if this hook fails (default: true)
    #[serde(default = "default_continue_on_error")]
    pub continue_on_error: bool,

    /// Optional name for logging/debugging
    #[serde(default)]
    pub name: Option<String>,

    /// Content- and generation-bound authority for a plugin-contributed hook.
    /// Never accepted from TOML; only the reviewed staged adapter may attach
    /// it after parsing immutable bytes.
    #[serde(skip)]
    pub plugin_authority: Option<crate::plugins::types::PluginAuthority>,
}

fn default_timeout() -> u64 {
    30
}

fn default_continue_on_error() -> bool {
    true
}

impl Hook {
    /// Create a new hook with minimal configuration
    #[allow(dead_code)] // Public builder API, used in tests
    pub fn new(event: HookEvent, command: &str) -> Self {
        Self {
            event,
            command: command.to_string(),
            condition: None,
            timeout_secs: 30,
            background: false,
            continue_on_error: true,
            name: None,
            plugin_authority: None,
        }
    }

    /// Builder: set condition
    #[allow(dead_code)] // Public builder API, used in tests
    pub fn with_condition(mut self, condition: HookCondition) -> Self {
        self.condition = Some(condition);
        self
    }

    /// Builder: set timeout
    #[allow(dead_code)] // Public builder API, used in tests
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Builder: run in background
    #[allow(dead_code)] // Public builder API, used in tests
    pub fn background(mut self) -> Self {
        self.background = true;
        self
    }

    /// Builder: set name
    #[allow(dead_code)] // Public builder API, used in tests
    pub fn with_name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }
}

/// A configured hook that can never behave the way it is written.
///
/// Reported by [`HooksConfig::validate`] and, for rejections, surfaced in
/// `/hooks list` so a broken hook is visible instead of silently inert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookConfigProblem {
    /// Hook `name`, or `None` for an unnamed entry.
    pub name: Option<String>,
    /// The event the hook is registered for, or `None` when the problem is
    /// with a setting in the `[hooks]` table itself rather than with one
    /// entry — `default_timeout_secs` governs every hook, so pinning its
    /// rejection on an arbitrary hook would misreport the blast radius.
    pub event: Option<HookEvent>,
    /// What is wrong, in one line, with no paths or payload content.
    pub detail: String,
    /// `true` when the hook is dropped at load and will never run.
    pub rejected: bool,
}

impl HookConfigProblem {
    /// Stable, redaction-safe one-line rendering for logs and `/hooks`.
    #[must_use]
    pub fn summary(&self) -> String {
        let disposition = if self.rejected { "rejected" } else { "warning" };
        let Some(event) = self.event else {
            return format!("{disposition}: `[hooks]` setting — {}", self.detail);
        };
        // The name is operator-supplied and lands in `/hooks list` and the
        // tracing stream; bound it and strip control characters here rather
        // than trusting every caller to remember.
        let label = super::executor::sanitize_hook_label(self.name.as_deref());
        format!(
            "{disposition}: `{}` hook `{label}` — {}",
            event.as_str(),
            self.detail
        )
    }
}

/// Configuration for hooks (loaded from config.toml)
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HooksConfig {
    /// List of hooks to execute
    #[serde(default)]
    pub hooks: Vec<Hook>,

    /// Global enable/disable for all hooks
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// Global timeout override. When set this **replaces** every hook's own
    /// `timeout_secs` rather than only filling in for hooks that omit one —
    /// see `HookExecutor::execute_sync_inner`. Documented as-implemented in
    /// `docs/HOOKS.md`; leave unset for per-hook timeouts.
    #[serde(default)]
    pub default_timeout_secs: Option<u64>,

    /// Working directory for hook execution (default: workspace)
    #[serde(default)]
    pub working_dir: Option<PathBuf>,

    /// Problems found by [`HooksConfig::validate`] at load time. Never read
    /// from or written to `config.toml`; populated by
    /// [`HooksConfig::load_with_project`] so `/hooks` can show a rejected
    /// hook instead of leaving it silently inert.
    #[serde(skip)]
    pub problems: Vec<HookConfigProblem>,
}

fn default_enabled() -> bool {
    true
}

impl HooksConfig {
    /// Load global hooks merged with project-local `.codewhale/hooks.toml` (#3026).
    ///
    /// Project hooks are executable repository configuration, so they are only
    /// honored after the workspace has been trusted in user-owned config.
    /// Trusted project hooks are appended after global hooks.  A malformed
    /// trusted project file logs a warning and falls back to global-only.
    pub fn load_with_project(global: HooksConfig, workspace: &Path) -> HooksConfig {
        Self::load_with_project_and_plugins(global, workspace, None)
    }

    /// Merge global, reviewed plugin, then trusted project hooks.
    ///
    /// Project hooks intentionally remain last because that is the existing
    /// tie-breaking contract for mutable `message_submit` transformations.
    /// Plugin files are read only from Codewhale's immutable staged snapshot;
    /// their attached authority is rechecked at every process-spawn boundary.
    pub fn load_with_project_and_plugins(
        global: HooksConfig,
        workspace: &Path,
        plugins: Option<&crate::plugins::PluginRegistry>,
    ) -> HooksConfig {
        let mut merged = global;
        if let Some(plugins) = plugins {
            let (sources, adapter_errors) = crate::plugins::runtime::active_component_sources(
                plugins,
                crate::plugins::activation::PluginActivationCapability::Hooks,
            );
            for error in adapter_errors {
                merged.problems.push(HookConfigProblem {
                    name: None,
                    event: None,
                    detail: error,
                    rejected: true,
                });
            }
            for source in sources {
                match load_plugin_hook_component(&source.path, &source.authority) {
                    Ok(mut plugin) => {
                        merged.problems.append(&mut plugin.problems);
                        merged.hooks.append(&mut plugin.hooks);
                    }
                    Err(error) => merged.problems.push(HookConfigProblem {
                        name: Some(source.plugin_name),
                        event: None,
                        detail: error,
                        rejected: true,
                    }),
                }
            }
        }
        let project_path = workspace.join(".codewhale").join("hooks.toml");
        if project_path.exists() && workspace_allows_project_hooks(workspace) {
            match read_project_hooks_file(&project_path) {
                Ok(contents) => match toml::from_str::<HooksConfig>(&contents) {
                    Ok(project) => merged.hooks.extend(project.hooks),
                    Err(e) => tracing::warn!(
                        "Failed to parse project hooks at {}: {e}; falling back to global hooks only",
                        project_path.display()
                    ),
                },
                Err(e) => tracing::warn!(
                    "Failed to read project hooks at {}: {e}; falling back to global hooks only",
                    project_path.display()
                ),
            }
        }
        // Validation runs on every path, not just the project-hooks path, so a
        // globally-configured hook that can never match is rejected too.
        merged.apply_validation();
        merged
    }

    /// Report every configured hook that cannot behave as written.
    ///
    /// A condition that references context the event never carries can never
    /// match, so a hook wearing one is inert — the dangerous version of that
    /// is a `deny` gate the operator believes is armed. Those are reported as
    /// `rejected` and dropped by [`Self::apply_validation`] rather than left
    /// to fail silently at dispatch time. Problems that only affect how a
    /// hook is scheduled are reported as warnings and the hook still runs.
    #[must_use]
    pub fn validate(&self) -> Vec<HookConfigProblem> {
        self.validate_settings()
            .into_iter()
            .chain(self.validate_indexed().into_iter().map(|(_, p)| p))
            .collect()
    }

    /// Problems with the `[hooks]` table itself, independent of any entry.
    ///
    /// `default_timeout_secs = 0` is the one that matters: it *replaces* every
    /// hook's own `timeout_secs`, so the per-hook `timeout_secs = 0` rejection
    /// does nothing to stop one line from killing every hook in the config
    /// before it can produce output — including a `tool_call_before` gate,
    /// which then fails closed on every tool call.
    fn validate_settings(&self) -> Vec<HookConfigProblem> {
        let mut problems = Vec::new();
        if self.default_timeout_secs == Some(0) {
            problems.push(HookConfigProblem {
                name: None,
                event: None,
                detail: "`default_timeout_secs = 0` would expire every hook immediately; \
                         the override is ignored and per-hook `timeout_secs` applies"
                    .to_string(),
                rejected: true,
            });
        }
        problems
    }

    /// [`Self::validate`], but each problem is paired with the index of the
    /// entry that produced it.
    ///
    /// The index is the hook's identity for rejection purposes. Keying on
    /// `(name, event)` instead would make one invalid unnamed `session_start`
    /// entry delete *every* unnamed `session_start` entry, and one invalid
    /// `gate` delete every other hook also called `gate` — innocent hooks
    /// dropped because they share a label with a broken one.
    fn validate_indexed(&self) -> Vec<(usize, HookConfigProblem)> {
        let mut problems = Vec::new();
        for (index, hook) in self.hooks.iter().enumerate() {
            let mut condition_rejections = Vec::new();
            collect_condition_problems(hook.event, hook.condition.as_ref(), &mut |detail| {
                condition_rejections.push(detail);
            });

            let mut push = |detail: String, rejected: bool| {
                problems.push((
                    index,
                    HookConfigProblem {
                        name: hook.name.clone(),
                        event: Some(hook.event),
                        detail,
                        rejected,
                    },
                ));
            };
            // A condition that can never match makes the hook inert, so it is
            // dropped; the rest only affect how the hook is scheduled, so the
            // hook still runs and the problem is a warning.
            for detail in condition_rejections {
                push(detail, true);
            }

            if hook.background && !hook.event.honors_background() {
                push(
                    format!(
                        "`background = true` is not honored for `{}`; its stdout is the \
                         contract, so it always runs in the foreground",
                        hook.event.as_str()
                    ),
                    false,
                );
            } else if hook.background && hook.event.can_steer() {
                push(
                    format!(
                        "`background = true` makes this `{}` hook observer-only — it is \
                         submitted and never awaited, so it cannot steer the turn",
                        hook.event.as_str()
                    ),
                    false,
                );
            }

            if hook.timeout_secs == 0 {
                push(
                    "`timeout_secs = 0` expires immediately; the command is killed \
                     before it can produce output"
                        .to_string(),
                    true,
                );
            }

            if hook.command.trim().is_empty() {
                push("`command` is empty".to_string(), true);
            }
        }
        problems
    }

    /// Run [`Self::validate_indexed`], drop every rejected hook, and record the
    /// problems so `/hooks` and the logs can show them.
    ///
    /// Rejection is by position, so a broken entry never takes an innocent one
    /// with it just because the two share a name (or share the absence of one).
    fn apply_validation(&mut self) {
        let inherited_problems = std::mem::take(&mut self.problems);
        let setting_problems = self.validate_settings();
        // Reject the value, not just report it: the executor reads
        // `default_timeout_secs` directly, so leaving `Some(0)` in place would
        // make the warning cosmetic.
        if setting_problems.iter().any(|p| p.rejected) {
            self.default_timeout_secs = self.default_timeout_secs.filter(|secs| *secs > 0);
        }
        let problems = self.validate_indexed();
        for problem in setting_problems
            .iter()
            .chain(problems.iter().map(|(_, p)| p))
        {
            tracing::warn!(target: "hooks", "{}", problem.summary());
        }
        let rejected: std::collections::HashSet<usize> = problems
            .iter()
            .filter(|(_, problem)| problem.rejected)
            .map(|(index, _)| *index)
            .collect();
        if !rejected.is_empty() {
            let mut index = 0;
            self.hooks.retain(|_| {
                let keep = !rejected.contains(&index);
                index += 1;
                keep
            });
        }
        self.problems = inherited_problems
            .into_iter()
            .chain(setting_problems)
            .chain(problems.into_iter().map(|(_, problem)| problem))
            .collect();
    }

    /// Get hooks for a specific event
    pub fn hooks_for_event(&self, event: HookEvent) -> Vec<&Hook> {
        if !self.enabled {
            return Vec::new();
        }
        self.hooks.iter().filter(|h| h.event == event).collect()
    }

    /// The timeout the runtime will actually apply to `hook`.
    ///
    /// `[hooks].default_timeout_secs` *replaces* the per-hook value when set.
    /// This is the single owner of that rule: the executor enforces it and
    /// `/hooks list` displays it, so the listing cannot advertise a per-hook
    /// number the runtime will not use.
    #[must_use]
    pub fn effective_timeout_secs(&self, hook: &Hook) -> u64 {
        // `filter`, not `unwrap_or`: `apply_validation` already strips a zero
        // override at load, but a `HooksConfig` can also be built in code, and
        // a zero here means "kill every hook before it speaks".
        self.default_timeout_secs
            .filter(|secs| *secs > 0)
            .unwrap_or(hook.timeout_secs)
    }

    /// `true` when `[hooks].default_timeout_secs` is overriding per-hook
    /// timeouts, so surfaces can name the provenance of the number they show.
    #[must_use]
    pub fn timeout_is_overridden(&self) -> bool {
        // An ignored zero override is not provenance: `/hooks list` must not
        // credit a number to a setting the runtime refused to apply.
        self.default_timeout_secs.is_some_and(|secs| secs > 0)
    }
}

fn load_plugin_hook_component(
    component: &Path,
    authority: &crate::plugins::types::PluginAuthority,
) -> Result<HooksConfig, String> {
    let mut paths = if component.is_file() {
        vec![component.to_path_buf()]
    } else if component.is_dir() {
        let mut paths = std::fs::read_dir(component)
            .map_err(|error| format!("failed to read plugin Hooks component: {error}"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("toml"))
            .collect::<Vec<_>>();
        paths.sort();
        paths
    } else {
        return Err("plugin Hooks component is unavailable".to_string());
    };
    if paths.is_empty() {
        return Err("plugin Hooks component contains no TOML configuration".to_string());
    }

    let mut merged = HooksConfig::default();
    for path in paths.drain(..) {
        let contents = read_project_hooks_file(&path)
            .map_err(|error| format!("failed to read plugin Hooks file: {error}"))?;
        let mut parsed: HooksConfig = toml::from_str(&contents)
            .map_err(|error| format!("failed to parse plugin Hooks file: {error}"))?;
        if parsed.working_dir.is_some() {
            return Err(
                "plugin Hooks may not set working_dir; hooks run in the active workspace"
                    .to_string(),
            );
        }
        parsed.apply_validation();
        if !parsed.enabled {
            continue;
        }
        if let Some(timeout) = parsed.default_timeout_secs.filter(|value| *value > 0) {
            for hook in &mut parsed.hooks {
                hook.timeout_secs = timeout;
            }
        }
        for hook in &mut parsed.hooks {
            hook.plugin_authority = Some(authority.clone());
        }
        merged.problems.append(&mut parsed.problems);
        merged.hooks.append(&mut parsed.hooks);
    }
    Ok(merged)
}

fn workspace_allows_project_hooks(workspace: &Path) -> bool {
    crate::config::is_workspace_trusted(workspace)
}

/// Walk a condition tree and report every predicate the event can never
/// satisfy. `all` / `any` are walked so a nested unsupported predicate is
/// caught rather than hidden behind a combinator.
fn collect_condition_problems(
    event: HookEvent,
    condition: Option<&HookCondition>,
    reject: &mut impl FnMut(String),
) {
    let Some(condition) = condition else {
        return;
    };
    match condition {
        HookCondition::Always => {}
        HookCondition::ToolName { .. } | HookCondition::ToolCategory { .. } => {
            if !event.provides_tool_identity() {
                reject(format!(
                    "`{}` never carries a tool name, so this tool condition can never match",
                    event.as_str()
                ));
            }
        }
        HookCondition::Mode { .. } => {
            if !event.provides_mode() {
                reject(format!(
                    "`{}` runs with a narrow context that has no mode, so a `mode` \
                     condition can never match; scope it with `tool_name` or \
                     `tool_category` instead",
                    event.as_str()
                ));
            }
        }
        HookCondition::ExitCode { .. } => {
            if !event.provides_exit_code() {
                reject(format!(
                    "`{}` has no completed process to read an exit code from; \
                     `exit_code` conditions are only supported on `tool_call_after` \
                     and `on_error`",
                    event.as_str()
                ));
            }
        }
        HookCondition::All { conditions } | HookCondition::Any { conditions } => {
            for nested in conditions {
                // Reborrow rather than pass `reject` itself: `&mut F` also
                // implements `FnMut`, so passing it directly would recurse in
                // the type parameter and never finish monomorphizing.
                collect_condition_problems(event, Some(nested), &mut *reject);
            }
        }
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    /// The eleven event names are a public contract: they appear in
    /// `config.toml`, in `/hooks events`, and in `docs/HOOKS.md`. A rename is
    /// a breaking change, and a new variant must be added deliberately.
    #[test]
    fn all_eleven_event_names_are_stable_and_exhaustive() {
        let names: Vec<&str> = ALL_HOOK_EVENTS.iter().map(|e| e.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "session_start",
                "session_end",
                "turn_end",
                "message_submit",
                "tool_call_before",
                "tool_call_after",
                "mode_change",
                "on_error",
                "subagent_spawn",
                "subagent_complete",
                "shell_env",
            ]
        );

        // Exhaustiveness: every variant appears exactly once. The `match` here
        // fails to compile if a variant is added without updating the list.
        for event in ALL_HOOK_EVENTS {
            let covered = match event {
                HookEvent::SessionStart
                | HookEvent::SessionEnd
                | HookEvent::TurnEnd
                | HookEvent::MessageSubmit
                | HookEvent::ToolCallBefore
                | HookEvent::ToolCallAfter
                | HookEvent::ModeChange
                | HookEvent::OnError
                | HookEvent::SubagentSpawn
                | HookEvent::SubagentComplete
                | HookEvent::ShellEnv => true,
            };
            assert!(covered);
        }
        let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
        assert_eq!(unique.len(), 11);
    }

    /// Serde round-trip for every event name, in the exact `event = "..."`
    /// spelling users write in `config.toml`.
    #[test]
    fn every_event_name_round_trips_through_serde() {
        for event in ALL_HOOK_EVENTS {
            let json = serde_json::to_string(&event).expect("serialize");
            assert_eq!(json, format!("\"{}\"", event.as_str()));
            let parsed: HookEvent = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(parsed, event);

            let toml_src = format!("event = \"{}\"\ncommand = \"true\"\n", event.as_str());
            let hook: Hook = toml::from_str(&toml_src).expect("hook parses from minimal toml");
            assert_eq!(hook.event, event);
        }
    }

    /// Backward compatibility: a pre-existing hook table with only the two
    /// required keys still parses, and the defaults are the documented ones.
    #[test]
    fn minimal_hook_toml_keeps_its_documented_defaults() {
        let hook: Hook = toml::from_str(
            r#"
event = "session_start"
command = "echo hi"
"#,
        )
        .expect("parse");
        assert_eq!(hook.timeout_secs, 30);
        assert!(!hook.background);
        assert!(hook.continue_on_error);
        assert!(hook.condition.is_none());
        assert!(hook.name.is_none());
    }

    /// `problems` is runtime-only state. It must never appear in a serialized
    /// config, and its absence must not break deserialization.
    #[test]
    fn config_problems_are_not_part_of_the_serialized_config() {
        let config = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::SessionStart, "true")],
            problems: vec![HookConfigProblem {
                name: Some("x".to_string()),
                event: Some(HookEvent::SessionStart),
                detail: "example".to_string(),
                rejected: true,
            }],
            ..HooksConfig::default()
        };
        let serialized = serde_json::to_string(&config).expect("serialize");
        assert!(!serialized.contains("problems"), "{serialized}");
        assert!(!serialized.contains("example"), "{serialized}");

        let reparsed: HooksConfig = serde_json::from_str(&serialized).expect("reparse");
        assert!(reparsed.problems.is_empty());
        assert_eq!(reparsed.hooks.len(), 1);

        // And a config that predates the field still deserializes.
        let legacy: HooksConfig = toml::from_str(
            r#"
enabled = true

[[hooks]]
event = "session_start"
command = "echo hi"
"#,
        )
        .expect("legacy config parses");
        assert!(legacy.problems.is_empty());
        assert_eq!(legacy.hooks.len(), 1);
    }

    /// The steering allowlist. Exactly three events can change what Codewhale
    /// does; every other event defaults to observer.
    #[test]
    fn steering_allowlist_is_exactly_three_events() {
        let steering: Vec<&str> = ALL_HOOK_EVENTS
            .iter()
            .filter(|e| e.can_steer())
            .map(|e| e.as_str())
            .collect();
        assert_eq!(
            steering,
            vec!["message_submit", "tool_call_before", "shell_env"]
        );

        assert_eq!(
            HookEvent::MessageSubmit.steering(),
            HookSteering::TransformsSubmittedText
        );
        assert_eq!(
            HookEvent::ToolCallBefore.steering(),
            HookSteering::DecidesToolCall
        );
        assert_eq!(
            HookEvent::ShellEnv.steering(),
            HookSteering::ContributesShellEnv
        );

        for event in ALL_HOOK_EVENTS {
            if !steering.contains(&event.as_str()) {
                assert_eq!(
                    event.steering(),
                    HookSteering::Observer,
                    "`{}` must default to observer",
                    event.as_str()
                );
            }
        }
    }

    /// Observer-only is a claim about Codewhale's control flow, not about the
    /// command. This test exists so the distinction is written down somewhere
    /// executable: an observer hook is still an arbitrary shell command and
    /// its external side effects are entirely real.
    #[test]
    fn observer_only_still_runs_a_real_command_with_real_side_effects() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("observer-side-effect.txt");
        assert!(!marker.exists());

        let command = if cfg!(windows) {
            format!("echo touched> {}", marker.display())
        } else {
            format!("echo touched > {}", marker.display())
        };
        let executor = crate::hooks::HookExecutor::new(
            HooksConfig {
                enabled: true,
                hooks: vec![Hook::new(HookEvent::SessionEnd, &command).with_name("observer")],
                ..HooksConfig::default()
            },
            dir.path().to_path_buf(),
        );

        let results = executor.execute(
            HookEvent::SessionEnd,
            &crate::hooks::HookContext::new().with_session_id("sess_test"),
        );

        // Codewhale ignored the result...
        assert_eq!(results.len(), 1);
        assert_eq!(HookEvent::SessionEnd.steering(), HookSteering::Observer);
        // ...and the command still changed the filesystem.
        assert!(
            marker.exists(),
            "an observer hook is not side-effect free; it just cannot steer"
        );
    }

    #[test]
    fn exit_code_conditions_are_rejected_only_where_no_exit_code_exists() {
        for event in ALL_HOOK_EVENTS {
            let config = HooksConfig {
                enabled: true,
                hooks: vec![
                    Hook::new(event, "true")
                        .with_name("gate")
                        .with_condition(HookCondition::ExitCode { code: 1 }),
                ],
                ..HooksConfig::default()
            };
            let problems = config.validate();
            if event.provides_exit_code() {
                assert!(
                    problems.is_empty(),
                    "`{}` should accept an exit_code condition: {problems:?}",
                    event.as_str()
                );
            } else {
                assert!(
                    problems.iter().any(|p| p.rejected),
                    "`{}` must reject an exit_code condition",
                    event.as_str()
                );
            }
        }
        assert!(HookEvent::ToolCallAfter.provides_exit_code());
        // `on_error` fires for tool failures with the tool name, call id, and
        // reported exit code attached (`tui/tool_routing.rs`), so scoping an
        // `on_error` hook by tool or exit code is a supported configuration —
        // it used to be rejected at load while the runtime and docs both
        // promised those fields.
        assert!(HookEvent::OnError.provides_exit_code());
        assert!(HookEvent::OnError.provides_tool_identity());
    }

    /// A tool-scoped `on_error` hook — the shape `docs/HOOKS.md` documents and
    /// `tool_routing.rs` supplies context for — must survive load intact.
    #[test]
    fn tool_scoped_on_error_hooks_load_and_dispatch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let global = HooksConfig {
            enabled: true,
            hooks: vec![
                Hook::new(HookEvent::OnError, "notify.sh")
                    .with_name("shell-failure")
                    .with_condition(HookCondition::All {
                        conditions: vec![
                            HookCondition::ToolName {
                                name: "exec_shell".to_string(),
                            },
                            HookCondition::ExitCode { code: 127 },
                        ],
                    }),
            ],
            ..HooksConfig::default()
        };

        let loaded = HooksConfig::load_with_project(global, dir.path());

        assert_eq!(loaded.hooks.len(), 1, "{:?}", loaded.problems);
        assert!(
            loaded.problems.iter().all(|p| !p.rejected),
            "{:?}",
            loaded.problems
        );
        assert_eq!(loaded.hooks_for_event(HookEvent::OnError).len(), 1);
    }

    /// A Windows crash code such as `0xC0000005` does not fit in `i32`. The
    /// predicate has to hold it, or the hook silently never matches.
    #[test]
    fn exit_code_conditions_hold_large_windows_crash_codes() {
        let hook: Hook = toml::from_str(
            r#"
event = "tool_call_after"
command = "echo crashed"
condition = { type = "exit_code", code = 3221225477 }
"#,
        )
        .expect("large exit code parses");
        assert!(matches!(
            hook.condition,
            Some(HookCondition::ExitCode {
                code: 3_221_225_477
            })
        ));

        // Old, small values keep parsing exactly as before.
        let legacy: Hook = toml::from_str(
            r#"
event = "tool_call_after"
command = "echo failed"
condition = { type = "exit_code", code = 1 }
"#,
        )
        .expect("small exit code still parses");
        assert!(matches!(
            legacy.condition,
            Some(HookCondition::ExitCode { code: 1 })
        ));
    }

    /// Rejection is per entry. One broken hook must not delete the hooks that
    /// merely share its name — or share its lack of one.
    #[test]
    fn rejection_drops_only_the_offending_entry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let global = HooksConfig {
            enabled: true,
            hooks: vec![
                // Two unnamed `session_start` entries; only the second is
                // invalid (an `exit_code` predicate that can never match).
                Hook::new(HookEvent::SessionStart, "echo innocent-unnamed"),
                Hook::new(HookEvent::SessionStart, "echo broken-unnamed")
                    .with_condition(HookCondition::ExitCode { code: 0 }),
                // Two hooks sharing the name `gate`; only the second is empty.
                Hook::new(HookEvent::ToolCallBefore, "echo innocent-gate").with_name("gate"),
                Hook::new(HookEvent::ToolCallBefore, "   ").with_name("gate"),
            ],
            ..HooksConfig::default()
        };

        let loaded = HooksConfig::load_with_project(global, dir.path());

        let surviving: Vec<&str> = loaded.hooks.iter().map(|h| h.command.as_str()).collect();
        assert_eq!(
            surviving,
            vec!["echo innocent-unnamed", "echo innocent-gate"],
            "an invalid entry took an innocent same-identity entry with it"
        );
        assert_eq!(
            loaded.problems.iter().filter(|p| p.rejected).count(),
            2,
            "{:?}",
            loaded.problems
        );
        assert_eq!(loaded.hooks_for_event(HookEvent::SessionStart).len(), 1);
        assert_eq!(loaded.hooks_for_event(HookEvent::ToolCallBefore).len(), 1);
    }

    #[test]
    fn effective_timeout_reports_the_global_override() {
        let hook = Hook::new(HookEvent::SessionStart, "true").with_timeout(90);

        let per_hook = HooksConfig::default();
        assert_eq!(per_hook.effective_timeout_secs(&hook), 90);
        assert!(!per_hook.timeout_is_overridden());

        let overridden = HooksConfig {
            default_timeout_secs: Some(5),
            ..HooksConfig::default()
        };
        assert_eq!(overridden.effective_timeout_secs(&hook), 5);
        assert!(overridden.timeout_is_overridden());
    }

    /// `timeout_secs = 0` was rejected per hook, but the override that
    /// *replaces* every hook's value was not checked at all — so a single
    /// `default_timeout_secs = 0` killed every hook before it could speak,
    /// including `tool_call_before` gates that then fail closed on every call.
    #[test]
    fn zero_default_timeout_is_rejected_and_ignored() {
        let hook = Hook::new(HookEvent::SessionStart, "true").with_timeout(90);
        let zeroed = HooksConfig {
            enabled: true,
            hooks: vec![hook.clone()],
            default_timeout_secs: Some(0),
            ..HooksConfig::default()
        };

        let problems = zeroed.validate();
        let problem = problems
            .iter()
            .find(|p| p.detail.contains("default_timeout_secs"))
            .expect("zero override reported");
        assert!(problem.rejected, "{problem:?}");
        assert!(
            problem.event.is_none(),
            "the override is not one hook's problem: {problem:?}"
        );
        assert!(problem.summary().contains("`[hooks]` setting"));

        // Even unvalidated, the accessors refuse the value rather than hand a
        // zero budget to the executor.
        assert_eq!(zeroed.effective_timeout_secs(&hook), 90);
        assert!(!zeroed.timeout_is_overridden());
    }

    /// The load path must strip the value, not merely warn about it: the
    /// executor reads `default_timeout_secs` and the hook itself is innocent,
    /// so it has to survive.
    #[test]
    fn zero_default_timeout_is_stripped_at_load_and_the_hook_survives() {
        let dir = tempfile::tempdir().expect("tempdir");
        let global = HooksConfig {
            enabled: true,
            hooks: vec![
                Hook::new(HookEvent::SessionStart, "true")
                    .with_name("greet")
                    .with_timeout(90),
            ],
            default_timeout_secs: Some(0),
            ..HooksConfig::default()
        };

        let loaded = HooksConfig::load_with_project(global, dir.path());
        assert_eq!(loaded.default_timeout_secs, None, "override not stripped");
        assert_eq!(loaded.hooks.len(), 1, "the hook itself was not at fault");
        assert_eq!(loaded.effective_timeout_secs(&loaded.hooks[0]), 90);
        assert!(
            loaded
                .problems
                .iter()
                .any(|p| p.rejected && p.event.is_none()),
            "{:?}",
            loaded.problems
        );
    }

    /// A positive override still loads untouched — the rejection is for zero
    /// only, not a general distrust of the setting.
    #[test]
    fn positive_default_timeout_survives_load() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loaded = HooksConfig::load_with_project(
            HooksConfig {
                enabled: true,
                hooks: vec![Hook::new(HookEvent::SessionStart, "true").with_timeout(90)],
                default_timeout_secs: Some(5),
                ..HooksConfig::default()
            },
            dir.path(),
        );
        assert_eq!(loaded.default_timeout_secs, Some(5));
        assert!(loaded.timeout_is_overridden());
        assert_eq!(loaded.effective_timeout_secs(&loaded.hooks[0]), 5);
        assert!(loaded.problems.is_empty(), "{:?}", loaded.problems);
    }

    /// Names are operator text and reach `/hooks list` and the tracing stream.
    #[test]
    fn problem_summaries_bound_and_defang_the_hook_name() {
        let problem = HookConfigProblem {
            name: Some(format!("\u{1b}[2Jgate\n{}", "n".repeat(500))),
            event: Some(HookEvent::ToolCallBefore),
            detail: "example detail".to_string(),
            rejected: true,
        };
        let summary = problem.summary();
        assert!(!summary.contains('\u{1b}'), "{summary}");
        assert!(!summary.contains('\n'), "{summary}");
        assert!(summary.contains("gate"), "{summary}");
        assert!(
            summary.chars().count() < 200,
            "unbounded summary: {} chars",
            summary.chars().count()
        );
    }

    #[test]
    fn mode_conditions_are_rejected_on_shell_env_only() {
        for event in ALL_HOOK_EVENTS {
            let config = HooksConfig {
                enabled: true,
                hooks: vec![
                    Hook::new(event, "true").with_condition(HookCondition::Mode {
                        mode: "plan".to_string(),
                    }),
                ],
                ..HooksConfig::default()
            };
            let rejected = config.validate().iter().any(|p| p.rejected);
            assert_eq!(
                rejected,
                matches!(event, HookEvent::ShellEnv),
                "unexpected mode-condition disposition for `{}`",
                event.as_str()
            );
        }
    }

    #[test]
    fn tool_conditions_are_rejected_on_events_with_no_tool() {
        for event in ALL_HOOK_EVENTS {
            for condition in [
                HookCondition::ToolName {
                    name: "exec_shell".to_string(),
                },
                HookCondition::ToolCategory {
                    category: "shell".to_string(),
                },
            ] {
                let config = HooksConfig {
                    enabled: true,
                    hooks: vec![Hook::new(event, "true").with_condition(condition)],
                    ..HooksConfig::default()
                };
                let rejected = config.validate().iter().any(|p| p.rejected);
                assert_eq!(
                    rejected,
                    !event.provides_tool_identity(),
                    "unexpected tool-condition disposition for `{}`",
                    event.as_str()
                );
            }
        }
    }

    #[test]
    fn unsupported_conditions_nested_in_combinators_are_still_rejected() {
        let config = HooksConfig {
            enabled: true,
            hooks: vec![
                Hook::new(HookEvent::SessionStart, "true")
                    .with_name("sneaky")
                    .with_condition(HookCondition::Any {
                        conditions: vec![
                            HookCondition::Always,
                            HookCondition::All {
                                conditions: vec![HookCondition::ExitCode { code: 0 }],
                            },
                        ],
                    }),
            ],
            ..HooksConfig::default()
        };
        let problems = config.validate();
        assert!(
            problems.iter().any(|p| p.rejected),
            "a nested unsupported predicate must not hide behind a combinator"
        );
    }

    #[test]
    fn rejected_hooks_are_dropped_at_load_and_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let global = HooksConfig {
            enabled: true,
            hooks: vec![
                Hook::new(HookEvent::SessionStart, "echo ok").with_name("good"),
                Hook::new(HookEvent::SessionStart, "echo never")
                    .with_name("inert")
                    .with_condition(HookCondition::ExitCode { code: 0 }),
            ],
            ..HooksConfig::default()
        };

        let loaded = HooksConfig::load_with_project(global, dir.path());

        assert_eq!(
            loaded.hooks.len(),
            1,
            "the inert hook must not survive load"
        );
        assert_eq!(loaded.hooks[0].name.as_deref(), Some("good"));
        assert!(loaded.problems.iter().any(|p| p.rejected));
        // It is also invisible to dispatch, not merely to the listing.
        assert_eq!(loaded.hooks_for_event(HookEvent::SessionStart).len(), 1);
    }

    #[test]
    fn empty_command_and_zero_timeout_are_rejected() {
        let config = HooksConfig {
            enabled: true,
            hooks: vec![
                Hook::new(HookEvent::SessionStart, "   ").with_name("blank"),
                Hook::new(HookEvent::SessionEnd, "true")
                    .with_name("instant")
                    .with_timeout(0),
            ],
            ..HooksConfig::default()
        };
        let problems = config.validate();
        assert_eq!(problems.iter().filter(|p| p.rejected).count(), 2);
    }

    #[test]
    fn background_flag_truth_is_reported_per_event() {
        // `shell_env` does not honor the flag at all — that is a warning, and
        // the hook still runs.
        let shell_env = HooksConfig {
            enabled: true,
            hooks: vec![
                Hook::new(HookEvent::ShellEnv, "true")
                    .with_name("creds")
                    .background(),
            ],
            ..HooksConfig::default()
        };
        let problems = shell_env.validate();
        assert_eq!(problems.len(), 1);
        assert!(!problems[0].rejected, "the hook still runs, in foreground");
        assert!(problems[0].detail.contains("not honored"));
        assert!(!HookEvent::ShellEnv.honors_background());

        // A background steering hook is honored scheduling, but it silently
        // stops steering — worth saying out loud.
        for event in [HookEvent::MessageSubmit, HookEvent::ToolCallBefore] {
            let config = HooksConfig {
                enabled: true,
                hooks: vec![Hook::new(event, "true").with_name("gate").background()],
                ..HooksConfig::default()
            };
            let problems = config.validate();
            assert_eq!(problems.len(), 1, "{}", event.as_str());
            assert!(!problems[0].rejected);
            assert!(problems[0].detail.contains("observer-only"));
            assert!(event.honors_background());
        }

        // A background observer hook is unremarkable.
        let observer = HooksConfig {
            enabled: true,
            hooks: vec![Hook::new(HookEvent::TurnEnd, "true").background()],
            ..HooksConfig::default()
        };
        assert!(observer.validate().is_empty());
    }

    #[test]
    fn problem_summaries_carry_no_command_or_path() {
        let problem = HookConfigProblem {
            name: Some("gate".to_string()),
            event: Some(HookEvent::ToolCallBefore),
            detail: "example detail".to_string(),
            rejected: true,
        };
        let summary = problem.summary();
        assert!(summary.contains("rejected"));
        assert!(summary.contains("tool_call_before"));
        assert!(summary.contains("gate"));

        let unnamed = HookConfigProblem {
            name: None,
            rejected: false,
            ..problem
        };
        assert!(unnamed.summary().contains("(unnamed)"));
        assert!(unnamed.summary().contains("warning"));
    }

    #[test]
    fn project_hook_file_read_is_bounded_before_toml_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("hooks.toml");
        std::fs::write(&path, "x".repeat(super::PROJECT_HOOKS_FILE_MAX_BYTES + 1))
            .expect("write oversized hook config");
        let error = super::read_project_hooks_file(&path)
            .expect_err("oversized project hook config must be rejected");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("1 MiB"));
    }
}
