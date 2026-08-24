//! `/memory` slash command — inspect and edit the user memory file.
//!
//! When the user-memory feature is opted-in (`[memory] enabled = true` in
//! config or `DEEPSEEK_MEMORY=on` in the environment), `/memory` shows
//! the current memory file path and contents inline. Subcommands let the
//! user clear or open the file:
//!
//! - `/memory` — show path + content
//! - `/memory show` — alias for the no-arg form
//! - `/memory clear` — replace the file contents with an empty marker
//! - `/memory path` — show only the resolved path
//! - `/memory help` — show command-specific help and the resolved path
//!
//! Editor integration (`/memory edit`) is intentionally minimal: the
//! command prints a copy-pasteable shell line to open the file in the
//! user's `$VISUAL` / `$EDITOR`, since the in-process external editor
//! plumbing requires terminal teardown that the slash-command handler
//! doesn't have access to.

use std::fs;
use std::path::Path;

use codewhale_command_contract::facets::{
    CommandMemoryContext, MemoryDeleteScope, MemoryGetOutcome, MemoryImportOutcome,
    MemoryRememberTarget,
};
use codewhale_command_contract::handler::{CommandCapabilities, CommandContexts, CommandHandler};
use codewhale_command_contract::metadata::{CommandInfo, RegisterCommand};

use crate::commands::CommandResult;

const MEMORY_USAGE: &str = "/memory [show|path|clear|edit|native ...|help]";

fn memory_help(path: &Path) -> String {
    format!(
        "Inspect or manage your persistent user-memory file.\n\n\
         Usage: {MEMORY_USAGE}\n\n\
         Current path: {}\n\n\
         Subcommands:\n\
           /memory          Show the resolved path and current contents\n\
           /memory show     Alias for the no-arg form\n\
           /memory path     Print just the resolved path\n\
           /memory clear    Replace the file contents with an empty marker\n\
           /memory edit     Print the editor command for this file\n\
           /memory native   Manage the local-native Markdown + FTS5 store\n\
           /memory help     Show this help\n\n\
         Quick capture: type `# foo` in the composer to append a timestamped\n\
         bullet without firing a turn.",
        path.display()
    )
}

fn native_command(
    workspace: &Path,
    memory: &dyn CommandMemoryContext,
    input: &str,
) -> CommandResult {
    let mut parts = input.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("status");
    let arg = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match command {
        "status" => match memory.status() {
            Ok(status) => CommandResult::message(format!(
                "native memory: {}\nsource: {}\nindex: {}",
                status.root.display(),
                status.source.display(),
                status.index.display()
            )),
            Err(err) => CommandResult::error(format!("native memory status failed: {err}")),
        },
        "path" => match memory.path() {
            Ok(root) => CommandResult::message(root.display().to_string()),
            Err(err) => CommandResult::error(format!("native memory path failed: {err}")),
        },
        "search" => {
            let Some(query) = arg else {
                return CommandResult::error("Usage: /memory native search <query>");
            };
            match memory.search(workspace, query, 10) {
                Ok(hits) if hits.is_empty() => CommandResult::message("No native memory matches."),
                Ok(hits) => CommandResult::message(
                    hits.into_iter()
                        .map(|hit| {
                            format!(
                                "{}:{}-{} {}",
                                hit.source.display(),
                                hit.line_start,
                                hit.line_end,
                                hit.text
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                ),
                Err(err) => CommandResult::error(format!("native memory search failed: {err}")),
            }
        }
        "remember" => {
            let Some(input) = arg else {
                return CommandResult::error(
                    "Usage: /memory native remember [global|workspace] <note>",
                );
            };
            let mut words = input.splitn(3, char::is_whitespace);
            let scope_word = words.next().unwrap_or_default();
            if scope_word == "workspace" {
                let Some(note) = words.next() else {
                    return CommandResult::error("Usage: /memory native remember workspace <note>");
                };
                let remembered = match memory.workspace_id(workspace) {
                    Ok(workspace_id) => {
                        memory.remember(MemoryRememberTarget::Workspace { workspace_id }, note)
                    }
                    Err(err) => {
                        // The adapter preserves the established identity text.
                        return CommandResult::error(err);
                    }
                };
                match remembered {
                    Ok(hit) => CommandResult::message(format!(
                        "native memory remembered at {}:{}",
                        hit.source.display(),
                        hit.line_start
                    )),
                    Err(err) => CommandResult::error(format!("native memory write failed: {err}")),
                }
            } else {
                match memory.remember(MemoryRememberTarget::Global, input) {
                    Ok(hit) => CommandResult::message(format!(
                        "native memory remembered at {}:{}",
                        hit.source.display(),
                        hit.line_start
                    )),
                    Err(err) => CommandResult::error(format!("native memory write failed: {err}")),
                }
            }
        }
        "import" => match memory.import() {
            Ok(MemoryImportOutcome::Imported { destination }) => CommandResult::message(format!(
                "legacy memory imported non-destructively into {}",
                destination.display()
            )),
            Ok(MemoryImportOutcome::Skipped) => {
                CommandResult::message("legacy memory was already imported or is empty")
            }
            Err(err) => CommandResult::error(format!("legacy memory import failed: {err}")),
        },
        "get" => {
            let Some(id) = arg.and_then(|value| value.parse::<i64>().ok()) else {
                return CommandResult::error("Usage: /memory native get <id>");
            };
            match memory.get(workspace, id) {
                Ok(MemoryGetOutcome::Found(hit)) => CommandResult::message(format!(
                    "{}:{}-{}\n{}",
                    hit.source.display(),
                    hit.line_start,
                    hit.line_end,
                    hit.text
                )),
                Ok(MemoryGetOutcome::NotFound) => {
                    CommandResult::error(format!("native memory entry {id} not found"))
                }
                Err(err) => CommandResult::error(format!("native memory get failed: {err}")),
            }
        }
        "export" => match memory.export() {
            Ok(export) if export.content.is_empty() => {
                CommandResult::message("Native memory is empty.")
            }
            Ok(export) => CommandResult::message(export.content),
            Err(err) => CommandResult::error(format!("native memory export failed: {err}")),
        },
        "reindex" => match memory.reindex() {
            Ok(result) => CommandResult::message(format!(
                "native memory reindexed: {} entries",
                result.entry_count
            )),
            Err(err) => CommandResult::error(format!("native memory reindex failed: {err}")),
        },
        "delete" | "clear" => {
            let scope = arg.unwrap_or("all");
            let result = match scope {
                "all" => memory.delete(MemoryDeleteScope::All),
                "global" => memory.delete(MemoryDeleteScope::Global),
                "workspace" => memory.delete_workspace(workspace),
                _ => {
                    return CommandResult::error(
                        "Usage: /memory native delete [all|global|workspace]",
                    );
                }
            };
            match result {
                Ok(_) => CommandResult::message(format!("native memory {scope} deleted")),
                Err(err) => CommandResult::error(format!("native memory delete failed: {err}")),
            }
        }
        _ => CommandResult::error(
            "Usage: /memory native [status|path|remember ...|import|search <query>|get <id>|export|reindex|delete]",
        ),
    }
}

fn memory(workspace: &Path, memory: &dyn CommandMemoryContext, arg: Option<&str>) -> CommandResult {
    if !memory.memory_enabled() {
        return CommandResult::error(
            "user memory is disabled. Enable with `[memory] enabled = true` in `~/.codewhale/config.toml` or `DEEPSEEK_MEMORY=on` in your environment, then restart the TUI.",
        );
    }

    let path = memory.memory_path();
    let sub = arg.unwrap_or("show").trim();

    if let Some(native_arg) = sub.strip_prefix("native").map(str::trim) {
        return native_command(workspace, memory, native_arg);
    }

    match sub {
        "" | "show" => {
            let body = match fs::read_to_string(&path) {
                Ok(text) if text.trim().is_empty() => format!(
                    "{}\n(empty — add via `# foo` from the composer or have the model use the `remember` tool)",
                    path.display()
                ),
                Ok(text) => format!("{}\n\n{}", path.display(), text.trim_end()),
                Err(_) => format!(
                    "{}\n(file does not exist yet — add via `# foo` from the composer to create it)",
                    path.display()
                ),
            };
            CommandResult::message(body)
        }
        "path" => CommandResult::message(path.display().to_string()),
        "clear" => match fs::write(&path, "") {
            Ok(()) => CommandResult::message(format!("memory cleared: {}", path.display())),
            Err(err) => CommandResult::error(format!("failed to clear {}: {err}", path.display())),
        },
        "edit" => CommandResult::message(format!(
            "to edit your memory file, run:\n\n  ${{VISUAL:-${{EDITOR:-vi}}}} {}",
            path.display()
        )),
        "help" => CommandResult::message(memory_help(&path)),
        _ => CommandResult::error(format!(
            "unknown subcommand `{sub}`. Try `/memory help`.\n\n{}",
            memory_help(&path)
        )),
    }
}

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "memory",
    aliases: &[],
    usage: "/memory [show|path|clear|edit|help]",
    description_key: "cmd_memory_description",
};

pub(in crate::commands) struct MemoryCmd;

impl RegisterCommand<CommandResult> for MemoryCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn handler() -> CommandHandler<CommandResult> {
        CommandHandler::Contextual {
            capabilities: CommandCapabilities::WORKSPACE.union(CommandCapabilities::MEMORY),
            handler: memory_contextual,
        }
    }
}

fn memory_contextual(contexts: CommandContexts<'_>, arg: Option<&str>) -> CommandResult {
    let parts = contexts.into_parts();
    let Some(workspace) = parts.workspace.as_deref() else {
        return CommandResult::error("Command capability unavailable: workspace");
    };
    let Some(memory_ctx) = parts.memory.as_deref() else {
        return CommandResult::error("Command capability unavailable: memory");
    };
    memory(&workspace.workspace(), memory_ctx, arg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    use codewhale_command_contract::facets::{
        CommandWorkspaceContext, MemoryDelete, MemoryExport, MemoryHit, MemoryReindex,
        MemoryRemembered, MemoryStatus,
    };

    struct FakeWorkspace {
        path: PathBuf,
    }

    impl CommandWorkspaceContext for FakeWorkspace {
        fn workspace(&self) -> PathBuf {
            self.path.clone()
        }

        fn work_state_snapshot(&self) -> Result<Option<String>, String> {
            Ok(None)
        }

        fn operation_digest(&mut self) -> Result<String, String> {
            Ok("No active operations or to-do items.".to_string())
        }
    }

    /// Programmable fake memory facet driving every handler branch.
    struct FakeMemory {
        enabled: bool,
        path: PathBuf,
        status: Result<MemoryStatus, String>,
        root: Result<PathBuf, String>,
        workspace_id: Result<String, String>,
        search: Result<Vec<MemoryHit>, String>,
        remember: Result<MemoryRemembered, String>,
        import: Result<MemoryImportOutcome, String>,
        get: Result<MemoryGetOutcome, String>,
        export: Result<MemoryExport, String>,
        reindex: Result<MemoryReindex, String>,
        delete: Result<(), String>,
        delete_workspace: Result<(), String>,
    }

    impl Default for FakeMemory {
        fn default() -> Self {
            Self {
                enabled: true,
                path: PathBuf::from("/mem/user-memory.md"),
                status: Ok(MemoryStatus {
                    root: PathBuf::from("/mem/root"),
                    source: PathBuf::from("/mem/root/global/MEMORY.md"),
                    index: PathBuf::from("/mem/root/index.sqlite3"),
                }),
                root: Ok(PathBuf::from("/mem/root")),
                workspace_id: Ok("owner/repo".to_string()),
                search: Ok(vec![MemoryHit {
                    source: PathBuf::from("/mem/root/global/MEMORY.md"),
                    line_start: 2,
                    line_end: 2,
                    text: "alpha hit".to_string(),
                }]),
                remember: Ok(MemoryRemembered {
                    source: PathBuf::from("/mem/root/global/MEMORY.md"),
                    line_start: 3,
                }),
                import: Ok(MemoryImportOutcome::Skipped),
                get: Ok(MemoryGetOutcome::Found(MemoryHit {
                    source: PathBuf::from("/mem/root/global/MEMORY.md"),
                    line_start: 2,
                    line_end: 2,
                    text: "found entry".to_string(),
                })),
                export: Ok(MemoryExport {
                    content: "# memory\n\n- bullet".to_string(),
                }),
                reindex: Ok(MemoryReindex { entry_count: 4 }),
                delete: Ok(()),
                delete_workspace: Ok(()),
            }
        }
    }

    impl CommandMemoryContext for FakeMemory {
        fn memory_path(&self) -> PathBuf {
            self.path.clone()
        }

        fn memory_enabled(&self) -> bool {
            self.enabled
        }

        fn status(&self) -> Result<MemoryStatus, String> {
            self.status.clone()
        }

        fn path(&self) -> Result<PathBuf, String> {
            self.root.clone()
        }

        fn workspace_id(&self, _workspace: &Path) -> Result<String, String> {
            self.workspace_id.clone()
        }

        fn search(
            &self,
            _workspace: &Path,
            _query: &str,
            _limit: usize,
        ) -> Result<Vec<MemoryHit>, String> {
            self.search.clone()
        }

        fn remember(
            &self,
            _target: MemoryRememberTarget,
            _note: &str,
        ) -> Result<MemoryRemembered, String> {
            self.remember.clone()
        }

        fn import(&self) -> Result<MemoryImportOutcome, String> {
            self.import.clone()
        }

        fn get(&self, _workspace: &Path, _id: i64) -> Result<MemoryGetOutcome, String> {
            self.get.clone()
        }

        fn export(&self) -> Result<MemoryExport, String> {
            self.export.clone()
        }

        fn reindex(&self) -> Result<MemoryReindex, String> {
            self.reindex.clone()
        }

        fn delete(&self, _scope: MemoryDeleteScope) -> Result<MemoryDelete, String> {
            self.delete.clone().map(|()| MemoryDelete)
        }

        fn delete_workspace(&self, _workspace: &Path) -> Result<MemoryDelete, String> {
            self.delete_workspace.clone().map(|()| MemoryDelete)
        }
    }

    fn fake_workspace(tmpdir: &TempDir) -> FakeWorkspace {
        FakeWorkspace {
            path: tmpdir.path().to_path_buf(),
        }
    }

    fn message(result: CommandResult) -> String {
        result.message.expect("command message")
    }

    fn error(result: CommandResult) -> String {
        result
            .message
            .expect("command error")
            .strip_prefix("Error: ")
            .unwrap_or_default()
            .to_string()
    }

    // --- Existing 3 tests ported to fake facets (D6) ---

    #[test]
    fn memory_help_lists_subcommands_and_resolved_path() {
        let tmpdir = TempDir::new().expect("tempdir");
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let result = memory(&workspace.path, &fake, Some("help"));
        let msg = message(result);
        assert!(msg.contains("Usage: /memory [show|path|clear|edit|native ...|help]"));
        assert!(msg.contains("/memory edit"));
        assert!(msg.contains("/mem/user-memory.md"));
    }

    #[test]
    fn memory_unknown_subcommand_points_to_help() {
        let tmpdir = TempDir::new().expect("tempdir");
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let result = memory(&workspace.path, &fake, Some("wat"));
        let msg = message(result);
        assert!(msg.contains("Try `/memory help`"));
        assert!(msg.contains("/memory clear"));
    }

    #[test]
    fn memory_disabled_returns_enablement_hint() {
        let tmpdir = TempDir::new().expect("tempdir");
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory {
            enabled: false,
            ..FakeMemory::default()
        };
        let result = memory(&workspace.path, &fake, None);
        let msg = message(result);
        assert!(msg.contains("user memory is disabled"));
        assert!(msg.contains("DEEPSEEK_MEMORY=on"));
    }

    // --- Native operation matrix (D6/D9) ---

    #[test]
    fn native_status_renders_root_source_and_index() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let msg = message(memory(&workspace.path, &fake, Some("native status")));
        assert!(msg.contains("native memory: /mem/root"));
        assert!(msg.contains("source: /mem/root/global/MEMORY.md"));
        assert!(msg.contains("index: /mem/root/index.sqlite3"));
    }

    #[test]
    fn native_path_renders_root() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let msg = message(memory(&workspace.path, &fake, Some("native path")));
        assert_eq!(msg, "/mem/root");
    }

    #[test]
    fn native_search_renders_hits_empty_and_errors() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let msg = message(memory(&workspace.path, &fake, Some("native search alpha")));
        assert_eq!(msg, "/mem/root/global/MEMORY.md:2-2 alpha hit");

        let empty = FakeMemory {
            search: Ok(Vec::new()),
            ..FakeMemory::default()
        };
        let msg = message(memory(&workspace.path, &empty, Some("native search zzz")));
        assert_eq!(msg, "No native memory matches.");

        let failing = FakeMemory {
            search: Err("index corrupt".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &failing, Some("native search zzz")));
        assert!(err.contains("native memory search failed: index corrupt"));

        let usage = memory(&workspace.path, &fake, Some("native search"));
        assert!(error(usage).contains("Usage: /memory native search <query>"));
    }

    #[test]
    fn native_remember_global_and_workspace() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();

        let global = message(memory(
            &workspace.path,
            &fake,
            Some("native remember global hello"),
        ));
        assert_eq!(
            global,
            "native memory remembered at /mem/root/global/MEMORY.md:3"
        );

        let workspace_note = message(memory(
            &workspace.path,
            &fake,
            Some("native remember workspace hello"),
        ));
        assert_eq!(
            workspace_note,
            "native memory remembered at /mem/root/global/MEMORY.md:3"
        );

        let missing_origin = FakeMemory {
            workspace_id: Err(
                "workspace memory requires a git repository with an origin".to_string()
            ),
            ..FakeMemory::default()
        };
        let err = error(memory(
            &workspace.path,
            &missing_origin,
            Some("native remember workspace hello"),
        ));
        assert_eq!(
            err,
            "workspace memory requires a git repository with an origin"
        );

        let failing = FakeMemory {
            remember: Err("disk full".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(
            &workspace.path,
            &failing,
            Some("native remember global hello"),
        ));
        assert_eq!(err, "native memory write failed: disk full");

        let usage = memory(&workspace.path, &fake, Some("native remember"));
        assert!(error(usage).contains("Usage: /memory native remember"));
    }

    #[test]
    fn native_import_imported_skipped_and_errors() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);

        let imported = FakeMemory {
            import: Ok(MemoryImportOutcome::Imported {
                destination: PathBuf::from("/mem/root/global/MEMORY.md"),
            }),
            ..FakeMemory::default()
        };
        let msg = message(memory(&workspace.path, &imported, Some("native import")));
        assert_eq!(
            msg,
            "legacy memory imported non-destructively into /mem/root/global/MEMORY.md"
        );

        let msg = message(memory(
            &workspace.path,
            &FakeMemory::default(),
            Some("native import"),
        ));
        assert_eq!(msg, "legacy memory was already imported or is empty");

        let failing = FakeMemory {
            import: Err("read failed".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &failing, Some("native import")));
        assert_eq!(err, "legacy memory import failed: read failed");
    }

    #[test]
    fn native_get_found_not_found_and_errors() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();

        let msg = message(memory(&workspace.path, &fake, Some("native get 5")));
        assert_eq!(msg, "/mem/root/global/MEMORY.md:2-2\nfound entry");

        let not_found = FakeMemory {
            get: Ok(MemoryGetOutcome::NotFound),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &not_found, Some("native get 5")));
        assert_eq!(err, "native memory entry 5 not found");

        let failing = FakeMemory {
            get: Err("db error".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &failing, Some("native get 5")));
        assert_eq!(err, "native memory get failed: db error");

        let usage = memory(&workspace.path, &fake, Some("native get abc"));
        assert!(error(usage).contains("Usage: /memory native get <id>"));
    }

    #[test]
    fn native_export_empty_content_and_errors() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();

        let msg = message(memory(&workspace.path, &fake, Some("native export")));
        assert_eq!(msg, "# memory\n\n- bullet");

        let empty = FakeMemory {
            export: Ok(MemoryExport {
                content: String::new(),
            }),
            ..FakeMemory::default()
        };
        let msg = message(memory(&workspace.path, &empty, Some("native export")));
        assert_eq!(msg, "Native memory is empty.");

        let failing = FakeMemory {
            export: Err("locked".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &failing, Some("native export")));
        assert_eq!(err, "native memory export failed: locked");
    }

    #[test]
    fn native_reindex_count_and_errors() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let msg = message(memory(&workspace.path, &fake, Some("native reindex")));
        assert_eq!(msg, "native memory reindexed: 4 entries");

        let failing = FakeMemory {
            reindex: Err("lock".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &failing, Some("native reindex")));
        assert_eq!(err, "native memory reindex failed: lock");
    }

    #[test]
    fn native_delete_scopes_and_errors() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();

        for scope in ["all", "global", "workspace"] {
            let msg = message(memory(
                &workspace.path,
                &fake,
                Some(&format!("native delete {scope}")),
            ));
            assert_eq!(msg, format!("native memory {scope} deleted"));
        }

        // Missing origin preserves the established identity text under the
        // delete prefix, matching the pre-migration behavior.
        let missing_origin = FakeMemory {
            workspace_id: Err(
                "workspace memory requires a git repository with an origin".to_string()
            ),
            delete_workspace: Err(
                "workspace memory requires a git repository with an origin".to_string()
            ),
            ..FakeMemory::default()
        };
        let err = error(memory(
            &workspace.path,
            &missing_origin,
            Some("native delete workspace"),
        ));
        assert_eq!(
            err,
            "native memory delete failed: workspace memory requires a git repository with an origin"
        );

        let failing = FakeMemory {
            delete: Err("busy".to_string()),
            ..FakeMemory::default()
        };
        let err = error(memory(&workspace.path, &failing, Some("native delete all")));
        assert_eq!(err, "native memory delete failed: busy");

        let usage = memory(&workspace.path, &fake, Some("native delete bogus"));
        assert!(error(usage).contains("Usage: /memory native delete [all|global|workspace]"));
    }

    #[test]
    fn native_unknown_subcommand_returns_usage() {
        let tmpdir = TempDir::new().unwrap();
        let workspace = fake_workspace(&tmpdir);
        let fake = FakeMemory::default();
        let err = error(memory(&workspace.path, &fake, Some("native bogus")));
        assert!(err.contains("Usage: /memory native"));
    }

    #[test]
    fn memory_registration_declares_exactly_workspace_and_memory() {
        let CommandHandler::Contextual {
            capabilities,
            handler,
        } = MemoryCmd::handler()
        else {
            panic!("memory must be contextual");
        };
        assert_eq!(
            capabilities,
            CommandCapabilities::WORKSPACE.union(CommandCapabilities::MEMORY)
        );
        assert!(!capabilities.contains(CommandCapabilities::PRESENTATION));
        assert!(!capabilities.contains(CommandCapabilities::MEDIA));

        // Missing facets fail safely instead of panicking.
        let missing = handler(CommandContexts::empty(), Some("help"));
        assert!(missing.is_error);
        assert_eq!(
            missing.message.as_deref(),
            Some("Error: Command capability unavailable: workspace")
        );
        assert_eq!(MemoryCmd::info().description_key, "cmd_memory_description");
        assert_eq!(MemoryCmd::info().name, "memory");
        assert_eq!(MemoryCmd::info().aliases, &[] as &[&str]);
    }

    #[test]
    fn memory_missing_memory_facet_fails_safely() {
        // An envelope carrying WORKSPACE but no MEMORY must fail safely with
        // the memory-capability error (never panic).
        let mut workspace = FakeWorkspace {
            path: PathBuf::from("/ws"),
        };
        let contexts = CommandContexts::empty().with_workspace(&mut workspace);
        let result = memory_contextual(contexts, Some("help"));
        assert!(result.is_error);
        assert_eq!(
            result.message.as_deref(),
            Some("Error: Command capability unavailable: memory")
        );
    }
}
