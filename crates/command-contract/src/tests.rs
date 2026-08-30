use std::path::{Path, PathBuf};

use codewhale_core::request::{Message, SystemPrompt};

use crate::*;

struct Session;
impl CommandSessionContext for Session {
    fn session_id(&self) -> Option<String> {
        Some("session".into())
    }
    fn api_messages(&self) -> Vec<Message> {
        vec![]
    }
    fn add_message(&mut self, _message: Message) {}
    fn queued_message_count(&self) -> usize {
        0
    }
    fn remove_queued_message(&mut self, _index: usize) -> Result<(), String> {
        Ok(())
    }
    fn total_tokens(&self) -> u64 {
        42
    }
}

struct Model;
impl CommandModelContext for Model {
    fn current_model(&self) -> String {
        "auto".into()
    }
    fn auto_model(&self) -> bool {
        true
    }
    fn set_model_selection(&mut self, _model: String, _provider: Option<CommandProviderId>) {}
    fn reasoning_effort(&self) -> CommandReasoningEffort {
        CommandReasoningEffort::Auto
    }
    fn provider_identity(&self) -> Option<CommandProviderId> {
        None
    }
    fn fallback_chain(&self) -> Vec<CommandProviderId> {
        vec![]
    }
}

struct Cost;
impl CommandCostContext for Cost {
    fn display_currency(&self) -> CommandCurrency {
        CommandCurrency::Usd
    }
    fn session_cost_for_currency(&self, _currency: CommandCurrency) -> f64 {
        1.0
    }
    fn subagent_cost_for_currency(&self, _currency: CommandCurrency) -> f64 {
        0.5
    }
    fn accrue_cost_estimate(&mut self, _amount: f64, _currency: CommandCurrency) {}
    fn record_turn_cost(
        &mut self,
        _amount: f64,
        _currency: CommandCurrency,
        _receipt: Option<String>,
    ) {
    }
}

struct Policy;
impl CommandModePolicyContext for Policy {
    fn mode(&self) -> CommandMode {
        CommandMode::Plan
    }
    fn set_mode(&mut self, _mode: CommandMode) {}
    fn approval_mode(&self) -> CommandApprovalMode {
        CommandApprovalMode::Suggest
    }
    fn allow_shell(&self) -> bool {
        false
    }
    fn set_shell_access(&mut self, _allow: bool) {}
    fn policy_locked(&self) -> bool {
        false
    }
}

struct Prompt;
impl CommandSystemPromptContext for Prompt {
    fn system_prompt(&self) -> Option<SystemPrompt> {
        None
    }
}

struct Skills;
impl CommandSkillsContext for Skills {
    fn active_skill(&self) -> Option<String> {
        None
    }
    fn active_skill_provenance(&self) -> Option<String> {
        None
    }
    fn refresh_skill_cache(&mut self) {}
}

struct Workspace;
impl CommandWorkspaceContext for Workspace {
    fn workspace(&self) -> PathBuf {
        PathBuf::from(".")
    }
    fn work_state_snapshot(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn operation_digest(&mut self) -> Result<String, String> {
        Ok("No active operations or to-do items.".to_string())
    }
}

#[test]
fn all_seven_shapes_are_object_safe() {
    fn session(_: &dyn CommandSessionContext) {}
    fn model(_: &dyn CommandModelContext) {}
    fn cost(_: &dyn CommandCostContext) {}
    fn policy(_: &dyn CommandModePolicyContext) {}
    fn prompt(_: &dyn CommandSystemPromptContext) {}
    fn skills(_: &dyn CommandSkillsContext) {}
    fn workspace(_: &dyn CommandWorkspaceContext) {}

    session(&Session);
    model(&Model);
    cost(&Cost);
    policy(&Policy);
    prompt(&Prompt);
    skills(&Skills);
    workspace(&Workspace);
}

#[test]
fn envelope_carries_independent_facets() {
    let mut session = Session;
    let mut model = Model;
    let parts = CommandContexts::empty()
        .with_session(&mut session)
        .with_model(&mut model)
        .into_parts();
    assert_eq!(parts.session.expect("session").total_tokens(), 42);
    assert!(parts.model.expect("model").auto_model());
    assert!(parts.cost.is_none());
}

fn pure(value: Option<&str>) -> String {
    value.unwrap_or_default().to_owned()
}
fn contextual(_contexts: CommandContexts<'_>, value: Option<&str>) -> String {
    value.unwrap_or_default().to_owned()
}

#[test]
fn handlers_are_plain_function_pointers() {
    let pure_handler = CommandHandler::Pure(pure);
    let contextual_handler = CommandHandler::Contextual(contextual);
    match pure_handler {
        CommandHandler::Pure(handler) => assert_eq!(handler(Some("x")), "x"),
        _ => unreachable!(),
    }
    match contextual_handler {
        CommandHandler::Contextual(handler) => {
            assert_eq!(handler(CommandContexts::empty(), Some("y")), "y")
        }
        _ => unreachable!(),
    }
}

struct Sample;
impl RegisterCommand<String> for Sample {
    fn info() -> &'static CommandInfo {
        static INFO: CommandInfo = CommandInfo {
            name: "sample",
            aliases: &["s"],
            usage: "/sample",
            description_key: "command.sample",
        };
        &INFO
    }
    fn handler() -> CommandHandler<String> {
        CommandHandler::Pure(pure)
    }
}

#[test]
fn registration_shape_has_no_app_dependency() {
    assert_eq!(Sample::info().name, "sample");
    assert!(matches!(Sample::handler(), CommandHandler::Pure(_)));
}

// ---------------------------------------------------------------------------
// FEAT-018: presentation, media, and digest capabilities (D2-D5)
// ---------------------------------------------------------------------------

struct Presentation;
impl CommandPresentationContext for Presentation {
    fn translate(&self, key: &str, replacements: &[(&str, &str)]) -> Result<String, String> {
        if key == "automation_usage" {
            return Ok("Usage: /automation [list|show <id>]".to_string());
        }
        if key == "mcp_recommended_unknown_id" {
            let command = replacements
                .iter()
                .find(|(name, _)| *name == "recommendations_command")
                .map(|(_, value)| *value)
                .unwrap_or("/mcp recommendations");
            return Ok(format!("Unknown recommended MCP ID (try {command})"));
        }
        // D3: unknown keys fail safely without echoing the raw lookup key.
        Err("unknown translation key".to_string())
    }
}

struct Media;
impl CommandMediaContext for Media {
    fn attach_media(&mut self, path: &Path) -> Result<MediaAttachmentReceipt, String> {
        if path.extension().and_then(|ext| ext.to_str()) == Some("png") {
            Ok(MediaAttachmentReceipt {
                kind: "image".to_string(),
                path: path.to_path_buf(),
            })
        } else {
            Err("Unsupported attachment type".to_string())
        }
    }
}

struct DigestWorkspace;
impl CommandWorkspaceContext for DigestWorkspace {
    fn workspace(&self) -> PathBuf {
        PathBuf::from(".")
    }
    fn work_state_snapshot(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn operation_digest(&mut self) -> Result<String, String> {
        Ok("No active operations or to-do items.".to_string())
    }
}

#[test]
fn new_capabilities_are_object_safe_and_independently_transportable() {
    fn presentation(_: &dyn CommandPresentationContext) {}
    fn media(_: &dyn CommandMediaContext) {}
    fn digest_workspace(_: &dyn CommandWorkspaceContext) {}

    presentation(&Presentation);
    media(&Media);
    digest_workspace(&DigestWorkspace);

    let mut presentation = Presentation;
    let mut media = Media;
    let parts = CommandContexts::empty()
        .with_presentation(&mut presentation)
        .with_media(&mut media)
        .into_parts();
    assert!(parts.presentation.is_some());
    assert!(parts.media.is_some());
    assert!(parts.session.is_none());
}

#[test]
fn translation_contract_resolves_known_keys_and_fails_safely() {
    let presentation = Presentation;
    assert_eq!(
        presentation
            .translate("automation_usage", &[])
            .expect("known key"),
        "Usage: /automation [list|show <id>]"
    );
    assert_eq!(
        presentation
            .translate(
                "mcp_recommended_unknown_id",
                &[("recommendations_command", "/mcp recommendations")],
            )
            .expect("known key with named replacement"),
        "Unknown recommended MCP ID (try /mcp recommendations)"
    );
    let unknown = presentation.translate("no_such_key", &[]);
    assert!(unknown.is_err(), "unknown key must fail safely");
    let err = unknown.unwrap_err();
    assert!(
        !err.contains("no_such_key"),
        "no raw lookup key exposure (D3)"
    );
}

#[test]
fn media_contract_is_atomic_and_returns_only_portable_data() {
    let mut media = Media;
    let ok = media
        .attach_media(Path::new("/tmp/photo.png"))
        .expect("png");
    assert_eq!(ok.kind, "image");
    assert_eq!(ok.path, PathBuf::from("/tmp/photo.png"));

    let err = media.attach_media(Path::new("/tmp/notes.txt")).unwrap_err();
    assert!(!err.is_empty(), "safe error string");
}

#[test]
fn digest_operation_returns_final_text_and_safe_errors() {
    let mut workspace = DigestWorkspace;
    assert_eq!(
        workspace.operation_digest().expect("digest"),
        "No active operations or to-do items."
    );
}

#[test]
fn envelope_rejects_duplicate_new_slots_deterministically() {
    struct SecondPresentation;
    impl CommandPresentationContext for SecondPresentation {
        fn translate(&self, _key: &str, _r: &[(&str, &str)]) -> Result<String, String> {
            Ok(String::new())
        }
    }
    struct SecondMedia;
    impl CommandMediaContext for SecondMedia {
        fn attach_media(&mut self, _p: &Path) -> Result<MediaAttachmentReceipt, String> {
            Err("unused".to_string())
        }
    }

    let mut a = Presentation;
    let mut b = SecondPresentation;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        CommandContexts::empty()
            .with_presentation(&mut a)
            .with_presentation(&mut b);
    }));
    assert!(result.is_err(), "duplicate presentation slot must assert");

    let mut a = Media;
    let mut b = SecondMedia;
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        CommandContexts::empty()
            .with_media(&mut a)
            .with_media(&mut b);
    }));
    assert!(result.is_err(), "duplicate media slot must assert");
}

// Project facet (FEAT-021 D1/D4)
// ---------------------------------------------------------------------------

/// Deterministic fake project facet over portable values only.
struct FakeProject {
    lsp_enabled: bool,
    share: ProjectShareProjection,
    goal: ProjectGoalState,
}

impl FakeProject {
    fn new() -> Self {
        Self {
            lsp_enabled: false,
            share: ProjectShareProjection {
                history_is_empty: true,
                history_len: 0,
                model: "deepseek-chat".to_string(),
                mode_label: "ACT".to_string(),
            },
            goal: ProjectGoalState {
                objective: Some("Ship FEAT-021".to_string()),
                status: ProjectGoalStatus::Active,
                pause_reason: None,
                started_at_elapsed_seconds: Some(42),
                time_used_seconds: 42,
                token_budget: Some(50_000),
                tokens_used: 1_000,
                session_total_tokens: 2_000,
                continuation_count: 3,
                pending_controls: false,
                last_known_objective: None,
                last_known_status: None,
                conversation_present: true,
                is_loading: false,
                goal_continuation_waiting: false,
            },
        }
    }
}

impl CommandProjectContext for FakeProject {
    fn lsp_enabled(&self) -> bool {
        self.lsp_enabled
    }

    fn lsp_set(&mut self, enabled: bool) -> Result<(), String> {
        self.lsp_enabled = enabled;
        Ok(())
    }

    fn share_projection(&self) -> ProjectShareProjection {
        self.share.clone()
    }

    fn goal_state(&self) -> ProjectGoalState {
        self.goal.clone()
    }
}

#[test]
fn project_facet_is_object_safe_and_typed() {
    fn project(_: &dyn CommandProjectContext) {}
    project(&FakeProject::new());

    let mut project = FakeProject::new();
    assert!(!project.lsp_enabled());
    project.lsp_set(true).unwrap();
    assert!(project.lsp_enabled());
    project.lsp_set(false).unwrap();
    assert!(!project.lsp_enabled());
}

#[test]
fn project_share_projection_preserves_semantic_values() {
    let project = FakeProject::new();
    let share = project.share_projection();
    assert!(share.history_is_empty);
    assert_eq!(share.history_len, 0);
    assert_eq!(share.model, "deepseek-chat");
    assert_eq!(share.mode_label, "ACT");
}

#[test]
fn project_goal_state_preserves_semantic_values() {
    let project = FakeProject::new();
    let goal = project.goal_state();
    assert_eq!(goal.objective.as_deref(), Some("Ship FEAT-021"));
    assert_eq!(goal.status, ProjectGoalStatus::Active);
    assert_eq!(goal.pause_reason, None);
    assert_eq!(goal.started_at_elapsed_seconds, Some(42));
    assert_eq!(goal.time_used_seconds, 42);
    assert_eq!(goal.token_budget, Some(50_000));
    assert_eq!(goal.tokens_used, 1_000);
    assert_eq!(goal.session_total_tokens, 2_000);
    assert_eq!(goal.continuation_count, 3);
    assert!(!goal.pending_controls);
    assert_eq!(goal.last_known_objective, None);
    assert_eq!(goal.last_known_status, None);
    assert!(goal.conversation_present);
    assert!(!goal.is_loading);
    assert!(!goal.goal_continuation_waiting);
}

#[test]
fn project_goal_status_variants_are_distinguishable() {
    let paused = ProjectGoalState {
        status: ProjectGoalStatus::Paused,
        pause_reason: Some("user".to_string()),
        ..FakeProject::new().goal
    };
    assert_eq!(paused.status, ProjectGoalStatus::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("user"));

    let complete = ProjectGoalState {
        status: ProjectGoalStatus::Complete,
        ..paused
    };
    assert_eq!(complete.status, ProjectGoalStatus::Complete);
    assert_ne!(complete.status, ProjectGoalStatus::Blocked);
}

#[test]
fn project_facet_transports_through_envelope_when_declared() {
    let mut project = FakeProject::new();
    let parts = CommandContexts::empty()
        .with_project(&mut project)
        .into_parts();
    assert!(parts.project.is_some());
    assert!(parts.session.is_none());

    // PROJECT combined with WORKSPACE (init) and PRESENTATION (goal).
    let mut workspace = Workspace;
    let parts = CommandContexts::empty()
        .with_project(&mut project)
        .with_workspace(&mut workspace)
        .into_parts();
    assert!(parts.project.is_some());
    assert!(parts.workspace.is_some());
    assert!(parts.presentation.is_none());
}

#[test]
fn envelope_rejects_duplicate_project_slot_deterministically() {
    let mut a = FakeProject::new();
    let mut b = FakeProject::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        CommandContexts::empty()
            .with_project(&mut a)
            .with_project(&mut b);
    }));
    assert!(result.is_err(), "duplicate project slot must assert");
}
