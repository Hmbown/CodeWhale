//! Settings system - Persistent user preferences
//!
//! Settings are stored at ~/.codewhale/settings.toml, with legacy fallbacks.
//!
//! TUI-specific preferences (theme, keybinds, font_size) that survive project
//! switches are stored separately in tui.toml. See [`TuiPrefs`].

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{expand_path, normalize_model_name};
use crate::localization::normalize_configured_locale;
use crate::palette::{normalize_hex_rgb_color, normalize_theme_setting};
use crate::tui::app::ReasoningEffort;

const SETTINGS_FILE_NAME: &str = "settings.toml";

/// Smallest Top work surface that can show its divider plus the compact
/// goal / to-do / Agent projection without turning the rail into invisible
/// keyboard state. Older releases accepted two rows, which left only one
/// content row and could hide every actionable item behind the goal title.
pub(crate) const WORK_SURFACE_TOP_HEIGHT_MIN: u16 = 5;
pub(crate) const WORK_SURFACE_TOP_HEIGHT_MAX: u16 = 16;
const TUI_PREFS_FILE_NAME: &str = "tui.toml";

/// How successful structured file mutations are represented in the live
/// transcript. Exact evidence is retained for inspection in every mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InlineDiffMode {
    /// Show a bounded red/green unified diff plus semantic change statistics.
    #[default]
    Full,
    /// Show only bounded semantic change statistics.
    Summary,
    /// Keep the calm File outcome row without any inline diff detail.
    Off,
}

impl InlineDiffMode {
    #[must_use]
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "summary" => Self::Summary,
            "off" => Self::Off,
            _ => Self::Full,
        }
    }

    #[must_use]
    pub const fn as_setting(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Summary => "summary",
            Self::Off => "off",
        }
    }
}

// ============================================================================
// TuiPrefs — ~/.codewhale/tui.toml
// ============================================================================

/// TUI-specific preferences that are decoupled from agent/project config so
/// they survive project switches (issue #437).
///
/// Stored at `~/.codewhale/tui.toml` on new installs, with
/// `~/.deepseek/tui.toml` retained as a legacy read fallback. When the file is
/// absent the values fall back to the `[tui]` section of the normal
/// `config.toml` (via [`TuiPrefs::load`]), and then to the struct's own
/// defaults.
///
/// # Example `~/.codewhale/tui.toml`
///
/// ```toml
/// theme    = "underwater"    # painted ocean field; "terminal" | "dark" | "light" | "grayscale" | ... remain available
/// font_size = 14
///
/// [keybinds]
/// submit   = "ctrl+enter"
/// new_line = "enter"
/// ```
//
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiPrefs {
    /// UI colour theme.
    /// Default `"underwater"`, the painted ocean field. `"terminal"` leaves
    /// foreground and background to the host terminal while retaining
    /// ANSI-safe semantic accents.
    pub theme: String,
    /// Terminal font size hint forwarded to supporting front-ends (e.g. the
    /// Tauri shell). `0` means "use terminal default". Default `0`.
    pub font_size: u16,
    /// Key-binding overrides. Each field accepts an xterm-style chord string
    /// such as `"ctrl+enter"`, `"alt+n"`, or `"f1"`.
    pub keybinds: KeybindPrefs,
}

impl Default for TuiPrefs {
    fn default() -> Self {
        Self {
            theme: "underwater".to_string(),
            font_size: 0,
            keybinds: KeybindPrefs::default(),
        }
    }
}

/// Per-action keybinding overrides stored inside [`TuiPrefs`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KeybindPrefs {
    /// Key to submit the current composer input to the model.
    /// Default: `"ctrl+enter"`.
    pub submit: Option<String>,
    /// Key to insert a literal newline inside the composer.
    /// Default: `"enter"`.
    pub new_line: Option<String>,
    /// Key to open the command palette.
    /// Default: `"ctrl+k"`.
    pub command_palette: Option<String>,
    /// Key to cancel / interrupt a running turn.
    /// Default: `"ctrl+c"`.
    pub cancel: Option<String>,
    /// Key to toggle the sidebar.
    /// Default: `"ctrl+b"`.
    pub toggle_sidebar: Option<String>,
}

impl TuiPrefs {
    /// Return the canonical path of the TUI preferences file:
    /// `~/.codewhale/tui.toml`, or legacy `~/.deepseek/tui.toml` when present.
    ///
    /// Tests may override the home directory through the canonical
    /// `CODEWHALE_CONFIG_PATH` environment variable. The parent directory of
    /// the pointed-to config is used instead of the default settings home.
    pub fn path() -> Result<PathBuf> {
        #[cfg(test)]
        {
            let honor_guarded_environment =
                crate::test_support::guarded_environment_provides_state_paths();
            crate::test_support::with_test_env_lock(|| {
                if honor_guarded_environment {
                    tui_prefs_path_from_environment()
                } else {
                    Ok(crate::test_support::unsealed_test_state_root().join(TUI_PREFS_FILE_NAME))
                }
            })
        }

        #[cfg(not(test))]
        tui_prefs_path_from_environment()
    }

    /// Load TUI preferences from `~/.codewhale/tui.toml` or a legacy fallback.
    ///
    /// If the file does not exist the struct defaults are returned — no error
    /// is produced. Parse errors surface as `Err` so the caller can warn the
    /// user without crashing the session.
    #[allow(dead_code)] // Startup currently only validates tui.toml parse; load is the persistence API.
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        #[cfg(test)]
        {
            crate::test_support::with_test_state_io_lock(|| Self::load_from_path(&path))
        }
        #[cfg(not(test))]
        Self::load_from_path(&path)
    }

    fn load_from_path(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read tui.toml from {}", path.display()))?;
        let prefs: TuiPrefs = match toml::from_str(&content) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Failed to parse {} (using defaults): {e:#}", path.display());
                return Ok(Self::default());
            }
        };
        Ok(prefs)
    }

    /// Save TUI preferences to `~/.codewhale/tui.toml` (or a legacy file when
    /// it already exists), creating the target directory if needed.
    #[allow(dead_code)] // Persistence API; no settings UI write path yet.
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;
        #[cfg(test)]
        {
            crate::test_support::with_test_state_io_lock(|| self.save_to_path(&path))
        }
        #[cfg(not(test))]
        self.save_to_path(&path)
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
        }
        let serialized = toml::to_string_pretty(self).context("Failed to serialize TuiPrefs")?;
        let body = if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read tui.toml at {}", path.display()))?;
            codewhale_config::merge_and_preserve_comments(&serialized, &raw).unwrap_or_else(|e| {
                tracing::warn!("failed to merge tui.toml comments, saving without them: {e:#}");
                serialized
            })
        } else {
            serialized
        };
        std::fs::write(path, body)
            .with_context(|| format!("Failed to write tui.toml to {}", path.display()))?;
        Ok(())
    }

    /// Validate field values and normalise them in place.
    ///
    /// Returns `Err` if an unrecognised `theme` value is found so callers can
    /// surface a helpful message rather than silently ignoring a typo.
    #[allow(dead_code)] // Persistence API; no settings UI write path yet.
    pub fn validate(&mut self) -> Result<()> {
        self.theme = normalize_theme_setting(&self.theme).map_err(anyhow::Error::msg)?;
        Ok(())
    }
}

fn tui_prefs_path_from_environment() -> Result<PathBuf> {
    // Honour the same env-var escape hatch used by Settings::path so that
    // integration tests can redirect all config I/O to a temp directory.
    if let Some(parent) = config_override_parent() {
        return Ok(parent.join("tui.toml"));
    }

    let primary = codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join(TUI_PREFS_FILE_NAME));
    if codewhale_config::codewhale_home_is_explicit() {
        return primary.ok_or_else(|| {
            anyhow::anyhow!("Failed to resolve tui.toml path: no Codewhale home found.")
        });
    }
    let legacy_home = codewhale_config::legacy_deepseek_home()
        .ok()
        .map(|home| home.join(TUI_PREFS_FILE_NAME));

    resolve_tui_prefs_path_from_candidates(primary, legacy_home)
}

fn resolve_tui_prefs_path_from_candidates(
    primary: Option<PathBuf>,
    legacy_home: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = primary.as_ref()
        && path.exists()
    {
        return Ok(path.clone());
    }

    if let Some(path) = legacy_home.as_ref()
        && path.exists()
    {
        return Ok(path.clone());
    }

    primary.or(legacy_home).ok_or_else(|| {
        anyhow::anyhow!("Failed to resolve tui preferences path: no home directory found.")
    })
}

/// User settings with defaults
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PinnedModel {
    /// Exact configured provider identity; labels never replace this value.
    pub provider: String,
    /// Exact provider-owned model id.
    pub model: String,
    /// Optional presentation-only label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Auto-compact conversations when they approach the model limit.
    pub auto_compact: bool,
    /// Context-window percentage that triggers pre-send auto-compaction when
    /// `auto_compact` is enabled. The hard token floor still applies.
    pub auto_compact_threshold_percent: f64,
    /// Whether the persisted settings file expressed an auto-compaction
    /// preference. Runtime defaults must not be written back as user intent
    /// when an unrelated setting is saved.
    #[serde(skip)]
    pub(crate) auto_compact_explicit: bool,
    /// Reduce status noise and collapse details more aggressively
    pub calm_mode: bool,
    /// Dense tool-run collapse mode: compact, expanded, or calm.
    pub tool_collapse_mode: String,
    /// Reduce decorative motion. This must never synthesize model text speed;
    /// streaming follows upstream deltas in both modes.
    pub low_motion: bool,
    /// Set when the persisted file existed but could not be parsed and the
    /// values above are defaults. Never serialized; surfaces must not present
    /// these defaults as saved.
    #[serde(skip)]
    pub load_error: Option<String>,
    /// Enable expressive live-state motion. This affects chrome and state
    /// affordances only; model text always follows upstream stream deltas.
    pub fancy_animations: bool,
    /// Focus-context texture prototype for modal views (#4823): `off`
    /// (default), `scrim` dims the area outside the focused modal, `grain`
    /// sprinkles deterministic dots over blank cells there. Static texture,
    /// never obscures text; unknown values fall back to `off` at render time.
    pub focus_texture: String,
    /// Ocean Tasks / To-do / Workers rail placement: top, left, or right.
    /// The lower edge remains owned by the composer and phase footer.
    pub work_surface_placement: String,
    /// Remembered total height (content plus divider) for top Work placement.
    pub work_surface_top_height: u16,
    /// Remembered total width (content plus divider) for side Work placement.
    pub work_surface_side_width: u16,
    /// Which panel the rail shows: tasks, agents, context, or pinned.
    /// Orthogonal to `work_surface_placement` (rail unification, 0.9.4).
    pub rail_panel: String,
    /// Runtime-only: whether the loaded settings document explicitly named
    /// `rail_panel`. The sidebar→rail migration must not override an
    /// explicit choice that happens to equal the default ("tasks").
    #[serde(skip)]
    pub(crate) rail_panel_explicit: bool,
    /// Runtime-only: whether the loaded settings document explicitly named
    /// `work_surface_placement`. A legacy hidden sidebar must become `off`,
    /// unless the user had already chosen a first-class rail placement.
    #[serde(skip)]
    pub(crate) work_surface_placement_explicit: bool,
    /// Runtime-only 30 FPS cap for terminals that flicker at high redraw
    /// rates. Separate from accessibility motion and text delivery.
    #[serde(skip)]
    pub constrained_frame_rate: bool,
    /// Enable terminal bracketed-paste mode. Default true. Disable if your
    /// terminal mishandles the `\e[?2004h` escape (rare; some legacy
    /// terminals over SSH+screen multiplex without the cap).
    pub bracketed_paste: bool,
    /// Enable rapid-key paste-burst detection for terminals that do not emit
    /// bracketed-paste events. Independent from `bracketed_paste`.
    pub paste_burst_detection: bool,
    /// Maximum number of file-mention popup candidates retained before the
    /// composer renders its visible window. The widget paginates by terminal
    /// height, so this is a data-side cap rather than a visible-row budget.
    pub mention_menu_limit: usize,
    /// Maximum workspace depth for `@`-mention completion walks. `0` means
    /// unlimited depth; use with care in very large repositories.
    pub mention_walk_depth: usize,
    /// `@`-mention completion behavior: fuzzy workspace search or deterministic
    /// directory browser.
    pub mention_menu_behavior: String,
    /// Show thinking blocks from the model
    pub show_thinking: bool,
    /// When true, thinking blocks render expanded by default instead of
    /// collapsed. Space still toggles collapse/expand. Useful for SSH/tmux
    /// users where the Space key may be captured by the terminal layer.
    #[serde(default)]
    pub thinking_default_expanded: bool,
    /// Collapsed completed-thought preview rows. Default 2 (compact).
    /// Set `10` for the older dump, or `0` for header-only. Full expand is
    /// still `thinking_default_expanded` / Space.
    #[serde(default = "default_thinking_preview_lines")]
    pub thinking_preview_lines: usize,
    /// Keep thinking visible while disabling its filled background treatment.
    pub thinking_highlight: bool,
    /// When true, Help/shortcuts groups start expanded. Default false folds
    /// the long tail. Type-to-filter still unfolds matches.
    #[serde(default)]
    pub help_expand_groups: bool,
    /// Pin the last user prompt at the top of the transcript when it has
    /// scrolled off. Default on.
    #[serde(default = "default_true")]
    pub pin_last_prompt: bool,
    /// Show detailed tool output
    pub show_tool_details: bool,
    /// Successful structured File mutation evidence: full, summary, or off.
    /// This affects inline presentation only; exact evidence remains available
    /// through the tool-details route in every mode.
    pub inline_diffs: String,
    /// UI locale: auto, en, ja, zh-Hans, zh-Hant, pt-BR, es-419, vi, ko,
    /// ca, de, fr, id, hi, ru, uk.
    /// Every shipped pack holds full `en.json` parity; nothing falls back.
    pub locale: String,
    /// Named UI theme. `"underwater"` is the fresh-install default and paints
    /// the ocean field. `"terminal"` fully inherits the host terminal's
    /// foreground/background. `"system"`, `"dark"`, `"light"`,
    /// `"grayscale"`, and the community presets: `"catppuccin-mocha"`,
    /// `"tokyo-night"`, `"dracula"`, `"gruvbox-dark"`. The
    /// `background_color` setting still overrides the surface color on top
    /// of the resolved theme.
    pub theme: String,
    /// Optional main TUI background color as a 6-digit hex RGB value.
    pub background_color: Option<String>,
    /// Composer layout density: compact, comfortable, spacious
    pub composer_density: String,
    /// Show a border around the composer input area
    pub composer_border: bool,
    /// Keep bare Enter available for multiline drafting. When enabled,
    /// Shift+Enter submits; Ctrl+J and Alt+Enter remain newline shortcuts.
    #[serde(default)]
    pub composer_multiline_mode: bool,
    /// Composer editing mode: "normal" (default) or "vim" for modal editing.
    /// When set to "vim" the composer starts in Normal mode; press i/a/o to
    /// enter Insert mode and Esc to return to Normal.
    pub composer_vim_mode: String,
    /// Transcript spacing rhythm: compact, comfortable, spacious
    pub transcript_spacing: String,
    /// Default mode: "agent" (Act), "plan", or "operate". Legacy permission
    /// shorthands are accepted for migration but never advertised as modes.
    pub default_mode: String,
    /// Legacy sidebar width as percentage of terminal width. Load-only
    /// migration shim (0.9.4 rail unification): read by
    /// `migrate_sidebar_settings_to_rail`, never written back.
    #[serde(skip_serializing)]
    pub sidebar_width_percent: u16,
    /// Legacy sidebar focus mode: pinned, auto, tasks, agents, context,
    /// hidden. Load-only migration shim, never written back.
    #[serde(skip_serializing)]
    pub sidebar_focus: String,
    /// Enable the session-context panel (#504). Shows working set, tokens,
    /// cost, MCP/LSP status, cycle count, and memory info.
    pub context_panel: bool,
    /// Show the persistent Sessions rail in the sidebar (#2934).
    ///
    /// Off by default: the rail spends sidebar rows that Work, Activity, and
    /// Agents already compete for, so it is opt-in rather than something a
    /// user discovers by having their layout change under them.
    #[serde(default, skip_serializing_if = "is_false")]
    pub sessions_rail: bool,
    /// Reattach to this workspace's most recent session on startup (#2934).
    ///
    /// Off by default. `--resume`/`--continue` remain the explicit paths and
    /// always take precedence; when this is on, startup still refuses to
    /// resume an archived, unreadable, or foreign-workspace session and falls
    /// back to a fresh transcript with a receipt. See
    /// [`crate::session_resume`] for the decision table.
    #[serde(default, skip_serializing_if = "is_false")]
    pub session_auto_resume: bool,
    /// Cost display currency: usd or cny.
    pub cost_currency: String,
    /// Maximum number of input history entries to save
    pub max_input_history: usize,
    /// Default provider override (e.g. "deepseek", "openai").
    pub default_provider: Option<String>,
    /// DeepSeek-only fallback model. Non-DeepSeek providers use the
    /// provider-scoped entry in [`Self::provider_models`] instead.
    pub default_model: Option<String>,
    /// Default reasoning effort selected from the TUI model picker.
    /// `None` falls back to `config.toml` and then the runtime default.
    pub reasoning_effort: Option<String>,
    /// TUI-only Shift+Tab posture: ask, auto-review, or full-access.
    /// An explicit/managed `config.toml` approval policy always takes
    /// precedence, so this preference cannot loosen project requirements.
    /// This is **tool-approval posture**, not filesystem scope — see
    /// [`Self::sandbox_mode`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_posture: Option<String>,
    /// Filesystem sandbox scope, independent of approval posture:
    /// `read-only | workspace-write | danger-full-access | external-sandbox`.
    /// Surfaced in Settings and the shell so "Full Access" (approval) is
    /// never confused with unrestricted filesystem writes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    /// Per-provider model overrides. Key is provider name (e.g. "openai"),
    /// value is the model id. Takes precedence over `default_model`.
    pub provider_models: Option<std::collections::HashMap<String, String>>,
    /// Provider-scoped model IDs intentionally enabled for the ordinary model
    /// picker. Missing on older files; current and saved provider choices are
    /// seeded at load time so the migration is additive and non-breaking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_models: Option<std::collections::HashMap<String, Vec<String>>>,
    /// Exact provider/model tuples pinned to the top of model choosers, in
    /// user-defined order. Stale entries remain persisted and visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pinned_models: Vec<PinnedModel>,
    /// Header status indicator next to the effort chip. Cycles through a
    /// per-turn animation keyed off `App::turn_started_at`:
    /// - `"cw"` (default): static typographic Codewhale mark.
    /// - `"whale"`: historical `🐳 → 🐋` 12-frame sequence
    ///   originally shipped in v0.3.5, removed in v0.8.x's "smoother TUI
    ///   streaming" pass, restored in v0.8.30. Idle frame is a steady `🐳`.
    /// - `"dots"`: the 6-frame geometric sequence (`◍ ◉ ◌ ◌ ◉ ◍`) that
    ///   replaced the whale during the dots era.
    /// - `"off"`: hide the indicator entirely.
    pub status_indicator: String,
    /// Whether to wrap each draw in DEC mode 2026 synchronized output
    /// (`\x1b[?2026h` … `\x1b[?2026l`). Synchronized output asks the
    /// terminal to defer rendering until the whole frame is staged so
    /// GPU-accelerated terminals (Ghostty, VS Code, Kitty, WezTerm)
    /// don't flash a blank intermediate frame.
    ///
    /// - `"auto"` (default): emit DEC 2026 unless an environment signal
    ///   says the active terminal mishandles it (currently Ptyxis 50.x
    ///   on VTE 0.84.x — see [`Settings::apply_env_overrides`]).
    /// - `"on"`: always emit DEC 2026 (override the auto opt-out).
    /// - `"off"`: never emit DEC 2026. Use this if your terminal flashes
    ///   the whole screen on every redraw — most often Ptyxis on
    ///   Ubuntu 26.04 today; historically also some legacy ssh+screen
    ///   stacks. The cost of `off` is brief tearing on terminals that
    ///   *do* support DEC 2026; it is purely a rendering-quality knob,
    ///   not a correctness one.
    pub synchronized_output: String,
    /// Follow symbolic links during workspace file discovery walks (`@`-mention
    /// completion, fuzzy resolve, and the file-index builder). When `false`
    /// (default) symlinked directories are skipped, which keeps walks fast and
    /// avoids accidentally traversing into system paths. Set to `true` to
    /// support symlink-based multi-project workspaces where several project
    /// directories are symlinked into a single hub directory.
    ///
    /// **Note**: The walker has built-in cycle detection that skips already-
    /// visited real paths, so symlink loops (A→B→A) will not cause infinite
    /// recursion. However, enabling this on workspaces with symlinks that
    /// point to large directory trees (e.g. `/usr`, home directories) can
    /// significantly increase first-turn latency and memory usage.
    pub workspace_follow_symlinks: bool,
    /// One-time Fleet + Hotbar introduction has been shown. Drives a single
    /// launch nudge (see `App::maybe_show_feature_intro`) so returning users
    /// see it exactly once and never on subsequent launches.
    pub feature_intro_shown: bool,
    /// One-time YOLO deprecation toast has been shown. Suppresses the repeat
    /// toast after the first sighting per install (persisted across sessions).
    pub yolo_deprecation_shown: bool,
    /// Persisted impression counts for action-triggered, ephemeral product
    /// guidance. Keys are stable tip identifiers; values are bounded by the
    /// behavioral-tip engine and omitted entirely before the first sighting.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub behavioral_tip_impressions: std::collections::BTreeMap<String, u8>,
    /// True only for the current load when `default_mode = "yolo"` was read
    /// from an older settings file. App startup uses this provenance to migrate
    /// the old bundled Full Access choice without weakening project or managed
    /// approval policy. It is never written back to disk.
    #[serde(skip)]
    pub(crate) legacy_yolo_default: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // Keep the persisted fallback `false`; startup code enables
            // auto-compaction by known model window when the user has not saved
            // an explicit preference. This preserves an explicit opt-out while
            // making long-session continuity the default runtime behavior.
            auto_compact: false,
            auto_compact_threshold_percent: 80.0,
            auto_compact_explicit: false,
            // #4095: default presentation is compact/calm; verbose detail is opt-in.
            calm_mode: true,
            tool_collapse_mode: "compact".to_string(),
            low_motion: false,
            load_error: None,
            fancy_animations: true,
            focus_texture: "off".to_string(),

            // Round 3 (2026-09-01): the bar's information lives under the
            // composer. Side rails are opt-in and fall back to the top strip
            // on narrow terminals.
            work_surface_placement: "bottom".to_string(),
            // Cap, not fixed height: the top strip auto-fits its rows and
            // only grows to this many lines (user request, 2026-07-23).
            work_surface_top_height: 8,
            work_surface_side_width: 30,
            rail_panel: "tasks".to_string(),
            rail_panel_explicit: false,
            work_surface_placement_explicit: false,
            constrained_frame_rate: false,
            bracketed_paste: true,
            paste_burst_detection: true,
            mention_menu_limit: 128,
            mention_walk_depth: 10,
            mention_menu_behavior: "fuzzy".to_string(),
            // Reasoning is useful when explicitly requested, but it should
            // never displace the actual conversation in the default TUI.
            show_thinking: false,
            thinking_default_expanded: false,
            thinking_preview_lines: default_thinking_preview_lines(),
            thinking_highlight: true,
            help_expand_groups: false,
            pin_last_prompt: true,
            show_tool_details: false,
            inline_diffs: "full".to_string(),
            locale: "auto".to_string(),
            theme: "underwater".to_string(),
            background_color: None,
            composer_density: "comfortable".to_string(),
            composer_border: true,
            composer_multiline_mode: false,
            composer_vim_mode: "normal".to_string(),
            transcript_spacing: "comfortable".to_string(),
            default_mode: "agent".to_string(),
            sidebar_width_percent: 28,
            sidebar_focus: "auto".to_string(),
            context_panel: false,
            sessions_rail: false,
            session_auto_resume: false,
            cost_currency: "usd".to_string(),
            max_input_history: 100,
            default_provider: None,
            default_model: None,
            reasoning_effort: None,
            permission_posture: None,
            sandbox_mode: None,
            provider_models: None,
            enabled_models: None,
            pinned_models: Vec::new(),
            // The whale lives in the terminal window title (OSC 0). The in-app
            // header defaults to the static typographic `cw` mark so the two
            // surfaces do not compete with a second spinner.
            status_indicator: "cw".to_string(),
            synchronized_output: "auto".to_string(),
            workspace_follow_symlinks: false,
            feature_intro_shown: false,
            yolo_deprecation_shown: false,
            behavioral_tip_impressions: std::collections::BTreeMap::new(),
            legacy_yolo_default: false,
        }
    }
}

/// The `calm` transcript preset (#3478): a coherent "beautiful/calm" bundle that
/// favors a quiet, readable transcript over debug-dense output. Presentation
/// only, and evidence-preserving — `show_thinking` is deliberately left untouched
/// (thinking stays visible) and tool runs only have their inline detail
/// collapsed, never hidden. Keyed by [`Settings::set`] names so the preset and a
/// single-key `/config` set share one validation path.
pub const CALM_PRESET_FIELDS: &[(&str, &str)] = &[
    ("calm_mode", "true"),
    ("tool_collapse", "calm"),
    ("transcript_spacing", "compact"),
    ("low_motion", "true"),
    ("fancy_animations", "false"),
    ("show_tool_details", "false"),
];

fn normalize_work_surface_placement(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "top" => "top",
        "bottom" => "bottom",
        "left" => "left",
        "right" => "right",
        "off" => "off",
        // Round 3 (2026-09-01): unknown values fall back to the product
        // default — the bar lives under the composer.
        _ => "bottom",
    }
}

fn normalize_rail_panel(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "agents" => "agents",
        "background" => "background",
        "files" => "files",
        "notepad" => "notepad",
        "context" => "context",
        "git" => "git",
        "price" => "price",
        // `pinned` folded into the tasks view (2026-09-02 dock views).
        _ => "tasks",
    }
}

/// Rail unification (0.9.4): carry the classic sidebar's settings forward
/// instead of stranding them. `sidebar_focus` picks the rail panel —
/// pinned/tasks/agents/context map onto the same-named panels, auto folds
/// into the auto-fitting Tasks panel (it is the shipped default for
/// `sidebar_focus`, and "show work when there is work" is what Tasks does;
/// folding it into the always-on Pinned strip inverted that intent for every
/// upgrading user), and hidden turns the rail off.
/// `sidebar_width_percent` maps onto the absolute side width at a
/// 120-column reference. Auto-collapse itself is deliberately dropped: the
/// rail hides via placement off. Explicit new keys win over migrated ones.
fn migrate_sidebar_settings_to_rail(s: &mut Settings) {
    match s.sidebar_focus.trim().to_ascii_lowercase().as_str() {
        "hidden" | "hide" | "closed" | "off" | "none" => {
            // A legacy hidden sidebar is an explicit intent. Preserve it even
            // now that fresh sessions prefer the responsive left rail, but do
            // not override a newer placement the user explicitly saved.
            if !s.work_surface_placement_explicit {
                s.work_surface_placement = "off".to_string();
            }
        }
        // #5141 let users pin a dedicated sessions panel in the classic
        // sidebar; on the unified rail the equivalent surface is the
        // first-class sessions rail, so carry the intent forward by
        // enabling it.
        "sessions" | "sessions_rail" | "session_history" => {
            s.sessions_rail = true;
        }
        panel @ ("pinned" | "work" | "plan" | "todos" | "tasks" | "activity" | "live"
        | "running" | "agents" | "subagents" | "sub-agents" | "context" | "session"
        // `rail_panel == "tasks"` is the default, so only treat it as unset
        // when the document did not name the key explicitly. Failing the
        // guard falls through to the no-op arm below, which is exactly what
        // the old nested `if` did.
        | "auto")
            if s.rail_panel == "tasks" && !s.rail_panel_explicit =>
        {
            s.rail_panel = match panel {
                // `auto` is the shipped *default* for `sidebar_focus`, so
                // this arm runs for anyone who has a settings.toml at all
                // — even one that only sets `theme`. Auto-collapse meant
                // "show work when there is work", which is exactly the
                // Tasks panel (it auto-fits, and an empty projection
                // reserves no rows). Folding it into the always-on Pinned
                // strip inverted the intent and made a 4-row band the
                // effective default for every upgrading user.
                "tasks" | "activity" | "live" | "running" | "auto" => "tasks",
                "agents" | "subagents" | "sub-agents" => "agents",
                "context" | "session" => "context",
                _ => "pinned",
            }
            .to_string();
        }
        _ => {}
    }
    if s.sidebar_width_percent != 28 {
        let cols = (u32::from(s.sidebar_width_percent) * 120 / 100) as u16;
        s.work_surface_side_width = cols.clamp(26, 80);
    }
}

fn normalize_inline_diffs(value: &str) -> &'static str {
    InlineDiffMode::parse(value).as_setting()
}

/// The `(key, value)` fields a named preset applies, or `None` for an unknown
/// name. Single source of truth shared by [`Settings::apply_preset`] and the
/// `/config preset` command so the bundle is never defined twice.
#[must_use]
pub fn preset_fields(name: &str) -> Option<&'static [(&'static str, &'static str)]> {
    match name.trim().to_ascii_lowercase().as_str() {
        "calm" => Some(CALM_PRESET_FIELDS),
        _ => None,
    }
}

impl Settings {
    /// Get the canonical settings file path.
    ///
    /// New writes should target `~/.codewhale/settings.toml`. Legacy
    /// DeepSeek-branded paths remain readable as fallbacks during load, but we
    /// no longer surface them as the primary path in `/config`.
    pub fn path() -> Result<PathBuf> {
        let (primary, _legacy_home, legacy_config_dir) = settings_path_candidates();
        primary.or(legacy_config_dir).ok_or_else(|| {
            anyhow::anyhow!("Failed to resolve settings path: no config directory found.")
        })
    }

    /// Load settings from disk, or return defaults if not found
    pub fn load() -> Result<Self> {
        let mut settings = Self::load_persisted()?;
        settings.apply_env_overrides();
        Ok(settings)
    }

    /// Load settings for a diagnostic without migrating a legacy file.
    ///
    /// This preserves the same candidate precedence, parser normalization, and
    /// environment overlays as [`Settings::load`]. Unlike an interactive
    /// startup, diagnostics must not create `~/.codewhale/settings.toml` just
    /// because they inspected a legacy `~/.deepseek/settings.toml` file.
    pub(crate) fn load_read_only() -> Result<Self> {
        let mut settings = Self::load_persisted_read_only()?;
        settings.apply_env_overrides();
        Ok(settings)
    }

    /// Load the normalized values stored on disk without terminal/runtime
    /// overlays. Configuration editors use this path so a value labelled
    /// "saved" never silently reports a tmux, SSH, or accessibility override.
    pub(crate) fn load_persisted() -> Result<Self> {
        with_settings_transaction(SettingsTransaction::load)
    }

    /// Load persisted values while the caller already holds the settings
    /// process mutex and adjacent file lock.
    fn load_persisted_locked() -> Result<Self> {
        let (primary, legacy_home, legacy_config_dir) = settings_path_candidates();
        Self::load_persisted_from_candidates(primary, legacy_home, legacy_config_dir)
    }

    /// Load normalized disk values for a diagnostic without creating a
    /// primary settings file from a legacy fallback.
    fn load_persisted_read_only() -> Result<Self> {
        let (primary, legacy_home, legacy_config_dir) = settings_path_candidates();
        Self::load_persisted_from_candidates_with_migration(
            primary,
            legacy_home,
            legacy_config_dir,
            false,
        )
    }

    fn load_persisted_from_candidates(
        primary: Option<PathBuf>,
        legacy_home: Option<PathBuf>,
        legacy_config_dir: Option<PathBuf>,
    ) -> Result<Self> {
        Self::load_persisted_from_candidates_with_migration(
            primary,
            legacy_home,
            legacy_config_dir,
            true,
        )
    }

    fn load_persisted_from_candidates_with_migration(
        primary: Option<PathBuf>,
        legacy_home: Option<PathBuf>,
        legacy_config_dir: Option<PathBuf>,
        migrate_legacy_file: bool,
    ) -> Result<Self> {
        #[cfg(test)]
        {
            crate::test_support::with_test_state_io_lock(|| {
                Self::load_persisted_from_candidates_with_migration_unlocked(
                    primary,
                    legacy_home,
                    legacy_config_dir,
                    migrate_legacy_file,
                )
            })
        }
        #[cfg(not(test))]
        Self::load_persisted_from_candidates_with_migration_unlocked(
            primary,
            legacy_home,
            legacy_config_dir,
            migrate_legacy_file,
        )
    }

    fn load_persisted_from_candidates_with_migration_unlocked(
        primary: Option<PathBuf>,
        legacy_home: Option<PathBuf>,
        legacy_config_dir: Option<PathBuf>,
        migrate_legacy_file: bool,
    ) -> Result<Self> {
        let write_path = primary
            .as_ref()
            .cloned()
            .or_else(|| legacy_config_dir.clone())
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to resolve settings path: no config directory found.")
            })?;
        let read_path =
            resolve_settings_path_from_candidates(primary, legacy_home, legacy_config_dir)
                .unwrap_or_else(|_| write_path.clone());

        let settings = if !read_path.exists() {
            Self::default()
        } else {
            let content = std::fs::read_to_string(&read_path)
                .with_context(|| format!("Failed to read settings from {}", read_path.display()))?;
            let parsed_document = toml::from_str::<toml::Value>(&content).ok();
            let mut s: Settings = match toml::from_str(&content) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse {} (using defaults): {e:#}",
                        read_path.display()
                    );
                    // Keep the app running on defaults, but carry the failure
                    // so a settings surface never labels them as saved.
                    Self {
                        load_error: Some(format!("{}: {e}", read_path.display())),
                        ..Self::default()
                    }
                }
            };
            // A persisted threshold is itself an explicit request for
            // auto-compaction. Older versions accepted this setting while
            // leaving the default `auto_compact = false`, silently turning the
            // requested trigger into a no-op. Preserve an explicit boolean
            // opt-out, but make threshold-only files effective on load.
            s.auto_compact_explicit = parsed_document
                .as_ref()
                .is_some_and(auto_compact_explicitly_configured_in_document);
            s.rail_panel_explicit = parsed_document
                .as_ref()
                .and_then(toml::Value::as_table)
                .is_some_and(|table| table.contains_key("rail_panel"));
            s.work_surface_placement_explicit = parsed_document
                .as_ref()
                .and_then(toml::Value::as_table)
                .is_some_and(|table| table.contains_key("work_surface_placement"));
            if parsed_document.as_ref().is_some_and(|document| {
                document.as_table().is_some_and(|table| {
                    !table.contains_key("auto_compact")
                        && (table.contains_key("auto_compact_threshold")
                            || table.contains_key("auto_compact_threshold_percent"))
                })
            }) {
                s.auto_compact = true;
            }

            // Compat boundary (2026-09-02): `ocean_treatment` was a modifier
            // on `theme`; the painted field is now the `underwater` theme
            // itself. Fold any persisted deepsea treatment into
            // `theme = "underwater"`, then drop the retired key on the next
            // ordinary save (the struct simply has no such field).
            if let Some(_treatment) = parsed_document
                .as_ref()
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("ocean_treatment"))
                .and_then(toml::Value::as_str)
                .filter(|treatment| {
                    matches!(
                        treatment.trim().to_ascii_lowercase().as_str(),
                        "deepsea" | "underwater" | "ombre" | "gradient" | "classic"
                    )
                })
            {
                s.theme = "underwater".to_string();
            }

            // "yolo" used to bundle two independent choices: Agent mode and
            // unrestricted approvals.  Keep that behavior on upgrade, but
            // store/show the two choices explicitly so Settings does not claim
            // the app starts in a fictional mode.
            let legacy_yolo_default = s.default_mode.trim().eq_ignore_ascii_case("yolo");
            s.legacy_yolo_default = legacy_yolo_default;
            s.default_mode = if legacy_yolo_default {
                "agent".to_string()
            } else {
                normalize_mode(&s.default_mode).to_string()
            };
            s.composer_density = normalize_composer_density(&s.composer_density).to_string();
            s.transcript_spacing = normalize_transcript_spacing(&s.transcript_spacing).to_string();
            s.tool_collapse_mode = normalize_tool_collapse_mode(&s.tool_collapse_mode).to_string();
            s.sidebar_focus = normalize_sidebar_focus(&s.sidebar_focus).to_string();
            // Rail unification (0.9.4) migration: the classic sidebar is
            // gone, so its settings carry forward instead of stranding.
            migrate_sidebar_settings_to_rail(&mut s);
            s.status_indicator = normalize_status_indicator(&s.status_indicator).to_string();
            s.work_surface_placement =
                normalize_work_surface_placement(&s.work_surface_placement).to_string();
            s.rail_panel = normalize_rail_panel(&s.rail_panel).to_string();
            // Migrate the unreadable 2..=4 legacy range in memory. The next
            // ordinary settings transaction persists the normalized value;
            // loading settings remains a read-only operation.
            s.work_surface_top_height = s
                .work_surface_top_height
                .clamp(WORK_SURFACE_TOP_HEIGHT_MIN, WORK_SURFACE_TOP_HEIGHT_MAX);
            s.work_surface_side_width = s.work_surface_side_width.clamp(26, 80);
            s.inline_diffs = normalize_inline_diffs(&s.inline_diffs).to_string();
            s.synchronized_output =
                normalize_synchronized_output(&s.synchronized_output).to_string();
            s.locale = normalize_configured_locale(&s.locale)
                .unwrap_or("en")
                .to_string();
            s.background_color = normalize_optional_background_color(s.background_color.as_deref());
            s.theme = normalize_settings_theme(&s.theme);
            s.default_model = s.default_model.as_deref().and_then(normalize_default_model);
            s.reasoning_effort = s
                .reasoning_effort
                .as_deref()
                .and_then(|value| normalize_reasoning_effort_setting(value).ok().flatten());
            s.permission_posture = s
                .permission_posture
                .as_deref()
                .and_then(normalize_permission_posture);
            if legacy_yolo_default && s.permission_posture.is_none() {
                s.permission_posture = Some("full-access".to_string());
            }
            s.sandbox_mode = s.sandbox_mode.as_deref().and_then(normalize_sandbox_mode);
            s
        };
        if migrate_legacy_file {
            migrate_settings_file_to_primary_if_needed(&write_path, &read_path);
        }
        Ok(settings)
    }

    /// Whether this load normalized a legacy `default_mode = "yolo"` value.
    ///
    /// This is migration provenance, not a user-facing mode. New writes accept
    /// only Agent or Plan and serialize the independent permission posture.
    pub(crate) fn legacy_yolo_default_detected(&self) -> bool {
        self.legacy_yolo_default
    }

    /// Whether the user explicitly persisted an auto-compaction preference.
    /// A threshold is intent to enable compaction unless an explicit boolean
    /// says otherwise. When all three keys are absent, callers may choose a
    /// model-aware default.
    pub fn auto_compact_explicitly_configured() -> bool {
        let candidates = settings_path_candidates();
        #[cfg(test)]
        {
            crate::test_support::with_test_state_io_lock(|| {
                auto_compact_explicitly_configured_from_candidates(candidates)
            })
        }
        #[cfg(not(test))]
        auto_compact_explicitly_configured_from_candidates(candidates)
    }
}

fn auto_compact_explicitly_configured_from_candidates(
    (primary, legacy_home, legacy_config_dir): (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>),
) -> bool {
    let Ok(path) = resolve_settings_path_from_candidates(primary, legacy_home, legacy_config_dir)
    else {
        return false;
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&content) else {
        return false;
    };
    auto_compact_explicitly_configured_in_document(&value)
}

fn auto_compact_explicitly_configured_in_document(value: &toml::Value) -> bool {
    value.as_table().is_some_and(|table| {
        table.contains_key("auto_compact")
            || table.contains_key("auto_compact_threshold")
            || table.contains_key("auto_compact_threshold_percent")
    })
}

/// The runtime overlay that forces `low_motion` on, when one wins over the
/// persisted value. Mirrors the precedence of
/// [`Settings::apply_env_overrides`] so a settings surface can name the real
/// owner instead of calling a forced value "saved".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionOverride {
    NoAnimationsEnv,
    VsCodeTerminal,
    TermiusTerminal,
    SshSession,
    TabbyTerminal,
    LegacyWindowsConsole,
}

impl MotionOverride {
    /// The literal token a person can look for in their environment.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::NoAnimationsEnv => "NO_ANIMATIONS",
            Self::VsCodeTerminal => "TERM_PROGRAM=vscode",
            Self::TermiusTerminal => "TERM_PROGRAM=Termius",
            Self::SshSession => "SSH_CLIENT/SSH_TTY",
            Self::TabbyTerminal => "TERM_PROGRAM=tabby",
            Self::LegacyWindowsConsole => "legacy Windows console",
        }
    }

    /// Whether the override comes from the environment (a variable or an
    /// SSH session) rather than from the terminal program itself.
    #[must_use]
    pub fn is_environment(self) -> bool {
        matches!(self, Self::NoAnimationsEnv | Self::SshSession)
    }
}

/// Detect which runtime overlay forces `low_motion`, in the order
/// [`Settings::apply_env_overrides`] applies them.
#[must_use]
pub fn detect_low_motion_override() -> Option<MotionOverride> {
    let env_nonempty = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if env_truthy("NO_ANIMATIONS") {
        return Some(MotionOverride::NoAnimationsEnv);
    }
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if term_program.eq_ignore_ascii_case("vscode") {
        return Some(MotionOverride::VsCodeTerminal);
    }
    if term_program == "Termius" {
        return Some(MotionOverride::TermiusTerminal);
    }
    if env_nonempty("SSH_CLIENT") || env_nonempty("SSH_TTY") {
        return Some(MotionOverride::SshSession);
    }
    if term_program.to_ascii_lowercase().contains("tabby") {
        return Some(MotionOverride::TabbyTerminal);
    }
    if detected_legacy_windows_console_host() {
        return Some(MotionOverride::LegacyWindowsConsole);
    }
    None
}

impl Settings {
    /// Apply environment-driven overlays after disk load. Used for
    /// platform a11y signals that should ignore the user's saved
    /// preference (#450). The env values are consulted at startup;
    /// changing them mid-session has no effect because settings are
    /// only re-read on `Settings::load()`.
    pub fn apply_env_overrides(&mut self) {
        if env_truthy("NO_ANIMATIONS") {
            self.low_motion = true;
            self.fancy_animations = false;
        }
        // VS Code (TERM_PROGRAM=vscode, #1356) and a few VTE terminals
        // (#1470) produce visible flicker at 120 FPS. Cap their redraw rate.
        // VS Code's xterm.js renderer also needs decorative
        // motion disabled: the underwater chrome added substantially more
        // independently moving cells than the original #1356 fix covered.
        // Ghostty is deliberately absent from this 30 FPS compatibility lane.
        // Its synchronized GPU renderer gets a dedicated 60 FPS atmosphere
        // cap in display_refresh; putting it here made the restored truecolor
        // ocean visibly step even though the terminal could keep up.
        // Like NO_ANIMATIONS above, this unconditionally overrides any
        // disk-loaded value — consistent precedence: env signals always win.
        let term_program = std::env::var("TERM_PROGRAM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        // Tabby renders through Electron/xterm.js. Its Windows IME bridge
        // can observe cursor-positioning sequences while a frame is still
        // being applied, so use the calmer rendering path there.
        let term_is_tabby = term_program.contains("tabby");
        let term_constrains_frame_rate = term_program == "vscode";
        let vte_env_constrains_frame_rate = std::env::var_os("TILIX_ID")
            .is_some_and(|v| !v.is_empty())
            || std::env::var_os("TERMINATOR_UUID").is_some_and(|v| !v.is_empty());
        if term_constrains_frame_rate || vte_env_constrains_frame_rate {
            self.constrained_frame_rate = true;
        }
        if term_program == "vscode" {
            self.low_motion = true;
            self.fancy_animations = false;
        }

        // Termius (TERM_PROGRAM=Termius) and SSH sessions exhibit the
        // same 120-FPS flicker class as VS Code — the SSH round-trip
        // races ahead of what the remote renderer can flush, so rapid
        // cursor-positioning sequences cycle through input boxes.
        // Drop both to the 30 FPS low-motion cap. Harvested from
        // PR #1479 by @CrepuscularIRIS / autoghclaw (closes #1433).
        //
        // SSH_CLIENT is exported by sshd for every TCP SSH session;
        // SSH_TTY is exported only for interactive PTY logins, so we
        // check both so non-PTY-allocating tools (rsync wrappers, etc.)
        // still pick this up if they end up running the TUI.
        let term_is_termius = std::env::var("TERM_PROGRAM").as_deref() == Ok("Termius");
        let in_ssh_session = std::env::var_os("SSH_CLIENT").is_some_and(|v| !v.is_empty())
            || std::env::var_os("SSH_TTY").is_some_and(|v| !v.is_empty());
        if term_is_termius || in_ssh_session {
            self.low_motion = true;
            self.fancy_animations = false;
        }
        if term_is_tabby {
            self.low_motion = true;
            self.fancy_animations = false;
            self.constrained_frame_rate = true;
            if self.synchronized_output.eq_ignore_ascii_case("auto") {
                self.synchronized_output = "off".to_string();
            }
        }

        // Multiplexers need a bounded redraw rate, not a different product.
        // Preserve authored motion and let the frame limiter protect tmux /
        // screen; NO_ANIMATIONS remains the explicit hard-off contract.
        let in_terminal_multiplexer = std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
            || std::env::var_os("STY").is_some_and(|v| !v.is_empty());
        if in_terminal_multiplexer {
            self.constrained_frame_rate = true;
        }

        // Plain Windows PowerShell / cmd.exe under legacy ConHost exposes none
        // of the modern terminal markers below. Keep rendering calmer there:
        // lower the motion rate, disable animated chrome, and avoid DEC 2026
        // synchronized-output wrapping unless the user explicitly forced it on.
        if detected_legacy_windows_console_host() {
            self.low_motion = true;
            self.fancy_animations = false;
            if self.synchronized_output.eq_ignore_ascii_case("auto") {
                self.synchronized_output = "off".to_string();
            }
        }

        // Ptyxis 50.x (the new default terminal on Ubuntu 26.04) ships with
        // VTE 0.84.x which mishandles DEC mode 2026 synchronized output: the
        // begin/end pair is parsed but each wrapped frame still triggers a
        // full-viewport flash on the GPU compositor side, so any TUI that
        // uses DEC 2026 to avoid tearing instead gets visible flicker on
        // every redraw. gnome-terminal 3.58 on the same VTE renders cleanly,
        // so we can't broaden the opt-out to all VTE-based terminals —
        // only the Ptyxis-specific signals trigger it. Confirmed
        // user-visible regression starting with Ubuntu 26.04's default
        // terminal swap; cargo-installed binaries are not exempt because
        // the bug is in the terminal, not the binary.
        //
        // Only flip `auto` to `off`; respect an explicit `"on"` so users
        // who upgrade Ptyxis or want to confirm the fix landed upstream
        // can override the heuristic from the persisted settings.toml or
        // `/set synchronized_output on`.
        if self.synchronized_output.eq_ignore_ascii_case("auto") && detected_ptyxis_terminal() {
            self.synchronized_output = "off".to_string();
        }
    }

    /// Run one atomic load → mutate → save cycle against `settings.toml`.
    ///
    /// **Every writer that reads the whole file, changes some fields, and writes
    /// the whole file back must go through here** (or through
    /// [`SettingsTransaction`] for the multi-step shape). `save` serializes the
    /// complete struct, so two unsynchronized writers that each did their own
    /// `load_persisted` will each write back the *other's* pre-image: whichever
    /// saves last silently reverts the other's field. Locking `save` alone does
    /// not help, because the stale read already happened before the lock.
    ///
    /// Two locks are taken (see [`with_settings_transaction`]): a process-wide
    /// mutex keyed by the resolved settings path, which covers writers that
    /// never share an object — a background startup-default drain and a
    /// synchronous Shift+Tab permission write, the concrete pair that lost
    /// `default_mode` / `permission_posture` against each other — and a
    /// cross-process file lock, which covers a second Codewhale process on the
    /// same home directory.
    ///
    /// The closure must not call `transact`, [`with_settings_transaction`],
    /// `save`, or `load_persisted` itself — the lock is not re-entrant. Use
    /// [`with_settings_transaction`] when you need more than one save in one
    /// critical section.
    pub fn transact<T>(mutate: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        with_settings_transaction(|transaction| {
            let mut settings = transaction.load()?;
            let value = mutate(&mut settings)?;
            transaction.save(&settings)?;
            Ok(value)
        })
    }

    /// [`Self::transact`] for a mutation that may decide there is nothing to
    /// write. Returning `None` abandons the transaction without touching disk,
    /// so a "flag already set" early return does not rewrite the file.
    pub fn transact_opt<T>(
        mutate: impl FnOnce(&mut Self) -> Result<Option<T>>,
    ) -> Result<Option<T>> {
        with_settings_transaction(|transaction| {
            let mut settings = transaction.load()?;
            let Some(value) = mutate(&mut settings)? else {
                return Ok(None);
            };
            transaction.save(&settings)?;
            Ok(Some(value))
        })
    }

    /// Save settings to disk as a standalone, fully locked write.
    ///
    /// Prefer [`Self::transact`]: calling this on a `Settings` that was loaded
    /// outside a transaction writes back a snapshot that may already be stale
    /// for every field the caller did *not* mean to change. This entry point
    /// still takes both locks, so the bytes it writes are never interleaved with
    /// another writer's — it just cannot fix a stale read that already happened.
    ///
    /// Not callable from inside a transaction: the cross-process lock is not
    /// re-entrant, so a nested acquisition would deadlock against itself. Inside
    /// a critical section use [`SettingsTransaction::save`].
    #[cfg(test)]
    pub fn save(&self) -> Result<()> {
        with_settings_transaction(|transaction| transaction.save(self))
    }

    /// The write half of a settings transaction: serialize, merge comments, and
    /// replace the file atomically. The caller already holds both the
    /// process-wide mutex and the cross-process file lock.
    fn save_locked(&self, path: &Path) -> Result<()> {
        #[cfg(test)]
        {
            crate::test_support::with_test_state_io_lock(|| self.save_to_path(path))
        }
        #[cfg(not(test))]
        self.save_to_path(path)
    }

    fn save_to_path(&self, path: &Path) -> Result<()> {
        // Create config directory if it doesn't exist
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory {}", parent.display())
            })?;
        }

        let mut serialized =
            toml::to_string_pretty(self).context("Failed to serialize settings")?;
        if !self.auto_compact_explicit {
            let mut document = serialized
                .parse::<toml_edit::DocumentMut>()
                .context("Failed to prepare settings for persistence")?;
            document.remove("auto_compact");
            document.remove("auto_compact_threshold_percent");
            serialized = document.to_string();
        }
        let body = if path.exists() {
            let raw = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read settings at {}", path.display()))?;
            codewhale_config::merge_and_preserve_comments(&serialized, &raw).unwrap_or_else(|e| {
                tracing::warn!("failed to merge settings comments, saving without them: {e:#}");
                serialized
            })
        } else {
            serialized
        };
        atomically_replace_settings_file(path, body.as_bytes())
    }

    /// Set a single setting by key
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "auto_compact" | "compact" => {
                self.auto_compact = parse_bool(value)?;
                self.auto_compact_explicit = true;
            }
            "auto_compact_threshold" | "auto_compact_threshold_percent" => {
                self.auto_compact_threshold_percent =
                    parse_percent_setting("auto_compact_threshold_percent", value)?;
                self.auto_compact = true;
                self.auto_compact_explicit = true;
            }
            "calm_mode" | "calm" => {
                self.calm_mode = parse_bool(value)?;
            }
            "tool_collapse" | "tool_collapse_mode" | "collapse" => {
                let normalized = normalize_tool_collapse_mode(value);
                if !matches!(normalized, "compact" | "expanded" | "calm") {
                    return Err(anyhow::anyhow!(
                        "Failed to update setting: invalid tool collapse mode '{value}'. Expected: compact, expanded, or calm."
                    ));
                }
                self.tool_collapse_mode = normalized.to_string();
            }
            "low_motion" | "motion" => {
                self.low_motion = parse_bool(value)?;
            }
            "fancy_animations" | "fancy" | "animations" => {
                self.fancy_animations = parse_bool(value)?;
            }
            "focus_texture" | "texture" => {
                let normalized = value.trim().to_ascii_lowercase();
                if !matches!(normalized.as_str(), "off" | "scrim" | "grain") {
                    anyhow::bail!(
                        "Failed to update setting: invalid focus texture '{value}'. Expected: off, scrim, or grain."
                    );
                }
                self.focus_texture = normalized;
            }
            "work_surface_placement" | "work_surface" | "work_rail" => {
                let normalized = value.trim().to_ascii_lowercase();
                if !matches!(
                    normalized.as_str(),
                    "top" | "bottom" | "left" | "right" | "off"
                ) {
                    anyhow::bail!(
                        "Failed to update setting: invalid work surface placement '{value}'. Expected: top, bottom, left, right, or off."
                    );
                }
                self.work_surface_placement = normalized;
            }
            "rail_panel" | "rail" => {
                let normalized = value.trim().to_ascii_lowercase();
                // `pinned` stays accepted as a setting word; it folds into
                // the tasks view exactly like the load-time migration.
                if !matches!(
                    normalized.as_str(),
                    "tasks"
                        | "agents"
                        | "background"
                        | "files"
                        | "notepad"
                        | "context"
                        | "git"
                        | "price"
                        | "pinned"
                ) {
                    anyhow::bail!(
                        "Failed to update setting: invalid workbar panel '{value}'. Expected: tasks, agents, background, files, notepad, context, git, or price."
                    );
                }
                self.rail_panel = normalize_rail_panel(&normalized).to_string();
                self.rail_panel_explicit = true;
            }
            "work_surface_top_height" | "work_top_height" => {
                self.work_surface_top_height = parse_u16_range(
                    "work_surface_top_height",
                    value,
                    WORK_SURFACE_TOP_HEIGHT_MIN,
                    WORK_SURFACE_TOP_HEIGHT_MAX,
                )?;
            }
            "work_surface_side_width" | "work_side_width" => {
                self.work_surface_side_width =
                    parse_u16_range("work_surface_side_width", value, 26, 80)?;
            }
            "bracketed_paste" | "paste" => {
                self.bracketed_paste = parse_bool(value)?;
            }
            "paste_burst_detection" | "paste_burst" => {
                self.paste_burst_detection = parse_bool(value)?;
            }
            "mention_menu_limit" | "mention_limit" => {
                self.mention_menu_limit = parse_usize_setting("mention_menu_limit", value)?;
            }
            "mention_walk_depth" | "mention_depth" | "completions_walk_depth" => {
                self.mention_walk_depth = parse_usize_setting("mention_walk_depth", value)?;
            }
            "mention_menu_behavior" | "mention_behavior" | "mention_menu" => {
                self.mention_menu_behavior = normalize_mention_menu_behavior(value)?;
            }
            "show_thinking" | "thinking" => {
                self.show_thinking = parse_bool(value)?;
            }
            "thinking_default_expanded" | "thinking_expanded" => {
                self.thinking_default_expanded = parse_bool(value)?;
            }
            "thinking_preview_lines" | "thinking_preview" => {
                self.thinking_preview_lines =
                    parse_usize_setting("thinking_preview_lines", value)?.min(40);
            }
            "thinking_highlight" | "reasoning_highlight" => {
                self.thinking_highlight = parse_bool(value)?;
            }
            "help_expand_groups" | "help_expanded" => {
                self.help_expand_groups = parse_bool(value)?;
            }
            "pin_last_prompt" | "pin_prompt" => {
                self.pin_last_prompt = parse_bool(value)?;
            }
            "show_tool_details" | "tool_details" => {
                self.show_tool_details = parse_bool(value)?;
            }
            "inline_diffs" | "inline_diff" | "diffs" => {
                let normalized = value.trim().to_ascii_lowercase();
                if !matches!(normalized.as_str(), "full" | "summary" | "off") {
                    anyhow::bail!(
                        "Failed to update setting: invalid inline diff mode '{value}'. Expected: full, summary, or off."
                    );
                }
                self.inline_diffs = normalized;
            }
            "locale" | "language" => {
                let Some(locale) = normalize_configured_locale(value) else {
                    anyhow::bail!(
                        "Failed to update setting: invalid locale '{value}'. Expected: {}.",
                        crate::localization::configured_locale_values(", ")
                    );
                };
                self.locale = locale.to_string();
            }
            "theme" => {
                self.theme = normalize_theme_setting(value).map_err(anyhow::Error::msg)?;
            }
            "ui_theme" => {
                self.theme = normalize_theme_setting(value).map_err(anyhow::Error::msg)?;
            }
            "background_color" | "background" | "bg" => {
                self.background_color = normalize_background_color_setting(value)?;
            }
            "composer_density" | "composer" => {
                let normalized = normalize_composer_density(value);
                if !["compact", "comfortable", "spacious"].contains(&normalized) {
                    anyhow::bail!(
                        "Failed to update setting: invalid composer density '{value}'. Expected: compact, comfortable, spacious."
                    );
                }
                self.composer_density = normalized.to_string();
            }
            "composer_border" | "border" => {
                self.composer_border = parse_bool(value)?;
            }
            "composer_multiline_mode" | "multiline_mode" | "multiline" => {
                self.composer_multiline_mode = parse_bool(value)?;
            }
            "composer_vim_mode" | "vim_mode" | "vim" => {
                let normalized = value.trim().to_ascii_lowercase();
                if !["vim", "normal"].contains(&normalized.as_str()) {
                    anyhow::bail!(
                        "Failed to update setting: invalid composer vim mode '{value}'. Expected: normal, vim."
                    );
                }
                self.composer_vim_mode = normalized;
            }
            "transcript_spacing" | "spacing" => {
                let normalized = normalize_transcript_spacing(value);
                if !["compact", "comfortable", "spacious"].contains(&normalized) {
                    anyhow::bail!(
                        "Failed to update setting: invalid transcript spacing '{value}'. Expected: compact, comfortable, spacious."
                    );
                }
                self.transcript_spacing = normalized.to_string();
            }
            "status_indicator" | "indicator" => {
                let normalized = normalize_status_indicator(value);
                if !["cw", "whale", "dots", "off"].contains(&normalized) {
                    anyhow::bail!(
                        "Failed to update setting: invalid status indicator '{value}'. Expected: cw, whale, dots, off."
                    );
                }
                self.status_indicator = normalized.to_string();
            }
            "synchronized_output" | "sync_output" | "sync" => {
                let normalized = normalize_synchronized_output(value);
                if !["auto", "on", "off"].contains(&normalized) {
                    anyhow::bail!(
                        "Failed to update setting: invalid synchronized_output '{value}'. Expected: auto, on, off."
                    );
                }
                self.synchronized_output = normalized.to_string();
            }
            "workspace_follow_symlinks" | "follow_symlinks" => {
                self.workspace_follow_symlinks = parse_bool(value)?;
            }
            "default_mode" | "mode" => {
                // Act (wire: agent), Plan, and Operate are valid startup modes.
                // yolo remains a permission-migration alias, not a mode write.
                self.default_mode = match value.trim().to_ascii_lowercase().as_str() {
                    "agent" | "normal" | "act" | "work" | "edit" => "agent".to_string(),
                    "plan" => "plan".to_string(),
                    "operate" | "operation" | "ops" => "operate".to_string(),
                    _ => anyhow::bail!(
                        "Failed to update setting: invalid mode '{value}'. Expected: act (agent), plan, or operate."
                    ),
                };
            }
            "context_panel" | "context" | "session_panel" => {
                self.context_panel = parse_bool(value)?;
            }
            "sessions_rail" | "sessions_panel" | "session_rail" => {
                self.sessions_rail = parse_bool(value)?;
            }
            "session_auto_resume" | "auto_resume" => {
                self.session_auto_resume = parse_bool(value)?;
            }
            "cost_currency" | "currency" => {
                let Some(currency) = crate::pricing::CostCurrency::from_setting(value) else {
                    anyhow::bail!(
                        "Failed to update setting: invalid cost currency '{value}'. Expected: usd, cny, rmb, yuan."
                    );
                };
                self.cost_currency = match currency {
                    crate::pricing::CostCurrency::Usd => "usd",
                    crate::pricing::CostCurrency::Cny => "cny",
                }
                .to_string();
            }
            "max_history" | "history" => {
                let max: usize = value.parse().map_err(|_| {
                    anyhow::anyhow!(
                        "Failed to update setting: invalid max history '{value}'. Expected a positive number."
                    )
                })?;
                self.max_input_history = max;
            }
            "default_model" | "model" => {
                let trimmed = value.trim();
                if trimmed.is_empty()
                    || matches!(
                        trimmed.to_ascii_lowercase().as_str(),
                        "none" | "default" | "(default)"
                    )
                {
                    self.default_model = None;
                    return Ok(());
                }

                let Some(model) = normalize_default_model(trimmed) else {
                    anyhow::bail!(
                        "Failed to update setting: invalid model '{value}'. Expected: auto, a DeepSeek model ID (for example deepseek-v4-pro, deepseek-v4-flash), or none/default."
                    );
                };
                self.default_model = Some(model);
            }
            "reasoning_effort" | "effort" => {
                self.reasoning_effort = normalize_reasoning_effort_setting(value)?;
            }
            "permission_posture" | "permissions" => {
                self.permission_posture = normalize_permission_posture(value);
                if self.permission_posture.is_none() {
                    anyhow::bail!(
                        "Failed to update setting: invalid permission posture '{value}'. Expected: ask, auto-review, or full-access."
                    );
                }
            }
            "sandbox_mode" | "sandbox" | "filesystem_sandbox" => {
                self.sandbox_mode = normalize_sandbox_mode(value);
                if self.sandbox_mode.is_none() {
                    anyhow::bail!(
                        "Failed to update setting: invalid sandbox_mode '{value}'. Expected: read-only, workspace-write, danger-full-access, or external-sandbox."
                    );
                }
            }
            _ => {
                anyhow::bail!("Failed to update setting: unknown setting '{key}'.");
            }
        }
        Ok(())
    }

    /// Apply a named settings preset (#3478).
    ///
    /// Presets are the first bundled-settings mechanism: a single name applies a
    /// coherent group of presentation knobs. `calm` is the "beautiful/calm
    /// transcript" preset — it quiets motion and verbose tool output while
    /// **keeping evidence reachable**: thinking stays visible and tool runs stay
    /// expandable (only their inline detail is collapsed), so maintainer/release
    /// work is never blind to failures. Presentation only — no model, provider,
    /// routing, or safety setting is touched. Reuses [`Settings::set`] so each
    /// field goes through the same validation as a single-key set.
    ///
    /// Returns the keys changed, or an error for an unknown preset.
    pub fn apply_preset(&mut self, name: &str) -> Result<Vec<&'static str>> {
        let Some(bundle) = preset_fields(name) else {
            anyhow::bail!("Unknown preset '{}'. Available presets: calm", name.trim());
        };
        let mut changed = Vec::with_capacity(bundle.len());
        for (key, value) in bundle {
            self.set(key, value)?;
            changed.push(*key);
        }
        Ok(changed)
    }

    /// Get all settings as a displayable string
    pub fn display(&self, locale: crate::localization::Locale) -> String {
        use crate::localization::{MessageId, tr};
        let mut lines = Vec::new();
        lines.push(tr(locale, MessageId::SettingsTitle).to_string());
        lines.push("─────────────────────────────".to_string());
        lines.push(format!("  auto_compact:       {}", self.auto_compact));
        lines.push(format!(
            "  auto_compact_pct:   {:.0}",
            self.auto_compact_threshold_percent
        ));
        lines.push(format!("  calm_mode:          {}", self.calm_mode));
        lines.push(format!("  tool_collapse:      {}", self.tool_collapse_mode));
        lines.push(format!("  low_motion:         {}", self.low_motion));
        lines.push(format!("  fancy_animations:   {}", self.fancy_animations));
        lines.push(format!("  focus_texture:      {}", self.focus_texture));
        lines.push(format!(
            "  work_surface:       {}",
            self.work_surface_placement
        ));
        lines.push(format!(
            "  work_top_height:    {}",
            self.work_surface_top_height
        ));
        lines.push(format!(
            "  work_side_width:    {}",
            self.work_surface_side_width
        ));
        lines.push(format!("  rail_panel:         {}", self.rail_panel));
        lines.push(format!("  bracketed_paste:    {}", self.bracketed_paste));
        lines.push(format!(
            "  paste_burst_detect: {}",
            self.paste_burst_detection
        ));
        lines.push(format!("  mention_menu_limit: {}", self.mention_menu_limit));
        lines.push(format!("  mention_walk_depth: {}", self.mention_walk_depth));
        lines.push(format!(
            "  mention_behavior:   {}",
            self.mention_menu_behavior
        ));
        lines.push(format!("  show_thinking:      {}", self.show_thinking));
        lines.push(format!(
            "  thinking_expanded:   {}",
            self.thinking_default_expanded
        ));
        lines.push(format!(
            "  thinking_preview:    {}",
            self.thinking_preview_lines
        ));
        lines.push(format!("  thinking_highlight: {}", self.thinking_highlight));
        lines.push(format!(
            "  help_expand_groups:  {}",
            self.help_expand_groups
        ));
        lines.push(format!("  pin_last_prompt:    {}", self.pin_last_prompt));
        lines.push(format!("  show_tool_details:  {}", self.show_tool_details));
        lines.push(format!("  inline_diffs:      {}", self.inline_diffs));
        lines.push(format!("  locale:            {}", self.locale));
        lines.push(format!("  theme:              {}", self.theme));
        lines.push(format!(
            "  background_color:   {}",
            self.background_color.as_deref().unwrap_or("(default)")
        ));
        lines.push(format!("  composer_density:   {}", self.composer_density));
        lines.push(format!("  composer_border:    {}", self.composer_border));
        lines.push(format!(
            "  composer_multiline_mode: {}",
            self.composer_multiline_mode
        ));
        lines.push(format!("  composer_vim_mode:  {}", self.composer_vim_mode));
        lines.push(format!("  transcript_spacing: {}", self.transcript_spacing));
        lines.push(format!("  status_indicator:   {}", self.status_indicator));
        lines.push(format!(
            "  synchronized_output: {}",
            self.synchronized_output
        ));
        lines.push(format!(
            "  workspace_follow_symlinks: {}",
            self.workspace_follow_symlinks
        ));
        lines.push(format!("  default_mode:       {}", self.default_mode));
        lines.push(format!("  context_panel:      {}", self.context_panel));
        lines.push(format!("  cost_currency:      {}", self.cost_currency));
        lines.push(format!("  max_history:        {}", self.max_input_history));
        lines.push(format!(
            "  deepseek_fallback:  {}",
            self.default_model.as_deref().unwrap_or("(default)")
        ));
        lines.push(format!(
            "  default_provider:   {}",
            self.default_provider
                .as_deref()
                .unwrap_or("(config/default)")
        ));
        let mut provider_models = self
            .provider_models
            .as_ref()
            .map(|models| models.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        provider_models.sort_by_key(|(provider, _)| *provider);
        if provider_models.is_empty() {
            lines.push("  provider_models:    (none)".to_string());
        } else {
            lines.push("  provider_models:".to_string());
            for (provider, model) in provider_models {
                lines.push(format!("    {provider}: {model}"));
            }
        }
        lines.push(format!(
            "  reasoning_effort:   {}",
            self.reasoning_effort
                .as_deref()
                .unwrap_or("(config/default)")
        ));
        lines.push(format!(
            "  permission_posture: {}",
            self.permission_posture
                .as_deref()
                .unwrap_or("(config/default)")
        ));
        lines.push(format!(
            "  sandbox_mode:       {}  # filesystem scope (not approval)",
            self.sandbox_mode.as_deref().unwrap_or("(config/default)")
        ));
        lines.push(String::new());
        lines.push(format!(
            "{} {}",
            tr(locale, MessageId::SettingsConfigFile),
            Self::path().map_or_else(|_| "(unknown)".to_string(), |p| p.display().to_string())
        ));
        lines.join("\n")
    }

    /// Get available setting keys and their descriptions
    pub fn available_settings() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "auto_compact",
                "Auto-compact near the hard context limit: on/off (model-aware default)",
            ),
            (
                "auto_compact_threshold_percent",
                "Auto-compact trigger threshold percent: 10-100 (default 80; setting it enables auto-compaction unless auto_compact=false is explicit)",
            ),
            ("calm_mode", "Calmer UI defaults: on/off"),
            (
                "tool_collapse",
                "Dense tool-run collapse mode: collapsed (alias compact), expanded, calm",
            ),
            (
                "low_motion",
                "Reduce decorative motion without changing model text delivery: on/off",
            ),
            ("fancy_animations", "Expressive live-state motion: on/off"),
            (
                "focus_texture",
                "Modal focus-context texture prototype: off/scrim/grain (default off)",
            ),
            (
                "work_surface_placement",
                "Ocean Tasks/To-do/Workers rail placement: bottom (default)/top/left/right",
            ),
            (
                "work_surface_top_height",
                "Resizable To-do/Sub-agent top bar height: 2-16 rows",
            ),
            (
                "work_surface_side_width",
                "Resizable To-do/Sub-agent side bar width: 26-80 columns",
            ),
            (
                "rail_panel",
                "Which panel the rail shows: tasks/agents/context/pinned",
            ),
            (
                "bracketed_paste",
                "Terminal bracketed-paste mode: on/off (rare to disable)",
            ),
            (
                "paste_burst_detection",
                "Fallback rapid-key paste detection: on/off",
            ),
            (
                "mention_menu_limit",
                "Maximum @-mention popup candidates retained before rendering (default 128)",
            ),
            (
                "mention_walk_depth",
                "Maximum @-mention workspace walk depth; 0 means unlimited (default 10)",
            ),
            (
                "mention_menu_behavior",
                "@-mention completion behavior: fuzzy/browser (default fuzzy)",
            ),
            ("show_thinking", "Show model thinking: on/off"),
            (
                "thinking_default_expanded",
                "Expand model thinking by default; Space still toggles: on/off",
            ),
            (
                "thinking_preview_lines",
                "Collapsed completed-thought preview rows (default 2, 0=header-only, 10=older dump)",
            ),
            (
                "thinking_highlight",
                "Fill the thinking/reasoning background: on/off",
            ),
            (
                "help_expand_groups",
                "Start Help/shortcuts with every group expanded: on/off (default off)",
            ),
            (
                "pin_last_prompt",
                "Pin the last user prompt at the top when it scrolls off: on/off (default on)",
            ),
            ("show_tool_details", "Show detailed tool output: on/off"),
            (
                "inline_diffs",
                "Successful File mutation evidence: full/summary/off (exact detail is always retained)",
            ),
            (
                "base_url",
                "HTTP base URL for DeepSeek-compatible endpoints.",
            ),
            (
                "locale",
                "UI locale and default model language: auto, en, ja, zh-Hans, zh-Hant, pt-BR, es-419, vi, ko, ca, de, fr, id, hi, ru, uk; every shipped pack holds full English parity",
            ),
            (
                "theme",
                "UI theme: a compiled name or custom:<name> from the Codewhale themes directory",
            ),
            (
                "background_color",
                "Main TUI background color: #RRGGBB or default",
            ),
            (
                "composer_density",
                "Composer density: compact, comfortable, spacious",
            ),
            (
                "composer_border",
                "Show a border around the composer input area: on/off",
            ),
            (
                "composer_multiline_mode",
                "Enter inserts a newline and Shift+Enter sends: on/off",
            ),
            ("composer_vim_mode", "Composer editing mode: normal, vim"),
            (
                "transcript_spacing",
                "Transcript spacing: compact, comfortable, spacious",
            ),
            (
                "status_indicator",
                "Header status mark, shown before the route: cw, whale, dots, off",
            ),
            (
                "synchronized_output",
                "DEC 2026 synchronized output: auto, on, off (set off if your terminal flickers)",
            ),
            (
                "workspace_follow_symlinks",
                "Follow symbolic links during workspace file discovery walks: on/off (default off). Enable for symlink-based multi-project workspaces. Has built-in cycle detection but may increase latency on large symlinked trees.",
            ),
            (
                "default_mode",
                "Default mode: act (agent), plan, or operate",
            ),
            (
                "context_panel",
                "Show the session context workbar panel: on/off",
            ),
            (
                "sessions_rail",
                "Show the persistent Sessions workbar: on/off (default off)",
            ),
            (
                "session_auto_resume",
                "Reattach to this workspace's most recent session on startup: on/off (default off). --resume/--continue still win; archived, unreadable, or other-workspace sessions are never auto-resumed.",
            ),
            ("cost_currency", "Cost display currency: usd, cny"),
            ("max_history", "Max input history entries"),
            (
                "default_model",
                "DeepSeek fallback model: auto or a DeepSeek model ID (e.g. deepseek-v4-pro); other providers use provider_models",
            ),
            (
                "reasoning_effort",
                "Default thinking effort: auto, off, low, medium, high, max, or default",
            ),
        ]
    }

    /// Persist the model for a specific provider.
    pub fn set_model_for_provider(&mut self, provider: &str, model: &str) {
        self.provider_models
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(provider.to_string(), model.to_string());
        self.enable_model_for_provider(provider, model);
    }

    /// Add a model to a provider's enabled chooser set without removing prior
    /// choices. IDs are compared case-insensitively but preserve their wire
    /// spelling on disk.
    pub fn enable_model_for_provider(&mut self, provider: &str, model: &str) {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() || model.eq_ignore_ascii_case("auto") {
            return;
        }
        let models = self
            .enabled_models
            .get_or_insert_with(std::collections::HashMap::new)
            .entry(provider.to_string())
            .or_default();
        if !models
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(model))
        {
            models.push(model.to_string());
        }
    }

    /// Toggle one exact provider/model pin without touching credentials or
    /// the provider's default route.
    pub fn toggle_pinned_model(&mut self, provider: &str, model: &str) -> bool {
        let provider = provider.trim();
        let model = model.trim();
        if provider.is_empty() || model.is_empty() || model.eq_ignore_ascii_case("auto") {
            return false;
        }
        if let Some(index) = self.pinned_models.iter().position(|pin| {
            pin.provider.eq_ignore_ascii_case(provider) && pin.model.eq_ignore_ascii_case(model)
        }) {
            self.pinned_models.remove(index);
            return false;
        }
        self.pinned_models.push(PinnedModel {
            provider: provider.to_string(),
            model: model.to_string(),
            label: None,
        });
        true
    }

    #[allow(dead_code)] // label editing surface is exposed through settings serialization first
    pub fn set_pinned_model_label(
        &mut self,
        provider: &str,
        model: &str,
        label: Option<String>,
    ) -> bool {
        self.pinned_models
            .iter_mut()
            .find(|pin| {
                pin.provider.eq_ignore_ascii_case(provider) && pin.model.eq_ignore_ascii_case(model)
            })
            .map(|pin| {
                pin.label = label.filter(|value| !value.trim().is_empty());
                true
            })
            .unwrap_or(false)
    }

    pub fn move_pinned_model(&mut self, provider: &str, model: &str, delta: isize) -> bool {
        let Some(index) = self.pinned_models.iter().position(|pin| {
            pin.provider.eq_ignore_ascii_case(provider) && pin.model.eq_ignore_ascii_case(model)
        }) else {
            return false;
        };
        let target = if delta.is_negative() {
            index.saturating_sub(delta.unsigned_abs())
        } else {
            index.saturating_add(delta as usize)
        };
        let target = target.min(self.pinned_models.len().saturating_sub(1));
        if target == index {
            return false;
        }
        let pin = self.pinned_models.remove(index);
        self.pinned_models.insert(target, pin);
        true
    }

    /// Resolved boolean for whether the renderer should wrap each frame in
    /// DEC mode 2026 synchronized output. `auto` and `on` enable; `off`
    /// disables. The `auto` → `off` flip for known-bad terminals happens
    /// earlier in [`Self::apply_env_overrides`]; this method only inspects
    /// the final state.
    #[must_use]
    pub fn synchronized_output_enabled(&self) -> bool {
        !self.synchronized_output.eq_ignore_ascii_case("off")
    }

    /// Runtime bracketed-paste mode after terminal-host quirks are applied.
    ///
    /// This deliberately does not mutate [`Settings::bracketed_paste`]:
    /// `apply_env_overrides()` can run before saving settings, and a legacy
    /// conhost runtime fallback must not permanently disable bracketed paste
    /// when the same config is later used in Windows Terminal or another
    /// modern terminal.
    #[must_use]
    pub fn effective_bracketed_paste(&self) -> bool {
        self.bracketed_paste && !detected_legacy_windows_console_host()
    }
}

fn resolve_settings_path_from_candidates(
    primary: Option<PathBuf>,
    legacy_home: Option<PathBuf>,
    legacy_config_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = primary.as_ref()
        && path.exists()
    {
        return Ok(path.clone());
    }

    if let Some(path) = legacy_home
        && path.exists()
    {
        return Ok(path);
    }

    if let Some(path) = legacy_config_dir.as_ref()
        && path.exists()
    {
        return Ok(path.clone());
    }

    primary.or(legacy_config_dir).ok_or_else(|| {
        anyhow::anyhow!("Failed to resolve settings path: no config directory found.")
    })
}

/// Proof that the caller is inside the settings critical section.
///
/// Only [`with_settings_transaction`] can hand one out, so a `load`/`save` pair
/// on this type is by construction covered by both the process-wide mutex and
/// the cross-process file lock.
pub(crate) struct SettingsTransaction {
    path: PathBuf,
}

impl SettingsTransaction {
    /// Read the on-disk values inside the critical section.
    pub(crate) fn load(&self) -> Result<Settings> {
        Settings::load_persisted_locked()
    }

    /// Write the whole file inside the critical section.
    pub(crate) fn save(&self, settings: &Settings) -> Result<()> {
        settings.save_locked(&self.path)
    }
}

/// Run `operation` as one whole-file settings critical section.
///
/// Most callers want [`Settings::transact`]. Reach for this directly only when a
/// single logical change needs more than one save under one lock — the
/// Shift+Tab root-policy release is the motivating case: it commits the new
/// posture, unsets the shadowing root config key, and must restore the previous
/// posture if that unset fails. Splitting that into two `transact` calls would
/// let another writer observe (and rewrite over) the uncommitted middle state.
///
/// Two locks are taken, in this order, and both are held across disk I/O:
///
/// 1. A process-wide mutex keyed by the resolved settings path. It covers
///    writers that never share an object — a background startup-default drain
///    and a synchronous Shift+Tab permission write, the concrete pair that lost
///    `default_mode` / `permission_posture` against each other.
/// 2. An **advisory file lock on an adjacent `settings.toml.lock`**, following
///    the `codewhale_config::config_document` pattern. The process mutex says
///    nothing about a second Codewhale process (a second TUI, `codewhale exec`,
///    the runtime HTTP surface in another instance) doing its own
///    load/modify/save. Without a cross-process lock those two interleave and
///    the later save reverts the earlier one's field — last-save-wins across
///    processes, which is exactly the bug the in-process lock was added to
///    prevent in-process.
///
/// The lock file is only ever a lock: no settings content is written to it, so
/// a stale one carries nothing to lose.
///
/// There is exactly one permitted lock order for anything that touches
/// `settings.toml`, and every acquisition in the tree below obeys it:
///
/// ```text
/// StartupDefaultsWriter::write  →  settings process mutex  →  settings file lock  →  test env lock  →  test state-I/O lock
/// ```
///
/// Two consequences worth stating, because breaking either is a deadlock:
///
/// - A thread holding a transaction must never wait on
///   `StartupDefaultsWriter::write`. The queued-drain paths (`flush`,
///   `apply_blocking`) take `write` *first* and only then enter a transaction.
/// - Under `cfg(test)` path resolution enters the process-wide env barrier from
///   inside a transaction, so a background thread inside a transaction must be
///   enrolled in the sealing test's env scope (see `tui::startup_defaults`) or it
///   will park on a lock its own test holds.
///
/// Neither lock is re-entrant. `operation` must not call back into `transact`,
/// `Settings::save`, or this function.
pub(crate) fn with_settings_transaction<T>(
    operation: impl FnOnce(&SettingsTransaction) -> Result<T>,
) -> Result<T> {
    let path = Settings::path()?;
    let _process_guard = lock_settings_transaction(settings_transaction_mutex(&path));
    with_settings_file_lock(&path, || {
        operation(&SettingsTransaction { path: path.clone() })
    })
}

/// Hold an exclusive advisory lock on `<settings.toml>.lock` for `operation`.
///
/// The lock file is opened (not followed) with owner-only permissions and is
/// created if absent. Dropping the `fd_lock` guard — including on an unwind —
/// releases it, and the OS releases it if the process dies, so a crash cannot
/// wedge another Codewhale instance out of its settings.
fn with_settings_file_lock<T>(path: &Path, operation: impl FnOnce() -> Result<T>) -> Result<T> {
    use std::fs;

    let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) else {
        anyhow::bail!(
            "Failed to lock settings: {} has no parent directory",
            path.display()
        );
    };
    fs::create_dir_all(parent)
        .with_context(|| format!("Failed to create config directory {}", parent.display()))?;

    let mut lock_name = path
        .file_name()
        .context("Failed to lock settings: settings path has no file name")?
        .to_os_string();
    lock_name.push(".lock");
    let lock_path = parent.join(lock_name);
    reject_settings_lock_symlink(&lock_path)?;

    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let lock_file = options
        .open(&lock_path)
        .with_context(|| format!("Failed to open settings lock at {}", lock_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        lock_file
            .set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| {
                format!("Failed to secure settings lock at {}", lock_path.display())
            })?;
    }
    if !lock_file
        .metadata()
        .with_context(|| format!("Failed to inspect settings lock at {}", lock_path.display()))?
        .file_type()
        .is_file()
    {
        anyhow::bail!(
            "Refusing a non-regular settings lock at {}",
            lock_path.display()
        );
    }

    let mut lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock
        .write()
        .with_context(|| format!("Failed to acquire settings lock at {}", lock_path.display()))?;
    operation()
}

/// Refuse to lock through a symlink: a planted `settings.toml.lock -> …` would
/// otherwise let an attacker pick which file we create with our permissions.
fn reject_settings_lock_symlink(lock_path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(lock_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => anyhow::bail!(
            "Refusing a symlinked settings lock at {}",
            lock_path.display()
        ),
        Ok(_) | Err(_) => Ok(()),
    }
}

/// Replace `path` with `body` by writing an adjacent temporary file and
/// renaming it into place.
///
/// A direct `fs::write` truncates first, so any concurrent reader — another
/// Codewhale process, an editor, a `cat` — can observe a half-written file and
/// parse it as truncated TOML, silently losing every key past the tear. A
/// same-directory temp file plus the platform's replace primitive makes the
/// swap atomic for readers: they see either the whole previous file or the
/// whole new one.
///
/// The temp file inherits the existing file's permission bits when there is one
/// (so a user who tightened `settings.toml` keeps that), and is created
/// owner-only otherwise. `NamedTempFile` removes itself if anything below fails,
/// so a failed save leaves no debris and never damages the previous file.
fn atomically_replace_settings_file(path: &Path, body: &[u8]) -> Result<()> {
    use std::io::Write as _;

    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::Builder::new()
        .prefix(".settings-")
        .suffix(".tmp")
        .tempfile_in(dir)
        .with_context(|| format!("Failed to stage settings write in {}", dir.display()))?;
    tmp.write_all(body)
        .with_context(|| format!("Failed to write settings to {}", path.display()))?;
    tmp.flush()
        .with_context(|| format!("Failed to flush settings for {}", path.display()))?;
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("Failed to sync settings for {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path)
            .map(|metadata| metadata.permissions().mode() & 0o777)
            .unwrap_or(0o600);
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("Failed to set permissions for {}", path.display()))?;
    }

    #[cfg(windows)]
    if path.exists() {
        // `tempfile::persist` uses MoveFileExW on Windows. Under concurrent
        // reads that can expose a partially replaced destination. ReplaceFileW
        // is the native existing-file replacement operation and also preserves
        // the destination's ACLs and attributes.
        let mut temporary = tmp.into_temp_path();
        replace_existing_settings_file(path, &temporary)
            .with_context(|| format!("Failed to write settings to {}", path.display()))?;
        // ReplaceFileW consumed the temporary pathname. Do not ask TempPath to
        // clean up that now-nonexistent source when it drops.
        temporary.disable_cleanup(true);
        return Ok(());
    }

    tmp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("Failed to write settings to {}", path.display()))?;
    Ok(())
}

#[cfg(windows)]
fn replace_existing_settings_file(path: &Path, replacement: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_TEMPORARY, ReplaceFileW, SetFileAttributesW,
    };

    fn wide_path(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain(Some(0)).collect()
    }

    let path_wide = wide_path(path);
    let replacement_wide = wide_path(replacement);
    unsafe {
        // NamedTempFile marks its source with the temporary caching hint.
        // Clear it before publication, matching tempfile's persistence path.
        if SetFileAttributesW(replacement_wide.as_ptr(), FILE_ATTRIBUTE_NORMAL) == 0 {
            return Err(std::io::Error::last_os_error());
        }

        if ReplaceFileW(
            path_wide.as_ptr(),
            replacement_wide.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null(),
            std::ptr::null(),
        ) == 0
        {
            let error = std::io::Error::last_os_error();
            // Restore the hint so TempPath retains its normal cleanup behavior
            // when replacement fails and the source still exists.
            let _ = SetFileAttributesW(replacement_wide.as_ptr(), FILE_ATTRIBUTE_TEMPORARY);
            return Err(error);
        }
    }
    Ok(())
}

/// Per-settings-path transaction mutexes.
///
/// Keyed by path rather than global because tests seal `HOME` onto their own
/// temp dirs: two sealed tests write different files and have no reason to
/// serialize against each other. Production has exactly one entry, so the
/// registry never grows; entries are intentionally `'static` (leaked once) so a
/// transaction can hold a plain `MutexGuard` without also pinning the registry
/// lock it came from.
fn settings_transaction_mutex(path: &Path) -> &'static std::sync::Mutex<()> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, &'static Mutex<()>>>> = OnceLock::new();
    let key = path.to_path_buf();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mutex: &'static Mutex<()> = locks
        .entry(key)
        .or_insert_with(|| Box::leak(Box::new(Mutex::new(()))));
    drop(locks);
    mutex
}

/// Acquire a transaction lock.
///
/// The mutex protects ordering, not an invariant, so a panic inside one
/// transaction must not wedge settings persistence for the rest of the session:
/// a poisoned guard is recovered rather than propagated.
#[cfg(not(test))]
fn lock_settings_transaction(
    mutex: &'static std::sync::Mutex<()>,
) -> std::sync::MutexGuard<'static, ()> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Test build of [`lock_settings_transaction`], with a watchdog.
///
/// Production blocks indefinitely, which is correct — the only thing ahead of it
/// is a bounded settings transaction. In a test binary an indefinite wait is
/// indistinguishable from a lock-order inversion, and a hung test job reports
/// nothing. This is not a synchronization device: every honest acquisition
/// succeeds on the first `try_lock` or shortly after. It exists so a regression
/// fails loudly instead of hanging CI.
///
/// The deadline is generous on purpose. A transaction still reads and writes
/// under `cfg(test)`'s state-I/O barrier, and the cross-process file lock can be
/// held by a deliberately slow child process in the cross-process regressions —
/// so the watchdog only has to be longer than the slowest honest transaction and
/// shorter than a CI job timeout, not tight.
#[cfg(test)]
fn lock_settings_transaction(
    mutex: &'static std::sync::Mutex<()>,
) -> std::sync::MutexGuard<'static, ()> {
    use std::sync::TryLockError;

    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        match mutex.try_lock() {
            Ok(guard) => return guard,
            Err(TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "settings transaction lock was not released within {DEADLINE:?}. Some thread is \
             holding it across a load/modify/save that cannot finish — usually because it is \
             blocked on a lock this test already holds, or because a transaction was opened \
             re-entrantly. See Settings::transact."
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

fn settings_path_candidates() -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>) {
    #[cfg(test)]
    {
        let honor_guarded_environment =
            crate::test_support::guarded_environment_provides_state_paths();
        crate::test_support::with_test_env_lock(|| {
            if honor_guarded_environment {
                settings_path_candidates_from_environment()
            } else {
                (
                    Some(crate::test_support::unsealed_test_state_root().join(SETTINGS_FILE_NAME)),
                    None,
                    None,
                )
            }
        })
    }

    #[cfg(not(test))]
    settings_path_candidates_from_environment()
}

fn settings_path_candidates_from_environment() -> (Option<PathBuf>, Option<PathBuf>, Option<PathBuf>)
{
    // Allow tests to override the settings directory via the same env vars
    // used for config. CODEWHALE_CONFIG_PATH is canonical; the legacy alias
    // remains a read-only fallback for existing installs.
    if let Some(parent) = config_override_parent() {
        return (Some(parent.join(SETTINGS_FILE_NAME)), None, None);
    }

    let primary = codewhale_config::codewhale_home()
        .ok()
        .map(|home| home.join(SETTINGS_FILE_NAME));
    if codewhale_config::codewhale_home_is_explicit() {
        return (primary, None, None);
    }
    let legacy_home = codewhale_config::legacy_deepseek_home()
        .ok()
        .map(|home| home.join(SETTINGS_FILE_NAME));
    let legacy_config_dir =
        dirs::config_dir().map(|dir| dir.join("deepseek").join(SETTINGS_FILE_NAME));

    (primary, legacy_home, legacy_config_dir)
}

fn config_override_parent() -> Option<PathBuf> {
    fn read() -> Option<PathBuf> {
        for var in ["CODEWHALE_CONFIG_PATH", "DEEPSEEK_CONFIG_PATH"] {
            if let Ok(config_path) = std::env::var(var) {
                let config_path = config_path.trim();
                if !config_path.is_empty() {
                    return expand_path(config_path).parent().map(Path::to_path_buf);
                }
            }
        }
        None
    }

    #[cfg(test)]
    {
        crate::test_support::with_test_env_lock(read)
    }
    #[cfg(not(test))]
    {
        read()
    }
}

fn migrate_settings_file_to_primary_if_needed(primary: &Path, active_read_path: &Path) {
    use std::io::Write as _;

    if primary == active_read_path || primary.exists() || !active_read_path.exists() {
        return;
    }

    let Some(parent) = primary.parent() else {
        return;
    };

    if let Err(err) = std::fs::create_dir_all(parent) {
        tracing::warn!(
            "failed to create settings migration directory {}: {err}",
            parent.display()
        );
        return;
    }

    let migration = (|| -> Result<()> {
        let body = std::fs::read(active_read_path).with_context(|| {
            format!(
                "Failed to read legacy settings from {}",
                active_read_path.display()
            )
        })?;
        let mut tmp = tempfile::Builder::new()
            .prefix(".settings-migration-")
            .suffix(".tmp")
            .tempfile_in(parent)
            .with_context(|| {
                format!("Failed to stage settings migration in {}", parent.display())
            })?;
        tmp.write_all(&body).with_context(|| {
            format!(
                "Failed to stage legacy settings from {}",
                active_read_path.display()
            )
        })?;
        tmp.flush()
            .context("Failed to flush staged settings migration")?;
        tmp.as_file()
            .sync_all()
            .context("Failed to sync staged settings migration")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(active_read_path)
                .map(|metadata| metadata.permissions().mode() & 0o777)
                .unwrap_or(0o600);
            tmp.as_file()
                .set_permissions(std::fs::Permissions::from_mode(mode))
                .context("Failed to preserve legacy settings permissions")?;
        }

        match tmp.persist_noclobber(primary) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error).with_context(|| {
                format!(
                    "Failed to install migrated settings at {}",
                    primary.display()
                )
            }),
        }
    })();

    if let Err(err) = migration {
        tracing::warn!(
            "failed to migrate settings from {} to {}: {err}",
            active_read_path.display(),
            primary.display()
        );
    }
}

fn normalize_default_model(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.eq_ignore_ascii_case("auto") {
        Some("auto".to_string())
    } else {
        normalize_model_name(trimmed)
    }
}

fn normalize_permission_posture(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "ask" | "suggest" | "on-request" | "untrusted" => Some("ask".to_string()),
        "auto" | "auto-review" | "auto_review" => Some("auto-review".to_string()),
        "full" | "full-access" | "full_access" | "bypass" => Some("full-access".to_string()),
        _ => None,
    }
}

/// Normalize filesystem sandbox mode. Distinct from permission posture.
fn normalize_sandbox_mode(value: &str) -> Option<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "read-only" | "readonly" | "read_only" | "ro" => Some("read-only".to_string()),
        "workspace-write" | "workspace_write" | "workspace" | "workspace-only" => {
            Some("workspace-write".to_string())
        }
        "danger-full-access" | "danger_full_access" | "full-fs" | "full_filesystem"
        | "filesystem-full" => Some("danger-full-access".to_string()),
        "external-sandbox" | "external_sandbox" | "opensandbox" | "external" => {
            Some("external-sandbox".to_string())
        }
        _ => None,
    }
}

fn normalize_reasoning_effort_setting(value: &str) -> Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "default" | "(default)" | "config" | "configured" | "unset"
        )
    {
        return Ok(None);
    }

    ReasoningEffort::parse_strict(trimmed)
        .map(|effort| Some(effort.as_setting().to_string()))
        .map_err(|err| anyhow::anyhow!("Failed to update setting: {err}"))
}

/// Parse a boolean value from various formats
fn parse_bool(value: &str) -> Result<bool> {
    match value.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "enabled" => Ok(true),
        "off" | "false" | "no" | "0" | "disabled" => Ok(false),
        _ => {
            anyhow::bail!("Failed to parse boolean '{value}': expected on/off, true/false, yes/no.")
        }
    }
}

fn default_thinking_preview_lines() -> usize {
    2
}

fn default_true() -> bool {
    true
}

fn parse_usize_setting(key: &str, value: &str) -> Result<usize> {
    value.trim().parse::<usize>().map_err(|_| {
        anyhow::anyhow!(
            "Failed to update setting: invalid {key} '{value}'. Expected 0 or a positive integer."
        )
    })
}

fn parse_u16_range(key: &str, value: &str, min: u16, max: u16) -> Result<u16> {
    let parsed = value
        .trim()
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!("Invalid {key} '{value}': expected {min}-{max}"))?;
    if !(min..=max).contains(&parsed) {
        anyhow::bail!("Invalid {key} '{value}': expected {min}-{max}");
    }
    Ok(parsed)
}

fn parse_percent_setting(key: &str, value: &str) -> Result<f64> {
    let trimmed = value.trim().trim_end_matches('%').trim();
    let percent = trimmed.parse::<f64>().map_err(|_| {
        anyhow::anyhow!(
            "Failed to update setting: invalid {key} '{value}'. Expected a number from 10 to 100."
        )
    })?;
    if !(10.0..=100.0).contains(&percent) {
        anyhow::bail!(
            "Failed to update setting: invalid {key} '{value}'. Expected a number from 10 to 100."
        );
    }
    Ok(percent)
}

fn normalize_mention_menu_behavior(value: &str) -> Result<String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fuzzy" | "default" => Ok("fuzzy".to_string()),
        "browser" | "browse" | "file-browser" | "file_browser" => Ok("browser".to_string()),
        _ => {
            anyhow::bail!(
                "Failed to update setting: invalid mention_menu_behavior '{value}'. Expected: fuzzy, browser."
            )
        }
    }
}

fn normalize_mode(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "edit" => "agent",
        "normal" => "agent",
        "agent" | "act" | "work" => "agent",
        "plan" => "plan",
        // Operate is a first-class startup mode (Hunter 2026-07-24).
        "operate" | "operation" | "ops" => "operate",
        // yolo was mode+permission; keep mode as Act and migrate posture on load.
        "yolo" => "agent",
        _ => value,
    }
}

fn normalize_composer_density(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "compact" | "tight" => "compact",
        "comfortable" | "default" | "normal" => "comfortable",
        "spacious" | "loose" => "spacious",
        _ => value,
    }
}

fn normalize_transcript_spacing(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "compact" | "tight" => "compact",
        "comfortable" | "default" | "normal" => "comfortable",
        "spacious" | "loose" => "spacious",
        _ => value,
    }
}

fn normalize_tool_collapse_mode(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "compact" | "collapsed" | "collapse" | "default" | "on" | "true" => "compact",
        "expanded" | "expand" | "off" | "none" | "false" => "expanded",
        "calm" | "calm_mode" | "calm-mode" | "calm_only" | "calm-only" => "calm",
        _ => value,
    }
}

/// Normalize the `status_indicator` header chip setting. Accepts the
/// canonical names plus common aliases ("none"/"hidden" → "off",
/// "dot" → "dots"). Unknown values fall through unchanged so the parser
/// in `update_setting` can surface a clear error.
fn normalize_status_indicator(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "cw" | "mark" | "text" => "cw",
        // The whale emoji header chip is retired (2026-07-23): persisted
        // opt-ins migrate to the typographic mark on load.
        "whale" | "🐳" | "🐋" => "cw",
        "dots" | "dot" => "dots",
        "off" | "none" | "hidden" | "false" => "off",
        _ => value,
    }
}

/// Normalize the `synchronized_output` setting. Accepts the canonical
/// `"auto"` / `"on"` / `"off"` plus the usual truthy/falsey spellings.
/// Unknown values fall through unchanged so the parser in `set` can
/// surface a clear error.
fn normalize_synchronized_output(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "auto" | "default" => "auto",
        "on" | "true" | "yes" | "1" | "enabled" => "on",
        "off" | "false" | "no" | "0" | "disabled" => "off",
        _ => value,
    }
}

fn normalize_settings_theme(value: &str) -> String {
    // A malformed persisted selector must not turn into a painted application
    // background. Falling back to the underwater default keeps a single
    // compiled first-party theme until the user picks an explicit palette.
    normalize_theme_setting(value).unwrap_or_else(|_| "underwater".to_string())
}

/// Returns `true` when the active terminal is Ptyxis (the new default
/// terminal on Ubuntu 26.04). Used by [`Settings::apply_env_overrides`]
/// to flip `synchronized_output` from `auto` to `off` so DEC mode 2026
/// flicker on Ptyxis 50.x + VTE 0.84.x stops at the source.
///
/// We deliberately keep this narrow:
///
/// - `TERM_PROGRAM` matches `ptyxis` case-insensitively (the value
///   Ptyxis sets when it forwards a process-launch context).
/// - `PTYXIS_VERSION` is set to any non-empty value (the binary's
///   own version probe, present whether or not `TERM_PROGRAM` made it
///   into the child environment).
///
/// Either signal is sufficient. We do *not* trigger on `VTE_VERSION`
/// alone because gnome-terminal 3.58 ships with the same VTE 0.84.x
/// and renders cleanly — broadening the heuristic would regress every
/// gnome-terminal user.
pub fn detected_ptyxis_terminal() -> bool {
    if let Ok(program) = std::env::var("TERM_PROGRAM")
        && program.trim().to_ascii_lowercase().contains("ptyxis")
    {
        return true;
    }
    matches!(std::env::var("PTYXIS_VERSION"), Ok(v) if !v.trim().is_empty())
}

/// Returns `true` for the unmarked Windows console-host path used by plain
/// PowerShell / cmd.exe. Modern Windows terminals set at least one marker that
/// lets us keep the richer rendering path.
pub fn detected_legacy_windows_console_host() -> bool {
    cfg!(windows)
        && legacy_windows_console_host_env([
            std::env::var_os("WT_SESSION").as_deref(),
            std::env::var_os("ConEmuPID").as_deref(),
            std::env::var_os("TERM_PROGRAM").as_deref(),
            std::env::var_os("WEZTERM_EXECUTABLE").as_deref(),
            std::env::var_os("WEZTERM_PANE").as_deref(),
            std::env::var_os("ALACRITTY_WINDOW_ID").as_deref(),
            std::env::var_os("ANSICON").as_deref(),
            std::env::var_os("TERM").as_deref(),
        ])
}

fn legacy_windows_console_host_env(markers: [Option<&std::ffi::OsStr>; 8]) -> bool {
    fn has_value(value: Option<&std::ffi::OsStr>) -> bool {
        value.is_some_and(|v| !v.is_empty())
    }

    markers.into_iter().all(|value| !has_value(value))
}

fn normalize_optional_background_color(value: Option<&str>) -> Option<String> {
    value.and_then(|raw| normalize_background_color_setting(raw).ok().flatten())
}

fn normalize_background_color_setting(value: &str) -> Result<Option<String>> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed.to_ascii_lowercase().as_str(),
            "default" | "none" | "reset" | "off"
        )
    {
        return Ok(None);
    }

    normalize_hex_rgb_color(trimmed).map(Some).ok_or_else(|| {
        anyhow::anyhow!(
            "Failed to update setting: invalid background_color '{value}'. Expected #RRGGBB, RRGGBB, or default."
        )
    })
}

fn normalize_sidebar_focus(value: &str) -> &str {
    match value.trim().to_ascii_lowercase().as_str() {
        "pinned" | "visible" | "show" | "on" | "work" | "plan" | "todos" => "pinned",
        "tasks" | "activity" | "live" | "running" => "tasks",
        "agents" | "subagents" | "sub-agents" => "agents",
        "context" => "context",
        "sessions" | "sessions_rail" | "session_history" => "sessions",
        "hidden" | "hide" | "closed" | "off" | "none" => "hidden",
        _ => "auto",
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Resolve an environment variable as a boolean. Recognises the
/// common truthy spellings (`1`, `true`, `yes`, `on`) case-
/// insensitively. Used by [`Settings::apply_env_overrides`] for
/// platform a11y signals like `NO_ANIMATIONS`.
fn env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The override detector names the same winner `apply_env_overrides`
    /// applies: `NO_ANIMATIONS` is first in precedence and is environment,
    /// not terminal, authority.
    #[test]
    fn low_motion_override_detector_agrees_with_env_overlay() {
        let _lock = crate::test_support::lock_test_env();
        let _no_animations = crate::test_support::EnvVarGuard::set("NO_ANIMATIONS", "1");

        let detected = detect_low_motion_override();
        assert_eq!(detected, Some(MotionOverride::NoAnimationsEnv));
        assert!(detected.is_some_and(MotionOverride::is_environment));
        assert_eq!(detected.map(MotionOverride::label), Some("NO_ANIMATIONS"));

        let mut settings = Settings::default();
        assert!(!settings.low_motion);
        settings.apply_env_overrides();
        assert!(settings.low_motion, "the overlay forces low motion on");
        assert!(!settings.fancy_animations);
    }

    // -----------------------------------------------------------------------
    // Cross-process settings integrity
    // -----------------------------------------------------------------------
    //
    // The in-process mutex says nothing about a *second* Codewhale process on
    // the same home directory — a second TUI, `codewhale exec`, the runtime HTTP
    // surface in another instance. Two of those doing load/modify/save at once
    // is the same last-save-wins bug the in-process lock was added to prevent,
    // and no amount of thread-based testing can observe it: threads share the
    // mutex that makes the bug impossible. These regressions therefore drive a
    // real child process.
    //
    // The child is this same test binary, re-invoked with `--ignored --exact`
    // on the helper below. It inherits the sealed `HOME`/`CODEWHALE_HOME`
    // through its environment, so both processes resolve the same
    // `settings.toml`.

    /// Selects which child behavior [`settings_cross_process_child_helper`] runs.
    const CHILD_ROLE_ENV: &str = "CODEWHALE_TEST_SETTINGS_CHILD_ROLE";
    /// Path of the parent↔child handshake file. Its meaning is per-role: the
    /// slow writer *creates* it once its transaction is open; the reader *waits*
    /// for it as a stop signal.
    const CHILD_SIGNAL_ENV: &str = "CODEWHALE_TEST_SETTINGS_CHILD_SIGNAL";
    /// Where the child writes what it observed, for the parent to assert on.
    const CHILD_RESULT_ENV: &str = "CODEWHALE_TEST_SETTINGS_CHILD_RESULT";

    /// The other process in the cross-process regressions.
    ///
    /// Ignored so a normal `cargo test` never runs it directly; the parent tests
    /// invoke it explicitly with `--ignored --exact`. With no role set it is a
    /// no-op, so an accidental `--ignored` sweep stays green.
    #[test]
    #[ignore = "spawned as a child process by the cross-process settings regressions"]
    fn settings_cross_process_child_helper() {
        use std::time::{Duration, Instant};

        let Ok(role) = std::env::var(CHILD_ROLE_ENV) else {
            return;
        };
        // Under `cfg(test)` the settings path only honors the real environment
        // for a thread that holds this lock; without it the child would resolve
        // the isolated per-process test root and never touch the parent's file.
        // The child is a fresh process, so the acquisition is uncontended.
        let _env_lock = crate::test_support::lock_test_env();
        let inherited_home = std::env::var_os("CODEWHALE_HOME")
            .expect("settings child needs an inherited Codewhale home");
        let _state_home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", inherited_home);
        let signal = PathBuf::from(
            std::env::var(CHILD_SIGNAL_ENV).expect("child helper needs a signal path"),
        );

        match role.as_str() {
            // Hold the settings critical section open across a visible delay, so
            // the parent's transaction is guaranteed to arrive while this one is
            // mid-flight.
            "slow-writer" => {
                with_settings_transaction(|transaction| {
                    let mut settings = transaction.load()?;
                    settings.default_mode = "operate".to_string();
                    // Announce *after* the read: from here on, any parent write
                    // that is not excluded by the lock will be lost by the save
                    // below.
                    std::fs::write(&signal, b"loaded").expect("write the handshake file");
                    std::thread::sleep(Duration::from_millis(1_500));
                    transaction.save(&settings)
                })
                .expect("the child transaction must commit");
            }
            // Read the raw file as fast as possible while the parent rewrites
            // it, and report how many reads were torn.
            "reader" => {
                let result = PathBuf::from(
                    std::env::var(CHILD_RESULT_ENV).expect("reader needs a result path"),
                );
                let path = Settings::path().expect("resolve the shared settings path");
                let ready = result.with_extension("ready");
                let deadline = Instant::now() + Duration::from_secs(60);
                let (mut reads, mut torn) = (0_u64, 0_u64);

                // A ready marker must mean that the reader has actually run.
                // On Windows the child can otherwise create the marker, lose
                // its time slice, and perform no reads before the parent
                // completes every write and signals it to stop.
                loop {
                    assert!(
                        Instant::now() < deadline,
                        "reader did not observe the seeded settings file"
                    );
                    match std::fs::read_to_string(&path) {
                        Ok(raw)
                            if !raw.is_empty() && toml::from_str::<toml::Value>(&raw).is_ok() =>
                        {
                            reads += 1;
                            break;
                        }
                        Ok(_) | Err(_) => std::thread::yield_now(),
                    }
                }
                std::fs::write(&ready, b"ready").expect("announce that the reader is ready");

                while !path_exists_for_test(&signal) && Instant::now() < deadline {
                    let Ok(raw) = std::fs::read_to_string(&path) else {
                        // The file legitimately does not exist yet.
                        continue;
                    };
                    reads += 1;
                    // Both failure shapes a truncate-then-write produces: the
                    // momentarily empty file, and a prefix that stops mid-value.
                    if raw.is_empty() || toml::from_str::<toml::Value>(&raw).is_err() {
                        torn += 1;
                    }
                }
                std::fs::write(&result, format!("{reads} {torn}")).expect("write the result file");
            }
            other => panic!("unknown child role {other}"),
        }
    }

    fn path_exists_for_test(path: &Path) -> bool {
        std::fs::metadata(path).is_ok()
    }

    /// Spawn this test binary as a child running the helper above in `role`.
    fn spawn_settings_child(
        role: &str,
        home: &Path,
        signal: &Path,
        result: Option<&Path>,
    ) -> std::process::Child {
        let mut command = std::process::Command::new(
            std::env::current_exe().expect("the test binary path is the child program"),
        );
        command
            .arg("settings::tests::settings_cross_process_child_helper")
            .args(["--exact", "--ignored", "--test-threads", "1"])
            .env(CHILD_ROLE_ENV, role)
            .env(CHILD_SIGNAL_ENV, signal)
            .env("HOME", home)
            .env("USERPROFILE", home)
            .env("CODEWHALE_HOME", home.join(".codewhale"))
            .env_remove("DEEPSEEK_CONFIG_PATH")
            .env_remove("CODEWHALE_CONFIG_PATH")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(result) = result {
            command.env(CHILD_RESULT_ENV, result);
        }
        command.spawn().expect("spawn the settings child process")
    }

    fn seal_settings_home_for_test(home: &Path) -> Vec<crate::test_support::EnvVarGuard> {
        use crate::test_support::EnvVarGuard;
        vec![
            EnvVarGuard::set("HOME", home),
            EnvVarGuard::set("USERPROFILE", home),
            EnvVarGuard::set("CODEWHALE_HOME", home.join(".codewhale")),
            EnvVarGuard::remove("DEEPSEEK_CONFIG_PATH"),
            EnvVarGuard::remove("CODEWHALE_CONFIG_PATH"),
        ]
    }

    fn wait_for_file(path: &Path, what: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        while !path_exists_for_test(path) {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {what} at {}",
                path.display()
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Two processes mutating **disjoint** fields must both survive.
    ///
    /// The child opens a transaction, reads the pre-image, announces itself, and
    /// only then saves `default_mode`. The parent's `max_history` write arrives
    /// squarely inside that window. Without the cross-process lock the parent
    /// loads the same pre-image, saves, and is then overwritten wholesale by the
    /// child's later save — `max_history` silently reverts. With the lock the
    /// parent waits, re-reads the child's committed value, and both fields land.
    #[test]
    fn two_processes_mutating_disjoint_fields_do_not_last_save_wins() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _env = seal_settings_home_for_test(tmp.path());

        // A real pre-image, so "whichever saves last wins" has something to
        // revert rather than a fresh file.
        Settings::transact(|settings| settings.set("max_history", "100"))
            .expect("seed the settings file");
        let signal = tmp.path().join("child-transaction-open");

        let mut child = spawn_settings_child("slow-writer", tmp.path(), &signal, None);
        wait_for_file(&signal, "the child's open transaction");

        // The child is mid-transaction right now. This must block, not race.
        Settings::transact(|settings| settings.set("max_history", "321"))
            .expect("the parent write must land once the child releases the lock");

        let status = child.wait().expect("await the child process");
        assert!(status.success(), "the child transaction must succeed");

        let settled = Settings::load_persisted().expect("reload the shared settings");
        assert_eq!(
            settled.default_mode, "operate",
            "the child's field must survive the parent's whole-file save"
        );
        assert_eq!(
            settled.max_input_history, 321,
            "the parent's field must survive the child's whole-file save"
        );
    }

    /// A concurrent reader must never observe a half-written `settings.toml`.
    ///
    /// `fs::write` truncates before it writes, so any other process reading at
    /// the wrong moment sees an empty file or a prefix that stops mid-value —
    /// and parses it as a settings file that is simply missing everything past
    /// the tear. Writing to an adjacent temp file and renaming makes the swap
    /// atomic: a reader sees either the whole old file or the whole new one.
    #[test]
    fn concurrent_readers_never_observe_a_truncated_settings_file() {
        let _lock = crate::test_support::lock_test_env();
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let _env = seal_settings_home_for_test(tmp.path());

        // Make the file big enough that a non-atomic write has a real window.
        // A short file can be written in one syscall and hide the bug.
        Settings::transact(|settings| {
            settings.pinned_models = (0..400)
                .map(|index| PinnedModel {
                    provider: "deepseek".to_string(),
                    model: format!("pinned-model-{index:04}"),
                    label: Some(format!("Pinned model {index:04}")),
                })
                .collect();
            Ok(())
        })
        .expect("seed a large settings file");

        let stop = tmp.path().join("reader-stop");
        let result = tmp.path().join("reader-result");
        let ready = result.with_extension("ready");
        let mut child = spawn_settings_child("reader", tmp.path(), &stop, Some(&result));
        wait_for_file(&ready, "the settings reader to become ready");

        for index in 0..150 {
            Settings::transact(|settings| settings.set("max_history", &(100 + index).to_string()))
                .expect("the parent write must land");
        }

        std::fs::write(&stop, b"stop").expect("signal the reader to stop");
        let status = child.wait().expect("await the reader process");
        assert!(status.success(), "the reader must exit cleanly");

        let observed = std::fs::read_to_string(&result).expect("read the reader's report");
        let mut parts = observed.split_whitespace();
        let reads: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        let torn: u64 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
        assert!(
            reads > 0,
            "the reader observed nothing, so it proves nothing (report: {observed:?})"
        );
        assert_eq!(
            torn, 0,
            "{torn} of {reads} concurrent reads saw a truncated or unparseable settings file"
        );
    }

    #[test]
    fn focus_texture_defaults_off_and_validates() {
        let mut settings = Settings::default();
        assert_eq!(settings.focus_texture, "off");

        settings.set("focus_texture", "scrim").unwrap();
        assert_eq!(settings.focus_texture, "scrim");
        settings.set("texture", "grain").unwrap();
        assert_eq!(settings.focus_texture, "grain");
        settings.set("focus_texture", " OFF ").unwrap();
        assert_eq!(settings.focus_texture, "off");

        let err = settings.set("focus_texture", "static").unwrap_err();
        assert!(err.to_string().contains("off, scrim, or grain"));
    }

    #[test]
    fn retired_ocean_treatment_folds_into_the_underwater_theme() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "theme = \"light\"\nocean_treatment = \"deepsea\"\n")
            .expect("legacy settings");

        let settings = Settings::load_persisted_from_candidates(Some(path.clone()), None, None)
            .expect("legacy setting must remain readable");
        assert_eq!(
            settings.theme, "underwater",
            "the persisted painted field is the user-visible fact; it becomes the theme"
        );

        settings
            .save_to_path(&path)
            .expect("save normalized settings");
        let saved = std::fs::read_to_string(&path).expect("read normalized settings");
        assert!(
            !saved.contains("ocean_treatment"),
            "the retired key must not be written back: {saved}"
        );
        assert!(saved.contains("theme = \"underwater\""), "{saved}");
    }

    #[test]
    fn flat_ocean_treatment_leaves_the_theme_alone_and_is_dropped() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "theme = \"light\"\nocean_treatment = \"flat\"\n")
            .expect("legacy settings");

        let settings = Settings::load_persisted_from_candidates(Some(path.clone()), None, None)
            .expect("legacy setting must remain readable");
        assert_eq!(
            settings.theme, "light",
            "flat never opted into a painted field"
        );

        settings
            .save_to_path(&path)
            .expect("save normalized settings");
        let saved = std::fs::read_to_string(&path).expect("read normalized settings");
        assert!(!saved.contains("ocean_treatment"), "{saved}");
        assert!(saved.contains("theme = \"light\""), "{saved}");
    }

    #[test]
    fn work_surface_placement_persists_all_placements_with_bottom_default() {
        let mut settings = Settings::default();
        // Round 3 (2026-09-01): the bar's information lives under the
        // composer, so Bottom is the default.
        assert_eq!(settings.work_surface_placement, "bottom");

        for placement in ["bottom", "top", "left", "right", "off"] {
            settings
                .set("work_surface_placement", placement)
                .expect("valid placement");
            assert_eq!(settings.work_surface_placement, placement);
            let body = toml::to_string(&settings).expect("serialize settings");
            let restored: Settings = toml::from_str(&body).expect("restore settings");
            assert_eq!(restored.work_surface_placement, placement);
        }

        let err = settings
            .set("work_surface_placement", "diagonal")
            .expect_err("nonsense placement");
        assert!(err.to_string().contains("top, bottom, left, right, or off"));
        assert_eq!(settings.work_surface_placement, "off");
    }

    #[test]
    fn rail_panel_persists_every_dock_panel_and_folds_pinned_into_tasks() {
        let mut settings = Settings::default();
        assert_eq!(settings.rail_panel, "tasks");

        // Every panel the dock cycles through must survive `set` and a
        // settings.toml round trip — the dock persists all eight.
        for panel in [
            "tasks",
            "agents",
            "background",
            "files",
            "notepad",
            "context",
            "git",
            "price",
        ] {
            settings.set("rail_panel", panel).expect("valid panel");
            assert_eq!(settings.rail_panel, panel);
            let body = toml::to_string(&settings).expect("serialize settings");
            let restored: Settings = toml::from_str(&body).expect("restore settings");
            assert_eq!(restored.rail_panel, panel);
        }

        // `pinned` stays accepted as a setting word but persists as the
        // canonical tasks view, matching the load-time migration.
        settings.set("rail_panel", "agents").expect("reset panel");
        settings.set("rail_panel", "pinned").expect("pinned alias");
        assert_eq!(settings.rail_panel, "tasks");

        let err = settings
            .set("rail_panel", "auto")
            .expect_err("auto-collapse was dropped with the legacy sidebar");
        assert!(
            err.to_string()
                .contains("tasks, agents, background, files, notepad, context, git, or price")
        );
        assert_eq!(settings.rail_panel, "tasks");
    }

    #[test]
    fn work_surface_drag_sizes_round_trip_with_bounded_values() {
        let mut settings = Settings::default();
        settings.set("work_surface_top_height", "9").unwrap();
        settings.set("work_surface_side_width", "54").unwrap();
        let body = toml::to_string(&settings).expect("serialize settings");
        let restored: Settings = toml::from_str(&body).expect("restore settings");
        assert_eq!(restored.work_surface_top_height, 9);
        assert_eq!(restored.work_surface_side_width, 54);
        assert!(settings.set("work_surface_top_height", "17").is_err());
        assert!(settings.set("work_surface_top_height", "4").is_err());
        assert!(settings.set("work_surface_side_width", "25").is_err());
    }

    #[test]
    fn settings_load_migrates_unreadable_top_work_surface_height() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let settings_path = tmp.path().join("settings.toml");
        let legacy = "work_surface_placement = \"top\"\nwork_surface_top_height = 2\nrail_panel = \"pinned\"\n";
        std::fs::write(&settings_path, legacy).expect("settings");
        let _config_override =
            EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", tmp.path().join("config.toml"));

        let loaded = Settings::load().expect("load settings");

        assert_eq!(loaded.work_surface_top_height, WORK_SURFACE_TOP_HEIGHT_MIN);
        assert_eq!(loaded.work_surface_placement, "top");
        // `pinned` folded into the tasks view (2026-09-02 dock views).
        assert_eq!(loaded.rail_panel, "tasks");
        assert_eq!(
            std::fs::read_to_string(settings_path).expect("read unchanged settings"),
            legacy,
            "normalizing a legacy height at read time must not rewrite the user's file"
        );
    }

    #[test]
    fn inline_diffs_default_full_and_persist_exactly_one_mode() {
        let mut settings = Settings::default();
        assert_eq!(settings.inline_diffs, "full");
        assert_eq!(
            InlineDiffMode::parse(&settings.inline_diffs),
            InlineDiffMode::Full
        );

        for mode in ["summary", "off", "full"] {
            settings.set("inline_diffs", mode).expect("valid mode");
            assert_eq!(settings.inline_diffs, mode);
            let body = toml::to_string(&settings).expect("serialize settings");
            let restored: Settings = toml::from_str(&body).expect("restore settings");
            assert_eq!(restored.inline_diffs, mode);
        }

        let error = settings
            .set("inline_diffs", "compact")
            .expect_err("unknown mode must not be guessed");
        assert!(error.to_string().contains("full, summary, or off"));
        assert_eq!(settings.inline_diffs, "full");
    }

    #[test]
    fn thinking_highlight_is_independently_configurable_and_persisted() {
        let mut settings = Settings::default();
        assert!(settings.thinking_highlight);

        settings
            .set("thinking_highlight", "false")
            .expect("valid thinking highlight setting");
        assert!(!settings.thinking_highlight);

        let restored: Settings =
            toml::from_str(&toml::to_string(&settings).expect("serialize settings"))
                .expect("restore settings");
        assert!(!restored.thinking_highlight);
    }

    #[test]
    fn thinking_default_expanded_is_opt_in_and_persisted() {
        let mut settings = Settings::default();
        assert!(!settings.thinking_default_expanded);

        settings
            .set("thinking_default_expanded", "true")
            .expect("valid thinking expansion setting");
        assert!(settings.thinking_default_expanded);

        let restored: Settings =
            toml::from_str(&toml::to_string(&settings).expect("serialize settings"))
                .expect("restore settings");
        assert!(restored.thinking_default_expanded);
    }

    #[test]
    fn density_knobs_default_compact_and_persist() {
        let mut settings = Settings::default();
        assert_eq!(settings.thinking_preview_lines, 2);
        assert!(!settings.help_expand_groups);
        assert!(settings.pin_last_prompt);

        settings.set("thinking_preview_lines", "10").unwrap();
        settings.set("help_expand_groups", "true").unwrap();
        settings.set("pin_last_prompt", "false").unwrap();
        assert_eq!(settings.thinking_preview_lines, 10);
        assert!(settings.help_expand_groups);
        assert!(!settings.pin_last_prompt);

        let restored: Settings =
            toml::from_str(&toml::to_string(&settings).expect("serialize settings"))
                .expect("restore settings");
        assert_eq!(restored.thinking_preview_lines, 10);
        assert!(restored.help_expand_groups);
        assert!(!restored.pin_last_prompt);
    }

    /// Explicit animated baseline for env-force tests (#4095 flipped defaults to calm).
    fn animated_settings() -> Settings {
        Settings {
            calm_mode: false,
            low_motion: false,
            load_error: None,
            fancy_animations: true,
            show_tool_details: true,
            transcript_spacing: "comfortable".to_string(),
            ..Settings::default()
        }
    }

    #[test]
    fn apply_preset_calm_sets_bundle_and_preserves_evidence() {
        let mut settings = Settings::default();
        // Density is calm by default; motion is an independent axis.
        assert!(settings.calm_mode);
        assert!(!settings.show_thinking);

        let changed = settings.apply_preset("CALM").expect("calm preset applies");
        assert_eq!(
            changed,
            CALM_PRESET_FIELDS
                .iter()
                .map(|(k, _)| *k)
                .collect::<Vec<_>>()
        );

        assert!(settings.calm_mode);
        assert_eq!(settings.tool_collapse_mode, "calm");
        assert_eq!(settings.transcript_spacing, "compact");
        assert!(settings.low_motion);
        assert!(!settings.fancy_animations);
        assert!(!settings.show_tool_details);
        // Calm does not override the user's reasoning preference.
        assert!(!settings.show_thinking);
    }

    #[test]
    fn default_settings_use_comfortable_transcript_spacing() {
        let settings = Settings::default();
        assert!(settings.calm_mode);
        assert!(!settings.show_tool_details);
        assert!(!settings.low_motion);
        assert!(settings.fancy_animations);
        assert_eq!(settings.transcript_spacing, "comfortable");
        assert_eq!(settings.tool_collapse_mode, "compact");
        // Thinking is opt-in so the transcript stays focused on the chat.
        assert!(!settings.show_thinking);
    }

    #[test]
    fn behavioral_tip_impressions_are_backward_compatible_and_persist_when_seen() {
        let default_body = toml::to_string_pretty(&Settings::default()).expect("serialize");
        assert!(!default_body.contains("behavioral_tip_impressions"));

        let mut settings = Settings::default();
        settings
            .behavioral_tip_impressions
            .insert("planning_mode".to_string(), 1);
        let body = toml::to_string_pretty(&settings).expect("serialize");
        let restored: Settings = toml::from_str(&body).expect("restore settings");
        assert_eq!(
            restored
                .behavioral_tip_impressions
                .get("planning_mode")
                .copied(),
            Some(1)
        );
    }

    #[test]
    fn apply_preset_rejects_unknown_name() {
        let mut settings = Settings::default();
        let err = settings.apply_preset("turbo").expect_err("unknown preset");
        assert!(err.to_string().contains("Unknown preset"));
        assert!(preset_fields("calm").is_some());
        assert!(preset_fields("turbo").is_none());
    }

    #[test]
    fn default_settings_keep_auto_compact_as_unset_fallback() {
        let settings = Settings::default();
        // The persisted fallback remains false so a missing settings file does
        // not look like an explicit user preference. Startup resolves the
        // runtime default from the active model window unless the file contains
        // `auto_compact`.
        assert!(!settings.auto_compact);
        assert_eq!(settings.auto_compact_threshold_percent, 80.0);
        assert!(!settings.auto_compact_explicit);
    }

    #[test]
    fn auto_compact_remains_explicitly_configurable() {
        let mut settings = Settings::default();
        settings.set("auto_compact", "on").expect("enable");
        assert!(settings.auto_compact);
        assert!(settings.auto_compact_explicit);
        settings.set("auto_compact", "off").expect("disable");
        assert!(!settings.auto_compact);
    }

    #[test]
    fn unrelated_save_does_not_materialize_implicit_auto_compact_defaults() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        let settings = Settings {
            calm_mode: false,
            ..Settings::default()
        };

        settings.save_to_path(&path).expect("save settings");

        let body = std::fs::read_to_string(&path).expect("read settings");
        let document = toml::from_str::<toml::Value>(&body).expect("parse settings");
        assert!(!auto_compact_explicitly_configured_in_document(&document));
        let reloaded = Settings::load_persisted_from_candidates(Some(path), None, None)
            .expect("reload settings");
        assert!(!reloaded.auto_compact_explicit);
        assert!(!reloaded.auto_compact);
        assert!(!reloaded.calm_mode);
    }

    #[test]
    fn explicit_auto_compact_off_survives_save_and_reload() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        let mut settings = Settings::default();
        settings.set("auto_compact", "off").expect("disable");

        settings.save_to_path(&path).expect("save settings");

        assert!(auto_compact_explicitly_configured_from_candidates((
            Some(path.clone()),
            None,
            None,
        )));
        let reloaded = Settings::load_persisted_from_candidates(Some(path), None, None)
            .expect("reload settings");
        assert!(reloaded.auto_compact_explicit);
        assert!(!reloaded.auto_compact);
    }

    #[test]
    fn auto_compact_threshold_is_validated() {
        let mut settings = Settings::default();
        settings
            .set("auto_compact_threshold", "65%")
            .expect("threshold");
        assert!(settings.auto_compact, "a threshold expresses enable intent");
        assert_eq!(settings.auto_compact_threshold_percent, 65.0);
        assert!(settings.auto_compact_explicit);
        assert!(settings.set("auto_compact_threshold", "9").is_err());
        assert!(settings.set("auto_compact_threshold", "101").is_err());
    }

    #[test]
    fn threshold_only_persisted_config_enables_auto_compaction() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "auto_compact_threshold_percent = 65\n").expect("settings");

        let loaded = Settings::load_persisted_from_candidates(Some(path.clone()), None, None)
            .expect("load threshold-only settings");

        assert!(loaded.auto_compact);
        assert!(loaded.auto_compact_explicit);
        assert_eq!(loaded.auto_compact_threshold_percent, 65.0);
        assert!(auto_compact_explicitly_configured_from_candidates((
            Some(path),
            None,
            None,
        )));
    }

    #[test]
    fn explicit_auto_compact_off_overrides_a_persisted_threshold() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        std::fs::write(
            &path,
            "auto_compact = false\nauto_compact_threshold_percent = 65\n",
        )
        .expect("settings");

        let loaded = Settings::load_persisted_from_candidates(Some(path.clone()), None, None)
            .expect("load explicit opt-out");

        assert!(!loaded.auto_compact);
        assert!(loaded.auto_compact_explicit);
        assert!(auto_compact_explicitly_configured_from_candidates((
            Some(path),
            None,
            None,
        )));
    }

    #[test]
    fn default_settings_show_footer_water_strip() {
        let settings = Settings::default();
        assert!(
            settings.fancy_animations,
            "underwater presentation is the default"
        );
        assert!(!settings.low_motion);
        assert_eq!(settings.transcript_spacing, "comfortable");
    }

    #[test]
    fn retired_launch_screen_setting_is_accepted_and_dropped_on_save() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("settings.toml");
        std::fs::write(&path, "launch_screen = false\n").expect("legacy settings");

        let settings = Settings::load_persisted_from_candidates(Some(path.clone()), None, None)
            .expect("legacy setting must remain readable");
        settings
            .save_to_path(&path)
            .expect("save normalized settings");

        let saved = std::fs::read_to_string(&path).expect("read normalized settings");
        assert!(
            !saved.contains("launch_screen"),
            "the retired setting must not be written back: {saved}"
        );
    }

    #[test]
    fn legacy_sidebar_focus_migrates_to_rail_panel_and_placement() {
        let migrate = |focus: &str| {
            let mut settings = Settings {
                sidebar_focus: focus.to_string(),
                ..Settings::default()
            };
            migrate_sidebar_settings_to_rail(&mut settings);
            settings
        };

        assert_eq!(migrate("agents").rail_panel, "agents");
        assert_eq!(migrate("subagents").rail_panel, "agents");
        assert_eq!(migrate("context").rail_panel, "context");
        assert_eq!(migrate("session").rail_panel, "context");
        assert_eq!(migrate("tasks").rail_panel, "tasks");
        assert_eq!(migrate("activity").rail_panel, "tasks");
        assert_eq!(migrate("pinned").rail_panel, "pinned");
        assert_eq!(migrate("work").rail_panel, "pinned");
        // `auto` is the shipped default for `sidebar_focus`, so this arm is
        // the effective default for every upgrading user — it must land on
        // the panel that hides itself when there is nothing to show, not on
        // the always-on pinned strip.
        assert_eq!(migrate("auto").rail_panel, "tasks");
        // A hidden sidebar becomes rail placement off.
        let hidden = migrate("hidden");
        assert_eq!(hidden.work_surface_placement, "off");
        // #5141's pinned sessions panel carries forward as the first-class
        // sessions rail.
        assert!(migrate("sessions").sessions_rail);
        assert!(migrate("sessions_rail").sessions_rail);
        // An explicit `rail_panel = "tasks"` in the document wins over the
        // auto→pinned migration even though "tasks" is the default value.
        let mut explicit = Settings {
            sidebar_focus: "auto".to_string(),
            rail_panel: "tasks".to_string(),
            rail_panel_explicit: true,
            ..Settings::default()
        };
        migrate_sidebar_settings_to_rail(&mut explicit);
        assert_eq!(explicit.rail_panel, "tasks");
        // Placement panels keep their placement when the rail hides.
        let mut bottom = Settings {
            sidebar_focus: "hidden".to_string(),
            work_surface_placement: "bottom".to_string(),
            work_surface_placement_explicit: true,
            ..Settings::default()
        };
        migrate_sidebar_settings_to_rail(&mut bottom);
        // Bottom is a valid explicit placement now, so migration keeps it.
        assert_eq!(bottom.work_surface_placement, "bottom");
    }

    #[test]
    fn legacy_sidebar_width_maps_to_side_columns_and_new_keys_win() {
        let mut settings = Settings {
            sidebar_width_percent: 40,
            ..Settings::default()
        };
        migrate_sidebar_settings_to_rail(&mut settings);
        assert_eq!(settings.work_surface_side_width, 48);

        // The default percent leaves the default side width alone.
        let mut settings = Settings::default();
        migrate_sidebar_settings_to_rail(&mut settings);
        assert_eq!(settings.work_surface_side_width, 30);

        // An explicit rail panel wins over the migrated sidebar focus.
        let mut settings = Settings {
            sidebar_focus: "context".to_string(),
            rail_panel: "agents".to_string(),
            ..Settings::default()
        };
        migrate_sidebar_settings_to_rail(&mut settings);
        assert_eq!(settings.rail_panel, "agents");
    }

    #[test]
    fn reasoning_effort_setting_normalizes_and_clears() {
        let mut settings = Settings::default();
        // `xhigh` and `ultra` are their own rungs since the thinking ladder,
        // so normalizing collapses spellings *within* a tier instead of
        // folding the top three tiers into `max`.
        for (input, stored) in [
            ("xhigh", "xhigh"),
            ("ultracode", "ultra"),
            ("maximum", "max"),
            ("minimal", "low"),
        ] {
            settings
                .set("reasoning_effort", input)
                .unwrap_or_else(|error| panic!("normalize {input}: {error}"));
            assert_eq!(settings.reasoning_effort.as_deref(), Some(stored));
        }
        settings
            .set("reasoning_effort", "default")
            .expect("clear effort");
        assert!(settings.reasoning_effort.is_none());
    }

    #[test]
    fn paste_burst_detection_is_configurable_independent_of_bracketed_paste() {
        let mut settings = Settings::default();
        assert!(settings.bracketed_paste);
        assert!(settings.paste_burst_detection);

        settings
            .set("paste_burst_detection", "off")
            .expect("disable paste burst fallback");
        assert!(settings.bracketed_paste);
        assert!(!settings.paste_burst_detection);

        settings
            .set("bracketed_paste", "off")
            .expect("disable bracketed paste");
        assert!(!settings.bracketed_paste);
        assert!(!settings.paste_burst_detection);
    }

    #[test]
    fn mention_completion_caps_are_configurable() {
        let mut settings = Settings::default();
        assert_eq!(settings.mention_menu_limit, 128);
        assert_eq!(settings.mention_walk_depth, 10);
        assert_eq!(settings.mention_menu_behavior, "fuzzy");
        let mention_help = Settings::available_settings()
            .into_iter()
            .find(|(key, _)| *key == "mention_walk_depth")
            .map(|(_, desc)| desc)
            .expect("mention_walk_depth help");
        assert!(
            mention_help.contains("default 10"),
            "help text still lists the pre-v0.8.50 default: {mention_help}"
        );

        settings
            .set("mention_menu_limit", "256")
            .expect("set mention menu limit");
        settings
            .set("mention_walk_depth", "0")
            .expect("allow unlimited walk depth");
        settings
            .set("mention_menu_behavior", "browser")
            .expect("set mention menu behavior");

        assert_eq!(settings.mention_menu_limit, 256);
        assert_eq!(settings.mention_walk_depth, 0);
        assert_eq!(settings.mention_menu_behavior, "browser");

        let err = settings
            .set("mention_walk_depth", "deep")
            .expect_err("non-numeric depth should fail");
        assert!(err.to_string().contains("invalid mention_walk_depth"));

        let err = settings
            .set("mention_menu_behavior", "random")
            .expect_err("unknown mention behavior should fail");
        assert!(err.to_string().contains("invalid mention_menu_behavior"));
    }

    #[test]
    fn locale_normalizes_supported_values_and_rejects_unknowns() {
        let mut settings = Settings::default();
        for (input, expected) in [
            ("ja_JP.UTF-8", "ja"),
            ("zh-CN", "zh-Hans"),
            ("zh-TW", "zh-Hant"),
            ("zh-Hant", "zh-Hant"),
            ("es-MX", "es-419"),
            ("vi_VN.UTF-8", "vi"),
            ("ko-KR", "ko"),
            ("ca-ES", "ca"),
            ("de_DE.UTF-8", "de"),
            ("fr-FR", "fr"),
            ("id-ID", "id"),
            ("hi_IN.UTF-8", "hi"),
            ("ru-RU", "ru"),
            ("uk_UA.UTF-8", "uk"),
        ] {
            settings
                .set("locale", input)
                .unwrap_or_else(|err| panic!("set locale {input}: {err}"));
            assert_eq!(settings.locale, expected);
        }

        settings.set("language", "pt-PT").expect("set pt fallback");
        assert_eq!(settings.locale, "pt-BR");

        let err = settings
            .set("locale", "ar")
            .expect_err("Arabic is planned, not shipped");
        assert!(err.to_string().contains("invalid locale"));
    }

    #[test]
    fn default_settings_resolve_to_the_underwater_theme() {
        // Slice C: the fresh-install default is the underwater theme, end to
        // end from `Settings::default()` through theme resolution.
        let settings = Settings::default();
        assert_eq!(settings.theme, "underwater");
        let (name, id, theme) =
            crate::palette::resolve_theme_setting(&settings.theme, None).expect("default resolves");
        assert_eq!(id, crate::palette::ThemeId::Underwater);
        assert_eq!(name, "underwater");
        assert_eq!(theme.name, "underwater");
    }

    #[test]
    fn theme_normalizes_supported_values_and_rejects_unknowns() {
        let mut settings = Settings::default();
        assert_eq!(settings.theme, "underwater");

        settings.set("theme", "grayscale").expect("set grayscale");
        assert_eq!(settings.theme, "grayscale");

        settings.set("ui_theme", "black-white").expect("set alias");
        assert_eq!(settings.theme, "grayscale");

        settings.set("theme", "whale").expect("set dark alias");
        assert_eq!(settings.theme, "dark");

        settings
            .set("theme", "tokyonight")
            .expect("set community theme alias");
        assert_eq!(settings.theme, "tokyo-night");

        settings
            .set("theme", "solarized")
            .expect("set solarized alias");
        assert_eq!(settings.theme, "solarized-light");

        settings
            .set("theme", "custom:Ocean_1")
            .expect("custom selector validation must not depend on the file system");
        assert_eq!(settings.theme, "custom:ocean_1");

        let err = settings
            .set("theme", "nord")
            .expect_err("unknown theme should fail");
        assert!(err.to_string().contains("invalid theme"));
    }

    #[test]
    fn background_color_normalizes_hex_and_accepts_default() {
        let mut settings = Settings::default();
        settings
            .set("background_color", "#1A1b26")
            .expect("set custom background");
        assert_eq!(settings.background_color.as_deref(), Some("#1a1b26"));

        settings
            .set("background", "default")
            .expect("reset custom background");
        assert_eq!(settings.background_color, None);
    }

    #[test]
    fn background_color_rejects_invalid_hex() {
        let mut settings = Settings::default();
        let err = settings
            .set("background_color", "#123")
            .expect_err("short hex should fail");
        assert!(err.to_string().contains("invalid background_color"));
    }

    #[test]
    fn cost_currency_normalizes_yuan_aliases_and_rejects_unknowns() {
        let mut settings = Settings::default();
        assert_eq!(settings.cost_currency, "usd");

        settings.set("cost_currency", "yuan").expect("set yuan");
        assert_eq!(settings.cost_currency, "cny");

        settings.set("currency", "rmb").expect("set rmb");
        assert_eq!(settings.cost_currency, "cny");

        let err = settings
            .set("cost_currency", "eur")
            .expect_err("unsupported currency");
        assert!(err.to_string().contains("invalid cost currency"));
    }

    #[test]
    fn context_panel_is_configurable() {
        let mut settings = Settings::default();
        assert!(!settings.context_panel);

        settings
            .set("context_panel", "on")
            .expect("enable context panel");
        assert!(settings.context_panel);

        settings
            .set("session_panel", "off")
            .expect("disable context panel via alias");
        assert!(!settings.context_panel);
    }

    #[test]
    fn tool_collapse_mode_is_configurable() {
        let mut settings = Settings::default();
        assert_eq!(settings.tool_collapse_mode, "compact");

        settings
            .set("tool_collapse", "expanded")
            .expect("expanded mode");
        assert_eq!(settings.tool_collapse_mode, "expanded");

        settings.set("collapse", "calm-only").expect("calm alias");
        assert_eq!(settings.tool_collapse_mode, "calm");

        settings.set("collapse", "off").expect("off alias");
        assert_eq!(settings.tool_collapse_mode, "expanded");

        // Issue #3256 proposes `collapsed` as the default verbosity name;
        // accept it (and the bare verb) as an alias of the canonical `compact`.
        settings
            .set("tool_collapse", "collapsed")
            .expect("collapsed alias");
        assert_eq!(settings.tool_collapse_mode, "compact");
        settings.set("tool_collapse", "expanded").expect("reset");
        settings
            .set("tool_collapse", "collapse")
            .expect("collapse alias");
        assert_eq!(settings.tool_collapse_mode, "compact");

        let err = settings
            .set("tool_collapse", "mystery")
            .expect_err("invalid collapse mode");
        assert!(err.to_string().contains("invalid tool collapse mode"));
    }

    #[test]
    fn tool_collapse_threshold_is_not_a_settings_key() {
        // #3256: rollup min-run size stays a fixed runtime constant (3), not a
        // user setting — reject any accidental /set surface for it.
        let mut settings = Settings::default();
        let err = settings
            .set("tool_collapse_threshold", "5")
            .expect_err("threshold must not be configurable");
        assert!(
            err.to_string().contains("Unknown setting")
                || err.to_string().contains("unknown setting")
                || err.to_string().contains("Failed to update"),
            "unexpected error: {err}"
        );
        assert_eq!(settings.tool_collapse_mode, "compact");
        assert!(!settings.show_tool_details);
    }

    #[test]
    fn display_localizes_header_and_config_file_label() {
        let settings = Settings::default();
        let en = settings.display(crate::localization::Locale::En);
        assert!(en.contains("Settings:"), "english header missing:\n{en}");
        assert!(
            en.contains("Config file:"),
            "english config label missing:\n{en}"
        );

        let zh = settings.display(crate::localization::Locale::ZhHans);
        assert!(zh.contains("设置"), "chinese header missing:\n{zh}");
        assert!(
            zh.contains("配置文件"),
            "chinese config label missing:\n{zh}"
        );
    }

    #[test]
    fn display_separates_deepseek_fallback_from_provider_scoped_models() {
        let mut settings = Settings {
            default_provider: Some("zai".to_string()),
            default_model: Some("deepseek-v4-pro".to_string()),
            ..Settings::default()
        };
        settings.set_model_for_provider("zai", "GLM-5.2");
        settings.set_model_for_provider("deepseek", "deepseek-v4-flash");

        let display = settings.display(crate::localization::Locale::En);

        assert!(display.contains("deepseek_fallback:  deepseek-v4-pro"));
        assert!(display.contains("default_provider:   zai"));
        assert!(display.contains("    zai: GLM-5.2"));
        assert!(display.contains("    deepseek: deepseek-v4-flash"));
        assert!(!display.contains("  default_model:"));
    }

    #[test]
    fn provider_model_selection_additively_enables_models() {
        let mut settings = Settings::default();

        settings.set_model_for_provider("openrouter", "anthropic/claude-sonnet-4");
        settings.enable_model_for_provider("openrouter", "qwen/qwen3.7-plus");
        settings.enable_model_for_provider("openrouter", "QWEN/QWEN3.7-PLUS");
        settings.enable_model_for_provider("openrouter", "auto");

        assert_eq!(
            settings
                .provider_models
                .as_ref()
                .and_then(|models| models.get("openrouter")),
            Some(&"anthropic/claude-sonnet-4".to_string())
        );
        assert_eq!(
            settings
                .enabled_models
                .as_ref()
                .and_then(|models| models.get("openrouter")),
            Some(&vec![
                "anthropic/claude-sonnet-4".to_string(),
                "qwen/qwen3.7-plus".to_string(),
            ])
        );

        let encoded = toml::to_string(&settings).expect("serialize enabled models");
        let decoded: Settings = toml::from_str(&encoded).expect("deserialize enabled models");
        assert_eq!(decoded.enabled_models, settings.enabled_models);
    }

    /// Tests that mutate process-global `NO_ANIMATIONS` serialise
    /// through this guard so the cargo parallel runner doesn't
    /// observe interleaved overrides. Uses the process-wide test env
    /// lock so this serializes with the TERM_PROGRAM tests too —
    /// otherwise a `NO_ANIMATIONS=1` leak from this test family can
    /// flip a concurrent `TERM_PROGRAM=iTerm` test's `low_motion`
    /// assertion through the shared `apply_env_overrides` path.
    fn no_animations_test_guard() -> crate::test_support::TestEnvLock {
        crate::test_support::lock_test_env()
    }

    #[test]
    fn no_animations_env_forces_low_motion_on() {
        let _g = no_animations_test_guard();
        // SAFETY: tests in this group serialise through the guard.
        unsafe {
            std::env::set_var("NO_ANIMATIONS", "1");
        }
        let mut settings = animated_settings();
        assert!(!settings.low_motion, "default is animated");
        assert!(settings.fancy_animations, "default shows the water strip");
        settings.apply_env_overrides();
        assert!(settings.low_motion, "NO_ANIMATIONS=1 forces low_motion");
        assert!(
            !settings.fancy_animations,
            "NO_ANIMATIONS=1 keeps fancy off"
        );
        // SAFETY: cleanup under the guard.
        unsafe {
            std::env::remove_var("NO_ANIMATIONS");
        }
    }

    #[test]
    fn no_animations_env_overrides_user_opt_in() {
        let _g = no_animations_test_guard();
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("NO_ANIMATIONS", "true");
        }
        // User had explicitly opted into fancy animations on disk.
        let mut settings = Settings {
            fancy_animations: true,
            ..Settings::default()
        };
        settings.apply_env_overrides();
        assert!(
            !settings.fancy_animations,
            "platform NO_ANIMATIONS overrides user-opt-in fancy_animations"
        );
        assert!(settings.low_motion);
        // SAFETY: cleanup under the guard.
        unsafe {
            std::env::remove_var("NO_ANIMATIONS");
        }
    }

    #[test]
    fn no_animations_env_recognises_truthy_spellings_only() {
        let _g = no_animations_test_guard();
        let prev_wt_session = std::env::var_os("WT_SESSION");
        let prev_tmux = std::env::var_os("TMUX");
        let prev_sty = std::env::var_os("STY");
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        let prev_term = std::env::var_os("TERM");
        let prev_ssh_client = std::env::var_os("SSH_CLIENT");
        let prev_ssh_tty = std::env::var_os("SSH_TTY");
        let prev_tilix_id = std::env::var_os("TILIX_ID");
        let prev_terminator_uuid = std::env::var_os("TERMINATOR_UUID");

        // The test is about NO_ANIMATIONS only. On Windows CI, an unmarked
        // console host now independently enables low_motion, so mark the host
        // as non-legacy while checking falsy spellings.
        // Clear multiplexer markers for the same reason: they also force
        // low_motion independently of NO_ANIMATIONS.
        // Clear TERM_PROGRAM, SSH, and other terminal-specific variables as they
        // also force low_motion independently of NO_ANIMATIONS.
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::remove_var("TMUX");
            std::env::remove_var("STY");
            std::env::remove_var("TERM_PROGRAM");
            std::env::remove_var("TERM");
            std::env::remove_var("SSH_CLIENT");
            std::env::remove_var("SSH_TTY");
            std::env::remove_var("TILIX_ID");
            std::env::remove_var("TERMINATOR_UUID");
        }
        #[cfg(windows)]
        unsafe {
            std::env::set_var("WT_SESSION", "test");
        }
        for truthy in ["1", "true", "True", "YES", "on"] {
            // SAFETY: serialised by the guard.
            unsafe {
                std::env::set_var("NO_ANIMATIONS", truthy);
            }
            let mut s = animated_settings();
            s.apply_env_overrides();
            assert!(s.low_motion, "{truthy:?} should be truthy");
        }
        for falsy in ["0", "false", "no", "off", ""] {
            // SAFETY: serialised by the guard.
            unsafe {
                std::env::set_var("NO_ANIMATIONS", falsy);
            }
            let mut s = animated_settings();
            s.apply_env_overrides();
            assert!(!s.low_motion, "{falsy:?} should be falsy");
        }
        // SAFETY: cleanup under the guard.
        unsafe {
            std::env::remove_var("NO_ANIMATIONS");
            match prev_wt_session {
                Some(v) => std::env::set_var("WT_SESSION", v),
                None => std::env::remove_var("WT_SESSION"),
            }
            match prev_tmux {
                Some(v) => std::env::set_var("TMUX", v),
                None => std::env::remove_var("TMUX"),
            }
            match prev_sty {
                Some(v) => std::env::set_var("STY", v),
                None => std::env::remove_var("STY"),
            }
            match prev_term_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
            match prev_ssh_client {
                Some(v) => std::env::set_var("SSH_CLIENT", v),
                None => std::env::remove_var("SSH_CLIENT"),
            }
            match prev_ssh_tty {
                Some(v) => std::env::set_var("SSH_TTY", v),
                None => std::env::remove_var("SSH_TTY"),
            }
            match prev_tilix_id {
                Some(v) => std::env::set_var("TILIX_ID", v),
                None => std::env::remove_var("TILIX_ID"),
            }
            match prev_terminator_uuid {
                Some(v) => std::env::set_var("TERMINATOR_UUID", v),
                None => std::env::remove_var("TERMINATOR_UUID"),
            }
        }
    }

    /// Serialise tests that mutate `TERM_PROGRAM` through this guard.
    /// Uses the process-wide test env lock so this serializes not just
    /// with itself but with every other env-mutating test in the suite
    /// — otherwise a concurrent test that calls `animated_settings()`
    /// can read whatever value our two `set_var`s have raced into the
    /// env at that instant.
    fn term_program_test_guard() -> crate::test_support::TestEnvLock {
        crate::test_support::lock_test_env()
    }

    #[test]
    fn vscode_uses_calm_rendering_without_changing_text_cadence() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "vscode");
        }
        let mut settings = animated_settings();
        assert!(!settings.low_motion, "default is animated");
        settings.apply_env_overrides();
        assert!(
            settings.low_motion,
            "TERM_PROGRAM=vscode must disable decorative motion"
        );
        assert!(!settings.fancy_animations);
        assert!(
            settings.constrained_frame_rate,
            "TERM_PROGRAM=vscode should cap redraws without changing animation semantics"
        );
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn ghostty_term_program_keeps_full_motion_without_the_legacy_30_fps_cap() {
        let _g = term_program_test_guard();
        // Neutralize the SSH markers: production intentionally caps motion
        // over SSH, and the suite routinely runs inside one.
        let _ssh_client = crate::test_support::EnvVarGuard::remove("SSH_CLIENT");
        let _ssh_connection = crate::test_support::EnvVarGuard::remove("SSH_CONNECTION");
        let _ssh_tty = crate::test_support::EnvVarGuard::remove("SSH_TTY");
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "Ghostty");
        }
        let mut settings = animated_settings();
        assert!(!settings.low_motion, "default is animated");
        settings.apply_env_overrides();
        assert!(!settings.low_motion);
        assert!(settings.fancy_animations);
        assert!(!settings.constrained_frame_rate);
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn ghostty_term_fallback_keeps_full_motion_without_the_legacy_30_fps_cap() {
        let _g = term_program_test_guard();
        // Neutralize the SSH markers: production intentionally caps motion
        // over SSH, and the suite routinely runs inside one.
        let _ssh_client = crate::test_support::EnvVarGuard::remove("SSH_CLIENT");
        let _ssh_connection = crate::test_support::EnvVarGuard::remove("SSH_CONNECTION");
        let _ssh_tty = crate::test_support::EnvVarGuard::remove("SSH_TTY");
        let prev_program = std::env::var_os("TERM_PROGRAM");
        let prev_term = std::env::var_os("TERM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("TERM", "xterm-ghostty");
        }
        let mut settings = Settings::default();
        settings.apply_env_overrides();
        assert!(!settings.low_motion);
        assert!(settings.fancy_animations);
        assert!(!settings.constrained_frame_rate);
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
        }
    }

    #[test]
    fn non_vscode_term_program_does_not_force_low_motion() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        let prev_term = std::env::var_os("TERM");
        let prev_ssh_client = std::env::var_os("SSH_CLIENT");
        let prev_ssh_tty = std::env::var_os("SSH_TTY");
        let prev_tilix_id = std::env::var_os("TILIX_ID");
        let prev_terminator_uuid = std::env::var_os("TERMINATOR_UUID");
        let prev_tmux = std::env::var_os("TMUX");
        let prev_sty = std::env::var_os("STY");
        // SAFETY: serialised by the guard. Clear SSH_* so a real
        // SSH session running the test suite doesn't make this
        // assertion trivially fail — the SSH path is exercised
        // separately by `ssh_session_forces_low_motion_on`.
        unsafe {
            std::env::remove_var("SSH_CLIENT");
            std::env::remove_var("SSH_TTY");
            std::env::remove_var("TERM");
            std::env::remove_var("TILIX_ID");
            std::env::remove_var("TERMINATOR_UUID");
            std::env::remove_var("TMUX");
            std::env::remove_var("STY");
        }
        for program in ["iTerm.app", "Apple_Terminal", "WezTerm", "xterm-256color"] {
            // SAFETY: serialised by the guard.
            unsafe {
                std::env::set_var("TERM_PROGRAM", program);
            }
            let mut s = animated_settings();
            s.apply_env_overrides();
            assert!(
                !s.low_motion,
                "TERM_PROGRAM={program:?} should not force low_motion"
            );
        }
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_term {
                Some(v) => std::env::set_var("TERM", v),
                None => std::env::remove_var("TERM"),
            }
            if let Some(v) = prev_ssh_client {
                std::env::set_var("SSH_CLIENT", v);
            }
            if let Some(v) = prev_ssh_tty {
                std::env::set_var("SSH_TTY", v);
            }
            if let Some(v) = prev_tilix_id {
                std::env::set_var("TILIX_ID", v);
            }
            if let Some(v) = prev_terminator_uuid {
                std::env::set_var("TERMINATOR_UUID", v);
            }
            if let Some(v) = prev_tmux {
                std::env::set_var("TMUX", v);
            }
            if let Some(v) = prev_sty {
                std::env::set_var("STY", v);
            }
        }
    }

    #[test]
    fn tilix_and_terminator_cap_redraws_without_disabling_motion() {
        let _g = term_program_test_guard();
        // Neutralize the SSH markers: production intentionally caps motion
        // over SSH, and the suite routinely runs inside one.
        let _ssh_client = crate::test_support::EnvVarGuard::remove("SSH_CLIENT");
        let _ssh_connection = crate::test_support::EnvVarGuard::remove("SSH_CONNECTION");
        let _ssh_tty = crate::test_support::EnvVarGuard::remove("SSH_TTY");
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        let prev_tilix_id = std::env::var_os("TILIX_ID");
        let prev_terminator_uuid = std::env::var_os("TERMINATOR_UUID");
        let prev_wt_session = std::env::var_os("WT_SESSION");

        for (var, val) in [
            ("TILIX_ID", "d5b5b5d6-tilix-session"),
            ("TERMINATOR_UUID", "urn:uuid:terminator-session"),
        ] {
            // SAFETY: serialised by the guard.
            unsafe {
                std::env::remove_var("TERM_PROGRAM");
                std::env::remove_var("TILIX_ID");
                std::env::remove_var("TERMINATOR_UUID");
                std::env::set_var(var, val);
                // A native Windows test process without any modern-terminal
                // marker is intentionally treated as legacy ConHost. This
                // test isolates the VTE signal instead, so keep that separate
                // platform heuristic from changing its motion assertions.
                #[cfg(windows)]
                std::env::set_var("WT_SESSION", "codewhale-test");
            }
            let mut settings = animated_settings();
            assert!(!settings.low_motion, "default is animated");
            settings.apply_env_overrides();
            assert!(
                settings.constrained_frame_rate,
                "{var} must cap redraws to prevent VTE flicker (#1470)"
            );
            assert!(
                !settings.low_motion,
                "{var} must not change motion semantics"
            );
            assert!(
                settings.fancy_animations,
                "{var} must not disable the ocean treatment"
            );
        }

        // SAFETY: cleanup under the guard.
        unsafe {
            match prev_term_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_tilix_id {
                Some(v) => std::env::set_var("TILIX_ID", v),
                None => std::env::remove_var("TILIX_ID"),
            }
            match prev_terminator_uuid {
                Some(v) => std::env::set_var("TERMINATOR_UUID", v),
                None => std::env::remove_var("TERMINATOR_UUID"),
            }
            match prev_wt_session {
                Some(v) => std::env::set_var("WT_SESSION", v),
                None => std::env::remove_var("WT_SESSION"),
            }
        }
    }

    #[test]
    fn termius_term_program_forces_low_motion_on() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "Termius");
        }
        let mut settings = animated_settings();
        assert!(!settings.low_motion, "default is animated");
        settings.apply_env_overrides();
        assert!(
            settings.low_motion,
            "TERM_PROGRAM=Termius must enable low_motion to prevent flickering (#1433)"
        );
        assert!(
            !settings.fancy_animations,
            "TERM_PROGRAM=Termius must disable fancy_animations"
        );
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn legacy_windows_console_host_detects_unmarked_shell() {
        assert!(legacy_windows_console_host_env([
            None, None, None, None, None, None, None, None
        ]));
    }

    #[test]
    fn legacy_windows_console_host_excludes_modern_terminal_markers() {
        use std::ffi::OsStr;

        let marker = Some(OsStr::new("1"));
        assert!(!legacy_windows_console_host_env([
            marker, None, None, None, None, None, None, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, marker, None, None, None, None, None, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, None, marker, None, None, None, None, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, None, None, marker, None, None, None, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, None, None, None, marker, None, None, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, None, None, None, None, marker, None, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, None, None, None, None, None, marker, None
        ]));
        assert!(!legacy_windows_console_host_env([
            None, None, None, None, None, None, None, marker
        ]));
    }

    #[cfg(windows)]
    #[test]
    fn unmarked_windows_console_forces_calm_rendering() {
        let _g = term_program_test_guard();
        let vars = [
            "WT_SESSION",
            "ConEmuPID",
            "TERM_PROGRAM",
            "WEZTERM_EXECUTABLE",
            "WEZTERM_PANE",
            "ALACRITTY_WINDOW_ID",
            "ANSICON",
            "TERM",
            "SSH_CLIENT",
            "SSH_TTY",
            "NO_ANIMATIONS",
            "PTYXIS_VERSION",
        ];
        let prev: Vec<_> = vars
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();

        // SAFETY: serialised by the guard.
        unsafe {
            for name in vars {
                std::env::remove_var(name);
            }
        }

        let mut settings = animated_settings();
        assert!(!settings.low_motion, "default is animated");
        assert!(settings.fancy_animations, "default shows the water strip");
        assert_eq!(settings.synchronized_output, "auto");
        settings.apply_env_overrides();
        assert!(settings.low_motion);
        assert!(!settings.fancy_animations);
        assert!(
            settings.bracketed_paste,
            "env-only conhost fallback must not persistently mutate bracketed_paste (#1102)"
        );
        assert!(
            !settings.effective_bracketed_paste(),
            "legacy Windows console hosts do not support crossterm bracketed paste (#1102)"
        );
        assert_eq!(settings.synchronized_output, "off");

        // SAFETY: cleanup under the guard.
        unsafe {
            for (name, value) in prev {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn ssh_session_forces_low_motion_on() {
        let _g = term_program_test_guard();
        let prev_client = std::env::var_os("SSH_CLIENT");
        let prev_tty = std::env::var_os("SSH_TTY");
        let prev_term_program = std::env::var_os("TERM_PROGRAM");
        for (var, val) in [
            ("SSH_CLIENT", "192.168.1.100 50000 22"),
            ("SSH_TTY", "/dev/pts/0"),
        ] {
            // SAFETY: serialised by the guard.
            unsafe {
                std::env::remove_var("SSH_CLIENT");
                std::env::remove_var("SSH_TTY");
                // Clear TERM_PROGRAM so the test isolates the SSH signal
                // — otherwise a leaked `TERM_PROGRAM=vscode` from a
                // concurrent test would already have forced low_motion
                // and the SSH-only assertion below would be a tautology.
                std::env::remove_var("TERM_PROGRAM");
                std::env::set_var(var, val);
            }
            let mut s = Settings::default();
            s.apply_env_overrides();
            assert!(
                s.low_motion,
                "{var}={val:?} must enable low_motion to prevent flickering in SSH sessions (#1433)"
            );
            assert!(
                !s.fancy_animations,
                "{var}={val:?} must disable fancy_animations in SSH sessions (#1433)"
            );
        }
        // SAFETY: cleanup under the guard.
        unsafe {
            std::env::remove_var("SSH_CLIENT");
            std::env::remove_var("SSH_TTY");
            if let Some(v) = prev_client {
                std::env::set_var("SSH_CLIENT", v);
            }
            if let Some(v) = prev_tty {
                std::env::set_var("SSH_TTY", v);
            }
            match prev_term_program {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn terminal_multiplexer_caps_redraws_without_disabling_motion() {
        let _g = term_program_test_guard();
        let vars = [
            "TMUX",
            "STY",
            "TERM_PROGRAM",
            "SSH_CLIENT",
            "SSH_TTY",
            "TILIX_ID",
            "TERMINATOR_UUID",
            "NO_ANIMATIONS",
            "WT_SESSION",
        ];
        let prev: Vec<_> = vars
            .iter()
            .map(|name| (*name, std::env::var_os(name)))
            .collect();

        for (var, val) in [
            ("TMUX", "/tmp/tmux-501/default,1234,0"),
            ("STY", "1234.pts-0.host"),
        ] {
            // SAFETY: serialised by the guard.
            unsafe {
                for name in vars {
                    std::env::remove_var(name);
                }
                std::env::set_var(var, val);
                #[cfg(windows)]
                std::env::set_var("WT_SESSION", "codewhale-test");
            }
            let mut settings = animated_settings();
            assert!(!settings.low_motion, "default is animated");
            assert!(settings.fancy_animations, "default shows the water strip");
            settings.apply_env_overrides();
            assert!(!settings.low_motion, "{var} must preserve authored motion");
            assert!(
                settings.fancy_animations,
                "{var} must preserve Ocean motion"
            );
            assert!(
                settings.constrained_frame_rate,
                "{var}={val:?} must cap redraws under terminal multiplexers"
            );
        }

        // SAFETY: cleanup under the guard.
        unsafe {
            for (name, value) in prev {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // synchronized_output / Ptyxis flicker detection
    // ────────────────────────────────────────────────────────────────────────

    #[test]
    fn synchronized_output_defaults_to_auto_and_resolves_to_enabled() {
        let s = Settings::default();
        assert_eq!(s.synchronized_output, "auto");
        assert!(
            s.synchronized_output_enabled(),
            "auto must keep DEC 2026 on so terminals that support it stay tear-free"
        );
    }

    #[test]
    fn synchronized_output_off_disables_dec_2026() {
        let s = Settings {
            synchronized_output: "off".to_string(),
            ..Settings::default()
        };
        assert!(!s.synchronized_output_enabled());
    }

    #[test]
    fn synchronized_output_on_keeps_dec_2026_enabled() {
        let s = Settings {
            synchronized_output: "on".to_string(),
            ..Settings::default()
        };
        assert!(s.synchronized_output_enabled());
    }

    #[test]
    fn synchronized_output_set_command_accepts_aliases() {
        let mut s = Settings::default();
        for value in ["auto", "AUTO", "default"] {
            s.set("synchronized_output", value).expect("valid");
            assert_eq!(s.synchronized_output, "auto");
        }
        for value in ["on", "true", "yes", "1", "ENABLED"] {
            s.set("sync_output", value).expect("valid");
            assert_eq!(s.synchronized_output, "on");
        }
        for value in ["off", "false", "no", "0", "DISABLED"] {
            s.set("sync", value).expect("valid");
            assert_eq!(s.synchronized_output, "off");
        }
        let err = s
            .set("synchronized_output", "maybe")
            .expect_err("unknown value rejected");
        assert!(
            err.to_string().contains("synchronized_output"),
            "error names the offending key: {err}"
        );
    }

    #[test]
    fn composer_multiline_mode_defaults_off_and_accepts_boolean_aliases() {
        let mut settings = Settings::default();
        assert!(!settings.composer_multiline_mode);

        settings.set("multiline", "on").expect("enable multiline");
        assert!(settings.composer_multiline_mode);

        settings
            .set("composer_multiline_mode", "false")
            .expect("disable multiline");
        assert!(!settings.composer_multiline_mode);
    }

    #[test]
    fn ptyxis_term_program_flips_synchronized_output_off() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        let prev_ptyxis = std::env::var_os("PTYXIS_VERSION");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "Ptyxis");
            std::env::remove_var("PTYXIS_VERSION");
        }
        let mut s = Settings::default();
        assert_eq!(s.synchronized_output, "auto");
        s.apply_env_overrides();
        assert_eq!(
            s.synchronized_output, "off",
            "Ptyxis 50.x mishandles DEC 2026 — auto must flip to off so VTE 0.84 stops flickering"
        );
        assert!(
            !s.synchronized_output_enabled(),
            "resolved boolean must agree with stored string"
        );
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_ptyxis {
                Some(v) => std::env::set_var("PTYXIS_VERSION", v),
                None => std::env::remove_var("PTYXIS_VERSION"),
            }
        }
    }

    #[test]
    fn tabby_uses_calm_rendering_for_stable_ime_cursor() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "Tabby");
        }
        let mut settings = animated_settings();
        settings.apply_env_overrides();
        assert!(settings.low_motion);
        assert!(!settings.fancy_animations);
        assert!(settings.constrained_frame_rate);
        assert_eq!(settings.synchronized_output, "off");
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn ptyxis_version_env_alone_flips_synchronized_output_off() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        let prev_ptyxis = std::env::var_os("PTYXIS_VERSION");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::remove_var("TERM_PROGRAM");
            std::env::set_var("PTYXIS_VERSION", "50.1");
        }
        let mut s = Settings::default();
        s.apply_env_overrides();
        assert_eq!(
            s.synchronized_output, "off",
            "PTYXIS_VERSION alone is sufficient — Ptyxis sets this even when TERM_PROGRAM isn't propagated"
        );
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_ptyxis {
                Some(v) => std::env::set_var("PTYXIS_VERSION", v),
                None => std::env::remove_var("PTYXIS_VERSION"),
            }
        }
    }

    #[test]
    fn ptyxis_does_not_override_user_explicit_on() {
        // Users who set `synchronized_output = "on"` (e.g. to confirm a
        // Ptyxis upgrade fixed it) must keep DEC 2026 even on Ptyxis.
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "ptyxis");
        }
        let mut s = Settings {
            synchronized_output: "on".to_string(),
            ..Settings::default()
        };
        s.apply_env_overrides();
        assert_eq!(
            s.synchronized_output, "on",
            "explicit user override must beat the Ptyxis env heuristic"
        );
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn ptyxis_does_not_override_user_explicit_off() {
        // A user with `synchronized_output = "off"` on a non-Ptyxis
        // terminal stays off after env detection (no-op flip).
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        // SAFETY: serialised by the guard.
        unsafe {
            std::env::set_var("TERM_PROGRAM", "xterm-256color");
        }
        let mut s = Settings {
            synchronized_output: "off".to_string(),
            ..Settings::default()
        };
        s.apply_env_overrides();
        assert_eq!(s.synchronized_output, "off");
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
        }
    }

    #[test]
    fn non_ptyxis_term_programs_keep_synchronized_output_auto() {
        let _g = term_program_test_guard();
        let prev = std::env::var_os("TERM_PROGRAM");
        let prev_ptyxis = std::env::var_os("PTYXIS_VERSION");
        // SAFETY: clean slate so non-Ptyxis programs don't see a leaked
        // PTYXIS_VERSION from another test.
        unsafe {
            std::env::remove_var("PTYXIS_VERSION");
        }
        for program in [
            "iTerm.app",
            "Apple_Terminal",
            "WezTerm",
            "xterm-256color",
            "gnome-terminal-server",
            // The Ghostty / VS Code paths keep DEC 2026 enabled; both handle
            // synchronized output cleanly even though their motion policies
            // differ.
            "ghostty",
            "vscode",
        ] {
            // SAFETY: serialised by the guard.
            unsafe {
                std::env::set_var("TERM_PROGRAM", program);
            }
            let mut s = Settings::default();
            s.apply_env_overrides();
            assert_eq!(
                s.synchronized_output, "auto",
                "TERM_PROGRAM={program:?} must not opt out of DEC 2026"
            );
            assert!(
                s.synchronized_output_enabled(),
                "resolved boolean for {program:?} must stay enabled"
            );
        }
        // SAFETY: cleanup under the guard.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("TERM_PROGRAM", v),
                None => std::env::remove_var("TERM_PROGRAM"),
            }
            match prev_ptyxis {
                Some(v) => std::env::set_var("PTYXIS_VERSION", v),
                None => std::env::remove_var("PTYXIS_VERSION"),
            }
        }
    }

    // ────────────────────────────────────────────────────────────────────────
    // TuiPrefs tests
    // ────────────────────────────────────────────────────────────────────────

    /// Serialise tests that mutate `DEEPSEEK_CONFIG_PATH` through this guard
    /// so the parallel test runner doesn't observe interleaved env values.
    fn config_path_test_guard() -> crate::test_support::TestEnvLock {
        crate::test_support::lock_test_env()
    }

    /// The shared guard, under this module's historical name.
    ///
    /// It was a byte-for-byte copy of `EnvVarGuard` until #5359 gave the shared
    /// one a second job: recording which variables a test actually redirected,
    /// so state-path resolution can tell a sealed environment from a test that
    /// holds the lock for unrelated reasons. A private copy silently opts every
    /// caller here out of that record.
    use crate::test_support::EnvVarGuard as EnvVarRestore;

    #[test]
    fn startup_mode_writes_accept_act_plan_operate() {
        let mut settings = Settings::default();

        settings.set("default_mode", "plan").expect("plan mode");
        assert_eq!(settings.default_mode, "plan");
        settings
            .set("default_mode", "normal")
            .expect("legacy normal alias remains harmless");
        assert_eq!(settings.default_mode, "agent");
        settings
            .set("default_mode", "operate")
            .expect("operate is a valid startup mode");
        assert_eq!(settings.default_mode, "operate");
        settings
            .set("default_mode", "act")
            .expect("act alias maps to agent wire value");
        assert_eq!(settings.default_mode, "agent");

        let err = settings
            .set("default_mode", "yolo")
            .expect_err("yolo remains a permission migration alias, not a mode write");
        assert!(
            err.to_string().contains("act (agent), plan, or operate"),
            "{err}"
        );
    }

    #[test]
    fn legacy_startup_modes_migrate_without_losing_permission_intent() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let codewhale_home = tmp.path().join(".codewhale");
        std::fs::create_dir_all(&codewhale_home).expect("codewhale home");
        std::fs::write(
            codewhale_home.join("settings.toml"),
            "default_mode = \"yolo\"\n",
        )
        .expect("legacy settings");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", &codewhale_home);
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let loaded = Settings::load_persisted().expect("load legacy settings");

        assert_eq!(loaded.default_mode, "agent");
        assert_eq!(loaded.permission_posture.as_deref(), Some("full-access"));

        std::fs::write(
            codewhale_home.join("settings.toml"),
            "default_mode = \"operate\"\n",
        )
        .expect("operate startup settings");
        let loaded = Settings::load_persisted().expect("load operate settings");
        assert_eq!(loaded.default_mode, "operate");
        assert_eq!(loaded.permission_posture, None);
    }

    #[test]
    fn settings_path_defaults_to_codewhale_home_for_new_writes() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", tmp.path().join(".codewhale"));
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let got = Settings::path().expect("settings path");

        assert_eq!(got, tmp.path().join(".codewhale").join("settings.toml"));
    }

    #[test]
    fn settings_path_prefers_codewhale_home_even_when_legacy_exists() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let legacy_dir = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
        std::fs::write(legacy_dir.join("settings.toml"), "low_motion = true\n")
            .expect("legacy settings");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", tmp.path().join(".codewhale"));
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let got = Settings::path().expect("settings path");

        assert_eq!(got, tmp.path().join(".codewhale").join("settings.toml"));
    }

    #[test]
    fn settings_load_migrates_legacy_deepseek_home_into_codewhale_home_without_explicit_home() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join(".codewhale").join("settings.toml");
        let legacy_dir = tmp.path().join(".deepseek");
        let legacy_home = legacy_dir.join("settings.toml");
        std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
        std::fs::write(&legacy_home, "low_motion = true\n").expect("legacy settings");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::remove("CODEWHALE_HOME");
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let loaded = Settings::load_persisted().expect("load persisted settings");

        assert!(loaded.low_motion, "legacy settings should still be read");
        assert!(
            primary.exists(),
            "settings load should migrate to primary path"
        );
        let display = loaded.display(crate::localization::Locale::En);
        assert!(
            display.contains(&format!("Config file: {}", primary.display())),
            "settings display should surface the canonical codewhale path:\n{display}"
        );
    }

    #[test]
    fn settings_load_read_only_reads_legacy_home_without_creating_primary() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join(".codewhale").join("settings.toml");
        let legacy = tmp.path().join(".deepseek").join("settings.toml");
        let legacy_bytes =
            b"default_mode = \"plan\"\nlow_motion = false\nfancy_animations = true\n";
        std::fs::create_dir_all(legacy.parent().expect("legacy parent")).expect("legacy directory");
        std::fs::write(&legacy, legacy_bytes).expect("legacy settings");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::remove("CODEWHALE_HOME");
        let _home = EnvVarRestore::set("HOME", tmp.path());
        let _no_animations = EnvVarRestore::set("NO_ANIMATIONS", "1");

        let loaded = Settings::load_read_only().expect("read-only settings load");

        assert_eq!(loaded.default_mode, "plan");
        assert!(loaded.low_motion, "environment overlays still apply");
        assert!(
            !loaded.fancy_animations,
            "environment overlays still apply to parsed legacy settings"
        );
        assert!(
            !primary.exists(),
            "a diagnostic settings read must not create the primary settings path"
        );
        assert_eq!(
            std::fs::read(&legacy).expect("legacy settings after read"),
            legacy_bytes,
            "a diagnostic settings read must not rewrite the legacy settings file"
        );
    }

    #[test]
    fn settings_load_migrates_platform_legacy_fallback_into_codewhale_home_without_explicit_home() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join(".codewhale").join("settings.toml");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home =
            EnvVarRestore::set("CODEWHALE_HOME", primary.parent().expect("primary parent"));
        let legacy_config_dir = tmp
            .path()
            .join("platform-config")
            .join("deepseek")
            .join("settings.toml");
        std::fs::create_dir_all(legacy_config_dir.parent().expect("parent"))
            .expect("legacy config dir");
        std::fs::write(&legacy_config_dir, "low_motion = true\n").expect("legacy settings");

        // Exercise the same load and migration path with explicit candidates.
        // `dirs::config_dir()` uses the Win32 known-folder API on Windows, so
        // APPDATA/XDG environment overrides cannot isolate that process-global
        // location in a parallel test runner.
        let loaded = Settings::load_persisted_from_candidates(
            Some(primary.clone()),
            None,
            Some(legacy_config_dir),
        )
        .expect("load persisted settings");

        assert!(loaded.low_motion, "legacy settings should still be read");
        assert!(
            primary.exists(),
            "legacy fallback should be copied into primary"
        );
        let display = loaded.display(crate::localization::Locale::En);
        assert!(
            display.contains(&format!("Config file: {}", primary.display())),
            "settings display should surface the canonical codewhale path:\n{display}"
        );
    }

    #[test]
    fn settings_load_ignores_legacy_files_when_codewhale_home_is_explicit() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let explicit_home = tmp.path().join("isolated-codewhale");
        let legacy_dir = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
        std::fs::write(
            legacy_dir.join("settings.toml"),
            "theme = \"dracula\"\ncomposer_density = \"spacious\"\nsidebar_width_percent = 42\n",
        )
        .expect("legacy settings");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", &explicit_home);
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let loaded = Settings::load().expect("load settings");

        assert_eq!(
            loaded.theme, "underwater",
            "explicit CODEWHALE_HOME must not inherit ambient legacy settings"
        );
        assert_eq!(
            loaded.composer_density, "comfortable",
            "explicit CODEWHALE_HOME must not inherit ambient legacy settings"
        );
        assert_eq!(
            loaded.sidebar_width_percent, 28,
            "explicit CODEWHALE_HOME must not inherit ambient legacy settings"
        );
        assert!(
            !explicit_home.join("settings.toml").exists(),
            "ambient legacy settings must not be migrated into explicit CODEWHALE_HOME"
        );
    }

    #[test]
    fn settings_load_migrates_legacy_saved_auto_sidebar_focus_to_rail() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let settings_path = tmp.path().join("settings.toml");
        std::fs::write(&settings_path, "sidebar_focus = \"auto\"\n").expect("settings");
        let _config_override =
            EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", tmp.path().join("config.toml"));

        let loaded = Settings::load().expect("load settings");

        // A settings.toml that only names `sidebar_focus = "auto"` — the
        // shipped default — must not silently earn an always-on rail strip.
        assert_eq!(loaded.rail_panel, "tasks");
        assert_eq!(loaded.work_surface_placement, "bottom");
    }

    #[test]
    fn settings_load_migrates_hidden_sidebar_to_rail_off() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let settings_path = tmp.path().join("settings.toml");
        std::fs::write(&settings_path, "sidebar_focus = \"hidden\"\n").expect("settings");
        let _config_override =
            EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", tmp.path().join("config.toml"));

        let loaded = Settings::load().expect("load settings");

        assert_eq!(loaded.work_surface_placement, "off");
    }

    #[test]
    fn hidden_legacy_sidebar_does_not_override_an_explicit_new_rail_placement() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let settings_path = tmp.path().join("settings.toml");
        std::fs::write(
            &settings_path,
            "sidebar_focus = \"hidden\"\nwork_surface_placement = \"left\"\n",
        )
        .expect("settings");
        let _config_override =
            EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", tmp.path().join("config.toml"));

        let loaded = Settings::load().expect("load settings");

        assert_eq!(loaded.work_surface_placement, "left");
    }

    #[test]
    fn tui_prefs_path_defaults_to_codewhale_home_for_new_writes() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", tmp.path().join(".codewhale"));
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let got = TuiPrefs::path().expect("tui prefs path");

        assert_eq!(got, tmp.path().join(".codewhale").join("tui.toml"));
    }

    #[test]
    fn tui_prefs_path_ignores_legacy_home_when_codewhale_home_is_explicit() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let explicit_home = tmp.path().join("isolated-codewhale");
        let legacy_dir = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
        std::fs::write(legacy_dir.join("tui.toml"), "theme = \"light\"\n").expect("legacy prefs");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", &explicit_home);
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let got = TuiPrefs::path().expect("tui prefs path");

        assert_eq!(got, explicit_home.join("tui.toml"));
    }

    #[test]
    fn tui_prefs_path_reads_legacy_deepseek_home_when_present() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let primary = tmp.path().join(".codewhale").join("tui.toml");
        let legacy_dir = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
        let legacy_home = legacy_dir.join("tui.toml");
        std::fs::write(&legacy_home, "theme = \"light\"\n").expect("legacy prefs");

        let got = resolve_tui_prefs_path_from_candidates(Some(primary), Some(legacy_home.clone()))
            .expect("tui prefs path");

        assert_eq!(got, legacy_home);
    }

    #[test]
    fn tui_prefs_defaults_inherit_the_terminal_zero_font() {
        let prefs = TuiPrefs::default();
        assert_eq!(prefs.theme, "underwater");
        assert_eq!(prefs.font_size, 0);
        assert!(prefs.keybinds.submit.is_none());
        assert!(prefs.keybinds.new_line.is_none());
    }

    #[test]
    fn tui_prefs_validate_accepts_known_themes() {
        for theme in [
            "terminal",
            "dark",
            "light",
            "system",
            "grayscale",
            "catppuccin-mocha",
            "tokyo-night",
            "dracula",
            "gruvbox-dark",
            "solarized-light",
        ] {
            let mut prefs = TuiPrefs {
                theme: theme.to_string(),
                ..TuiPrefs::default()
            };
            prefs
                .validate()
                .unwrap_or_else(|e| panic!("validate({theme}) failed: {e}"));
            assert_eq!(prefs.theme, theme);
        }
    }

    #[test]
    fn tui_prefs_validate_normalises_theme_case() {
        let mut prefs = TuiPrefs {
            theme: "MONO".to_string(),
            ..TuiPrefs::default()
        };
        prefs
            .validate()
            .expect("MONO should normalise to grayscale");
        assert_eq!(prefs.theme, "grayscale");
    }

    #[test]
    fn tui_prefs_validate_rejects_unknown_theme() {
        let mut prefs = TuiPrefs {
            theme: "nord".to_string(),
            ..TuiPrefs::default()
        };
        let err = prefs.validate().expect_err("nord is not a valid theme");
        assert!(err.to_string().contains("invalid theme 'nord'"));
        assert!(err.to_string().contains("custom:<name>"));
    }

    #[test]
    fn tui_prefs_validate_custom_selector_without_loading_file() {
        let mut prefs = TuiPrefs {
            theme: "custom:Ocean_1".to_string(),
            ..TuiPrefs::default()
        };
        prefs
            .validate()
            .expect("selector validation must not depend on the file system");
        assert_eq!(prefs.theme, "custom:ocean_1");
    }

    #[test]
    fn tui_prefs_round_trips_through_toml() {
        let prefs = TuiPrefs {
            theme: "light".to_string(),
            font_size: 16,
            keybinds: KeybindPrefs {
                submit: Some("ctrl+enter".to_string()),
                new_line: Some("enter".to_string()),
                command_palette: None,
                cancel: None,
                toggle_sidebar: None,
            },
        };
        let serialised = toml::to_string_pretty(&prefs).expect("serialise");
        let de: TuiPrefs = toml::from_str(&serialised).expect("deserialise");
        assert_eq!(de.theme, "light");
        assert_eq!(de.font_size, 16);
        assert_eq!(de.keybinds.submit.as_deref(), Some("ctrl+enter"));
        assert_eq!(de.keybinds.new_line.as_deref(), Some("enter"));
        assert!(de.keybinds.command_palette.is_none());
    }

    #[test]
    fn tui_prefs_load_returns_defaults_when_file_absent() {
        let _g = config_path_test_guard();
        // Point config path at a non-existent location so tui.toml is absent.
        let tmp = std::env::temp_dir().join("dst_tui_prefs_absent_test");
        std::fs::create_dir_all(&tmp).unwrap();
        let _config_override = EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", tmp.join("config.toml"));
        let prefs = TuiPrefs::load().expect("load should not fail when file absent");
        assert_eq!(
            prefs.theme, "underwater",
            "should fall back to default theme"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tui_prefs_save_and_load_round_trip() {
        let _g = config_path_test_guard();
        let tmp = std::env::temp_dir().join("dst_tui_prefs_save_test");
        std::fs::create_dir_all(&tmp).unwrap();
        let _config_override = EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", tmp.join("config.toml"));

        let prefs = TuiPrefs {
            theme: "light".to_string(),
            font_size: 14,
            keybinds: KeybindPrefs {
                submit: Some("ctrl+enter".to_string()),
                ..KeybindPrefs::default()
            },
        };
        prefs.save().expect("save should succeed");

        let loaded = TuiPrefs::load().expect("load after save");
        assert_eq!(loaded.theme, "light");
        assert_eq!(loaded.font_size, 14);
        assert_eq!(loaded.keybinds.submit.as_deref(), Some("ctrl+enter"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tui_prefs_save_preserves_comments() {
        let _g = config_path_test_guard();
        let tmp = std::env::temp_dir().join("dst_tui_prefs_comment_test");
        std::fs::create_dir_all(&tmp).unwrap();
        let config_file = tmp.join("config.toml");
        let _config_override = EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", &config_file);

        // tui.toml lives next to config.toml
        let tui_path = tmp.join("tui.toml");
        std::fs::write(
            &tui_path,
            "# my theme comment\ntheme = \"dark\"\n# footer note\n",
        )
        .unwrap();

        let prefs = TuiPrefs {
            theme: "light".to_string(),
            ..TuiPrefs::default()
        };
        prefs.save().expect("save should succeed");

        let body = std::fs::read_to_string(&tui_path).expect("read tui.toml");
        assert!(body.contains("# my theme comment"), "comment lost: {body}");
        assert!(body.contains("# footer note"), "footer lost: {body}");
        assert!(body.contains("light"), "new value not written: {body}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn settings_save_preserves_comments() {
        let _g = config_path_test_guard();
        let tmp = std::env::temp_dir().join("dst_settings_comment_test");
        std::fs::create_dir_all(&tmp).unwrap();
        let config_file = tmp.join("config.toml");
        let _config_override = EnvVarRestore::set("DEEPSEEK_CONFIG_PATH", &config_file);

        // settings.toml lives next to config.toml
        let settings_path = tmp.join("settings.toml");
        std::fs::write(
            &settings_path,
            "# my setting\ncost_currency = \"usd\"\n# trailing\n",
        )
        .unwrap();

        // Load the existing file so we have a real struct to modify.
        let mut settings = Settings::load().expect("load settings");
        settings.cost_currency = "cny".to_string();
        settings.save().expect("save should succeed");

        let body = std::fs::read_to_string(&settings_path).expect("read settings.toml");
        assert!(body.contains("# my setting"), "comment lost: {body}");
        assert!(body.contains("# trailing"), "trailing lost: {body}");
        assert!(body.contains("cny"), "new value not written: {body}");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn tui_prefs_path_uses_home_codewhale_subdir_by_default() {
        let _g = config_path_test_guard();
        let tmp = tempfile::tempdir().expect("tempdir");
        let _config_override = EnvVarRestore::remove("DEEPSEEK_CONFIG_PATH");
        let _codewhale_home = EnvVarRestore::set("CODEWHALE_HOME", tmp.path().join(".codewhale"));
        let _home = EnvVarRestore::set("HOME", tmp.path());

        let got = TuiPrefs::path().expect("path should resolve");

        assert_eq!(got, tmp.path().join(".codewhale").join("tui.toml"));
    }

    #[test]
    fn pinned_models_are_exact_ordered_and_round_trip() {
        let mut settings = Settings::default();
        assert!(settings.toggle_pinned_model("zai", "glm-5.2"));
        assert!(settings.toggle_pinned_model("openrouter", "glm-5.2"));
        assert_eq!(settings.pinned_models[0].provider, "zai");
        assert!(settings.move_pinned_model("openrouter", "glm-5.2", -1));
        assert_eq!(settings.pinned_models[0].provider, "openrouter");
        assert!(settings.set_pinned_model_label("openrouter", "glm-5.2", Some("fast".to_string())));
        let encoded = toml::to_string(&settings).unwrap();
        let decoded: Settings = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.pinned_models, settings.pinned_models);
        assert!(!settings.toggle_pinned_model("openrouter", "glm-5.2"));
        assert_eq!(settings.pinned_models.len(), 1);
    }
}
