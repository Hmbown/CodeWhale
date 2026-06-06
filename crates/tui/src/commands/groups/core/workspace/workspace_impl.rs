use std::path::PathBuf;

use crate::commands::CommandResult;
use crate::tui::app::{App, AppAction};

pub(crate) fn workspace_switch(app: &mut App, arg: Option<&str>) -> CommandResult {
    let Some(raw_path) = arg.map(str::trim).filter(|path| !path.is_empty()) else {
        return CommandResult::message(format!("Current workspace: {}", app.workspace.display()));
    };

    let expanded = match expand_workspace_path(raw_path) {
        Ok(path) => path,
        Err(message) => return CommandResult::error(message),
    };
    let candidate = if expanded.is_absolute() {
        expanded
    } else {
        app.workspace.join(expanded)
    };

    if !candidate.exists() {
        return CommandResult::error(format!("Workspace does not exist: {}", candidate.display()));
    }
    if !candidate.is_dir() {
        return CommandResult::error(format!(
            "Workspace is not a directory: {}",
            candidate.display()
        ));
    }

    let workspace = candidate.canonicalize().unwrap_or(candidate);
    CommandResult::with_message_and_action(
        format!("Switching workspace to {}...", workspace.display()),
        AppAction::SwitchWorkspace { workspace },
    )
}

fn expand_workspace_path(path: &str) -> Result<PathBuf, String> {
    if path == "~" {
        return dirs::home_dir().ok_or_else(|| "Could not resolve home directory".to_string());
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let home =
            dirs::home_dir().ok_or_else(|| "Could not resolve home directory".to_string())?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::groups::core::test_support::create_test_app;
    use tempfile::tempdir;

    #[test]
    fn workspace_without_arg_shows_current_workspace() {
        let mut app = create_test_app();
        let result = workspace_switch(&mut app, None);
        let msg = result.message.expect("workspace should be shown");
        assert!(msg.contains("Current workspace:"));
        assert!(msg.contains("/tmp/test-workspace"));
        assert!(result.action.is_none());
    }

    #[test]
    fn workspace_existing_absolute_dir_returns_switch_action() {
        let mut app = create_test_app();
        let dir = tempdir().expect("temp dir");
        let result = workspace_switch(&mut app, Some(dir.path().to_str().unwrap()));
        assert!(matches!(
            result.action,
            Some(AppAction::SwitchWorkspace { workspace }) if workspace == dir.path().canonicalize().unwrap()
        ));
    }

    #[test]
    fn workspace_relative_dir_resolves_from_current_workspace() {
        let root = tempdir().expect("temp dir");
        let child = root.path().join("child");
        std::fs::create_dir(&child).expect("child dir");
        let mut app = create_test_app();
        app.workspace = root.path().to_path_buf();

        let result = workspace_switch(&mut app, Some("child"));

        assert!(matches!(
            result.action,
            Some(AppAction::SwitchWorkspace { workspace }) if workspace == child.canonicalize().unwrap()
        ));
    }

    #[test]
    fn workspace_rejects_missing_path() {
        let mut app = create_test_app();
        let result = workspace_switch(&mut app, Some("definitely-missing"));
        assert!(result.is_error);
        assert!(result.message.unwrap().contains("does not exist"));
    }

    #[test]
    fn workspace_rejects_file_path() {
        let root = tempdir().expect("temp dir");
        let file = root.path().join("file.txt");
        std::fs::write(&file, "not a directory").expect("test file");
        let mut app = create_test_app();

        let result = workspace_switch(&mut app, Some(file.to_str().unwrap()));

        assert!(result.is_error);
        assert!(result.message.unwrap().contains("not a directory"));
    }
}
