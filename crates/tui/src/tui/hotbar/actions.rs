use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::commands::{self, CommandInfo, CommandResult};
use crate::config::{ApiProvider, Config, model_completion_names_for_provider};
use crate::localization::Locale;
use crate::tui::app::{App, AppAction, AppMode, SidebarFocus};
use crate::tui::command_palette::{
    CommandPaletteView, build_entries as build_command_palette_entries,
};

pub const HOTBAR_COMPACT_LABEL_MAX_WIDTH: usize = 7;

/// Result of firing a hotbar action.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum HotbarDispatch {
    /// The action was fully handled by mutating [`App`].
    Handled,
    /// The event loop must handle an existing application action.
    AppAction(AppAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[allow(dead_code)]
pub enum HotbarActionCategory {
    App,
    Mode,
    Route,
    Slash,
    Mcp,
    Skill,
    Plugin,
}

impl HotbarActionCategory {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Mode => "mode",
            Self::Route => "route",
            Self::Slash => "slash",
            Self::Mcp => "mcp",
            Self::Skill => "skill",
            Self::Plugin => "plugin",
        }
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "app" => Some(Self::App),
            "mode" => Some(Self::Mode),
            "route" => Some(Self::Route),
            "slash" => Some(Self::Slash),
            "mcp" => Some(Self::Mcp),
            "skill" => Some(Self::Skill),
            "plugin" => Some(Self::Plugin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotbarArgsBehavior {
    None,
    Optional,
    Required,
}

impl HotbarArgsBehavior {
    #[must_use]
    fn for_command(info: &CommandInfo) -> Self {
        if info.requires_required_argument() {
            Self::Required
        } else if info.requires_argument() {
            Self::Optional
        } else {
            Self::None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum HotbarSafetyClass {
    LocalUi,
    LocalState,
    ConfigChange,
    ExternalInput,
    ExistingCommand,
    RequiresApproval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotbarRecommendation {
    Default,
    Eligible,
    Advanced,
}

impl HotbarRecommendation {
    #[must_use]
    #[allow(dead_code)]
    pub const fn is_recommendable(self) -> bool {
        matches!(self, Self::Default | Self::Eligible)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarActionMetadata {
    pub id: String,
    pub source_id: String,
    pub display_name: String,
    pub compact_label: String,
    pub description: String,
    pub category: HotbarActionCategory,
    pub args: HotbarArgsBehavior,
    pub safety: HotbarSafetyClass,
    pub recommendation: HotbarRecommendation,
}

impl HotbarActionMetadata {
    #[must_use]
    pub fn validation_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.id.trim().is_empty() {
            errors.push("id must not be empty".to_string());
        }
        if self.source_id.trim().is_empty() {
            errors.push(format!("{} source_id must not be empty", self.id));
        }
        if self.display_name.trim().is_empty() {
            errors.push(format!("{} display_name must not be empty", self.id));
        }
        if self.compact_label.trim().is_empty() {
            errors.push(format!("{} compact_label must not be empty", self.id));
        }
        if unicode_width::UnicodeWidthStr::width(self.compact_label.as_str())
            > HOTBAR_COMPACT_LABEL_MAX_WIDTH
        {
            errors.push(format!(
                "{} compact_label {:?} exceeds {} display cells",
                self.id, self.compact_label, HOTBAR_COMPACT_LABEL_MAX_WIDTH
            ));
        }
        if self.description.trim().is_empty() {
            errors.push(format!("{} description must not be empty", self.id));
        }
        errors
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotbarRecommendationEntry {
    pub metadata: HotbarActionMetadata,
    pub disabled_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotbarRecommendationOptions {
    pub max_total: usize,
    pub max_eligible_per_category: usize,
    pub include_required_args: bool,
}

impl HotbarRecommendationOptions {
    #[must_use]
    pub const fn for_setup_wizard() -> Self {
        Self {
            max_total: usize::MAX,
            max_eligible_per_category: usize::MAX,
            include_required_args: false,
        }
    }
}

impl Default for HotbarRecommendationOptions {
    fn default() -> Self {
        Self {
            max_total: usize::from(codewhale_config::HOTBAR_SLOT_COUNT),
            max_eligible_per_category: usize::MAX,
            include_required_args: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotbarSourceDispatchBoundary {
    /// The action is handled directly by existing in-app state mutation.
    DirectApp,
    /// The action routes through the existing provider/model picker apply path.
    ModelRoute,
    /// The action routes through the slash command registry/dispatcher.
    SlashCommand,
    /// The source is visible as a future hotbar source, but binding/dispatch is
    /// intentionally deferred until its safety contract is wired.
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotbarSourceSafetyMode {
    /// Pressing the bound hotbar slot directly fires the existing action path.
    DirectFire,
    /// Pressing the bound hotbar slot opens/prefills the composer for arguments.
    ComposerPrefill,
    /// The source must not register bindable actions until its gates are wired.
    Disabled,
    /// The source may dispatch only through an approval/trust-enforced path.
    ApprovalGated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotbarSourceDescriptor {
    pub category: HotbarActionCategory,
    pub boundary: HotbarSourceDispatchBoundary,
    pub safety_modes: &'static [HotbarSourceSafetyMode],
    pub dispatch_path: &'static str,
    pub status: &'static str,
}

impl HotbarSourceDescriptor {
    #[must_use]
    pub fn registers_dispatchable_actions(self) -> bool {
        self.boundary != HotbarSourceDispatchBoundary::Deferred
            && !self
                .safety_modes
                .contains(&HotbarSourceSafetyMode::Disabled)
    }
}

const HOTBAR_DIRECT_APP_SAFETY: &[HotbarSourceSafetyMode] = &[HotbarSourceSafetyMode::DirectFire];
const HOTBAR_MODE_SAFETY: &[HotbarSourceSafetyMode] = &[HotbarSourceSafetyMode::DirectFire];
const HOTBAR_ROUTE_SAFETY: &[HotbarSourceSafetyMode] = &[HotbarSourceSafetyMode::DirectFire];
const HOTBAR_SLASH_SAFETY: &[HotbarSourceSafetyMode] = &[
    HotbarSourceSafetyMode::DirectFire,
    HotbarSourceSafetyMode::ComposerPrefill,
];
const HOTBAR_DEFERRED_SAFETY: &[HotbarSourceSafetyMode] = &[
    HotbarSourceSafetyMode::Disabled,
    HotbarSourceSafetyMode::ApprovalGated,
];

const HOTBAR_SOURCE_DESCRIPTORS: &[HotbarSourceDescriptor] = &[
    HotbarSourceDescriptor {
        category: HotbarActionCategory::App,
        boundary: HotbarSourceDispatchBoundary::DirectApp,
        safety_modes: HOTBAR_DIRECT_APP_SAFETY,
        dispatch_path: "AppHotbarAction::dispatch",
        status: "dispatchable",
    },
    HotbarSourceDescriptor {
        category: HotbarActionCategory::Mode,
        boundary: HotbarSourceDispatchBoundary::DirectApp,
        safety_modes: HOTBAR_MODE_SAFETY,
        dispatch_path: "ModeHotbarAction::dispatch -> AppAction::ModeChanged",
        status: "dispatchable",
    },
    HotbarSourceDescriptor {
        category: HotbarActionCategory::Route,
        boundary: HotbarSourceDispatchBoundary::ModelRoute,
        safety_modes: HOTBAR_ROUTE_SAFETY,
        dispatch_path: "AppAction::SwitchModelRoute -> apply_model_picker_choice",
        status: "dispatchable",
    },
    HotbarSourceDescriptor {
        category: HotbarActionCategory::Slash,
        boundary: HotbarSourceDispatchBoundary::SlashCommand,
        safety_modes: HOTBAR_SLASH_SAFETY,
        dispatch_path: "commands::execute or composer prefill for required arguments",
        status: "dispatchable",
    },
    HotbarSourceDescriptor {
        category: HotbarActionCategory::Mcp,
        boundary: HotbarSourceDispatchBoundary::Deferred,
        safety_modes: HOTBAR_DEFERRED_SAFETY,
        dispatch_path: "command palette / MCP manager until tool args and approvals are wired",
        status: "exploratory",
    },
    HotbarSourceDescriptor {
        category: HotbarActionCategory::Skill,
        boundary: HotbarSourceDispatchBoundary::Deferred,
        safety_modes: HOTBAR_DEFERRED_SAFETY,
        dispatch_path: "skill activation receipts / command palette until approval gates are wired",
        status: "exploratory",
    },
    HotbarSourceDescriptor {
        category: HotbarActionCategory::Plugin,
        boundary: HotbarSourceDispatchBoundary::Deferred,
        safety_modes: HOTBAR_DEFERRED_SAFETY,
        dispatch_path: "plugin registry until tool args, approvals, and receipts are wired",
        status: "exploratory",
    },
];

#[must_use]
pub const fn hotbar_source_descriptors() -> &'static [HotbarSourceDescriptor] {
    HOTBAR_SOURCE_DESCRIPTORS
}

/// Adapter for one source of bindable hotbar actions.
pub trait HotbarActionSource {
    fn descriptor(&self) -> HotbarSourceDescriptor;
    fn register_actions(&self, registry: &mut HotbarActionRegistry);
}

/// Uniform interface for actions that can be bound to a hotbar slot.
#[allow(dead_code)]
pub trait HotbarAction: Send + Sync {
    /// Stable action id used in config and dispatch.
    fn id(&self) -> &str;

    /// Complete metadata used by renderers, setup wizard recommendations, and
    /// future source adapters.
    fn metadata(&self, locale: Locale) -> HotbarActionMetadata;

    /// Compact cell label. Built-ins keep this at seven characters or less.
    fn short_label(&self) -> &str;

    /// Source category, such as `app`, `route`, `slash`, `mcp`, `skill`, or
    /// `plugin`.
    fn category(&self) -> &str;

    /// Whether the action is currently active in the supplied app state.
    fn is_active(&self, app: &App) -> bool;

    /// Dynamic unavailable reason. `None` means the action is dispatchable
    /// through its normal safety path.
    fn disabled_reason(&self, _app: &App) -> Option<String> {
        None
    }

    /// Fire the action.
    fn dispatch(&self, app: &mut App) -> Result<HotbarDispatch>;
}

#[must_use]
pub fn recommend_hotbar_actions(
    app: &App,
    options: HotbarRecommendationOptions,
) -> Vec<HotbarRecommendationEntry> {
    let mut entries = app
        .hotbar_actions
        .iter()
        .filter_map(|action| {
            let metadata = action.metadata(app.ui_locale);
            if !metadata.recommendation.is_recommendable() {
                return None;
            }
            if matches!(metadata.args, HotbarArgsBehavior::Required)
                && !options.include_required_args
            {
                return None;
            }
            let disabled_reason = action.disabled_reason(app);
            if disabled_reason.is_some() {
                return None;
            }
            Some(HotbarRecommendationEntry {
                metadata,
                disabled_reason,
            })
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| compare_recommendation_metadata(&a.metadata, &b.metadata));

    let mut selected = Vec::new();
    let mut eligible_by_category: BTreeMap<HotbarActionCategory, usize> = BTreeMap::new();
    for entry in entries {
        if selected.len() >= options.max_total {
            break;
        }
        if !matches!(entry.metadata.recommendation, HotbarRecommendation::Default) {
            let count = eligible_by_category
                .entry(entry.metadata.category)
                .or_insert(0);
            if *count >= options.max_eligible_per_category {
                continue;
            }
            *count += 1;
        }
        selected.push(entry);
    }
    selected
}

#[must_use]
#[allow(dead_code)]
pub fn recommended_hotbar_bindings(
    app: &App,
    options: HotbarRecommendationOptions,
) -> Vec<codewhale_config::HotbarBindingToml> {
    recommend_hotbar_actions(app, options)
        .into_iter()
        .take(usize::from(codewhale_config::HOTBAR_SLOT_COUNT))
        .enumerate()
        .map(|(idx, entry)| codewhale_config::HotbarBindingToml {
            slot: u8::try_from(idx + 1).expect("recommended hotbar slot fits in u8"),
            action: entry.metadata.id,
            label: Some(entry.metadata.compact_label),
        })
        .collect()
}

fn default_hotbar_position(action_id: &str) -> Option<usize> {
    codewhale_config::DEFAULT_HOTBAR_ACTIONS
        .iter()
        .position(|default_id| *default_id == action_id)
}

fn compare_recommendation_metadata(a: &HotbarActionMetadata, b: &HotbarActionMetadata) -> Ordering {
    match (
        default_hotbar_position(&a.id),
        default_hotbar_position(&b.id),
    ) {
        (Some(a_pos), Some(b_pos)) => return a_pos.cmp(&b_pos),
        (Some(_), None) => return Ordering::Less,
        (None, Some(_)) => return Ordering::Greater,
        (None, None) => {}
    }

    a.category
        .cmp(&b.category)
        .then_with(|| {
            a.display_name
                .to_ascii_lowercase()
                .cmp(&b.display_name.to_ascii_lowercase())
        })
        .then_with(|| a.id.cmp(&b.id))
}

#[derive(Default, Clone)]
pub struct HotbarActionRegistry {
    actions: BTreeMap<String, Arc<dyn HotbarAction>>,
}

impl HotbarActionRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn with_builtins() -> Self {
        let mut registry = Self::new();
        registry.register_builtins();
        registry.register_slash_commands();
        registry
    }

    #[must_use]
    pub fn with_all_sources(
        config: &Config,
        active_provider: ApiProvider,
        active_model: &str,
        provider_models: &HashMap<String, String>,
    ) -> Self {
        let mut registry = Self::with_builtins();
        registry.register_configured_routes(config, active_provider, active_model, provider_models);
        registry.register_deferred_sources();
        registry
    }

    #[must_use]
    #[allow(dead_code)]
    pub fn with_configured_routes(
        config: &Config,
        active_provider: ApiProvider,
        active_model: &str,
        provider_models: &HashMap<String, String>,
    ) -> Self {
        let mut registry = Self::with_builtins();
        registry.register_configured_routes(config, active_provider, active_model, provider_models);
        registry
    }

    pub fn register(&mut self, action: impl HotbarAction + 'static) {
        let id = action.id().to_string();
        assert!(!id.trim().is_empty(), "hotbar action id must not be empty");
        assert!(
            self.actions.insert(id.clone(), Arc::new(action)).is_none(),
            "duplicate hotbar action id {id}"
        );
    }

    pub fn register_source(&mut self, source: &dyn HotbarActionSource) {
        let descriptor = source.descriptor();
        debug_assert!(
            hotbar_source_descriptors()
                .iter()
                .any(|registered| registered.category == descriptor.category
                    && registered.boundary == descriptor.boundary),
            "hotbar source descriptor must be registered: {descriptor:?}"
        );
        debug_assert!(!descriptor.dispatch_path.trim().is_empty());
        debug_assert!(!descriptor.status.trim().is_empty());
        debug_assert!(!descriptor.safety_modes.is_empty());
        let before = self.actions.len();
        source.register_actions(self);
        if !descriptor.registers_dispatchable_actions() {
            assert_eq!(
                self.actions.len(),
                before,
                "deferred hotbar source {:?} must not register dispatchable actions before safety gates are wired",
                descriptor.category
            );
        }
    }

    pub(crate) fn register_builtins(&mut self) {
        self.register_source(&BuiltinHotbarActionSource);
        self.register_source(&ModeHotbarActionSource);
    }

    pub(crate) fn register_slash_commands(&mut self) {
        self.register_source(&SlashCommandHotbarActionSource);
    }

    pub(crate) fn register_deferred_sources(&mut self) {
        self.register_source(&DeferredHotbarActionSource::new(HotbarActionCategory::Mcp));
        self.register_source(&DeferredHotbarActionSource::new(
            HotbarActionCategory::Skill,
        ));
        self.register_source(&DeferredHotbarActionSource::new(
            HotbarActionCategory::Plugin,
        ));
    }

    pub(crate) fn register_configured_routes(
        &mut self,
        config: &Config,
        active_provider: ApiProvider,
        active_model: &str,
        provider_models: &HashMap<String, String>,
    ) {
        let source = ConfiguredRouteHotbarActionSource {
            config,
            active_provider,
            active_model,
            provider_models,
        };
        self.register_source(&source);
    }
}

struct BuiltinHotbarActionSource;

impl HotbarActionSource for BuiltinHotbarActionSource {
    fn descriptor(&self) -> HotbarSourceDescriptor {
        HOTBAR_SOURCE_DESCRIPTORS
            .iter()
            .copied()
            .find(|descriptor| descriptor.category == HotbarActionCategory::App)
            .expect("app hotbar source descriptor exists")
    }

    fn register_actions(&self, registry: &mut HotbarActionRegistry) {
        registry.register(AppHotbarAction::new(
            "voice.toggle",
            "voice",
            "Voice input",
            "Toggle voice capture from the terminal microphone.",
            AppHotbarKind::VoiceToggle,
        ));
        registry.register(AppHotbarAction::new(
            "session.compact",
            "compact",
            "Compact session",
            "Compact the current conversation context.",
            AppHotbarKind::SessionCompact,
        ));
        registry.register(AppHotbarAction::new(
            "reasoning.cycle",
            "reason",
            "Cycle reasoning",
            "Cycle the configured reasoning effort for the active provider.",
            AppHotbarKind::ReasoningCycle,
        ));
        registry.register(AppHotbarAction::new(
            "sidebar.toggle",
            "side",
            "Toggle sidebar",
            "Show or hide the sidebar.",
            AppHotbarKind::SidebarToggle,
        ));
        registry.register(AppHotbarAction::new(
            "filetree.toggle",
            "files",
            "Toggle file tree",
            "Show or hide the workspace file tree.",
            AppHotbarKind::FileTreeToggle,
        ));
        registry.register(AppHotbarAction::new(
            "palette.open",
            "palette",
            "Command palette",
            "Open the command palette.",
            AppHotbarKind::PaletteOpen,
        ));
        registry.register(AppHotbarAction::new(
            "trust.toggle",
            "trust",
            "Toggle trust",
            "Enable or disable workspace trust mode.",
            AppHotbarKind::TrustToggle,
        ));
    }
}

struct ModeHotbarActionSource;

impl HotbarActionSource for ModeHotbarActionSource {
    fn descriptor(&self) -> HotbarSourceDescriptor {
        HOTBAR_SOURCE_DESCRIPTORS
            .iter()
            .copied()
            .find(|descriptor| descriptor.category == HotbarActionCategory::Mode)
            .expect("mode hotbar source descriptor exists")
    }

    fn register_actions(&self, registry: &mut HotbarActionRegistry) {
        registry.register(ModeHotbarAction::new(
            "mode.agent",
            "agent",
            "Agent mode",
            "Switch the conversation into Agent mode.",
            AppMode::Agent,
        ));
        registry.register(ModeHotbarAction::new(
            "mode.plan",
            "plan",
            "Plan mode",
            "Switch the conversation into Plan mode.",
            AppMode::Plan,
        ));
        registry.register(ModeHotbarAction::new(
            "mode.yolo",
            "yolo",
            "YOLO mode",
            "Switch the conversation into YOLO mode.",
            AppMode::Yolo,
        ));
    }
}

struct SlashCommandHotbarActionSource;

impl HotbarActionSource for SlashCommandHotbarActionSource {
    fn descriptor(&self) -> HotbarSourceDescriptor {
        HOTBAR_SOURCE_DESCRIPTORS
            .iter()
            .copied()
            .find(|descriptor| descriptor.category == HotbarActionCategory::Slash)
            .expect("slash hotbar source descriptor exists")
    }

    fn register_actions(&self, registry: &mut HotbarActionRegistry) {
        for info in commands::command_infos() {
            registry.register(SlashHotbarAction::new(info));
        }
    }
}

struct ConfiguredRouteHotbarActionSource<'a> {
    config: &'a Config,
    active_provider: ApiProvider,
    active_model: &'a str,
    provider_models: &'a HashMap<String, String>,
}

impl HotbarActionSource for ConfiguredRouteHotbarActionSource<'_> {
    fn descriptor(&self) -> HotbarSourceDescriptor {
        HOTBAR_SOURCE_DESCRIPTORS
            .iter()
            .copied()
            .find(|descriptor| descriptor.category == HotbarActionCategory::Route)
            .expect("route hotbar source descriptor exists")
    }

    fn register_actions(&self, registry: &mut HotbarActionRegistry) {
        for provider in ApiProvider::sorted_for_display() {
            if !crate::config::provider_is_configured_for_active(
                self.config,
                provider,
                self.active_provider,
            ) {
                continue;
            }
            for model in configured_route_models_for_provider(
                self.config,
                provider,
                self.active_provider,
                self.active_model,
                self.provider_models,
            ) {
                registry.register(RouteHotbarAction::new(provider, model));
            }
        }
    }
}

struct DeferredHotbarActionSource {
    category: HotbarActionCategory,
}

impl DeferredHotbarActionSource {
    const fn new(category: HotbarActionCategory) -> Self {
        Self { category }
    }
}

impl HotbarActionSource for DeferredHotbarActionSource {
    fn descriptor(&self) -> HotbarSourceDescriptor {
        HOTBAR_SOURCE_DESCRIPTORS
            .iter()
            .copied()
            .find(|descriptor| descriptor.category == self.category)
            .unwrap_or_else(|| {
                panic!(
                    "deferred hotbar source descriptor exists: {:?}",
                    self.category
                )
            })
    }

    fn register_actions(&self, _registry: &mut HotbarActionRegistry) {}
}

// ---------------------------------------------------------------------------
// Mode action
// ---------------------------------------------------------------------------

struct ModeHotbarAction {
    id: &'static str,
    short_label: &'static str,
    display_name: &'static str,
    description: &'static str,
    mode: AppMode,
}

impl ModeHotbarAction {
    const fn new(
        id: &'static str,
        short_label: &'static str,
        display_name: &'static str,
        description: &'static str,
        mode: AppMode,
    ) -> Self {
        Self {
            id,
            short_label,
            display_name,
            description,
            mode,
        }
    }
}

impl HotbarAction for ModeHotbarAction {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self, _locale: Locale) -> HotbarActionMetadata {
        HotbarActionMetadata {
            id: self.id.to_string(),
            source_id: "builtin".to_string(),
            display_name: self.display_name.to_string(),
            compact_label: self.short_label.to_string(),
            description: self.description.to_string(),
            category: HotbarActionCategory::Mode,
            args: HotbarArgsBehavior::None,
            safety: HotbarSafetyClass::LocalUi,
            recommendation: if codewhale_config::DEFAULT_HOTBAR_ACTIONS.contains(&self.id) {
                HotbarRecommendation::Default
            } else {
                HotbarRecommendation::Eligible
            },
        }
    }

    fn short_label(&self) -> &str {
        self.short_label
    }

    fn category(&self) -> &str {
        "mode"
    }

    fn is_active(&self, app: &App) -> bool {
        app.mode == self.mode
    }

    fn dispatch(&self, app: &mut App) -> Result<HotbarDispatch> {
        let changed = app.set_mode(self.mode);
        if changed {
            Ok(HotbarDispatch::AppAction(AppAction::ModeChanged(self.mode)))
        } else {
            Ok(HotbarDispatch::Handled)
        }
    }
}

impl HotbarActionRegistry {
    #[allow(dead_code)]
    #[must_use]
    pub fn get(&self, id: &str) -> Option<Arc<dyn HotbarAction>> {
        self.actions.get(id).cloned()
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    #[allow(dead_code)]
    pub fn iter(&self) -> impl Iterator<Item = &dyn HotbarAction> {
        self.actions.values().map(Arc::as_ref)
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn metadata(&self, locale: Locale) -> Vec<HotbarActionMetadata> {
        self.iter().map(|action| action.metadata(locale)).collect()
    }

    #[allow(dead_code)]
    #[must_use]
    pub fn metadata_validation_errors(&self, locale: Locale) -> Vec<String> {
        let mut errors = Vec::new();
        for action in self.iter() {
            let metadata = action.metadata(locale);
            if metadata.id != action.id() {
                errors.push(format!(
                    "{} metadata id {:?} does not match action id",
                    action.id(),
                    metadata.id
                ));
            }
            if metadata.compact_label != action.short_label() {
                errors.push(format!(
                    "{} metadata compact_label {:?} does not match short_label {:?}",
                    action.id(),
                    metadata.compact_label,
                    action.short_label()
                ));
            }
            if metadata.category.as_str() != action.category() {
                errors.push(format!(
                    "{} metadata category {:?} does not match category {:?}",
                    action.id(),
                    metadata.category.as_str(),
                    action.category()
                ));
            }
            errors.extend(metadata.validation_errors());
        }
        errors
    }
}

fn dispatch_command_result(app: &mut App, result: CommandResult) -> HotbarDispatch {
    app.status_message = result.message;
    result
        .action
        .map_or(HotbarDispatch::Handled, HotbarDispatch::AppAction)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppHotbarKind {
    VoiceToggle,
    SessionCompact,
    ReasoningCycle,
    SidebarToggle,
    FileTreeToggle,
    PaletteOpen,
    TrustToggle,
}

#[allow(dead_code)]
struct AppHotbarAction {
    id: &'static str,
    short_label: &'static str,
    display_name: &'static str,
    description: &'static str,
    kind: AppHotbarKind,
}

impl AppHotbarAction {
    const fn new(
        id: &'static str,
        short_label: &'static str,
        display_name: &'static str,
        description: &'static str,
        kind: AppHotbarKind,
    ) -> Self {
        Self {
            id,
            short_label,
            display_name,
            description,
            kind,
        }
    }

    fn safety(&self) -> HotbarSafetyClass {
        match self.kind {
            AppHotbarKind::VoiceToggle => HotbarSafetyClass::ExternalInput,
            AppHotbarKind::TrustToggle => HotbarSafetyClass::LocalState,
            AppHotbarKind::SessionCompact | AppHotbarKind::ReasoningCycle => {
                HotbarSafetyClass::LocalState
            }
            AppHotbarKind::SidebarToggle
            | AppHotbarKind::FileTreeToggle
            | AppHotbarKind::PaletteOpen => HotbarSafetyClass::LocalUi,
        }
    }

    fn recommendation(&self) -> HotbarRecommendation {
        if codewhale_config::DEFAULT_HOTBAR_ACTIONS.contains(&self.id) {
            HotbarRecommendation::Default
        } else {
            HotbarRecommendation::Eligible
        }
    }
}

impl HotbarAction for AppHotbarAction {
    fn id(&self) -> &str {
        self.id
    }

    fn metadata(&self, _locale: Locale) -> HotbarActionMetadata {
        HotbarActionMetadata {
            id: self.id.to_string(),
            source_id: "builtin".to_string(),
            display_name: self.display_name.to_string(),
            compact_label: self.short_label.to_string(),
            description: self.description.to_string(),
            category: HotbarActionCategory::App,
            args: HotbarArgsBehavior::None,
            safety: self.safety(),
            recommendation: self.recommendation(),
        }
    }

    fn short_label(&self) -> &str {
        self.short_label
    }

    fn category(&self) -> &str {
        "app"
    }

    fn is_active(&self, app: &App) -> bool {
        match self.kind {
            AppHotbarKind::VoiceToggle => app.voice_enabled,
            AppHotbarKind::SessionCompact => app.is_compacting,
            AppHotbarKind::ReasoningCycle => {
                !app.auto_model && app.reasoning_effort != crate::tui::app::ReasoningEffort::Off
            }
            AppHotbarKind::SidebarToggle => app.sidebar_focus != SidebarFocus::Hidden,
            AppHotbarKind::FileTreeToggle => app.file_tree.is_some(),
            AppHotbarKind::PaletteOpen => false,
            AppHotbarKind::TrustToggle => app.trust_mode,
        }
    }

    fn disabled_reason(&self, app: &App) -> Option<String> {
        match self.kind {
            AppHotbarKind::ReasoningCycle if app.auto_model => {
                Some("Reasoning effort is controlled by auto model routing.".to_string())
            }
            _ => None,
        }
    }

    fn dispatch(&self, app: &mut App) -> Result<HotbarDispatch> {
        match self.kind {
            AppHotbarKind::VoiceToggle => {
                let result = crate::commands::voice::voice(app);
                Ok(dispatch_command_result(app, result))
            }
            AppHotbarKind::SessionCompact => {
                if app.is_compacting {
                    app.status_message = Some("Compaction is already running.".to_string());
                    return Ok(HotbarDispatch::Handled);
                }
                Ok(HotbarDispatch::AppAction(AppAction::CompactContext))
            }
            AppHotbarKind::ReasoningCycle => {
                if app.auto_model {
                    bail!("Reasoning effort is controlled by auto model routing.");
                }
                app.reasoning_effort = app
                    .reasoning_effort
                    .cycle_next_for_provider(app.api_provider);
                app.last_effective_reasoning_effort = None;
                app.update_model_compaction_budget();
                app.status_message = Some(format!(
                    "Reasoning effort: {}",
                    app.reasoning_effort
                        .display_label_for_provider(app.api_provider)
                ));
                Ok(HotbarDispatch::AppAction(AppAction::UpdateCompaction(
                    app.compaction_config(),
                )))
            }
            AppHotbarKind::SidebarToggle => {
                if app.sidebar_focus == SidebarFocus::Hidden {
                    app.set_sidebar_focus(SidebarFocus::Pinned);
                    app.status_message = Some("Sidebar focus: pinned".to_string());
                } else {
                    app.set_sidebar_focus(SidebarFocus::Hidden);
                    app.status_message = Some("Sidebar hidden".to_string());
                }
                Ok(HotbarDispatch::Handled)
            }
            AppHotbarKind::FileTreeToggle => {
                if app.file_tree.is_some() {
                    app.file_tree = None;
                    app.status_message = Some("File tree closed".to_string());
                } else {
                    app.file_tree = Some(crate::tui::file_tree::FileTreeState::new(&app.workspace));
                    app.status_message =
                        Some("File tree: ↑/↓ navigate  Enter select  Esc close".to_string());
                }
                app.needs_redraw = true;
                Ok(HotbarDispatch::Handled)
            }
            AppHotbarKind::PaletteOpen => {
                app.view_stack
                    .push(CommandPaletteView::new(build_command_palette_entries(
                        app.ui_locale,
                        &app.skills_dir,
                        app.skills_scan_codewhale_only,
                        &app.workspace,
                        &app.mcp_config_path,
                        app.mcp_snapshot.as_ref(),
                    )));
                Ok(HotbarDispatch::Handled)
            }
            AppHotbarKind::TrustToggle => {
                app.trust_mode = !app.trust_mode;
                app.status_message = Some(if app.trust_mode {
                    "Workspace trust mode enabled.".to_string()
                } else {
                    "Workspace trust mode disabled.".to_string()
                });
                Ok(HotbarDispatch::Handled)
            }
        }
    }
}

#[allow(dead_code)]
struct SlashHotbarAction {
    info: &'static CommandInfo,
    id: String,
    short_label: String,
}

impl SlashHotbarAction {
    fn new(info: &'static CommandInfo) -> Self {
        Self {
            info,
            id: format!("slash.{}", info.name),
            short_label: info.name.chars().take(7).collect(),
        }
    }

    fn prefill_composer(&self, app: &mut App) {
        app.clear_input_recoverable();
        app.input = format!("/{} ", self.info.name);
        app.cursor_position = app.input.chars().count();
        app.slash_menu_hidden = false;
        app.needs_redraw = true;
        app.status_message = Some(format!(
            "Command needs arguments; complete {}",
            app.input.trim_end()
        ));
    }
}

impl HotbarAction for SlashHotbarAction {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self, locale: Locale) -> HotbarActionMetadata {
        let recommendation = match self.info.discovery() {
            crate::commands::traits::CommandDiscovery::Primary => HotbarRecommendation::Eligible,
            crate::commands::traits::CommandDiscovery::Advanced
            | crate::commands::traits::CommandDiscovery::Compatibility => {
                HotbarRecommendation::Advanced
            }
        };
        HotbarActionMetadata {
            id: self.id.clone(),
            source_id: format!("command:{}", self.info.name),
            display_name: format!("/{}", self.info.name),
            compact_label: self.short_label.clone(),
            description: self.info.description_for(locale).to_string(),
            category: HotbarActionCategory::Slash,
            args: HotbarArgsBehavior::for_command(self.info),
            safety: HotbarSafetyClass::ExistingCommand,
            recommendation,
        }
    }

    fn short_label(&self) -> &str {
        &self.short_label
    }

    fn category(&self) -> &str {
        "slash"
    }

    fn is_active(&self, _app: &App) -> bool {
        false
    }

    fn dispatch(&self, app: &mut App) -> Result<HotbarDispatch> {
        if self.info.requires_required_argument() {
            self.prefill_composer(app);
            return Ok(HotbarDispatch::Handled);
        }

        let input = format!("/{}", self.info.name);
        let result = commands::execute(&input, app);
        Ok(dispatch_command_result(app, result))
    }
}

#[allow(dead_code)]
struct RouteHotbarAction {
    provider: ApiProvider,
    model: String,
    id: String,
    short_label: String,
}

impl RouteHotbarAction {
    fn new(provider: ApiProvider, model: String) -> Self {
        let trimmed_model = model.trim().to_string();
        Self {
            provider,
            id: route_action_id(provider, &trimmed_model),
            short_label: crate::tui::ui_text::truncate_line_to_width(
                provider.as_str(),
                HOTBAR_COMPACT_LABEL_MAX_WIDTH,
            ),
            model: trimmed_model,
        }
    }
}

impl HotbarAction for RouteHotbarAction {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self, _locale: Locale) -> HotbarActionMetadata {
        HotbarActionMetadata {
            id: self.id.clone(),
            source_id: format!("route:{}", self.provider.as_str()),
            display_name: format!("{} · {}", self.provider.display_name(), self.model),
            compact_label: self.short_label.clone(),
            description: format!(
                "Switch to {} on {} through the existing /model route path.",
                self.model,
                self.provider.display_name()
            ),
            category: HotbarActionCategory::Route,
            args: HotbarArgsBehavior::None,
            safety: HotbarSafetyClass::ConfigChange,
            recommendation: HotbarRecommendation::Eligible,
        }
    }

    fn short_label(&self) -> &str {
        &self.short_label
    }

    fn category(&self) -> &str {
        "route"
    }

    fn is_active(&self, app: &App) -> bool {
        !app.auto_model
            && app.api_provider == self.provider
            && app.model.trim().eq_ignore_ascii_case(self.model.trim())
    }

    fn dispatch(&self, _app: &mut App) -> Result<HotbarDispatch> {
        Ok(HotbarDispatch::AppAction(AppAction::SwitchModelRoute {
            provider: self.provider,
            model: self.model.clone(),
        }))
    }
}

fn configured_route_models_for_provider(
    config: &Config,
    provider: ApiProvider,
    active_provider: ApiProvider,
    active_model: &str,
    provider_models: &HashMap<String, String>,
) -> Vec<String> {
    let mut models = Vec::new();
    if provider == active_provider {
        push_route_model(&mut models, active_model);
    }
    if let Some(model) = provider_models.get(provider.as_str()) {
        push_route_model(&mut models, model);
    }
    if let Some(model) = config
        .provider_config_for(provider)
        .and_then(|provider| provider.model.as_deref())
    {
        push_route_model(&mut models, model);
    }
    for model in model_completion_names_for_provider(provider)
        .into_iter()
        .filter(|model| !model.trim().eq_ignore_ascii_case("auto"))
        .take(1)
    {
        push_route_model(&mut models, model);
    }
    models
}

fn push_route_model(models: &mut Vec<String>, model: &str) {
    let trimmed = model.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("auto") {
        return;
    }
    if models
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(trimmed))
    {
        return;
    }
    models.push(trimmed.to_string());
}

fn route_action_id(provider: ApiProvider, model: &str) -> String {
    format!("route.{}.{}", provider.as_str(), model.trim())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};
    use std::path::PathBuf;

    use crate::config::{ApiProvider, Config};
    use crate::tui::app::{ReasoningEffort, TuiOptions};
    use crate::tui::views::ModalKind;

    use super::*;

    fn test_app() -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: PathBuf::from("."),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: true,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        let mut app = App::new(options, &Config::default());
        app.ui_locale = crate::localization::Locale::En;
        app
    }

    struct TestHotbarAction {
        id: &'static str,
    }

    impl HotbarAction for TestHotbarAction {
        fn id(&self) -> &str {
            self.id
        }

        fn metadata(&self, _locale: Locale) -> HotbarActionMetadata {
            HotbarActionMetadata {
                id: self.id.to_string(),
                source_id: "test".to_string(),
                display_name: "Test action".to_string(),
                compact_label: "test".to_string(),
                description: "Test action descriptor".to_string(),
                category: HotbarActionCategory::App,
                args: HotbarArgsBehavior::None,
                safety: HotbarSafetyClass::LocalUi,
                recommendation: HotbarRecommendation::Eligible,
            }
        }

        fn short_label(&self) -> &str {
            "test"
        }

        fn category(&self) -> &str {
            "app"
        }

        fn is_active(&self, _app: &App) -> bool {
            false
        }

        fn dispatch(&self, _app: &mut App) -> Result<HotbarDispatch> {
            Ok(HotbarDispatch::Handled)
        }
    }

    struct DeferredTestHotbarSource;

    impl HotbarActionSource for DeferredTestHotbarSource {
        fn descriptor(&self) -> HotbarSourceDescriptor {
            HOTBAR_SOURCE_DESCRIPTORS
                .iter()
                .copied()
                .find(|descriptor| descriptor.category == HotbarActionCategory::Mcp)
                .expect("mcp descriptor exists")
        }

        fn register_actions(&self, registry: &mut HotbarActionRegistry) {
            registry.register(TestHotbarAction {
                id: "mcp.deferred-test",
            });
        }
    }

    #[test]
    #[should_panic(expected = "duplicate hotbar action id duplicate.action")]
    fn registry_rejects_duplicate_action_ids() {
        let mut registry = HotbarActionRegistry::new();
        registry.register(TestHotbarAction {
            id: "duplicate.action",
        });
        registry.register(TestHotbarAction {
            id: "duplicate.action",
        });
    }

    #[test]
    fn registry_metadata_contract_covers_registered_actions() {
        let registry = HotbarActionRegistry::with_builtins();
        let errors = registry.metadata_validation_errors(Locale::En);
        assert!(errors.is_empty(), "metadata validation failed: {errors:?}");

        let metadata = registry.metadata(Locale::En);
        assert_eq!(metadata.len(), registry.len());

        let ids = metadata
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        let mut sorted_ids = ids.clone();
        sorted_ids.sort_unstable();
        assert_eq!(
            ids, sorted_ids,
            "registry metadata should have stable id order"
        );
        assert_eq!(
            ids.iter().copied().collect::<BTreeSet<_>>().len(),
            ids.len(),
            "metadata ids must be unique"
        );

        for entry in metadata {
            assert_eq!(
                HotbarActionCategory::parse(entry.category.as_str()),
                Some(entry.category)
            );
            let entry_errors = entry.validation_errors();
            assert!(
                entry_errors.is_empty(),
                "metadata entry failed validation: {entry_errors:?}"
            );
            assert!(
                unicode_width::UnicodeWidthStr::width(entry.compact_label.as_str())
                    <= HOTBAR_COMPACT_LABEL_MAX_WIDTH,
                "compact label should be validated: {entry:?}"
            );
        }
    }

    #[test]
    fn source_descriptors_cover_dispatch_boundaries() {
        let descriptors = hotbar_source_descriptors();
        let categories = descriptors
            .iter()
            .map(|descriptor| descriptor.category)
            .collect::<BTreeSet<_>>();

        assert_eq!(
            categories,
            BTreeSet::from([
                HotbarActionCategory::App,
                HotbarActionCategory::Mode,
                HotbarActionCategory::Route,
                HotbarActionCategory::Slash,
                HotbarActionCategory::Mcp,
                HotbarActionCategory::Skill,
                HotbarActionCategory::Plugin,
            ])
        );
        assert_eq!(
            descriptors
                .iter()
                .find(|descriptor| descriptor.category == HotbarActionCategory::App)
                .map(|descriptor| (
                    descriptor.boundary,
                    descriptor.safety_modes,
                    descriptor.registers_dispatchable_actions()
                )),
            Some((
                HotbarSourceDispatchBoundary::DirectApp,
                HOTBAR_DIRECT_APP_SAFETY,
                true
            ))
        );
        assert_eq!(
            descriptors
                .iter()
                .find(|descriptor| descriptor.category == HotbarActionCategory::Mode)
                .map(|descriptor| (
                    descriptor.boundary,
                    descriptor.safety_modes,
                    descriptor.registers_dispatchable_actions()
                )),
            Some((
                HotbarSourceDispatchBoundary::DirectApp,
                HOTBAR_MODE_SAFETY,
                true
            ))
        );
        assert_eq!(
            descriptors
                .iter()
                .find(|descriptor| descriptor.category == HotbarActionCategory::Route)
                .map(|descriptor| (
                    descriptor.boundary,
                    descriptor.safety_modes,
                    descriptor.registers_dispatchable_actions()
                )),
            Some((
                HotbarSourceDispatchBoundary::ModelRoute,
                HOTBAR_ROUTE_SAFETY,
                true
            ))
        );
        assert_eq!(
            descriptors
                .iter()
                .find(|descriptor| descriptor.category == HotbarActionCategory::Slash)
                .map(|descriptor| (
                    descriptor.boundary,
                    descriptor.safety_modes,
                    descriptor.registers_dispatchable_actions()
                )),
            Some((
                HotbarSourceDispatchBoundary::SlashCommand,
                HOTBAR_SLASH_SAFETY,
                true
            ))
        );
        for category in [
            HotbarActionCategory::Mcp,
            HotbarActionCategory::Skill,
            HotbarActionCategory::Plugin,
        ] {
            let descriptor = descriptors
                .iter()
                .find(|descriptor| descriptor.category == category)
                .unwrap_or_else(|| panic!("missing descriptor for {category:?}"));
            assert_eq!(descriptor.boundary, HotbarSourceDispatchBoundary::Deferred);
            assert_eq!(descriptor.safety_modes, HOTBAR_DEFERRED_SAFETY);
            assert_eq!(descriptor.status, "exploratory");
            assert!(
                !descriptor.registers_dispatchable_actions(),
                "deferred {category:?} source must not be dispatchable"
            );
        }
    }

    #[test]
    #[should_panic(expected = "deferred hotbar source Mcp must not register dispatchable actions")]
    fn deferred_sources_cannot_register_dispatchable_actions() {
        let mut registry = HotbarActionRegistry::new();
        registry.register_source(&DeferredTestHotbarSource);
    }

    #[test]
    fn deferred_source_adapters_do_not_register_actions() {
        let mut registry = HotbarActionRegistry::new();
        for category in [
            HotbarActionCategory::Mcp,
            HotbarActionCategory::Skill,
            HotbarActionCategory::Plugin,
        ] {
            let before = registry.len();
            registry.register_source(&DeferredHotbarActionSource::new(category));
            assert_eq!(
                registry.len(),
                before,
                "deferred {category:?} adapter must not add bindable hotbar actions"
            );
        }
    }

    #[test]
    fn source_adapters_register_previous_default_registry_surface() {
        let mut registry = HotbarActionRegistry::new();
        registry.register_source(&BuiltinHotbarActionSource);
        registry.register_source(&ModeHotbarActionSource);
        registry.register_source(&SlashCommandHotbarActionSource);

        let adapter_ids = registry
            .iter()
            .map(|action| action.id().to_string())
            .collect::<Vec<_>>();
        let default_ids = HotbarActionRegistry::with_builtins()
            .iter()
            .map(|action| action.id().to_string())
            .collect::<Vec<_>>();

        assert_eq!(adapter_ids, default_ids);
        assert_eq!(
            BuiltinHotbarActionSource.descriptor().category,
            HotbarActionCategory::App
        );
        assert_eq!(
            ModeHotbarActionSource.descriptor().category,
            HotbarActionCategory::Mode
        );
        assert_eq!(
            SlashCommandHotbarActionSource.descriptor().category,
            HotbarActionCategory::Slash
        );
    }

    #[test]
    fn slash_source_matches_command_palette_command_entries() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let palette_slash_ids = build_command_palette_entries(
            Locale::En,
            tmp.path(),
            true,
            tmp.path(),
            &tmp.path().join("mcp.json"),
            None,
        )
        .into_iter()
        .filter(|entry| entry.section() == crate::tui::command_palette::PaletteSection::Command)
        .filter_map(|entry| {
            entry
                .label
                .strip_prefix('/')
                .map(|name| format!("slash.{name}"))
        })
        .collect::<BTreeSet<_>>();

        let mut registry = HotbarActionRegistry::new();
        registry.register_source(&SlashCommandHotbarActionSource);
        let hotbar_slash_ids = registry
            .iter()
            .map(|action| action.id().to_string())
            .collect::<BTreeSet<_>>();

        assert_eq!(hotbar_slash_ids, palette_slash_ids);
    }

    #[test]
    fn all_sources_keep_deferred_categories_non_bindable() {
        let registry = HotbarActionRegistry::with_all_sources(
            &Config::default(),
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            &HashMap::new(),
        );
        let categories = registry
            .metadata(Locale::En)
            .into_iter()
            .map(|metadata| metadata.category)
            .collect::<BTreeSet<_>>();

        assert!(categories.contains(&HotbarActionCategory::App));
        assert!(categories.contains(&HotbarActionCategory::Mode));
        assert!(categories.contains(&HotbarActionCategory::Route));
        assert!(categories.contains(&HotbarActionCategory::Slash));
        for deferred in [
            HotbarActionCategory::Mcp,
            HotbarActionCategory::Skill,
            HotbarActionCategory::Plugin,
        ] {
            assert!(
                !categories.contains(&deferred),
                "{deferred:?} must stay descriptor-only until dispatch safety is wired"
            );
        }
        assert!(
            registry.metadata_validation_errors(Locale::En).is_empty(),
            "all-source metadata must remain valid"
        );
    }

    #[test]
    fn default_hotbar_actions_have_registered_default_metadata() {
        let registry = HotbarActionRegistry::with_builtins();

        for id in codewhale_config::DEFAULT_HOTBAR_ACTIONS {
            let action = registry
                .get(id)
                .unwrap_or_else(|| panic!("missing default hotbar action {id}"));
            let metadata = action.metadata(Locale::En);
            let expected_category = if id.starts_with("mode.") {
                HotbarActionCategory::Mode
            } else {
                HotbarActionCategory::App
            };
            assert_eq!(metadata.category, expected_category);
            assert_eq!(metadata.args, HotbarArgsBehavior::None);
            assert_eq!(metadata.recommendation, HotbarRecommendation::Default);
            assert!(
                metadata.recommendation.is_recommendable(),
                "default action must be recommendable: {metadata:?}"
            );
            assert!(!metadata.display_name.trim().is_empty());
            assert!(!metadata.description.trim().is_empty());
        }
    }

    #[test]
    fn slash_action_metadata_describes_args_and_recommendations() {
        let registry = HotbarActionRegistry::with_builtins();

        let compact = registry
            .get("slash.compact")
            .expect("compact slash action")
            .metadata(Locale::En);
        assert_eq!(compact.category, HotbarActionCategory::Slash);
        assert_eq!(compact.source_id, "command:compact");
        assert_eq!(compact.display_name, "/compact");
        assert_eq!(compact.args, HotbarArgsBehavior::None);
        assert_eq!(compact.safety, HotbarSafetyClass::ExistingCommand);
        assert_eq!(compact.recommendation, HotbarRecommendation::Eligible);

        let mode = registry
            .get("slash.mode")
            .expect("mode slash action")
            .metadata(Locale::En);
        assert_eq!(mode.args, HotbarArgsBehavior::Optional);

        let rename = registry
            .get("slash.rename")
            .expect("rename slash action")
            .metadata(Locale::En);
        assert_eq!(rename.args, HotbarArgsBehavior::Required);
        assert_eq!(rename.recommendation, HotbarRecommendation::Advanced);
    }

    #[test]
    fn app_action_metadata_exposes_dynamic_disabled_reason() {
        let registry = HotbarActionRegistry::with_builtins();
        let reasoning = registry.get("reasoning.cycle").expect("reasoning action");
        let mut app = test_app();

        let metadata = reasoning.metadata(Locale::En);
        assert_eq!(metadata.category, HotbarActionCategory::App);
        assert_eq!(metadata.safety, HotbarSafetyClass::LocalState);
        assert_eq!(metadata.recommendation, HotbarRecommendation::Eligible);
        assert!(reasoning.disabled_reason(&app).is_none());

        app.auto_model = true;
        assert_eq!(
            reasoning.disabled_reason(&app).as_deref(),
            Some("Reasoning effort is controlled by auto model routing.")
        );
    }

    #[test]
    fn hotbar_recommendations_default_to_stable_slot_order() {
        let app = test_app();

        let recommendations =
            recommend_hotbar_actions(&app, HotbarRecommendationOptions::default());

        assert_eq!(
            recommendations
                .iter()
                .map(|entry| entry.metadata.id.as_str())
                .collect::<Vec<_>>(),
            codewhale_config::DEFAULT_HOTBAR_ACTIONS
        );
        assert!(recommendations.iter().all(|entry| {
            entry.metadata.recommendation == HotbarRecommendation::Default
                && entry.disabled_reason.is_none()
        }));
    }

    #[test]
    fn hotbar_recommendations_exclude_disabled_actions() {
        let mut app = test_app();
        app.auto_model = true;

        let recommendations =
            recommend_hotbar_actions(&app, HotbarRecommendationOptions::for_setup_wizard());

        assert!(
            !recommendations
                .iter()
                .any(|entry| entry.metadata.id == "reasoning.cycle")
        );
    }

    #[test]
    fn hotbar_recommendations_exclude_required_args_by_default() {
        let app = test_app();

        let recommendations =
            recommend_hotbar_actions(&app, HotbarRecommendationOptions::for_setup_wizard());

        assert!(
            !recommendations
                .iter()
                .any(|entry| entry.metadata.id == "slash.rename")
        );
    }

    #[test]
    fn hotbar_recommendations_limit_eligible_actions_by_category() {
        let app = test_app();
        let recommendations = recommend_hotbar_actions(
            &app,
            HotbarRecommendationOptions {
                max_total: usize::MAX,
                max_eligible_per_category: 1,
                include_required_args: false,
            },
        );

        for default_id in codewhale_config::DEFAULT_HOTBAR_ACTIONS {
            assert!(
                recommendations
                    .iter()
                    .any(|entry| entry.metadata.id == default_id),
                "default recommendation {default_id} should not be category-capped"
            );
        }
        let slash_recommendations = recommendations
            .iter()
            .filter(|entry| entry.metadata.category == HotbarActionCategory::Slash)
            .collect::<Vec<_>>();
        assert_eq!(slash_recommendations.len(), 1);
    }

    #[test]
    fn recommended_hotbar_bindings_serialize_action_ids_and_labels() {
        let app = test_app();

        let bindings = recommended_hotbar_bindings(&app, HotbarRecommendationOptions::default());

        assert_eq!(
            bindings
                .iter()
                .map(|binding| binding.action.as_str())
                .collect::<Vec<_>>(),
            codewhale_config::DEFAULT_HOTBAR_ACTIONS
        );
        assert_eq!(
            bindings
                .iter()
                .map(|binding| (binding.slot, binding.label.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (1, Some("voice")),
                (2, Some("compact")),
                (3, Some("plan")),
                (4, Some("agent")),
                (5, Some("yolo")),
                (6, Some("palette")),
                (7, Some("side")),
                (8, Some("trust")),
            ]
        );

        let config = codewhale_config::ConfigToml {
            hotbar: Some(bindings.clone()),
            ..Default::default()
        };
        let serialized = toml::to_string_pretty(&config).expect("serialize hotbar recommendations");
        let round_tripped: codewhale_config::ConfigToml =
            toml::from_str(&serialized).expect("deserialize hotbar recommendations");
        assert_eq!(round_tripped.hotbar, Some(bindings));
    }

    #[test]
    fn builtins_register_expected_actions() {
        let mut registry = HotbarActionRegistry::new();
        registry.register_builtins();
        let ids = registry.iter().map(HotbarAction::id).collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec![
                "filetree.toggle",
                "mode.agent",
                "mode.plan",
                "mode.yolo",
                "palette.open",
                "reasoning.cycle",
                "session.compact",
                "sidebar.toggle",
                "trust.toggle",
                "voice.toggle",
            ]
        );
        assert!(registry.get("missing.action").is_none());
        for action in registry.iter() {
            let expected_category = if action.id().starts_with("mode.") {
                "mode"
            } else {
                "app"
            };
            assert_eq!(action.category(), expected_category);
            assert!(
                unicode_width::UnicodeWidthStr::width(action.short_label())
                    <= HOTBAR_COMPACT_LABEL_MAX_WIDTH,
                "{} has an overlong short label",
                action.id()
            );
        }
    }

    #[test]
    fn app_starts_with_builtin_hotbar_registry() {
        let app = test_app();
        assert!(app.hotbar_actions.len() > HotbarActionRegistry::with_builtins().len());
        assert!(app.hotbar_actions.get("mode.agent").is_some());
        assert!(app.hotbar_actions.get("slash.help").is_some());
        assert!(app.hotbar_actions.get("slash.mode").is_some());
        assert!(
            app.hotbar_actions
                .iter()
                .any(|action| action.metadata(Locale::En).category == HotbarActionCategory::Route)
        );
    }

    #[test]
    fn configured_routes_register_provider_model_actions() {
        let mut config = Config::default();
        config
            .provider_config_for_mut(ApiProvider::Openrouter)
            .model = Some("anthropic/claude-sonnet-4".to_string());
        let mut provider_models = HashMap::new();
        provider_models.insert(
            ApiProvider::Openrouter.as_str().to_string(),
            "openai/gpt-4o".to_string(),
        );
        let registry = HotbarActionRegistry::with_configured_routes(
            &config,
            ApiProvider::Deepseek,
            "deepseek-v4-pro",
            &provider_models,
        );

        let active = registry
            .get("route.deepseek.deepseek-v4-pro")
            .expect("active DeepSeek route");
        assert_eq!(active.category(), "route");
        assert_eq!(
            active.metadata(Locale::En).safety,
            HotbarSafetyClass::ConfigChange
        );

        let openrouter = registry
            .get("route.openrouter.anthropic/claude-sonnet-4")
            .expect("configured OpenRouter route");
        let metadata = openrouter.metadata(Locale::En);
        assert_eq!(metadata.category, HotbarActionCategory::Route);
        assert!(metadata.display_name.contains("OpenRouter"));
        assert!(metadata.display_name.contains("anthropic/claude-sonnet-4"));

        let mut app = test_app();
        assert_eq!(
            openrouter.dispatch(&mut app).expect("dispatch route"),
            HotbarDispatch::AppAction(AppAction::SwitchModelRoute {
                provider: ApiProvider::Openrouter,
                model: "anthropic/claude-sonnet-4".to_string(),
            })
        );
    }

    #[test]
    fn slash_commands_register_as_hotbar_actions() {
        let registry = HotbarActionRegistry::with_builtins();

        for info in commands::command_infos() {
            let action_id = format!("slash.{}", info.name);
            let action = registry
                .get(&action_id)
                .unwrap_or_else(|| panic!("missing slash hotbar action for /{}", info.name));
            assert_eq!(action.category(), "slash");
            assert!(!action.is_active(&test_app()));
            assert!(
                unicode_width::UnicodeWidthStr::width(action.short_label())
                    <= HOTBAR_COMPACT_LABEL_MAX_WIDTH,
                "{action_id} has an overlong short label"
            );
        }
    }

    #[test]
    fn slash_hotbar_action_dispatches_argless_command() {
        let registry = HotbarActionRegistry::with_builtins();
        let mode = registry.get("slash.mode").expect("mode slash action");
        let mut app = test_app();

        assert_eq!(
            mode.dispatch(&mut app).expect("dispatch /mode"),
            HotbarDispatch::AppAction(AppAction::OpenModePicker)
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn slash_hotbar_action_dispatches_optional_argument_command_with_no_args() {
        let registry = HotbarActionRegistry::with_builtins();
        let task = registry.get("slash.task").expect("task slash action");
        let mut app = test_app();

        assert_eq!(
            task.dispatch(&mut app).expect("dispatch /task"),
            HotbarDispatch::AppAction(AppAction::TaskList)
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn slash_hotbar_action_prefills_required_argument_command() {
        let registry = HotbarActionRegistry::with_builtins();
        let rename = registry.get("slash.rename").expect("rename slash action");
        let mut app = test_app();
        app.input = "draft".to_string();
        app.cursor_position = app.input.chars().count();

        assert_eq!(
            rename.dispatch(&mut app).expect("dispatch /rename"),
            HotbarDispatch::Handled
        );
        assert_eq!(app.input, "/rename ");
        assert_eq!(app.cursor_position, app.input.chars().count());
        assert_eq!(app.clear_undo_buffer.as_deref(), Some("draft"));
        assert_eq!(
            app.status_message.as_deref(),
            Some("Command needs arguments; complete /rename")
        );
    }

    #[test]
    fn mode_actions_report_active_state_and_dispatch() {
        let registry = HotbarActionRegistry::with_builtins();
        let plan = registry.get("mode.plan").expect("plan action");
        let agent = registry.get("mode.agent").expect("agent action");
        let yolo = registry.get("mode.yolo").expect("yolo action");
        let mut app = test_app();

        assert!(agent.is_active(&app));
        assert!(!plan.is_active(&app));

        assert_eq!(
            plan.dispatch(&mut app).expect("dispatch plan"),
            HotbarDispatch::AppAction(AppAction::ModeChanged(AppMode::Plan))
        );
        assert_eq!(app.mode, AppMode::Plan);
        assert!(plan.is_active(&app));
        assert!(!agent.is_active(&app));

        assert_eq!(
            yolo.dispatch(&mut app).expect("dispatch yolo"),
            HotbarDispatch::AppAction(AppAction::ModeChanged(AppMode::Yolo))
        );
        assert!(app.allow_shell);
        assert!(app.trust_mode);
        assert!(yolo.is_active(&app));
    }

    #[test]
    fn compact_action_emits_existing_app_action() {
        let registry = HotbarActionRegistry::with_builtins();
        let compact = registry.get("session.compact").expect("compact action");
        let mut app = test_app();

        assert!(!compact.is_active(&app));
        assert_eq!(
            compact.dispatch(&mut app).expect("dispatch compact"),
            HotbarDispatch::AppAction(AppAction::CompactContext)
        );
        app.is_compacting = true;
        assert!(compact.is_active(&app));
        assert_eq!(
            compact
                .dispatch(&mut app)
                .expect("dispatch compact while busy"),
            HotbarDispatch::Handled
        );
        assert_eq!(
            app.status_message.as_deref(),
            Some("Compaction is already running.")
        );
    }

    #[test]
    fn reasoning_cycle_updates_effort_and_compaction() {
        let registry = HotbarActionRegistry::with_builtins();
        let reasoning = registry.get("reasoning.cycle").expect("reasoning action");
        let mut app = test_app();
        app.api_provider = ApiProvider::Deepseek;
        app.reasoning_effort = ReasoningEffort::Off;

        assert!(!reasoning.is_active(&app));
        assert!(matches!(
            reasoning.dispatch(&mut app).expect("dispatch reasoning"),
            HotbarDispatch::AppAction(AppAction::UpdateCompaction(_))
        ));
        assert_eq!(app.reasoning_effort, ReasoningEffort::High);
        assert!(reasoning.is_active(&app));
        assert_eq!(
            app.status_message.as_deref(),
            Some("Reasoning effort: high")
        );

        app.auto_model = true;
        assert!(!reasoning.is_active(&app));
        assert!(reasoning.dispatch(&mut app).is_err());
    }

    #[test]
    fn reasoning_cycle_uses_codex_effort_tiers() {
        let registry = HotbarActionRegistry::with_builtins();
        let reasoning = registry.get("reasoning.cycle").expect("reasoning action");
        let mut app = test_app();
        app.api_provider = ApiProvider::OpenaiCodex;
        app.auto_model = false;
        app.reasoning_effort = ReasoningEffort::Low;

        for (expected_effort, expected_label) in [
            (ReasoningEffort::Medium, "medium"),
            (ReasoningEffort::High, "high"),
            (ReasoningEffort::Max, "xhigh"),
            (ReasoningEffort::Low, "low"),
        ] {
            assert!(matches!(
                reasoning.dispatch(&mut app).expect("dispatch reasoning"),
                HotbarDispatch::AppAction(AppAction::UpdateCompaction(_))
            ));
            assert_eq!(app.reasoning_effort, expected_effort);
            let expected_message = format!("Reasoning effort: {expected_label}");
            assert_eq!(
                app.status_message.as_deref(),
                Some(expected_message.as_str())
            );
        }
    }

    #[test]
    fn sidebar_toggle_reports_visibility_and_dispatches() {
        let registry = HotbarActionRegistry::with_builtins();
        let sidebar = registry.get("sidebar.toggle").expect("sidebar action");
        let mut app = test_app();
        app.sidebar_focus = SidebarFocus::Pinned;

        assert!(sidebar.is_active(&app));
        assert_eq!(
            sidebar.dispatch(&mut app).expect("dispatch sidebar hide"),
            HotbarDispatch::Handled
        );
        assert_eq!(app.sidebar_focus, SidebarFocus::Hidden);
        assert!(!sidebar.is_active(&app));

        sidebar.dispatch(&mut app).expect("dispatch sidebar show");
        assert_eq!(app.sidebar_focus, SidebarFocus::Pinned);
        assert!(sidebar.is_active(&app));
    }

    #[tokio::test]
    async fn filetree_toggle_reports_open_state_and_dispatches() {
        let registry = HotbarActionRegistry::with_builtins();
        let filetree = registry.get("filetree.toggle").expect("filetree action");
        let mut app = test_app();

        assert!(!filetree.is_active(&app));
        assert_eq!(
            filetree.dispatch(&mut app).expect("dispatch filetree open"),
            HotbarDispatch::Handled
        );
        assert!(app.file_tree.is_some());
        assert!(filetree.is_active(&app));

        filetree
            .dispatch(&mut app)
            .expect("dispatch filetree close");
        assert!(app.file_tree.is_none());
        assert!(!filetree.is_active(&app));
    }

    #[test]
    fn palette_action_opens_command_palette() {
        let registry = HotbarActionRegistry::with_builtins();
        let palette = registry.get("palette.open").expect("palette action");
        let mut app = test_app();

        assert!(!palette.is_active(&app));
        assert_eq!(
            palette.dispatch(&mut app).expect("dispatch palette"),
            HotbarDispatch::Handled
        );
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::CommandPalette));
    }

    #[test]
    fn trust_toggle_reports_trust_state_and_dispatches() {
        let registry = HotbarActionRegistry::with_builtins();
        let trust = registry.get("trust.toggle").expect("trust action");
        let mut app = test_app();
        app.trust_mode = false;

        assert!(!trust.is_active(&app));
        assert_eq!(
            trust.dispatch(&mut app).expect("dispatch trust on"),
            HotbarDispatch::Handled
        );
        assert!(app.trust_mode);
        assert!(trust.is_active(&app));

        trust.dispatch(&mut app).expect("dispatch trust off");
        assert!(!app.trust_mode);
        assert!(!trust.is_active(&app));
    }

    #[test]
    fn voice_toggle_dispatches_the_voice_command() {
        let registry = HotbarActionRegistry::with_builtins();
        let voice = registry.get("voice.toggle").expect("voice action");
        let mut app = test_app();

        assert!(!voice.is_active(&app));
        // The toggle is wired to the /voice command. With a recorder on the
        // host it arms voice input and defers capture to the UI event loop;
        // without one it fails gracefully with a localized error. No audio
        // is recorded in either case.
        let result = voice.dispatch(&mut app).expect("dispatch voice");
        assert!(app.status_message.is_some());
        // The old placeholder message must be gone — voice is implemented.
        assert_ne!(
            app.status_message.as_deref(),
            Some("Voice input is not available in this terminal session yet.")
        );
        if app.voice_enabled {
            assert_eq!(
                result,
                HotbarDispatch::AppAction(crate::tui::app::AppAction::VoiceCapture)
            );
            assert!(voice.is_active(&app));
            // A second press toggles voice input back off.
            let off = voice.dispatch(&mut app).expect("dispatch voice off");
            assert_eq!(off, HotbarDispatch::Handled);
            assert!(!app.voice_enabled);
            assert!(!voice.is_active(&app));
        } else {
            assert_eq!(result, HotbarDispatch::Handled);
        }
    }
}
