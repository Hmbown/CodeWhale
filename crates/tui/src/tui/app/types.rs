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

/// The user-facing operating mode. Defined in codewhale-config; re-exported
/// here so `crate::tui::app::types::AppMode` keeps working.
pub use codewhale_config::AppMode;

/// Localized, TUI-only presentation of [`AppMode`]. Kept out of
/// codewhale-config so the mode type does not depend on the locale packs.
pub trait AppModeUi {
    /// Localized short name for the mode picker (user-facing surface only).
    fn display_name_localized(self, locale: Locale) -> Cow<'static, str>;
    /// Localized one-line hint for the mode picker (user-facing surface only).
    fn picker_hint_localized(self, locale: Locale) -> Cow<'static, str>;
}

impl AppModeUi for AppMode {
    /// Localized short name for the mode picker (user-facing surface only).
    fn display_name_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                AppMode::Agent => MessageId::AppModeAgent,
                AppMode::Plan => MessageId::AppModePlan,
                AppMode::Operate => MessageId::AppModeOperate,
            },
        )
    }

    /// Localized one-line hint for the mode picker (user-facing surface only).
    fn picker_hint_localized(self, locale: Locale) -> Cow<'static, str> {
        tr(
            locale,
            match self {
                AppMode::Agent => MessageId::AppModeAgentHint,
                AppMode::Plan => MessageId::AppModePlanHint,
                AppMode::Operate => MessageId::AppModeOperateHint,
            },
        )
    }
}

/// Reasoning-effort tier, mirrored across DeepSeek and Codex effort pickers.
///
/// The config file accepts every supported string value for forward-compat with
/// providers that expose the full spectrum; DeepSeek currently collapses
/// `Low`/`Medium` → `high`. OpenAI Codex normalizes inherited DeepSeek-only
/// `Off` to `Low` and displays/sends `Max` as `xhigh` at the provider
/// boundary. The default keyboard cycler walks the three DeepSeek-distinct
/// tiers: `Off` → `High` → `Max` → `Off`; provider-aware callers should use
/// [`ReasoningEffort::cycle_next_in`] with the route's effort list. Auto
/// routing has no concrete provider yet, so
/// [`ReasoningEffort::cycle_next_for_auto_model`] retains the full
/// provider-neutral preference vocabulary until dispatch.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningEffort {
    Off,
    Minimal,
    Low,
    Medium,
    High,
    XHigh,
    Ultra,
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
            ReasoningEffort::Minimal => Self::Low,
            ReasoningEffort::Low => Self::Low,
            ReasoningEffort::Medium => Self::Medium,
            ReasoningEffort::High => Self::High,
            ReasoningEffort::XHigh => Self::Max,
            ReasoningEffort::Ultra => Self::Max,
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
            "xhigh" => Ok(Self::XHigh),
            "auto" | "automatic" => Ok(Self::Auto),
            "ultra" | "ultracode" => Ok(Self::Ultra),
            "max" | "maximum" => Ok(Self::Max),
            _ => Err(format!(
                "Unrecognized reasoning effort {trimmed:?}. Expected: auto, off, low, medium, high, xhigh, or max."
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
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Ultra => "ultra",
            Self::Auto => "auto",
            Self::Max => "max",
        }
    }

    /// Short label for the header chip.
    #[must_use]
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "med",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Ultra => "ultra",
            Self::Auto => "auto",
            Self::Max => "max",
        }
    }

    /// Provider-facing label for user-visible surfaces.
    #[must_use]
    pub fn display_label_for_provider(self, provider: ApiProvider) -> &'static str {
        match (provider, self.normalize_for_provider(provider)) {
            (ApiProvider::OpenaiCodex, Self::Minimal) => "low",
            (ApiProvider::OpenaiCodex, Self::Low) => "low",
            (ApiProvider::OpenaiCodex, Self::Medium) => "medium",
            (ApiProvider::OpenaiCodex, Self::High) => "high",
            (ApiProvider::OpenaiCodex, Self::XHigh | Self::Ultra | Self::Max) => "xhigh",
            (ApiProvider::Xai, Self::XHigh) => "xhigh",
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
    /// low/high/max) while rounding `medium` up to `high`. Models that publish
    /// a Models.dev `reasoning_options` effort list keep that vocabulary
    /// instead of the historic Low/Medium collapse. Generic Moonshot and
    /// every other non-Codex route retain the historic high coercion.
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
        // Ollama's current OpenAI-compatible Chat Completions contract
        // documents the complete none/low/medium/high/max ladder. Keep every
        // real tier distinct for normal turns; only Codewhale-only synonyms
        // are folded onto the nearest documented spelling.
        if provider == ApiProvider::OllamaCloud {
            return match normalized {
                Self::Minimal => Self::Low,
                Self::XHigh | Self::Ultra => Self::Max,
                other => other,
            };
        }
        if let Some(values) = Self::catalog_effort_values(provider, wire_model) {
            return Self::clamp_to_catalog_efforts(normalized, provider, wire_model, &values);
        }
        match normalized {
            Self::Low | Self::Medium => Self::High,
            other => other,
        }
    }

    pub(crate) fn catalog_default(provider: ApiProvider, wire_model: &str) -> Option<Self> {
        let offering = crate::provider_lake::catalog_offering_for_model(provider, wire_model)?;
        offering.reasoning_options.iter().find_map(|option| {
            option
                .get("type")
                .and_then(|value| value.as_str())
                .filter(|kind| kind.eq_ignore_ascii_case("effort"))?;
            option
                .get("default")
                .and_then(|value| value.as_str())
                .and_then(Self::from_catalog_token)
        })
    }

    pub(crate) fn catalog_effort_values(
        provider: ApiProvider,
        wire_model: &str,
    ) -> Option<Vec<Self>> {
        let offering = crate::provider_lake::catalog_offering_for_model(provider, wire_model)?;
        let mut efforts = Vec::new();
        for option in &offering.reasoning_options {
            if !option
                .get("type")
                .and_then(|value| value.as_str())
                .is_some_and(|kind| kind.eq_ignore_ascii_case("effort"))
            {
                continue;
            }
            let Some(values) = option.get("values").and_then(|value| value.as_array()) else {
                continue;
            };
            for value in values {
                if let Some(effort) = value.as_str().and_then(Self::from_catalog_token)
                    && !efforts.contains(&effort)
                {
                    efforts.push(effort);
                }
            }
        }
        (!efforts.is_empty()).then_some(efforts)
    }

    fn from_catalog_token(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "off" | "disabled" | "none" | "false" => Some(Self::Off),
            "minimal" | "minimum" => Some(Self::Minimal),
            "low" | "light" => Some(Self::Low),
            "medium" | "mid" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::XHigh),
            "ultra" | "ultracode" => Some(Self::Ultra),
            "max" | "maximum" => Some(Self::Max),
            "auto" | "automatic" | "adaptive" => Some(Self::Auto),
            _ => None,
        }
    }

    fn clamp_to_catalog_efforts(
        normalized: Self,
        provider: ApiProvider,
        wire_model: &str,
        values: &[Self],
    ) -> Self {
        if matches!(normalized, Self::Auto) || values.contains(&normalized) {
            return normalized;
        }
        let aliased = match normalized {
            Self::Minimal if values.contains(&Self::Low) => Self::Low,
            Self::Max | Self::Ultra if values.contains(&Self::XHigh) => Self::XHigh,
            Self::Off => Self::catalog_default(provider, wire_model).unwrap_or(Self::High),
            other => other,
        };
        if values.contains(&aliased) {
            aliased
        } else {
            Self::catalog_default(provider, wire_model).unwrap_or(Self::High)
        }
    }

    #[must_use]
    pub fn api_value_for_provider(self, provider: ApiProvider) -> Option<&'static str> {
        if provider != ApiProvider::OpenaiCodex {
            return self.api_value();
        }
        Some(match self.normalize_for_provider(provider) {
            Self::Minimal => "low",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Ultra => "xhigh",
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
            Self::Minimal | Self::Low | Self::Medium | Self::High | Self::XHigh | Self::Ultra => {
                Self::Max
            }
            Self::Max => Self::Off,
        }
    }

    /// Advance through an exact-route effort list. Unknown current values
    /// enter at the first listed tier so a persisted `max` on an `xhigh`
    /// ladder, or `off` on an always-thinking model, still moves.
    #[must_use]
    pub fn cycle_next_in(self, efforts: &[Self]) -> Self {
        if efforts.is_empty() {
            return self.cycle_next();
        }
        if let Some(index) = self.index_in(efforts) {
            return efforts[(index + 1) % efforts.len()];
        }
        efforts[0]
    }

    fn index_in(self, efforts: &[Self]) -> Option<usize> {
        efforts
            .iter()
            .position(|&effort| effort == self)
            .or_else(|| {
                let aliases: &[Self] = match self {
                    Self::Max | Self::Ultra => &[Self::XHigh],
                    Self::XHigh => &[Self::Max],
                    Self::Minimal => &[Self::Low],
                    Self::Low => &[Self::Minimal],
                    _ => return None,
                };
                aliases
                    .iter()
                    .find_map(|alias| efforts.iter().position(|&effort| effort == *alias))
            })
    }

    /// Cycle the unresolved auto-model preference without applying any
    /// provider's normalization rules prematurely.
    #[must_use]
    pub fn cycle_next_for_auto_model(self) -> Self {
        match self {
            Self::Auto => Self::Off,
            Self::Off => Self::Minimal,
            Self::Minimal => Self::Low,
            Self::Low => Self::Medium,
            Self::Medium => Self::High,
            Self::High => Self::XHigh,
            Self::XHigh => Self::Ultra,
            Self::Ultra => Self::Max,
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

/// Configuration required to bootstrap the TUI.
#[derive(Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct TuiOptions {
    pub model: String,
    pub workspace: PathBuf,
    pub config_path: Option<PathBuf>,
    pub config_profile: Option<String>,
    pub allow_shell: bool,
    /// Screen the TUI starts on (alternate screen, or a full-height inline
    /// viewport that leaves the host scrollback intact).
    pub screen_mode: ScreenMode,
    /// Capture mouse input for internal scrolling/selection, on the screen
    /// the session starts on.
    pub use_mouse_capture: bool,
    /// The user's mouse-capture answer with the screen factored out (CLI
    /// flag, `tui.mouse_capture`, or the host default). `/fullscreen` and
    /// `/inline` re-derive `use_mouse_capture` from it, so the documented
    /// default keeps applying after a runtime switch.
    pub mouse_capture_preference: bool,
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
    /// True once this turn has been painted into `history` as `HistoryCell::User`.
    /// Queue/offline submit echoes before the model runs; Immediate prepare skips
    /// a second paint when this is set so drained queued turns do not double.
    pub history_echoed: bool,
}

/// Prefix for the bounded, tool-less model turn produced by `/workflow`.
///
/// The marker travels with the queued message so a draft that waits behind an
/// active turn keeps the same no-tools policy when it is eventually sent.
pub(crate) const WORKFLOW_DRAFT_INSTRUCTION_PREFIX: &str = "[codewhale.workflow-draft.v1]";

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
            history_echoed: false,
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

    #[must_use]
    pub(crate) fn is_workflow_draft(&self) -> bool {
        self.skill_instruction
            .as_deref()
            .is_some_and(|instruction| instruction.starts_with(WORKFLOW_DRAFT_INSTRUCTION_PREFIX))
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

/// A typed goal-control request accepted by the TUI and delivered to the
/// engine mailbox. Keeping this separate from transcript text lets the host
/// persist, retry, and reconcile controls without impersonating the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoalControlIntent {
    SetStatus {
        status: crate::tools::goal::GoalStatus,
        clear: bool,
    },
    SetObjective {
        objective: String,
        token_budget: Option<u32>,
    },
}

/// One accepted goal control waiting for its authoritative GoalUpdated
/// receipt. `dispatched` distinguishes mailbox backpressure from an operation
/// already ordered in the engine channel; both remain pending until receipt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PendingGoalControl {
    pub intent: GoalControlIntent,
    pub dispatched: bool,
}

/// Which screen the TUI paints on.
///
/// This is the single source of truth for the alternate screen: `App` stores
/// the mode and derives `use_alt_screen()` from it, so a switch cannot leave
/// the flag and the live terminal disagreeing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenMode {
    /// Alternate screen buffer. The TUI owns the whole terminal; the host
    /// scrollback is preserved but unreachable until the session exits.
    #[default]
    Fullscreen,
    /// A ratatui inline viewport the full height of the terminal, with no
    /// alternate screen. The shell's scrollback stays intact and scrollable
    /// after exit, at the cost of the TUI no longer owning a private buffer.
    Inline,
}

impl ScreenMode {
    /// Whether this mode runs on the alternate screen buffer.
    #[must_use]
    pub const fn uses_alt_screen(self) -> bool {
        matches!(self, Self::Fullscreen)
    }

    /// Whether mouse capture is on for this screen, given the user's
    /// preference. This is the one rule: startup and the `/fullscreen` ·
    /// `/inline` switch both ask it. Capture needs the alternate screen —
    /// inline mode exists so the terminal owns selection and scrollback.
    #[must_use]
    pub const fn mouse_capture(self, preferred: bool) -> bool {
        self.uses_alt_screen() && preferred
    }

    /// Canonical name, as `/screen` prints it and `parse` accepts it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Fullscreen => "fullscreen",
            Self::Inline => "inline",
        }
    }

    /// Parse a user-supplied mode word, including the legacy
    /// `tui.alternate_screen` vocabulary (`auto`/`always` → fullscreen,
    /// `never` → inline) so the existing config key keeps selecting a real
    /// behaviour instead of being parsed and ignored.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "fullscreen" | "full" | "alt" | "alt-screen" | "auto" | "always" => {
                Some(Self::Fullscreen)
            }
            "inline" | "scrollback" | "never" | "off" => Some(Self::Inline),
            _ => None,
        }
    }
}

/// Actions emitted by the UI event loop.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    Quit,
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
    /// Open the named, keyless DS4 local-runtime preset for review and save.
    OpenDs4Setup,
    /// Open a beginner provider setup template by catalog id (#5350).
    OpenTemplateSetup {
        template_id: String,
    },
    /// Open the beginner provider template list.
    OpenProviderTemplateList,
    /// Run the xAI/Grok device-code flow with the TUI temporarily suspended.
    StartXaiDeviceLogin,
    /// Run native ChatGPT PKCE sign-in with the TUI temporarily suspended.
    StartChatgptPkceLogin,
    StartChatgptRevoke,
    /// Open the `/mode` picker modal for Act / Plan / Operate.
    OpenModePicker,
    /// Switch the live terminal between `/fullscreen` and `/inline`. Handled
    /// where the ratatui `Terminal` lives, because stock ratatui cannot change
    /// an existing terminal's viewport — the switch rebuilds it behind a probe.
    SetScreenMode(ScreenMode),
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
    /// Open the `/workflows` run dashboard — live and retained workflow runs.
    OpenWorkflowsManager,
    /// Open the unified, read-only extensions inventory on a specific tab.
    OpenExtensions {
        tab: crate::tui::views::extensions::ExtensionsTab,
    },
    /// Open `/fleet` — the saved named-Fleet list (the primary Fleet surface).
    OpenFleetList,
    /// Open the `/fleet` roster — the saved-party view of the agent team.
    OpenFleetRoster,
    /// Open the selected v2 Fleet editor, or legacy profile setup when no
    /// named Fleet is selected.
    OpenFleetSetup,
    /// `/fleet add`: validate the provider against the live config, write
    /// the member rows, and mark the engine roster stale.
    FleetAddModel {
        provider: String,
        model: String,
        roles: Vec<String>,
    },
    /// `/fleet remove`: drop every member row pinning the route and mark the
    /// engine roster stale.
    FleetRemoveModel {
        provider: String,
        model: String,
    },
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
    /// Send a built-in Workflow planning turn with separate user-visible text
    /// and bounded runtime guidance. Draft instructions carry a typed marker
    /// that makes the dispatch path expose no tools for that turn.
    WorkflowInstruction {
        display: String,
        instruction: String,
    },
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
    /// Set or replace the goal objective (`/goal <objective>`). The engine
    /// owns the goal and starts the first goal turn itself as runtime
    /// steering; the objective is never sent as a raw user message.
    SetGoalObjective {
        objective: String,
        token_budget: Option<u32>,
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
    /// Live remaining-credit lookup for prepaid providers (`/balance`).
    FetchBalance,
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
    /// Apply `/config search.provider` to the live Config and engine.
    UpdateSearchProvider {
        provider: crate::config::SearchProvider,
    },
    /// Apply `/config prompt_suggestion` to the live Config.
    UpdatePromptSuggestion {
        enabled: bool,
    },
    /// Apply one `/config notifications` scalar to the live Config.
    UpdateNotification {
        update: crate::config::NotificationConfigUpdate,
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
    /// Retry one failed/timed-out server through the engine-owned live pool.
    Retry {
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
