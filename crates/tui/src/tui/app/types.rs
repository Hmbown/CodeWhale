//! Plain data types shared across the TUI: modes, effort/collapse/display
//! enums, the public `TuiOptions` construction bag, queued-message records,
//! and the action enums drained by the event loop.
//!
//! Everything here is pure data (plus parsing/labeling helpers that need no
//! `App` state). All items are re-exported from `app.rs` so existing
//! `crate::tui::app::X` paths are unchanged.

use super::*;

/// What an interactive setting selection actually did.
///
/// The three cases are genuinely different to the user, and the boolean this
/// replaced conflated the last two: a refused selection and an accepted one
/// that only wrote the startup default both returned `false`, so every caller
/// reported "already in that mode" and showed no receipt for the write.
///
/// Only [`Self::Changed`] means live session state moved — that is the case
/// that must still emit an `AppAction` so the engine is resynchronized.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSelection {
    /// Live state moved, and the startup default was persisted.
    Changed,
    /// Live state already matched, and the startup default was persisted. This
    /// is the normal shape after a session restore, where the live value and
    /// the startup default legitimately disagree.
    PersistedSame,
    /// Refused by the turn lock (#2982). Nothing was written anywhere.
    Refused,
}

impl SettingSelection {
    /// Whether live state moved — i.e. whether the engine needs resyncing.
    #[must_use]
    pub fn changed_live_state(self) -> bool {
        matches!(self, Self::Changed)
    }

    /// Whether the selection was accepted at all (either case that persisted).
    #[must_use]
    #[cfg(test)]
    pub fn accepted(self) -> bool {
        !matches!(self, Self::Refused)
    }
}

/// Supported application modes for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppMode {
    Agent,
    #[allow(dead_code)]
    Auto,
    /// Legacy compatibility alias; resolves to [`Self::Agent`] + bypass approvals.
    Yolo,
    Plan,
    Operate,
}

/// Reasoning-effort tier, mirrored across DeepSeek and Codex effort pickers.
///
/// The config file accepts all six string values for forward-compat with
/// providers that expose the full spectrum; DeepSeek currently collapses
/// `Low`/`Medium` → `high`. OpenAI Codex normalizes inherited DeepSeek-only
/// `Off` to `Low` and displays/sends `Max` as `xhigh` at the provider
/// boundary. The default keyboard cycler walks the three DeepSeek-distinct
/// tiers: `Off` → `High` → `Max` → `Off`; provider-aware callers should use
/// [`ReasoningEffort::cycle_next_for_provider`]. Auto routing has no concrete
/// provider yet, so [`ReasoningEffort::cycle_next_for_auto_model`] retains the
/// full provider-neutral preference vocabulary until dispatch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Off,
    Low,
    Medium,
    High,
    Auto,
    #[default]
    Max,
}

/// Provider-effective reasoning state used by durable receipts and visible
/// requested-to-effective labels.
///
/// Some routes, notably first-party GLM-5-Turbo, support a thinking toggle but
/// publish no effort tiers. Keeping that state distinct prevents a requested
/// `max` from being displayed or persisted as an effective `max` claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectiveReasoningEffort {
    Tier(ReasoningEffort),
    ThinkingEnabledGranularityUnavailable,
    Unavailable,
}

/// Exact provider/model route whose prompt can be inspected or replayed.
///
/// Auto-model sessions keep `model == "auto"` as the user's selection, so
/// cache operations must carry the last concrete route separately. The base
/// URL is absent after restoring an older session because saved Auto receipts
/// intentionally do not persist raw endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheReplayTarget {
    pub(crate) provider: ApiProvider,
    pub(crate) provider_identity: String,
    /// Additive exact provider id used by persisted-route resolution.
    /// `None` is meaningful for the legacy root-level `custom` route.
    pub(crate) provider_id: Option<String>,
    pub(crate) model: String,
    pub(crate) base_url: Option<String>,
}

impl EffectiveReasoningEffort {
    /// Reconstruct a safe request tier for cache replay and inspection.
    ///
    /// Routes with an enabled-but-untiered receipt collapse every non-Off
    /// request to the same wire toggle, so High is the canonical value that
    /// keeps reasoning enabled without claiming a granular effective tier.
    #[must_use]
    pub(crate) const fn request_tier_for_replay(self) -> Option<ReasoningEffort> {
        match self {
            Self::Tier(tier) => Some(tier),
            Self::ThinkingEnabledGranularityUnavailable => Some(ReasoningEffort::High),
            Self::Unavailable => None,
        }
    }
}

impl From<EffectiveReasoningEffort> for crate::work_graph::ReasoningEffortTier {
    fn from(value: EffectiveReasoningEffort) -> Self {
        match value {
            EffectiveReasoningEffort::Tier(tier) => tier.into(),
            EffectiveReasoningEffort::ThinkingEnabledGranularityUnavailable => {
                Self::ThinkingEnabledGranularityUnavailable
            }
            EffectiveReasoningEffort::Unavailable => Self::Unavailable,
        }
    }
}

impl From<crate::work_graph::ReasoningEffortTier> for EffectiveReasoningEffort {
    fn from(value: crate::work_graph::ReasoningEffortTier) -> Self {
        use crate::work_graph::ReasoningEffortTier as Tier;
        match value {
            Tier::Off => Self::Tier(ReasoningEffort::Off),
            Tier::Low => Self::Tier(ReasoningEffort::Low),
            Tier::Medium => Self::Tier(ReasoningEffort::Medium),
            Tier::High => Self::Tier(ReasoningEffort::High),
            Tier::Auto => Self::Tier(ReasoningEffort::Auto),
            Tier::Max => Self::Tier(ReasoningEffort::Max),
            Tier::ThinkingEnabledGranularityUnavailable => {
                Self::ThinkingEnabledGranularityUnavailable
            }
            Tier::Unavailable => Self::Unavailable,
        }
    }
}

impl From<ReasoningEffort> for crate::work_graph::ReasoningEffortTier {
    fn from(value: ReasoningEffort) -> Self {
        match value {
            ReasoningEffort::Off => Self::Off,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::Auto => Self::Auto,
            ReasoningEffort::Max => Self::Max,
        }
    }
}

impl ReasoningEffort {
    /// Parse an operator-supplied effort value.
    ///
    /// This is deliberately the one canonical spelling table for every
    /// human-facing route.  Callers that read an old persisted config may use
    /// [`Self::from_setting`] for its compatibility fallback, but a new CLI,
    /// settings, or tool input must reject an unknown value instead of quietly
    /// turning it into `max`.
    pub fn parse_strict(value: &str) -> Result<Self, String> {
        let trimmed = value.trim();
        match trimmed.to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" | "false" => Ok(Self::Off),
            "low" | "minimum" | "minimal" | "light" => Ok(Self::Low),
            "medium" | "mid" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "auto" | "automatic" => Ok(Self::Auto),
            "max" | "maximum" | "xhigh" | "ultra" | "ultracode" => Ok(Self::Max),
            _ => Err(format!(
                "Unrecognized reasoning effort {trimmed:?}. Expected: auto, off, low, medium, high, or max."
            )),
        }
    }

    /// Parse a persisted config-file string into an effort tier. Unknown
    /// legacy values fall back to the default (`Max`) so an old malformed
    /// settings file never prevents startup.  New user input should use
    /// [`Self::parse_strict`] instead.
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        Self::parse_strict(value).unwrap_or_default()
    }

    #[must_use]
    pub fn from_setting_for_provider(value: &str, provider: ApiProvider) -> Self {
        Self::from_setting(value).normalize_for_provider(provider)
    }

    /// Canonical lowercase label used for config storage and UI hints.
    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Auto => "auto",
            Self::Max => "max",
        }
    }

    /// Short label for the header chip.
    #[must_use]
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "med",
            Self::High => "high",
            Self::Auto => "auto",
            Self::Max => "max",
        }
    }

    /// Provider-facing label for user-visible surfaces.
    #[must_use]
    pub fn display_label_for_provider(self, provider: ApiProvider) -> &'static str {
        match (provider, self.normalize_for_provider(provider)) {
            (ApiProvider::OpenaiCodex, Self::Low) => "low",
            (ApiProvider::OpenaiCodex, Self::Medium) => "medium",
            (ApiProvider::OpenaiCodex, Self::High) => "high",
            (ApiProvider::OpenaiCodex, Self::Max) => "xhigh",
            (_, effort) => effort.short_label(),
        }
    }

    /// Value forwarded to the engine/client. `None` means "provider default"
    /// (for `Off` we still emit `"off"` so the client can inject
    /// `thinking = {"type": "disabled"}`).
    #[must_use]
    pub fn api_value(self) -> Option<&'static str> {
        Some(self.as_setting())
    }

    #[must_use]
    pub fn normalize_for_provider(self, provider: ApiProvider) -> Self {
        if provider != ApiProvider::OpenaiCodex {
            return self;
        }
        match self {
            Self::Off => Self::Low,
            Self::Auto => Self::Medium,
            other => other,
        }
    }

    /// Resolve an effort against the exact provider route that will receive
    /// the request. Both K3 routes are always-thinking, so `off` becomes the
    /// lowest supported tier. The Kimi Code membership route otherwise keeps
    /// its low/high/max mapping; direct Moonshot K3 additionally maps `medium`
    /// to `high`. First-party DeepSeek routes keep `low` (the wire documents
    /// low/high/max) while rounding `medium` up to `high`. Generic Moonshot
    /// and every other non-Codex route retain the historic high coercion.
    /// This intentionally does not change [`Self::normalize_for_provider`],
    /// whose generic wire semantics are used by older callers that do not yet
    /// have a route receipt.
    #[must_use]
    pub fn normalize_for_route(
        self,
        provider: ApiProvider,
        base_url: &str,
        wire_model: &str,
    ) -> Self {
        let normalized = self.normalize_for_provider(provider);
        if crate::config::is_exact_kimi_code_k3_route(provider, base_url, wire_model) {
            return match normalized {
                Self::Off => Self::Low,
                other => other,
            };
        }
        if crate::config::is_exact_direct_moonshot_k3_route(provider, base_url, wire_model) {
            return match normalized {
                Self::Off => Self::Low,
                Self::Medium => Self::High,
                other => other,
            };
        }
        if provider == ApiProvider::OpenaiCodex {
            return normalized;
        }
        // First-party DeepSeek routes document `reasoning_effort` low/high/max
        // on the wire (no medium), so `low` is a real, cheaper tier there and
        // must reach the wire as low; `medium` rounds up to high because the
        // dialect has no such value (#52).
        if matches!(provider, ApiProvider::Deepseek | ApiProvider::DeepseekCN) {
            return match normalized {
                Self::Low => Self::Low,
                Self::Medium => Self::High,
                other => other,
            };
        }
        match normalized {
            Self::Low | Self::Medium => Self::High,
            other => other,
        }
    }

    #[must_use]
    pub fn api_value_for_provider(self, provider: ApiProvider) -> Option<&'static str> {
        if provider != ApiProvider::OpenaiCodex {
            return self.api_value();
        }
        Some(match self.normalize_for_provider(provider) {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Max => "xhigh",
            Self::Off => "low",
            Self::Auto => "medium",
        })
    }

    /// Provider-facing value after exact-route normalization.
    #[must_use]
    pub fn api_value_for_route(
        self,
        provider: ApiProvider,
        base_url: &str,
        wire_model: &str,
    ) -> Option<&'static str> {
        self.normalize_for_route(provider, base_url, wire_model)
            .api_value_for_provider(provider)
    }

    #[must_use]
    pub fn as_setting_for_provider(self, provider: ApiProvider) -> &'static str {
        self.api_value_for_provider(provider)
            .unwrap_or_else(|| self.as_setting())
    }

    /// Persist the canonical setting after exact-route normalization.
    #[must_use]
    pub fn as_setting_for_route(
        self,
        provider: ApiProvider,
        base_url: &str,
        wire_model: &str,
    ) -> &'static str {
        self.normalize_for_route(provider, base_url, wire_model)
            .as_setting_for_provider(provider)
    }

    /// Cycle through the three behaviorally distinct tiers.
    #[must_use]
    pub fn cycle_next(self) -> Self {
        match self {
            Self::Off => Self::High,
            Self::Auto => Self::Off,
            Self::Low | Self::Medium | Self::High => Self::Max,
            Self::Max => Self::Off,
        }
    }

    #[must_use]
    pub fn cycle_next_for_provider(self, provider: ApiProvider) -> Self {
        if provider != ApiProvider::OpenaiCodex {
            return self.cycle_next();
        }
        match self.normalize_for_provider(provider) {
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Max,
            Self::Max => Self::Low,
            Self::Off | Self::Auto => Self::Low,
        }
    }

    /// Cycle the unresolved auto-model preference without applying any
    /// provider's normalization rules prematurely.
    #[must_use]
    pub fn cycle_next_for_auto_model(self) -> Self {
        match self {
            Self::Auto => Self::Off,
            Self::Off => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::Max,
            Self::Max => Self::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerDensity {
    Compact,
    Comfortable,
    Spacious,
}

impl ComposerDensity {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" | "tight" => Self::Compact,
            "spacious" | "loose" => Self::Spacious,
            _ => Self::Comfortable,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptSpacing {
    Compact,
    Comfortable,
    Spacious,
}

impl TranscriptSpacing {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "compact" | "tight" => Self::Compact,
            "spacious" | "loose" => Self::Spacious,
            _ => Self::Comfortable,
        }
    }
}

/// Controls how dense tool-call runs are collapsed in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCollapseMode {
    /// Collapse qualifying tool runs by default.
    ///
    /// Collapsed success cells keep the tool-name + arg/command summary as the
    /// single intent line (#3256 decision): that is already the model-visible
    /// call summary, so a second "intent" source is not required.
    Compact,
    /// Never collapse tool runs automatically.
    Expanded,
    /// Collapse only when calm mode is active.
    Calm,
}

impl ToolCollapseMode {
    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "expanded" | "off" | "none" => Self::Expanded,
            "calm" | "calm-mode" | "calm_only" | "calm-only" => Self::Calm,
            // `collapsed`/`collapse` are issue #3256's preferred names for the
            // default; treat them like the canonical `compact`.
            _ => Self::Compact,
        }
    }

    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Expanded => "expanded",
            Self::Calm => "calm",
        }
    }

    #[must_use]
    pub fn is_active(self, calm_mode: bool) -> bool {
        match self {
            Self::Compact => true,
            Self::Expanded => false,
            Self::Calm => calm_mode,
        }
    }
}

impl AppMode {
    /// Productive keyboard cycle: Plan -> Act -> Operate -> Plan.
    ///
    /// `Auto` remains an internal variant while the real implementation is
    /// redesigned; do not expose it through user-facing mode selection (#3733).
    /// `Yolo` is kept for parse/back-compat only and is not in the Tab cycle.
    /// Operate joins the visible cycle because ordinary messages can now
    /// coordinate background workers without requiring a Workflow definition.
    pub const CYCLE: [Self; 3] = [Self::Plan, Self::Agent, Self::Operate];

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "agent" | "act" | "auto" | "1" => Some(Self::Agent),
            "plan" | "2" => Some(Self::Plan),
            "operate" | "operation" | "ops" | "3" => Some(Self::Operate),
            // Invisible one-way permission shorthand only — never a visible mode.
            "yolo" | "4" | "bypass" | "bypass-permissions" | "bypasspermissions" => {
                Some(Self::Yolo)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn from_setting(value: &str) -> Self {
        // Unreleased Multitask never shipped; normalize leftover settings to Operate.
        match value.trim().to_ascii_lowercase().as_str() {
            "multitask" | "multi" | "5" => Self::Operate,
            other => Self::parse(other).unwrap_or(Self::Agent),
        }
    }

    #[must_use]
    pub fn as_setting(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Auto => "agent",
            // Write current permission vocabulary, not the legacy YOLO label.
            Self::Yolo => "agent",
            Self::Plan => "plan",
            Self::Operate => "operate",
        }
    }

    /// Short label used in the UI footer.
    pub fn label(self) -> &'static str {
        match self {
            AppMode::Agent => "ACT",
            AppMode::Auto => "ACT",
            AppMode::Yolo => "ACT",
            AppMode::Plan => "PLAN",
            AppMode::Operate => "OPERATE",
        }
    }

    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            AppMode::Agent => "Act",
            AppMode::Auto => "Act",
            AppMode::Yolo => "Act",
            AppMode::Plan => "Plan",
            AppMode::Operate => "Operate",
        }
    }

    #[must_use]
    pub fn number(self) -> char {
        match self {
            AppMode::Agent | AppMode::Auto | AppMode::Yolo => '1',
            AppMode::Plan => '2',
            AppMode::Operate => '3',
        }
    }

    #[must_use]
    pub fn uses_agent_baseline(self) -> bool {
        matches!(self, Self::Agent | Self::Auto | Self::Operate)
    }

    /// Operate gets a higher parallel launch floor so background fan-out is
    /// not throttled to a single slot when config is low.
    #[must_use]
    pub fn mode_delegation_launch_floor(self) -> usize {
        match self {
            Self::Operate => 4,
            _ => 1,
        }
    }

    /// Localized short name for the mode picker (user-facing surface only).
    #[must_use]
    pub fn display_name_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                AppMode::Agent | AppMode::Auto | AppMode::Yolo => MessageId::AppModeAgent,
                AppMode::Plan => MessageId::AppModePlan,
                AppMode::Operate => MessageId::AppModeOperate,
            },
        )
    }

    /// Localized one-line hint for the mode picker (user-facing surface only).
    #[must_use]
    pub fn picker_hint_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                AppMode::Agent | AppMode::Auto | AppMode::Yolo => MessageId::AppModeAgentHint,
                AppMode::Plan => MessageId::AppModePlanHint,
                AppMode::Operate => MessageId::AppModeOperateHint,
            },
        )
    }

    #[allow(dead_code)]
    /// Description shown in help or onboarding text.
    pub fn description(self) -> &'static str {
        match self {
            AppMode::Agent | AppMode::Auto => {
                "Act mode - direct work in the current session with tools"
            }
            AppMode::Yolo => "Act mode with Full Access (legacy compatibility setting)",
            AppMode::Plan => "Plan mode - research and design before implementing",
            AppMode::Operate => "Operate mode - send tasks while Fleet workers run in parallel",
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        let Some(index) = Self::CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Agent;
        };
        Self::CYCLE[(index + 1) % Self::CYCLE.len()]
    }

    #[must_use]
    pub fn previous(self) -> Self {
        let Some(index) = Self::CYCLE.iter().position(|mode| *mode == self) else {
            return Self::Agent;
        };
        Self::CYCLE[(index + Self::CYCLE.len() - 1) % Self::CYCLE.len()]
    }
}

/// Configuration required to bootstrap the TUI.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct TuiOptions {
    pub model: String,
    pub workspace: PathBuf,
    pub config_path: Option<PathBuf>,
    pub config_profile: Option<String>,
    pub allow_shell: bool,
    /// Use the alternate screen buffer (fullscreen TUI).
    pub use_alt_screen: bool,
    /// Capture mouse input for internal scrolling/selection.
    pub use_mouse_capture: bool,
    /// Enable terminal bracketed-paste mode (OSC `?2004h` / `?2004l`). Defaults
    /// on; settable via `bracketed_paste = false` in `settings.toml` for the
    /// rare terminal that mishandles it.
    pub use_bracketed_paste: bool,
    /// Maximum number of concurrent sub-agents.
    pub max_subagents: usize,
    #[allow(dead_code)]
    pub skills_dir: PathBuf,
    #[allow(dead_code)]
    pub memory_path: PathBuf,
    #[allow(dead_code)]
    pub notes_path: PathBuf,
    #[allow(dead_code)]
    pub mcp_config_path: PathBuf,
    #[allow(dead_code)]
    pub use_memory: bool,
    /// Start in agent mode (defaults to agent; --yolo starts in YOLO)
    pub start_in_agent_mode: bool,
    /// Skip onboarding screens
    pub skip_onboarding: bool,
    /// Auto-approve tool executions (yolo mode)
    pub yolo: bool,
    /// Resume a previous session by ID
    pub resume_session_id: Option<String>,
    /// Pre-populate the composer with this text when the TUI starts.
    /// Used by `deepseek pr <N>` (#451) to drop the model into a
    /// session with the PR context already typed — the user can edit
    /// before sending or hit Enter to fire as-is.
    pub initial_input: Option<InitialInput>,
    /// One-line receipt to show once at startup.
    ///
    /// Auto-resume uses this to say what it did — reattached, or fell back to
    /// a fresh transcript because the candidate was missing, unreadable, or
    /// recorded against a different workspace (#2934). Silence is the correct
    /// value when nothing happened worth reporting.
    pub startup_notice: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InitialInput {
    /// Pre-populate the composer and wait for the user to press Enter.
    ///
    /// Used by `codewhale pr <N>` (#451) to drop the model into a session
    /// with the PR context already typed so the user can edit before sending.
    Prefill(String),
    /// Pre-populate the composer, submit it once startup is ready, then keep
    /// the interactive session open for follow-up messages (#2370).
    Submit(String),
    /// Begin account-owned web remote control after the TUI is initialized.
    RemoteControl,
}

// === Sub-state structs for App field organization (#377) ===

/// Vim modal editing mode for the composer input area.
///
/// Enabled via `[composer] mode = "vim"` in `settings.toml`.  When the
/// composer vim mode is active the user starts in `Normal` mode and presses
/// `i`, `a`, or `o` to enter `Insert` mode.  `Esc` from `Insert` returns to
/// `Normal`.  Standard vim motions (`h`/`j`/`k`/`l`, `w`/`b`, `0`/`$`, `x`,
/// `dd`) work in `Normal` mode.  `Visual` is reserved for future selection
/// support and currently behaves like `Normal`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VimMode {
    /// Normal / command mode — motions and operators, no text insertion.
    #[default]
    Normal,
    /// Insert mode — characters are appended at the cursor as typed.
    Insert,
    /// Visual mode — reserved for future selection support.
    Visual,
}

impl VimMode {}

/// Message queued while the engine is busy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
    pub display: String,
    pub skill_instruction: Option<String>,
    pub skill_provenance: Option<crate::plugins::types::PluginAuthority>,
}

/// How a freshly-typed user input should be sent.
///
/// Picked by [`App::decide_composer_submit`] when the user submits a
/// non-empty composer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitDisposition {
    /// Engine idle and online: send immediately.
    Immediate,
    /// Park on `queued_messages` (offline, or engine busy — #382).
    Queue,
    /// Amend the active turn immediately (#382).
    Steer,
    /// Park on `queued_messages` for dispatch after TurnComplete.
    /// Legacy path; #382 unified busy states under `Queue`.
    #[allow(dead_code)]
    QueueFollowUp,
}

/// Enter-shaped gestures understood by the composer state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerSubmitChord {
    Enter,
    CtrlEnter,
}

/// The complete result of resolving a submit gesture against composer state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerSubmitAction {
    Submit(SubmitDisposition),
    /// Promote the oldest already-queued message into the active turn.
    SendQueuedNow,
    Noop,
}

/// Detailed tool payload attached to a history cell.
#[derive(Debug, Clone)]
pub struct ToolDetailRecord {
    pub tool_id: String,
    pub tool_name: String,
    pub input: Value,
    pub output: Option<String>,
}

/// Lightweight task view for sidebar rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskPanelEntry {
    pub id: String,
    pub status: String,
    pub prompt_summary: String,
    pub duration_ms: Option<u64>,
    pub kind: TaskPanelEntryKind,
    pub stale: bool,
    pub elapsed_since_output_ms: Option<u64>,
    pub owner_agent_id: Option<String>,
    pub owner_agent_name: Option<String>,
    /// #2889: structured current activity for the Work panel.
    pub current_tool: Option<String>,
    pub role: Option<String>,
    pub files_touched: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPanelEntryKind {
    Background,
}

impl QueuedMessage {
    pub fn new(display: String, skill_instruction: Option<String>) -> Self {
        Self {
            display,
            skill_instruction,
            skill_provenance: None,
        }
    }

    #[must_use]
    pub fn with_skill_provenance(
        mut self,
        provenance: Option<crate::plugins::types::PluginAuthority>,
    ) -> Self {
        self.skill_provenance = provenance;
        self
    }

    #[allow(dead_code)] // Tests and queue helpers use the display-only form; send path resolves @mentions.
    pub fn content(&self) -> String {
        if let Some(skill_instruction) = self.skill_instruction.as_ref() {
            format!(
                "{skill_instruction}\n\n---\n\nUser request: {}",
                self.display
            )
        } else {
            self.display.clone()
        }
    }
}

// === Actions ===

/// Actions emitted by the UI event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    Quit,
    #[allow(dead_code)] // For explicit /save command
    SaveSession(PathBuf),
    #[allow(dead_code)] // For explicit /load command
    LoadSession(PathBuf),
    RemoteControl(crate::remote_control::RemoteControlAction),
    SyncSession {
        session_id: Option<String>,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        model: String,
        workspace: PathBuf,
        mode: AppMode,
    },
    OpenConfigEditor(ConfigUiMode),
    OpenConfigView,
    /// Open the native git worktree manager.
    OpenWorktreeManager,
    /// Open the `/model` two-pane picker (Pro/Flash + Off/High/Max).
    OpenModelPicker,
    /// Open the `/provider` picker modal — DeepSeek / NVIDIA NIM / OpenRouter
    /// / Novita with inline API-key prompt for un-configured providers (#52).
    OpenProviderPicker,
    /// Open the `/provider` picker in setup/catalog mode, optionally focused on
    /// a built-in provider that needs credentials before first use.
    OpenProviderSetup {
        provider: Option<ApiProvider>,
    },
    /// Run the xAI/Grok device-code flow with the TUI temporarily suspended.
    StartXaiDeviceLogin,
    /// Open the `/mode` picker modal for Act / Plan / Operate.
    OpenModePicker,
    /// Refresh the engine prompt after the UI operating mode changes.
    ModeChanged(AppMode),
    /// Synchronize a saved top-level approval policy into the live Config,
    /// then refresh the engine prompt from the App's updated permission mode.
    ApprovalPolicyPersisted {
        policy: Option<String>,
    },
    /// Reload the active user permission rules after `/permissions` safely
    /// removes one from the sibling `permissions.toml`.
    PermissionRulesChanged,
    /// Rebuild the engine's Skill/MCP catalogue from the App's newly replaced
    /// immutable plugin snapshot after trust, enable, revoke, or reload.
    PluginRegistryChanged,
    /// Open the `/statusline` multi-select picker for footer items.
    OpenStatusPicker,
    /// Open the `/feedback` picker for GitHub issue/security destinations.
    OpenFeedbackPicker,
    /// Open the `/theme` picker modal with live preview of every preset.
    OpenThemePicker,
    /// Open the `/skills` manager — audit inventory + owned mutations.
    OpenSkillsManager,
    /// Open `/fleet` — the saved named-Fleet list (the primary Fleet surface).
    OpenFleetList,
    /// Open the `/fleet` roster — the saved-party view of the agent team.
    OpenFleetRoster,
    /// Open the `/fleet` profile authoring wizard.
    OpenFleetSetup,
    /// Open the `/hotbar` setup wizard.
    OpenHotbarSetup,
    /// Open the constitution-first `/setup` wizard shell.
    OpenSetupWizard,
    /// Open the constitution-first `/setup` wizard at a specific step.
    OpenSetupWizardAt {
        step: codewhale_config::SetupStep,
    },
    /// Record that the bundled/default constitution should be used.
    UseBundledConstitution,
    /// Open the exact effective base-prompt preview for the next turn (#3928).
    ///
    /// Handled where the session config lives, so the preview is built by the
    /// same function the dispatch path uses. Human-only: it issues no provider
    /// request and expands no tool catalog.
    PreviewEffectiveBasePrompt,
    /// Disable the Hotbar: persist `hotbar = []` and clear the live slots.
    DisableHotbar,
    /// Restore the default recommended Hotbar slots: remove the `hotbar` key so
    /// the resolver falls back to the built-in defaults.
    RestoreHotbarDefaults,
    /// Open an external URL in the system browser.
    OpenExternalUrl {
        url: String,
        label: String,
    },
    /// Send a message to the AI (normal chat mode).
    SendMessage(String),
    /// Cancel a running sub-agent through the engine manager.
    CancelSubAgent {
        agent_id: String,
    },
    /// Update the runtime goal status (`/goal pause|resume|clear|…`) without
    /// dispatching a model turn. The UI layer translates this into
    /// `Op::SetGoalStatus`.
    SetGoalStatus {
        status: crate::tools::goal::GoalStatus,
        clear: bool,
    },
    ListSubAgents,
    /// Ask the engine to describe the exact next outbound request
    /// (`/preview-request`, #1004). The engine is the authority: only it can
    /// rebuild the current tool catalog, MCP state, gates, and resolved route.
    PreviewOutboundRequest {
        /// Render the manifest as JSON instead of the human-readable table.
        json: bool,
        /// Render the exact base prompt only. Never includes runtime/system layers.
        base_prompt_only: bool,
        /// Optional text used only to resolve `auto` reasoning/routing. Never
        /// added to the conversation and never sent to a provider.
        hypothetical_prompt: Option<String>,
    },
    /// Show bounded read-only text without copying it into transcript history.
    OpenTextPager {
        title: String,
        content: String,
    },
    FetchModels,
    /// Force a Models.dev live-catalog refresh into ProviderLake (#4187).
    RefreshModelsDevCatalog,
    CacheWarmup,
    /// Switch the active LLM backend (DeepSeek vs NVIDIA NIM) without
    /// restarting the process. The runtime rebuilds its API client from
    /// the updated config. `model` overrides the post-switch model
    /// (already normalized but not yet provider-prefixed).
    SwitchProvider {
        provider: ApiProvider,
        model: Option<String>,
    },
    /// Switch provider+model through the same apply path as a `/model` route
    /// row. Used by Hotbar route slots so dispatch does not hand-mutate config.
    SwitchModelRoute {
        provider: ApiProvider,
        model: String,
    },
    UpdateCompaction(CompactionConfig),
    UpdateStreamChunkTimeout(u64),
    UpdateSubagentRuntimeConfig {
        enabled: bool,
        max_subagents: usize,
        launch_concurrency: usize,
        max_spawn_depth: u32,
        api_timeout_secs: u64,
        heartbeat_timeout_secs: u64,
    },
    /// Enable or disable the background advisor watcher for this session (#3982).
    SetAdvisorEnabled {
        enabled: bool,
    },
    /// Open the live transcript overlay through a terminal-safe command path.
    OpenLiveTranscript,
    /// Open the whole-turn inspector (Ctrl+Alt+O, /turn inspect).
    OpenTurnInspector,
    OpenContextInspector,
    CompactContext {
        /// Optional user focus from `/compact <focus>`, forwarded into the
        /// successor-brief summary prompt.
        focus: Option<String>,
    },
    PurgeContext,
    TaskAdd {
        prompt: String,
    },
    TaskList,
    TaskShow {
        id: String,
    },
    TaskCancel {
        id: String,
    },
    Automation(AutomationAction),
    ShellJob(ShellJobAction),
    Mcp(McpUiAction),
    /// Switch to a different config profile without restarting.
    SwitchProfile {
        /// Profile name to load.
        profile: String,
    },
    /// Switch the workspace used by tools, hooks, tasks, and session metadata.
    SwitchWorkspace {
        workspace: PathBuf,
    },
    /// Record from the microphone and route the transcription into the
    /// composer (or auto-send it). Emitted by `/voice` and the voice hotbar
    /// action; handled in the UI event loop where the live `Config` supplies
    /// provider credentials.
    VoiceCapture,
    /// Export and share the current session as a web URL.
    ShareSession {
        history_len: usize,
        model: String,
        mode: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationAction {
    List,
    Show(String),
    Pause(String),
    Resume(String),
    Delete {
        id: String,
        confirmation: Option<String>,
    },
    Run(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellJobAction {
    List,
    Show {
        id: String,
    },
    Poll {
        id: String,
        wait: bool,
    },
    SendStdin {
        id: String,
        input: String,
        close: bool,
    },
    Cancel {
        id: String,
    },
    CancelAll,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpUiAction {
    Show,
    Init {
        force: bool,
    },
    AddStdio {
        name: String,
        command: String,
        args: Vec<String>,
    },
    AddHttp {
        name: String,
        url: String,
        transport: Option<String>,
    },
    Enable {
        name: String,
    },
    Disable {
        name: String,
    },
    Remove {
        name: String,
    },
    Login {
        name: String,
        scopes: Vec<String>,
    },
    Logout {
        name: String,
    },
    /// List consent-gated external MCP import candidates with provenance.
    ImportList,
    /// Approve importing one discovered external server into user mcp.json.
    ImportApprove {
        name: String,
    },
    /// Decline an external candidate (durable until source content changes).
    ImportDecline {
        name: String,
    },
    Validate,
    Reload,
}
