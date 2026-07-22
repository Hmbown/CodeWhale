//! Dedicated registry for user-defined markdown slash commands.
//!
//! This module owns the user-command boundary. Built-in command metadata and
//! dispatch remain in the normal command registry; user commands are loaded
//! from markdown files into this registry and are attempted before built-ins.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, SystemTime};

use crate::tui::app::{App, AppAction, HuntVerdict};

use super::CommandResult;
use super::user_commands;

static USER_COMMAND_REGISTRY: OnceLock<RwLock<UserCommandRegistryState>> = OnceLock::new();

#[derive(Debug, Clone, Default)]
struct UserCommandRegistryState {
    initialized: bool,
    workspace: Option<PathBuf>,
    command_dirs_snapshot: Vec<CommandDirSnapshot>,
    registry: UserCommandRegistry,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandDirSnapshot {
    path: PathBuf,
    modified: Option<SystemTime>,
    files: Vec<CommandFileSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandFileSnapshot {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserCommandMetadata {
    pub name: String,
    pub body: String,
    pub description: Option<String>,
    pub usage: Option<String>,
    pub arguments: Option<String>,
    pub argument_hint: Option<String>,
    pub allowed_tools: Option<Vec<String>>,
    pub pausable: bool,
    pub aliases: Vec<String>,
    pub hidden: bool,
}

impl UserCommandMetadata {
    /// User-facing invocation syntax. `argument-hint` remains the legacy
    /// fallback for existing command files; `arguments` is the final fallback
    /// when no complete `usage` string is supplied.
    pub(crate) fn display_usage(&self) -> Option<&str> {
        [&self.usage, &self.argument_hint, &self.arguments]
            .into_iter()
            .filter_map(Option::as_deref)
            .find(|value| !value.trim().is_empty())
            .map(str::trim)
    }

    /// Whether selecting this command should leave the composer open for
    /// arguments. These fields describe presentation only; dispatch keeps the
    /// existing permissive `$ARGUMENTS`/`$1` template semantics.
    pub(crate) fn takes_arguments(&self) -> bool {
        self.arguments
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            // Preserve the legacy contract exactly: the presence of
            // `argument-hint`, including an explicitly empty value, made the
            // palette insert rather than immediately execute the command.
            || self.argument_hint.is_some()
            || self
                .usage
                .as_deref()
                .is_some_and(|usage| usage_describes_arguments(&self.name, usage))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Debug, Clone, Default)]
pub struct UserCommandRegistry {
    commands: HashMap<String, UserCommandMetadata>,
    aliases: HashMap<String, String>,
    load_errors: Vec<LoadError>,
    invalid_commands: HashMap<String, String>,
}

impl UserCommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load(workspace: Option<&Path>) -> Self {
        // The user_commands module is the permanent lower-level file scanning
        // and parsing boundary; this registry owns metadata, shadowing, and
        // dispatch. See docs/architecture/command-dispatch.md.
        Self::load_from_paths(&user_commands::commands_dirs(workspace))
    }

    pub(crate) fn load_from_paths(paths: &[PathBuf]) -> Self {
        let mut loaded = Vec::new();
        let mut seen = HashSet::new();
        let mut registry = Self::new();

        for dir in paths {
            let mut directory_commands = user_commands::load_commands_from_dir(dir);
            directory_commands.sort_by(|a, b| a.0.cmp(&b.0));
            for (name, content) in directory_commands {
                let canonical = normalize_name(&name);
                if seen.insert(canonical.clone()) {
                    loaded.push((name, content, dir.join(format!("{canonical}.md"))));
                } else {
                    registry.record_load_error(
                        dir.join(format!("{canonical}.md")),
                        format!(
                            "User command '/{canonical}' is defined more than once; using the first definition"
                        ),
                    );
                }
            }
        }
        registry.load_from_entries(loaded);
        registry
    }

    #[cfg(test)]
    pub fn from_loaded(commands: Vec<(String, String)>) -> Self {
        let mut registry = Self::new();
        let loaded = commands
            .into_iter()
            .map(|(name, content)| {
                let path = PathBuf::from(format!("{}.md", normalize_name(&name)));
                (name, content, path)
            })
            .collect();
        registry.load_from_entries(loaded);
        registry
    }

    fn load_from_entries(&mut self, commands: Vec<(String, String, PathBuf)>) {
        let parsed_commands = commands
            .into_iter()
            .map(|(name, content, path)| {
                let (metadata, errors) = parse_metadata(name, &content, &path);
                (metadata, errors, path)
            })
            .collect::<Vec<_>>();
        let canonical_names = parsed_commands
            .iter()
            .map(|(metadata, _, _)| metadata.name.clone())
            .collect::<HashSet<_>>();

        for (metadata, errors, path) in parsed_commands {
            for error in &errors {
                self.record_load_error(error.path.clone(), error.message.clone());
            }

            if self.commands.contains_key(&metadata.name) {
                self.record_load_error(
                    path.clone(),
                    format!(
                        "User command '/{}' is defined more than once; using the first definition",
                        metadata.name
                    ),
                );
                continue;
            }

            // A malformed losing duplicate must not poison the valid command
            // that already won precedence. Only the selected definition owns
            // the dispatch-time error for its canonical name and aliases.
            for error in errors {
                self.invalid_commands
                    .entry(metadata.name.clone())
                    .or_insert(error.message);
            }

            for alias in &metadata.aliases {
                let alias = alias.to_ascii_lowercase();
                if canonical_names.contains(&alias) {
                    self.record_load_error(
                        path.clone(),
                        format!(
                            "User command alias '/{alias}' for '/{}' duplicates canonical user command '/{alias}'; ignoring this alias",
                            metadata.name
                        ),
                    );
                    continue;
                }
                if let Some(existing) = self.aliases.get(&alias) {
                    self.record_load_error(
                        path.clone(),
                        format!(
                            "User command alias '/{alias}' for '/{}' duplicates user command '/{existing}'; using the first alias definition",
                            metadata.name
                        ),
                    );
                    continue;
                }
                self.aliases.insert(alias, metadata.name.clone());
            }

            self.commands.insert(metadata.name.clone(), metadata);
        }
    }

    fn record_load_error(&mut self, path: PathBuf, message: String) {
        self.load_errors.push(LoadError { path, message });
    }

    pub fn get(&self, name: &str) -> Option<&UserCommandMetadata> {
        let key = normalize_name(name);
        self.commands.get(&key).or_else(|| {
            self.aliases
                .get(&key)
                .and_then(|canonical| self.commands.get(canonical))
        })
    }

    #[cfg(test)]
    pub fn get_by_alias(&self, alias: &str) -> Option<&UserCommandMetadata> {
        let key = normalize_name(alias);
        self.aliases
            .get(&key)
            .and_then(|canonical| self.commands.get(canonical))
    }

    #[cfg(test)]
    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.commands.keys().cloned().collect();
        names.sort();
        names
    }

    pub fn iter(&self) -> impl Iterator<Item = &UserCommandMetadata> {
        self.commands.values()
    }

    #[cfg(test)]
    pub fn is_valid(&self) -> bool {
        self.load_errors.is_empty()
    }

    #[cfg(test)]
    pub fn load_errors(&self) -> &[LoadError] {
        &self.load_errors
    }

    fn dispatch_error(&self, name: &str) -> Option<String> {
        let key = normalize_name(name);
        self.invalid_commands.get(&key).cloned().or_else(|| {
            self.aliases
                .get(&key)
                .and_then(|canonical| self.invalid_commands.get(canonical))
                .cloned()
        })
    }
}

fn parse_metadata(
    name: String,
    content: &str,
    path: &Path,
) -> (UserCommandMetadata, Vec<LoadError>) {
    let filename_name = normalize_name(&name);
    let (metadata, body) = user_commands::parse_frontmatter(content);
    let mut command = UserCommandMetadata {
        name: filename_name.clone(),
        body: body.to_string(),
        description: None,
        usage: None,
        arguments: None,
        argument_hint: None,
        allowed_tools: None,
        pausable: false,
        aliases: Vec::new(),
        hidden: false,
    };
    let mut configured_name = None;

    for (key, value) in metadata {
        match key.as_str() {
            "name" => configured_name = Some(value),
            "description" => command.description = Some(value),
            "usage" => command.usage = Some(value),
            "arguments" => command.arguments = Some(value),
            "argument-hint" => command.argument_hint = Some(value),
            "allowed-tools" => {
                command.allowed_tools = Some(user_commands::parse_allowed_tools(&value));
            }
            "pausable" => command.pausable = value.trim().eq_ignore_ascii_case("true"),
            "aliases" | "alias" => {
                command.aliases = value
                    .split(',')
                    .map(normalize_name)
                    .filter(|alias| !alias.is_empty())
                    .collect();
            }
            "hidden" => command.hidden = value.trim().eq_ignore_ascii_case("true"),
            _ => {}
        }
    }

    let mut errors = Vec::new();
    if let Some(configured_name) = configured_name {
        if let Some(normalized) = normalize_configured_name(&configured_name) {
            command.name = normalized;
        } else {
            errors.push(LoadError {
                path: path.to_path_buf(),
                message: format!(
                    "User command '/{filename_name}' has invalid frontmatter name {configured_name:?}; expected one slash-command token"
                ),
            });
        }
    }
    errors.extend(validate_command_content(&command.name, content, path));

    (command, errors)
}

fn validate_command_content(canonical: &str, content: &str, path: &Path) -> Vec<LoadError> {
    let mut errors = Vec::new();
    if canonical.is_empty() {
        errors.push(LoadError {
            path: path.to_path_buf(),
            message: "User command has an empty command name".to_string(),
        });
    }
    if content.trim().is_empty() {
        errors.push(LoadError {
            path: path.to_path_buf(),
            message: format!("User command '/{canonical}' is empty"),
        });
    }

    let Some(first_line_end) = content.find('\n') else {
        return errors;
    };
    let first = content[..first_line_end].trim_end_matches('\r');
    if !is_frontmatter_delimiter(first.trim()) {
        return errors;
    }

    let mut saw_closing = false;
    for raw_line in content[first_line_end + 1..].split_inclusive('\n') {
        let line = raw_line.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        if is_frontmatter_delimiter(trimmed) {
            saw_closing = true;
            break;
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some((key, _)) = line.split_once(':')
            && !key.trim().is_empty()
        {
            continue;
        }
        errors.push(LoadError {
            path: path.to_path_buf(),
            message: format!(
                "User command '/{canonical}' has invalid frontmatter line {trimmed:?}; expected key: value"
            ),
        });
        break;
    }

    if !saw_closing {
        errors.push(LoadError {
            path: path.to_path_buf(),
            message: format!(
                "User command '/{canonical}' has invalid frontmatter; missing closing --- delimiter"
            ),
        });
    }

    errors
}

fn is_frontmatter_delimiter(value: &str) -> bool {
    value.chars().all(|ch| ch == '-') && value.len() >= 3
}

fn normalize_name(name: &str) -> String {
    name.trim().trim_start_matches('/').to_ascii_lowercase()
}

fn normalize_configured_name(name: &str) -> Option<String> {
    let name = name.trim();
    let name = name.strip_prefix('/').unwrap_or(name);
    (!name.is_empty() && !name.contains('/') && !name.contains(char::is_whitespace))
        .then(|| name.to_ascii_lowercase())
}

fn usage_describes_arguments(name: &str, usage: &str) -> bool {
    let usage = usage.trim();
    if usage.is_empty() {
        return false;
    }
    let bare_usage = usage.trim_start_matches('/');
    !bare_usage.eq_ignore_ascii_case(name)
}

fn normalize_workspace(workspace: Option<&Path>) -> Option<PathBuf> {
    workspace.map(Path::to_path_buf)
}

fn command_dirs_snapshot(workspace: Option<&Path>) -> Vec<CommandDirSnapshot> {
    user_commands::commands_dirs(workspace)
        .into_iter()
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .ok();
            let mut files = Vec::new();
            if let Ok(entries) = std::fs::read_dir(&path) {
                for entry in entries.flatten() {
                    let file_path = entry.path();
                    if file_path.extension().and_then(|ext| ext.to_str()) != Some("md") {
                        continue;
                    }
                    let Ok(metadata) = entry.metadata() else {
                        continue;
                    };
                    files.push(CommandFileSnapshot {
                        path: file_path,
                        modified: metadata.modified().ok(),
                        len: metadata.len(),
                    });
                }
            }
            files.sort_by(|a, b| a.path.cmp(&b.path));
            CommandDirSnapshot {
                path,
                modified,
                files,
            }
        })
        .collect()
}

fn registry_lock() -> &'static RwLock<UserCommandRegistryState> {
    USER_COMMAND_REGISTRY.get_or_init(|| RwLock::new(UserCommandRegistryState::default()))
}

fn registry_needs_reload(
    guard: &UserCommandRegistryState,
    workspace: &Option<PathBuf>,
    snapshot: &[CommandDirSnapshot],
) -> bool {
    !guard.initialized || guard.workspace != *workspace || guard.command_dirs_snapshot != snapshot
}

#[cfg(test)]
pub fn reload(workspace: Option<&Path>) {
    let workspace = normalize_workspace(workspace);
    let snapshot = command_dirs_snapshot(workspace.as_deref());
    reload_with_snapshot(workspace, snapshot);
}

#[cfg(test)]
fn reload_with_snapshot(workspace: Option<PathBuf>, snapshot: Vec<CommandDirSnapshot>) {
    let replacement = UserCommandRegistry::load(workspace.as_deref());
    let mut guard = registry_lock()
        .write()
        .expect("user command registry lock poisoned");
    guard.initialized = true;
    guard.workspace = workspace;
    guard.command_dirs_snapshot = snapshot;
    guard.registry = replacement;
}

#[cfg(test)]
pub fn current_registry() -> UserCommandRegistry {
    registry_lock()
        .read()
        .expect("user command registry lock poisoned")
        .registry
        .clone()
}

#[cfg(test)]
pub fn registry_for_workspace(workspace: Option<&Path>) -> UserCommandRegistry {
    with_registry_for_workspace(workspace, Clone::clone)
}

pub fn with_registry_for_workspace<R>(
    workspace: Option<&Path>,
    f: impl FnOnce(&UserCommandRegistry) -> R,
) -> R {
    let workspace = normalize_workspace(workspace);
    let snapshot = command_dirs_snapshot(workspace.as_deref());
    let lock = registry_lock();
    {
        let guard = lock.read().expect("user command registry lock poisoned");
        if !registry_needs_reload(&guard, &workspace, &snapshot) {
            return f(&guard.registry);
        }
    }

    let replacement = UserCommandRegistry::load(workspace.as_deref());
    let mut guard = lock.write().expect("user command registry lock poisoned");
    if registry_needs_reload(&guard, &workspace, &snapshot) {
        guard.initialized = true;
        guard.workspace = workspace;
        guard.command_dirs_snapshot = snapshot;
        guard.registry = replacement;
    }
    f(&guard.registry)
}

pub fn try_dispatch(app: &mut App, input: &str) -> Option<CommandResult> {
    let parts: Vec<&str> = input.trim().splitn(2, ' ').collect();
    let command = normalize_name(parts.first().copied().unwrap_or_default());
    let args = parts.get(1).copied().unwrap_or("").trim();

    let (dispatch_error, metadata) =
        with_registry_for_workspace(Some(&app.workspace), |registry| {
            (
                registry.dispatch_error(&command),
                registry.get(&command).cloned(),
            )
        });
    if let Some(error) = dispatch_error {
        return Some(CommandResult::error(error));
    }

    let metadata = metadata?;

    app.hunt.quarry = None;
    app.hunt.started_at = None;
    app.hunt.verdict = HuntVerdict::Hunting;
    app.hunt.token_budget = None;
    app.hunt.tokens_used = 0;
    app.hunt.time_used_seconds = 0;
    app.hunt.continuation_count = 0;
    app.active_allowed_tools = None;
    app.pausable = false;
    app.paused = false;
    app.paused_quarry = None;
    let mut todos_cleared = false;
    for _ in 0..10 {
        if let Ok(mut todos) = app.todos.try_lock() {
            todos.clear();
            todos_cleared = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if !todos_cleared {
        tracing::warn!(target: "commands", "todos lock contended or poisoned — previous todos not cleared");
    }

    let mut plan_cleared = false;
    for _ in 0..10 {
        if let Ok(mut plan) = app.plan_state.try_lock() {
            *plan = crate::tools::plan::PlanState::default();
            plan_cleared = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    if !plan_cleared {
        tracing::warn!(target: "commands", "plan_state lock contended or poisoned — previous plan not cleared");
    }

    if let Some(description) = metadata.description.clone() {
        app.hunt.quarry = Some(description);
        app.hunt.started_at = Some(std::time::Instant::now());
    }
    if let Some(tools) = metadata.allowed_tools.clone() {
        app.active_allowed_tools = Some(tools);
    }
    app.pausable = metadata.pausable;

    let message = user_commands::apply_template(&metadata.body, args);
    Some(CommandResult::action(AppAction::SendMessage(message)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn registry_loads_markdown_metadata() {
        let registry = UserCommandRegistry::from_loaded(vec![(
            "review".to_string(),
            "---\ndescription: Review code\nusage: /review <file>\narguments: <file>\nargument-hint: <legacy-file>\nallowed-tools: read, grep\npausable: true\n---\nReview $ARGUMENTS".to_string(),
        )]);

        let command = registry.get("review").expect("command loaded");
        assert_eq!(command.description.as_deref(), Some("Review code"));
        assert_eq!(command.usage.as_deref(), Some("/review <file>"));
        assert_eq!(command.arguments.as_deref(), Some("<file>"));
        assert_eq!(command.argument_hint.as_deref(), Some("<legacy-file>"));
        assert_eq!(command.display_usage(), Some("/review <file>"));
        assert!(command.takes_arguments());
        assert_eq!(
            command.allowed_tools,
            Some(vec!["read".to_string(), "grep".to_string()])
        );
        assert!(command.pausable);
        assert_eq!(command.body, "Review $ARGUMENTS");
    }

    #[test]
    fn frontmatter_name_replaces_filename_canonical_name() {
        let registry = UserCommandRegistry::from_loaded(vec![(
            "workflow-file".to_string(),
            "---\nname: /Review-Target\ndescription: Review target\n---\nreview $ARGUMENTS"
                .to_string(),
        )]);

        let command = registry.get("review-target").expect("renamed command");
        assert_eq!(command.name, "review-target");
        assert_eq!(command.body, "review $ARGUMENTS");
        assert!(
            registry.get("workflow-file").is_none(),
            "the filename is only a default; retaining it requires an explicit alias"
        );
    }

    #[test]
    fn filename_remains_the_default_name_without_frontmatter_override() {
        let registry = UserCommandRegistry::from_loaded(vec![(
            "Filename-Default".to_string(),
            "plain body".to_string(),
        )]);

        assert_eq!(registry.names(), vec!["filename-default"]);
        assert_eq!(
            registry.get("/filename-default").unwrap().body,
            "plain body"
        );
    }

    #[test]
    fn registry_names_are_sorted() {
        let registry = UserCommandRegistry::from_loaded(vec![
            ("zeta".to_string(), "Z".to_string()),
            ("alpha".to_string(), "A".to_string()),
        ]);
        assert_eq!(registry.names(), vec!["alpha", "zeta"]);
    }

    #[test]
    fn registry_loads_from_paths_with_first_name_wins() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        std::fs::write(first.path().join("shadow.md"), "first").unwrap();
        std::fs::write(second.path().join("shadow.md"), "second").unwrap();

        let registry = UserCommandRegistry::load_from_paths(&[
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);

        assert_eq!(registry.get("shadow").unwrap().body, "first");
    }

    #[test]
    fn frontmatter_name_collision_uses_directory_then_filename_precedence() {
        let first = TempDir::new().unwrap();
        let second = TempDir::new().unwrap();
        std::fs::write(
            first.path().join("z-workspace.md"),
            "---\nname: shared\n---\nworkspace body",
        )
        .unwrap();
        std::fs::write(
            second.path().join("a-global.md"),
            "---\nname: shared\n---\nglobal body",
        )
        .unwrap();

        let registry = UserCommandRegistry::load_from_paths(&[
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);

        assert_eq!(registry.get("shared").unwrap().body, "workspace body");
        assert!(registry.load_errors().iter().any(|error| {
            error.message.contains("User command '/shared'")
                && error.message.contains("defined more than once")
        }));
    }

    #[test]
    fn alias_lookup_uses_metadata_aliases() {
        let registry = UserCommandRegistry::from_loaded(vec![(
            "canonical".to_string(),
            "---\naliases: short, other\n---\nBody".to_string(),
        )]);
        assert_eq!(registry.get_by_alias("short").unwrap().name, "canonical");
        assert_eq!(registry.get("/other").unwrap().body, "Body");
    }

    #[test]
    fn reload_and_current_registry_compile_sentinel() {
        reload(None);
        let registry = current_registry();
        assert!(registry.is_valid());
    }

    fn write_workspace_command(workspace: &Path, name: &str, content: &str) {
        let dir = workspace.join(".codewhale").join("commands");
        std::fs::create_dir_all(&dir).expect("create commands dir");
        std::fs::write(dir.join(format!("{name}.md")), content).expect("write command");
    }

    fn test_app(workspace: PathBuf) -> App {
        let options = crate::tui::app::TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
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
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &crate::config::Config::default())
    }

    fn sent_message(result: CommandResult) -> String {
        match result.action {
            Some(AppAction::SendMessage(message)) => message,
            other => panic!("expected SendMessage action, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_prefers_user_command_over_builtin_with_same_name() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(tmp.path(), "help", "custom help $ARGUMENTS");
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/help links", &mut app);

        assert!(!result.is_error);
        assert_eq!(sent_message(result), "custom help links");
    }

    #[test]
    fn dispatch_prefers_user_alias_over_builtin_alias() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(
            tmp.path(),
            "attach-review",
            "---\nalias: image\n---\ncustom alias $ARGUMENTS",
        );
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/image screenshot.png", &mut app);

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(sent_message(result), "custom alias screenshot.png");
    }

    #[test]
    fn hidden_user_commands_still_dispatch_directly() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(
            tmp.path(),
            "internal-workflow",
            "---\nname: secret\nhidden: true\ndescription: Internal workflow\n---\nsecret $ARGUMENTS",
        );
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/secret now", &mut app);

        assert!(!result.is_error);
        assert_eq!(sent_message(result), "secret now");
        assert_eq!(app.hunt.quarry.as_deref(), Some("Internal workflow"));
    }

    #[test]
    fn dispatch_uses_frontmatter_name_arguments_and_allowed_tools() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(
            tmp.path(),
            "deploy-workflow",
            "---\nname: ship\nusage: /ship <target>\narguments: <target>\nallowed-tools: Read_File, Grep_Files\n---\nship $1 with $ARGUMENTS",
        );
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/ship moon base", &mut app);

        assert!(!result.is_error, "{:?}", result.message);
        assert_eq!(sent_message(result), "ship moon with moon base");
        assert_eq!(
            app.active_allowed_tools,
            Some(vec!["read_file".to_string(), "grep_files".to_string()])
        );
        assert!(
            try_dispatch(&mut app, "/deploy-workflow").is_none(),
            "the source filename must not remain an implicit dispatch alias"
        );
    }

    #[test]
    fn empty_allowed_tools_frontmatter_blocks_all_tools() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(
            tmp.path(),
            "locked",
            "---\nallowed-tools: \"\"\n---\nrun nothing",
        );
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/locked", &mut app);

        assert!(!result.is_error);
        assert_eq!(app.active_allowed_tools, Some(Vec::new()));
    }

    #[test]
    fn dispatch_clears_previous_command_state() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(tmp.path(), "plain", "plain command");
        let mut app = test_app(tmp.path().to_path_buf());

        app.hunt.quarry = Some("old objective".to_string());
        app.hunt.started_at = Some(std::time::Instant::now());
        app.hunt.verdict = crate::tui::app::HuntVerdict::Escaped;
        app.hunt.token_budget = Some(42);
        app.hunt.tokens_used = 100;
        app.hunt.time_used_seconds = 5;
        app.hunt.continuation_count = 2;
        app.active_allowed_tools = Some(vec!["bash".to_string()]);
        app.pausable = true;
        app.paused = true;
        app.paused_quarry = Some("old objective".to_string());
        {
            let mut todos = app.todos.try_lock().expect("todos lock");
            todos.add(
                "leftover task".to_string(),
                crate::tools::todo::TodoStatus::Pending,
            );
        }
        {
            let mut plan = app.plan_state.try_lock().expect("plan_state lock");
            plan.update(crate::tools::plan::UpdatePlanArgs {
                title: Some("leftover plan".to_string()),
                objective: Some("old goal".to_string()),
                ..Default::default()
            });
        }

        let result = crate::commands::execute("/plain", &mut app);

        assert!(!result.is_error);
        assert_eq!(app.hunt.quarry, None);
        assert_eq!(app.hunt.started_at, None);
        assert_eq!(app.hunt.verdict, crate::tui::app::HuntVerdict::Hunting);
        assert_eq!(app.hunt.token_budget, None);
        assert_eq!(app.hunt.tokens_used, 0);
        assert_eq!(app.hunt.time_used_seconds, 0);
        assert_eq!(app.hunt.continuation_count, 0);
        assert_eq!(app.active_allowed_tools, None);
        assert!(!app.pausable);
        assert!(!app.paused);
        assert!(app.paused_quarry.is_none());
        assert!(
            app.todos
                .try_lock()
                .expect("todos lock")
                .snapshot()
                .items
                .is_empty(),
            "previous command's todos must be cleared on new command dispatch"
        );
        assert!(
            app.plan_state
                .try_lock()
                .expect("plan_state lock")
                .is_empty(),
            "previous command's plan must be cleared on new command dispatch"
        );
    }

    #[test]
    fn duplicate_user_alias_keeps_first_command_and_records_user_command_error() {
        let registry = UserCommandRegistry::from_loaded(vec![
            (
                "first".to_string(),
                "---\nalias: shared\n---\nfirst body".to_string(),
            ),
            (
                "second".to_string(),
                "---\nalias: shared\n---\nsecond body".to_string(),
            ),
        ]);

        let command = registry.get("shared").expect("alias resolves");
        assert_eq!(command.name, "first");
        assert_eq!(command.body, "first body");
        assert!(
            registry.load_errors().iter().any(|error| error
                .message
                .contains("User command alias '/shared'")
                && error.message.contains("/second")),
            "duplicate alias should be recorded as a user-command load error: {:?}",
            registry.load_errors()
        );
    }

    #[test]
    fn alias_conflicting_with_canonical_user_command_is_rejected_consistently() {
        let registry = UserCommandRegistry::from_loaded(vec![
            (
                "alpha".to_string(),
                "---\nalias: beta\n---\nalpha body".to_string(),
            ),
            (
                "renamed-beta".to_string(),
                "---\nname: beta\n---\nbeta body".to_string(),
            ),
        ]);

        let command = registry.get("beta").expect("canonical command resolves");
        assert_eq!(command.name, "beta");
        assert_eq!(command.body, "beta body");
        assert!(
            registry.load_errors().iter().any(|error| error
                .message
                .contains("User command alias '/beta'")
                && error
                    .message
                    .contains("duplicates canonical user command '/beta'")),
            "alias/canonical conflict should be recorded: {:?}",
            registry.load_errors()
        );
    }

    #[test]
    fn duplicate_user_command_name_records_user_command_error() {
        let registry = UserCommandRegistry::from_loaded(vec![
            ("review".to_string(), "first".to_string()),
            ("review".to_string(), "second".to_string()),
        ]);

        assert_eq!(registry.get("review").unwrap().body, "first");
        assert!(
            registry
                .load_errors()
                .iter()
                .any(|error| error.message.contains("User command '/review'")
                    && error.message.contains("defined more than once")),
            "duplicate name should be recorded as a user-command load error: {:?}",
            registry.load_errors()
        );
    }

    #[test]
    fn malformed_losing_name_override_does_not_poison_valid_winner() {
        let registry = UserCommandRegistry::from_loaded(vec![
            (
                "first-file".to_string(),
                "---\nname: shared\n---\nfirst body".to_string(),
            ),
            (
                "second-file".to_string(),
                "---\nname: shared\nnot valid frontmatter\n---\nsecond body".to_string(),
            ),
        ]);

        assert_eq!(registry.get("shared").unwrap().body, "first body");
        assert_eq!(registry.dispatch_error("shared"), None);
        assert!(registry.load_errors().iter().any(|error| {
            error.message.contains("invalid frontmatter") && error.path.ends_with("second-file.md")
        }));
        assert!(registry.load_errors().iter().any(|error| {
            error.message.contains("defined more than once")
                && error.path.ends_with("second-file.md")
        }));
    }

    #[test]
    fn invalid_frontmatter_dispatch_returns_user_command_error_without_builtin_fallback() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(
            tmp.path(),
            "help",
            "---\ndescription: Custom help\nnot valid yaml\n---\ncustom help",
        );
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/help", &mut app);

        assert!(result.is_error);
        let message = result.message.expect("error message");
        assert!(message.contains("User command '/help'"), "{message}");
        assert!(message.contains("invalid frontmatter"), "{message}");
    }

    #[test]
    fn malformed_file_is_recoverable_and_valid_sibling_still_dispatches() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(
            tmp.path(),
            "broken",
            "---\ndescription: Broken\nnot valid frontmatter\n---\nbroken body",
        );
        write_workspace_command(
            tmp.path(),
            "healthy",
            "---\ndescription: Healthy\n---\nhealthy $ARGUMENTS",
        );
        let mut app = test_app(tmp.path().to_path_buf());

        let healthy = crate::commands::execute("/healthy now", &mut app);
        assert!(!healthy.is_error, "{:?}", healthy.message);
        assert_eq!(sent_message(healthy), "healthy now");

        let broken = crate::commands::execute("/broken", &mut app);
        assert!(broken.is_error);
        assert!(
            broken
                .message
                .as_deref()
                .is_some_and(|message| message.contains("invalid frontmatter"))
        );
    }

    #[test]
    fn invalid_frontmatter_name_is_recoverable_under_filename_default() {
        let registry = UserCommandRegistry::from_loaded(vec![(
            "recoverable".to_string(),
            "---\nname: two words\n---\nbody".to_string(),
        )]);

        assert!(registry.get("recoverable").is_some());
        assert!(registry.dispatch_error("recoverable").is_some());
        assert!(registry.load_errors().iter().any(|error| {
            error
                .message
                .contains("invalid frontmatter name \"two words\"")
        }));
    }

    #[test]
    fn frontmatter_line_with_empty_key_is_invalid() {
        let registry = UserCommandRegistry::from_loaded(vec![(
            "bad".to_string(),
            "---\n: value\n---\nbody".to_string(),
        )]);

        assert!(
            registry.load_errors().iter().any(|error| error
                .message
                .contains("invalid frontmatter line \": value\"")),
            "empty frontmatter key should be invalid: {:?}",
            registry.load_errors()
        );
    }

    #[test]
    fn registry_reloads_when_existing_command_file_changes() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(tmp.path(), "live", "first");

        assert_eq!(
            registry_for_workspace(Some(tmp.path()))
                .get("live")
                .unwrap()
                .body,
            "first"
        );

        write_workspace_command(tmp.path(), "live", "second body with different length");

        assert_eq!(
            registry_for_workspace(Some(tmp.path()))
                .get("live")
                .unwrap()
                .body,
            "second body with different length"
        );
    }

    #[test]
    fn empty_user_command_dispatch_returns_user_command_error() {
        let tmp = TempDir::new().unwrap();
        write_workspace_command(tmp.path(), "empty", "\n\t  ");
        let mut app = test_app(tmp.path().to_path_buf());

        let result = crate::commands::execute("/empty", &mut app);

        assert!(result.is_error);
        let message = result.message.expect("error message");
        assert!(message.contains("User command '/empty'"), "{message}");
        assert!(message.contains("empty"), "{message}");
    }
}
