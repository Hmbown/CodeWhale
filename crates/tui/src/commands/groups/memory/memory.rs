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

use crate::commands::CommandResult;
use crate::tui::app::App;

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

fn native_store(app: &App) -> crate::native_memory::NativeMemoryStore {
    let root = app
        .memory_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("memory");
    crate::native_memory::NativeMemoryStore::new(root)
}

fn native_command(app: &App, input: &str) -> CommandResult {
    let store = native_store(app);
    let mut parts = input.splitn(2, char::is_whitespace);
    let command = parts.next().unwrap_or("status");
    let arg = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match command {
        "status" => CommandResult::message(format!(
            "native memory: {}\nsource: {}\nindex: {}",
            store.root().display(),
            store.global_path().display(),
            store.index_path().display()
        )),
        "path" => CommandResult::message(store.root().display().to_string()),
        "search" => {
            let Some(query) = arg else {
                return CommandResult::error("Usage: /memory native search <query>");
            };
            match store.search_for_workspace(&app.workspace, query, 10) {
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
                let workspace_id =
                    match crate::native_memory::NativeMemoryStore::workspace_id(&app.workspace) {
                        Ok(Some(id)) => id,
                        Ok(None) => {
                            return CommandResult::error(
                                "workspace memory requires a git repository with an origin",
                            );
                        }
                        Err(err) => {
                            return CommandResult::error(format!(
                                "failed to resolve workspace identity: {err}"
                            ));
                        }
                    };
                match store.remember(
                    crate::native_memory::MemoryScope::Workspace,
                    Some(&workspace_id),
                    note,
                ) {
                    Ok(hit) => CommandResult::message(format!(
                        "native memory remembered at {}:{}",
                        hit.source.display(),
                        hit.line_start
                    )),
                    Err(err) => CommandResult::error(format!("native memory write failed: {err}")),
                }
            } else {
                match store.remember(crate::native_memory::MemoryScope::Global, None, input) {
                    Ok(hit) => CommandResult::message(format!(
                        "native memory remembered at {}:{}",
                        hit.source.display(),
                        hit.line_start
                    )),
                    Err(err) => CommandResult::error(format!("native memory write failed: {err}")),
                }
            }
        }
        "import" => match store.import_legacy(&app.memory_path) {
            Ok(true) => CommandResult::message(format!(
                "legacy memory imported non-destructively into {}",
                store.global_path().display()
            )),
            Ok(false) => CommandResult::message("legacy memory was already imported or is empty"),
            Err(err) => CommandResult::error(format!("legacy memory import failed: {err}")),
        },
        "reindex" => match store.reindex() {
            Ok(count) => {
                CommandResult::message(format!("native memory reindexed: {count} entries"))
            }
            Err(err) => CommandResult::error(format!("native memory reindex failed: {err}")),
        },
        "delete" | "clear" => match store.delete_all(None, None) {
            Ok(()) => CommandResult::message("native memory deleted"),
            Err(err) => CommandResult::error(format!("native memory delete failed: {err}")),
        },
        _ => CommandResult::error(
            "Usage: /memory native [status|path|remember ...|import|search <query>|reindex|delete]",
        ),
    }
}

fn memory(app: &mut App, arg: Option<&str>) -> CommandResult {
    if !app.use_memory {
        return CommandResult::error(
            "user memory is disabled. Enable with `[memory] enabled = true` in `~/.codewhale/config.toml` or `DEEPSEEK_MEMORY=on` in your environment, then restart the TUI.",
        );
    }

    let path = app.memory_path.clone();
    let sub = arg.unwrap_or("show").trim();

    if let Some(native_arg) = sub.strip_prefix("native").map(str::trim) {
        return native_command(app, native_arg);
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

pub(in crate::commands) const COMMAND_INFO: crate::commands::traits::CommandInfo =
    crate::commands::traits::CommandInfo {
        name: "memory",
        aliases: &[],
        usage: "/memory [show|path|clear|edit|help]",
        description_id: crate::localization::MessageId::CmdMemoryDescription,
    };

pub(in crate::commands) struct MemoryCmd;

impl crate::commands::traits::RegisterCommand for MemoryCmd {
    fn info() -> &'static crate::commands::traits::CommandInfo {
        &COMMAND_INFO
    }

    fn execute(
        app: &mut crate::tui::app::App,
        arg: Option<&str>,
    ) -> crate::commands::CommandResult {
        memory(app, arg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app_with_memory(tmpdir: &TempDir, use_memory: bool) -> App {
        let options = TuiOptions {
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: use_memory,
            ..crate::test_support::test_tui_options(tmpdir.path().to_path_buf())
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn memory_help_lists_subcommands_and_resolved_path() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = create_test_app_with_memory(&tmpdir, true);
        let result = memory(&mut app, Some("help"));
        let msg = result.message.expect("help should return text");
        assert!(msg.contains("Usage: /memory [show|path|clear|edit|help]"));
        assert!(msg.contains("/memory edit"));
        assert!(msg.contains(app.memory_path.to_string_lossy().as_ref()));
    }

    #[test]
    fn memory_unknown_subcommand_points_to_help() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = create_test_app_with_memory(&tmpdir, true);
        let result = memory(&mut app, Some("wat"));
        let msg = result
            .message
            .expect("unknown subcommand should return text");
        assert!(msg.contains("Try `/memory help`"));
        assert!(msg.contains("/memory clear"));
    }

    #[test]
    fn memory_disabled_returns_enablement_hint() {
        let tmpdir = TempDir::new().expect("tempdir");
        let mut app = create_test_app_with_memory(&tmpdir, false);
        let result = memory(&mut app, None);
        let msg = result.message.expect("disabled memory should return text");
        assert!(msg.contains("user memory is disabled"));
        assert!(msg.contains("DEEPSEEK_MEMORY=on"));
    }
}
