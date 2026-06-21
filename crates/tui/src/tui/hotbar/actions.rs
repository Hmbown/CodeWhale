use std::collections::BTreeMap;
use std::sync::Arc;

use anyhow::{Result, bail};

use crate::commands::{self, CommandInfo, CommandResult};
use crate::tui::app::{App, AppAction, AppMode, SidebarFocus};
use crate::tui::command_palette::{
    CommandPaletteView, build_entries as build_command_palette_entries,
};

/// Result of firing a hotbar action.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum HotbarDispatch {
    /// The action was fully handled by mutating [`App`].
    Handled,
    /// The event loop must handle an existing application action.
    AppAction(AppAction),
}

/// Uniform interface for actions that can be bound to a hotbar slot.
#[allow(dead_code)]
pub trait HotbarAction: Send + Sync {
    /// Stable action id used in config and dispatch.
    fn id(&self) -> &str;

    /// Compact cell label. Built-ins keep this at seven characters or less.
    fn short_label(&self) -> &str;

    /// Source category, such as `app`, `slash`, `mcp`, `skill`, or `plugin`.
    fn category(&self) -> &str;

    /// Whether the action is currently active in the supplied app state.
    fn is_active(&self, app: &App) -> bool;

    /// Fire the action.
    fn dispatch(&self, app: &mut App) -> Result<HotbarDispatch>;
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

    pub fn register(&mut self, action: impl HotbarAction + 'static) {
        self.actions
            .insert(action.id().to_string(), Arc::new(action));
    }

    pub(crate) fn register_builtins(&mut self) {
        self.register(AppHotbarAction::new(
            "voice.toggle",
            "voice",
            AppHotbarKind::VoiceToggle,
        ));
        self.register(AppHotbarAction::new(
            "session.compact",
            "compact",
            AppHotbarKind::SessionCompact,
        ));
        self.register(AppHotbarAction::new(
            "mode.plan",
            "plan",
            AppHotbarKind::Mode(AppMode::Plan),
        ));
        self.register(AppHotbarAction::new(
            "mode.agent",
            "agent",
            AppHotbarKind::Mode(AppMode::Agent),
        ));
        self.register(AppHotbarAction::new(
            "mode.yolo",
            "yolo",
            AppHotbarKind::Mode(AppMode::Yolo),
        ));
        self.register(AppHotbarAction::new(
            "reasoning.cycle",
            "reason",
            AppHotbarKind::ReasoningCycle,
        ));
        self.register(AppHotbarAction::new(
            "sidebar.toggle",
            "side",
            AppHotbarKind::SidebarToggle,
        ));
        self.register(AppHotbarAction::new(
            "filetree.toggle",
            "files",
            AppHotbarKind::FileTreeToggle,
        ));
        self.register(AppHotbarAction::new(
            "palette.open",
            "palette",
            AppHotbarKind::PaletteOpen,
        ));
        self.register(AppHotbarAction::new(
            "trust.toggle",
            "trust",
            AppHotbarKind::TrustToggle,
        ));
    }

    pub(crate) fn register_slash_commands(&mut self) {
        for info in commands::command_infos() {
            self.register(SlashHotbarAction::new(info));
        }
    }

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
    Mode(AppMode),
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
    kind: AppHotbarKind,
}

impl AppHotbarAction {
    const fn new(id: &'static str, short_label: &'static str, kind: AppHotbarKind) -> Self {
        Self {
            id,
            short_label,
            kind,
        }
    }
}

impl HotbarAction for AppHotbarAction {
    fn id(&self) -> &str {
        self.id
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
            AppHotbarKind::Mode(mode) => app.mode == mode,
            AppHotbarKind::ReasoningCycle => {
                !app.auto_model && app.reasoning_effort != crate::tui::app::ReasoningEffort::Off
            }
            AppHotbarKind::SidebarToggle => app.sidebar_focus != SidebarFocus::Hidden,
            AppHotbarKind::FileTreeToggle => app.file_tree.is_some(),
            AppHotbarKind::PaletteOpen => false,
            AppHotbarKind::TrustToggle => app.trust_mode,
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
            AppHotbarKind::Mode(mode) => {
                let changed = app.set_mode(mode);
                if changed {
                    Ok(HotbarDispatch::AppAction(AppAction::ModeChanged(mode)))
                } else {
                    Ok(HotbarDispatch::Handled)
                }
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

#[cfg(test)]
mod tests {
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
            assert_eq!(action.category(), "app");
            assert!(
                action.short_label().chars().count() <= 7,
                "{} has an overlong short label",
                action.id()
            );
        }
    }

    #[test]
    fn app_starts_with_builtin_hotbar_registry() {
        let app = test_app();
        assert_eq!(
            app.hotbar_actions.len(),
            HotbarActionRegistry::with_builtins().len()
        );
        assert!(app.hotbar_actions.get("mode.agent").is_some());
        assert!(app.hotbar_actions.get("slash.help").is_some());
        assert!(app.hotbar_actions.get("slash.mode").is_some());
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
                action.short_label().chars().count() <= 7,
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
