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
    let contextual_handler = CommandHandler::Contextual {
        capabilities: CommandCapabilities::NONE,
        handler: contextual,
    };
    match pure_handler {
        CommandHandler::Pure(handler) => assert_eq!(handler(Some("x")), "x"),
        _ => unreachable!(),
    }
    match contextual_handler {
        CommandHandler::Contextual {
            capabilities,
            handler,
        } => {
            assert!(capabilities.is_empty());
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

// ---------------------------------------------------------------------------
// FEAT-019: memory capability, typed outcomes, and workspace scoping (D1-D9)
// ---------------------------------------------------------------------------

/// Deterministic fake memory facet over portable values only. Tracks the
/// workspace argument discipline (D8): only workspace-scoped methods receive
/// the workspace path.
struct FakeMemory {
    hits: Vec<MemoryHit>,
    remembered_result: Option<MemoryRemembered>,
    workspace_id_result: Result<String, String>,
}

impl FakeMemory {
    fn new() -> Self {
        Self {
            hits: vec![MemoryHit {
                source: PathBuf::from("/mem/source.md"),
                line_start: 3,
                line_end: 5,
                text: "reviewed note".to_string(),
            }],
            remembered_result: Some(MemoryRemembered {
                source: PathBuf::from("/mem/global.md"),
                line_start: 7,
            }),
            workspace_id_result: Ok("owner/repo".to_string()),
        }
    }
}

impl CommandMemoryContext for FakeMemory {
    fn memory_path(&self) -> PathBuf {
        PathBuf::from("/mem/user-memory.md")
    }

    fn memory_enabled(&self) -> bool {
        true
    }

    fn status(&self) -> Result<MemoryStatus, String> {
        Ok(MemoryStatus {
            root: PathBuf::from("/mem/memory"),
            source: PathBuf::from("/mem/memory/global/global.md"),
            index: PathBuf::from("/mem/memory/index.db"),
        })
    }

    fn path(&self) -> Result<PathBuf, String> {
        Ok(PathBuf::from("/mem/memory"))
    }

    fn workspace_id(&self, _workspace: &Path) -> Result<String, String> {
        self.workspace_id_result.clone()
    }

    fn search(
        &self,
        _workspace: &Path,
        query: &str,
        limit: usize,
    ) -> Result<Vec<MemoryHit>, String> {
        if query.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self.hits.iter().take(limit).cloned().collect())
    }

    fn remember(
        &self,
        _target: MemoryRememberTarget,
        note: &str,
    ) -> Result<MemoryRemembered, String> {
        if note.is_empty() {
            return Err("empty note".to_string());
        }
        Ok(self.remembered_result.clone().unwrap_or(MemoryRemembered {
            source: PathBuf::from("/mem/global.md"),
            line_start: 1,
        }))
    }

    fn import(&self) -> Result<MemoryImportOutcome, String> {
        Ok(MemoryImportOutcome::Skipped)
    }

    fn get(&self, _workspace: &Path, id: i64) -> Result<MemoryGetOutcome, String> {
        if id == 42 {
            Ok(MemoryGetOutcome::Found(self.hits[0].clone()))
        } else {
            Ok(MemoryGetOutcome::NotFound)
        }
    }

    fn export(&self) -> Result<MemoryExport, String> {
        Ok(MemoryExport {
            content: "# memory\n\n- bullet".to_string(),
        })
    }

    fn reindex(&self) -> Result<MemoryReindex, String> {
        Ok(MemoryReindex { entry_count: 3 })
    }

    fn delete(&self, scope: MemoryDeleteScope) -> Result<MemoryDelete, String> {
        match scope {
            MemoryDeleteScope::All => Ok(MemoryDelete),
            MemoryDeleteScope::Global => Ok(MemoryDelete),
        }
    }

    fn delete_workspace(&self, _workspace: &Path) -> Result<MemoryDelete, String> {
        Ok(MemoryDelete)
    }
}

/// Recording fake that captures remember targets and delete scopes to prove
/// the typed target/scope discipline (D2/D8/D9). Interior mutability lets the
/// contract-level test assert exactly which operations the handler drives.
#[derive(Default)]
struct RecordingMemory {
    remembered_targets: std::cell::RefCell<Vec<MemoryRememberTarget>>,
    delete_scopes: std::cell::RefCell<Vec<String>>,
    workspace_deletes: std::cell::Cell<usize>,
}

impl RecordingMemory {
    fn new() -> Self {
        Self::default()
    }

    fn recorded_targets(&self) -> Vec<MemoryRememberTarget> {
        self.remembered_targets.borrow().clone()
    }

    fn recorded_delete_scopes(&self) -> Vec<String> {
        self.delete_scopes.borrow().clone()
    }

    fn recorded_workspace_deletes(&self) -> usize {
        self.workspace_deletes.get()
    }
}

impl CommandMemoryContext for RecordingMemory {
    fn memory_path(&self) -> PathBuf {
        PathBuf::from("/mem/user-memory.md")
    }

    fn memory_enabled(&self) -> bool {
        true
    }

    fn status(&self) -> Result<MemoryStatus, String> {
        unreachable!("recording fake")
    }

    fn path(&self) -> Result<PathBuf, String> {
        unreachable!("recording fake")
    }

    fn workspace_id(&self, _workspace: &Path) -> Result<String, String> {
        Ok("owner/repo".to_string())
    }

    fn search(
        &self,
        _workspace: &Path,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<MemoryHit>, String> {
        unreachable!("recording fake")
    }

    fn remember(
        &self,
        target: MemoryRememberTarget,
        _note: &str,
    ) -> Result<MemoryRemembered, String> {
        self.remembered_targets.borrow_mut().push(target);
        Ok(MemoryRemembered {
            source: PathBuf::from("/mem/global.md"),
            line_start: 1,
        })
    }

    fn import(&self) -> Result<MemoryImportOutcome, String> {
        unreachable!("recording fake")
    }

    fn get(&self, _workspace: &Path, _id: i64) -> Result<MemoryGetOutcome, String> {
        unreachable!("recording fake")
    }

    fn export(&self) -> Result<MemoryExport, String> {
        unreachable!("recording fake")
    }

    fn reindex(&self) -> Result<MemoryReindex, String> {
        unreachable!("recording fake")
    }

    fn delete(&self, scope: MemoryDeleteScope) -> Result<MemoryDelete, String> {
        self.delete_scopes.borrow_mut().push(match scope {
            MemoryDeleteScope::All => "all".to_string(),
            MemoryDeleteScope::Global => "global".to_string(),
        });
        Ok(MemoryDelete)
    }

    fn delete_workspace(&self, _workspace: &Path) -> Result<MemoryDelete, String> {
        self.workspace_deletes.set(self.workspace_deletes.get() + 1);
        Ok(MemoryDelete)
    }
}

#[test]
fn memory_facet_is_object_safe_and_typed() {
    fn memory(_: &dyn CommandMemoryContext) {}
    let fake = FakeMemory::new();
    memory(&fake);

    assert_eq!(fake.memory_path(), PathBuf::from("/mem/user-memory.md"));
    assert!(fake.memory_enabled());
    let status = fake.status().expect("status");
    assert_eq!(status.root, PathBuf::from("/mem/memory"));
    assert_eq!(status.source, PathBuf::from("/mem/memory/global/global.md"));
    assert_eq!(status.index, PathBuf::from("/mem/memory/index.db"));
}

#[test]
fn memory_typed_results_preserve_semantic_distinctions() {
    let fake = FakeMemory::new();

    // Search returns semantic hits, never preformatted messages.
    let hits = fake.search(Path::new("/ws"), "note", 10).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source, PathBuf::from("/mem/source.md"));
    assert_eq!(hits[0].line_start, 3);
    assert_eq!(hits[0].line_end, 5);
    assert_eq!(hits[0].text, "reviewed note");
    assert!(
        fake.search(Path::new("/ws"), "", 10)
            .expect("empty")
            .is_empty()
    );

    // Get distinguishes found from not-found without an error string.
    assert!(matches!(
        fake.get(Path::new("/ws"), 42),
        Ok(MemoryGetOutcome::Found(_))
    ));
    assert_eq!(
        fake.get(Path::new("/ws"), 1).expect("get"),
        MemoryGetOutcome::NotFound
    );

    // Export carries the raw document, not a command response.
    let exported = fake.export().expect("export");
    assert_eq!(exported.content, "# memory\n\n- bullet");

    // Reindex carries the typed count.
    assert_eq!(fake.reindex().expect("reindex").entry_count, 3);

    // Remember distinguishes global from workspace via the typed target.
    let global = fake
        .remember(MemoryRememberTarget::Global, "note")
        .expect("global remember");
    assert_eq!(global.source, PathBuf::from("/mem/global.md"));
    assert_eq!(global.line_start, 7);
    let workspace = fake
        .remember(
            MemoryRememberTarget::Workspace {
                workspace_id: "owner/repo".to_string(),
            },
            "note",
        )
        .expect("workspace remember");
    assert_eq!(workspace.source, PathBuf::from("/mem/global.md"));

    // Import distinguishes imported from skipped.
    assert_eq!(fake.import().expect("import"), MemoryImportOutcome::Skipped);
    assert_eq!(
        MemoryImportOutcome::Imported {
            destination: PathBuf::from("/mem/global.md")
        },
        MemoryImportOutcome::Imported {
            destination: PathBuf::from("/mem/global.md")
        }
    );

    // Remember rejects empty notes with a safe error, never a panic.
    assert!(fake.remember(MemoryRememberTarget::Global, "").is_err());

    // Zero-field delete outcome stays distinguishable.
    assert_eq!(fake.delete(MemoryDeleteScope::All), Ok(MemoryDelete));
}

#[test]
fn memory_delete_and_remember_targets_are_typed_and_scoped() {
    let memory = RecordingMemory::new();
    let _ = memory.delete(MemoryDeleteScope::All);
    let _ = memory.delete(MemoryDeleteScope::Global);
    let _ = memory.delete_workspace(Path::new("/ws"));
    let _ = memory.remember(MemoryRememberTarget::Global, "a");
    let _ = memory.remember(
        MemoryRememberTarget::Workspace {
            workspace_id: "owner/repo".to_string(),
        },
        "b",
    );

    // The non-workspace delete method receives exactly the all/global scopes;
    // workspace deletion goes through the distinct typed method (D8/D9).
    assert_eq!(memory.recorded_delete_scopes(), vec!["all", "global"]);
    assert_eq!(memory.recorded_workspace_deletes(), 1);

    // Remember targets preserve the typed global/workspace distinction.
    assert_eq!(
        memory.recorded_targets(),
        vec![
            MemoryRememberTarget::Global,
            MemoryRememberTarget::Workspace {
                workspace_id: "owner/repo".to_string(),
            },
        ]
    );
}

#[test]
fn capabilities_declare_exact_memory_authority() {
    let workspace = CommandCapabilities::WORKSPACE;
    let memory = CommandCapabilities::MEMORY;
    let workspace_memory = workspace.union(memory);

    assert_eq!(
        workspace_memory,
        CommandCapabilities::WORKSPACE | CommandCapabilities::MEMORY
    );
    assert_ne!(workspace_memory, workspace);
    assert_ne!(workspace_memory, memory);
    assert!(workspace_memory.contains(CommandCapabilities::WORKSPACE));
    assert!(workspace_memory.contains(CommandCapabilities::MEMORY));
    assert!(!workspace.contains(CommandCapabilities::MEMORY));
    assert!(!memory.contains(CommandCapabilities::WORKSPACE));
    assert!(CommandCapabilities::NONE.is_empty());
    // No presentation or media authority is declared for the memory group.
    assert!(!workspace_memory.contains(CommandCapabilities::PRESENTATION));
    assert!(!workspace_memory.contains(CommandCapabilities::MEDIA));
    // Existing capability identities stay stable.
    assert_ne!(CommandCapabilities::SESSION, CommandCapabilities::MODEL);
}

#[test]
fn memory_facet_transports_through_envelope_when_declared() {
    let mut memory = FakeMemory::new();
    let parts = CommandContexts::empty()
        .with_memory(&mut memory)
        .into_parts();
    assert!(parts.memory.is_some());
    assert!(parts.session.is_none());
    assert!(parts.workspace.is_none());

    // Undeclared slots stay absent when the memory facet is carried alone.
    let mut workspace = Workspace;
    let parts = CommandContexts::empty()
        .with_memory(&mut memory)
        .with_workspace(&mut workspace)
        .into_parts();
    assert!(parts.memory.is_some());
    assert!(parts.workspace.is_some());
    assert!(parts.presentation.is_none());
    assert!(parts.media.is_none());
}

#[test]
fn envelope_rejects_duplicate_memory_slot_deterministically() {
    let mut a = FakeMemory::new();
    let mut b = FakeMemory::new();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        CommandContexts::empty()
            .with_memory(&mut a)
            .with_memory(&mut b);
    }));
    assert!(result.is_err(), "duplicate memory slot must assert");
}
