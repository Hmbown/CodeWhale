//! /init command - Generate AGENTS.md for project

use std::fmt::Write;
use std::io::Read;
use std::path::Path;

use crate::tui::app::App;

use super::CommandResult;

/// Generate an AGENTS.md file for the current project
pub fn init(app: &mut App) -> CommandResult {
    let workspace = &app.workspace;

    // Ensure .deepseek/ is gitignored if we're inside a git repo.
    ensure_deepseek_gitignored(workspace);

    // Check if AGENTS.md already exists — update it in place rather than refusing.
    let agents_path = workspace.join("AGENTS.md");
    let already_exists = agents_path.exists();

    // Detect project type and generate appropriate content
    let content = generate_project_doc(workspace);

    // Write the file
    match std::fs::write(&agents_path, &content) {
        Ok(()) => {
            let verb = if already_exists { "Updated" } else { "Created" };
            CommandResult::message(format!(
                "{verb} AGENTS.md at {}\n\nEdit this file to customize agent behavior for your project.",
                agents_path.display()
            ))
        }
        Err(e) => CommandResult::error(format!("Failed to write AGENTS.md: {e}")),
    }
}

/// If `workspace` is inside a git repository, ensure `.deepseek/` is listed
/// in the nearest `.gitignore` so that snapshots, instructions, and other
/// workspace-local state are not accidentally committed.
fn ensure_deepseek_gitignored(workspace: &Path) {
    // Only act if this workspace is a git repo.
    if !workspace.join(".git").exists() {
        return;
    }

    let gitignore = workspace.join(".gitignore");
    let entry = ".deepseek/";

    // Read existing contents (if any) and check whether the entry is already present.
    // Check both with and without trailing slash to catch variants like
    // ".deepseek" and ".deepseek/".
    if let Ok(existing) = std::fs::read_to_string(&gitignore) {
        let entry_no_slash = entry.trim_end_matches('/');
        if existing.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == entry || trimmed == entry_no_slash
        }) {
            return; // already ignored
        }
    }

    // Append the entry. If .gitignore doesn't exist yet, create it with a header.
    // Ensure there's a trailing newline before our entry to avoid joining with
    // a previous unterminated line.
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)
    {
        // If the file is non-empty and doesn't end with a newline, add one first.
        if let Ok(meta) = file.metadata()
            && meta.len() > 0
        {
            // Read last byte to check for trailing newline.
            if let Ok(mut f) = std::fs::File::open(&gitignore) {
                use std::io::Seek;
                if f.seek(std::io::SeekFrom::End(-1)).is_ok() {
                    let mut buf = [0u8; 1];
                    if f.read_exact(&mut buf).is_ok() && buf[0] != b'\n' {
                        let _ = writeln!(file);
                    }
                }
            }
        }
        let _ = writeln!(file, "{entry}");
    }
}

/// Generate project documentation based on detected project type
fn generate_project_doc(workspace: &Path) -> String {
    let mut doc = String::new();

    // Header
    doc.push_str("# Project Instructions\n\n");
    doc.push_str("This file provides context for AI assistants working on this project.\n\n");

    // Detect project type
    let project_info = detect_project_type(workspace);
    doc.push_str(&project_info);

    // Add standard sections
    doc.push_str("\n## Guidelines\n\n");
    doc.push_str("- Follow existing code style and patterns\n");
    doc.push_str("- Write tests for new functionality\n");
    doc.push_str("- Keep changes focused and atomic\n");
    doc.push_str("- Document public APIs\n");

    doc.push_str("\n## Important Notes\n\n");
    doc.push_str("<!-- Add project-specific notes here -->\n");

    doc
}

/// Detect project type and return relevant information
fn detect_project_type(workspace: &Path) -> String {
    let mut info = String::new();

    // Check for Rust project
    if workspace.join("Cargo.toml").exists() {
        info.push_str("## Project Type: Rust\n\n");
        info.push_str("### Commands\n");
        info.push_str("- Build: `cargo build`\n");
        info.push_str("- Test: `cargo test`\n");
        info.push_str("- Run: `cargo run`\n");
        info.push_str("- Check: `cargo check`\n");
        info.push_str("- Format: `cargo fmt`\n");
        info.push_str("- Lint: `cargo clippy`\n\n");

        // Try to extract project name from Cargo.toml
        if let Some(name) = std::fs::read_to_string(workspace.join("Cargo.toml"))
            .ok()
            .and_then(|content| extract_cargo_name(&content))
        {
            let _ = write!(info, "### Project: {name}\n\n");
        }
    }
    // Check for Node.js project
    else if workspace.join("package.json").exists() {
        info.push_str("## Project Type: Node.js\n\n");
        info.push_str("### Commands\n");
        info.push_str("- Install: `npm install`\n");
        info.push_str("- Test: `npm test`\n");
        info.push_str("- Build: `npm run build`\n");
        info.push_str("- Start: `npm start`\n\n");

        // Check for common frameworks
        if workspace.join("next.config.js").exists() || workspace.join("next.config.ts").exists() {
            info.push_str("### Framework: Next.js\n\n");
        } else if workspace.join("vite.config.js").exists()
            || workspace.join("vite.config.ts").exists()
        {
            info.push_str("### Framework: Vite\n\n");
        }
    }
    // Check for Python project
    else if workspace.join("pyproject.toml").exists() || workspace.join("setup.py").exists() {
        info.push_str("## Project Type: Python\n\n");
        info.push_str("### Commands\n");
        if workspace.join("pyproject.toml").exists() {
            info.push_str("- Install: `pip install -e .`\n");
        }
        info.push_str("- Test: `pytest`\n");
        info.push_str("- Format: `black .`\n");
        info.push_str("- Lint: `ruff check .`\n\n");
    }
    // Check for Go project
    else if workspace.join("go.mod").exists() {
        info.push_str("## Project Type: Go\n\n");
        info.push_str("### Commands\n");
        info.push_str("- Build: `go build`\n");
        info.push_str("- Test: `go test ./...`\n");
        info.push_str("- Run: `go run .`\n");
        info.push_str("- Format: `go fmt ./...`\n\n");
    }
    // Unknown project type
    else {
        info.push_str("## Project Type: Unknown\n\n");
        info.push_str("<!-- Add build/test commands here -->\n\n");
    }

    // Check for README
    if workspace.join("README.md").exists() {
        info.push_str("### Documentation\n");
        info.push_str("See README.md for project overview.\n\n");
    }

    // Check for .gitignore
    if workspace.join(".gitignore").exists() {
        info.push_str("### Version Control\n");
        info.push_str("This project uses Git. See .gitignore for excluded files.\n\n");
    }

    info
}

/// Extract project name from Cargo.toml
fn extract_cargo_name(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("name") && line.contains('=') {
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() == 2 {
                let name = parts[1].trim().trim_matches('"').trim_matches('\'');
                return Some(name.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::{App, TuiOptions};
    use tempfile::TempDir;

    fn create_test_app_with_tmpdir(tmpdir: &TempDir) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace: tmpdir.path().to_path_buf(),
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: tmpdir.path().join("skills"),
            memory_path: tmpdir.path().join("memory.md"),
            notes_path: tmpdir.path().join("notes.txt"),
            mcp_config_path: tmpdir.path().join("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    #[test]
    fn test_init_creates_agents_md() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = init(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Created AGENTS.md"));
        let agents_path = tmpdir.path().join("AGENTS.md");
        assert!(agents_path.exists());
    }

    #[test]
    fn test_init_updates_if_exists() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        // Create file first with stale content
        let agents_path = tmpdir.path().join("AGENTS.md");
        std::fs::write(&agents_path, "existing stale content").unwrap();
        let result = init(&mut app);
        assert!(!result.is_error);
        assert!(result.message.is_some());
        assert!(result.message.unwrap().contains("Updated AGENTS.md"));
        let new_content = std::fs::read_to_string(&agents_path).unwrap();
        assert!(new_content.contains("# Project Instructions"));
        assert!(!new_content.contains("existing stale content"));
    }

    #[test]
    fn test_detect_project_type_rust() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(
            tmpdir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"",
        )
        .unwrap();
        let info = detect_project_type(tmpdir.path());
        assert!(info.contains("Project Type: Rust"));
        assert!(info.contains("cargo build"));
        assert!(info.contains("cargo test"));
    }

    #[test]
    fn test_detect_project_type_node() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("package.json"), "{}").unwrap();
        let info = detect_project_type(tmpdir.path());
        assert!(info.contains("Project Type: Node.js"));
        assert!(info.contains("npm install"));
    }

    #[test]
    fn test_detect_project_type_python() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("pyproject.toml"), "[project]").unwrap();
        let info = detect_project_type(tmpdir.path());
        assert!(info.contains("Project Type: Python"));
    }

    #[test]
    fn test_detect_project_type_go() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("go.mod"), "module test").unwrap();
        let info = detect_project_type(tmpdir.path());
        assert!(info.contains("Project Type: Go"));
    }

    #[test]
    fn test_detect_project_type_unknown() {
        let tmpdir = TempDir::new().unwrap();
        let info = detect_project_type(tmpdir.path());
        assert!(info.contains("Project Type: Unknown"));
    }

    #[test]
    fn test_extract_cargo_name() {
        let cargo = r#"
[package]
name = "my-project"
version = "1.0.0"
"#;
        assert_eq!(extract_cargo_name(cargo), Some("my-project".to_string()));
    }

    #[test]
    fn test_extract_cargo_name_single_quotes() {
        let cargo = r#"name = 'single-quoted'"#;
        assert_eq!(extract_cargo_name(cargo), Some("single-quoted".to_string()));
    }

    #[test]
    fn test_extract_cargo_name_not_found() {
        let cargo = "[package]\nversion = \"1.0.0\"";
        assert_eq!(extract_cargo_name(cargo), None);
    }

    #[test]
    fn ensure_deepseek_gitignored_creates_gitignore() {
        let tmpdir = TempDir::new().unwrap();
        // Simulate a git repo.
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".deepseek/"));
    }

    #[test]
    fn ensure_deepseek_gitignored_appends_to_existing() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        std::fs::write(tmpdir.path().join(".gitignore"), "target/\n").unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(content.contains("target/"));
        assert!(content.contains(".deepseek/"));
    }

    #[test]
    fn ensure_deepseek_gitignored_idempotent() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();

        ensure_deepseek_gitignored(tmpdir.path());
        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert_eq!(content.matches(".deepseek/").count(), 1);
    }

    #[test]
    fn ensure_deepseek_gitignored_skips_non_git_repo() {
        let tmpdir = TempDir::new().unwrap();
        // No .git directory — not a git repo.

        ensure_deepseek_gitignored(tmpdir.path());

        assert!(!tmpdir.path().join(".gitignore").exists());
    }

    #[test]
    fn ensure_deepseek_gitignored_handles_no_trailing_newline() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        // Write a file that does NOT end with a newline.
        std::fs::write(tmpdir.path().join(".gitignore"), "target/").unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        // Must have both entries on separate lines.
        assert!(content.contains("target/"));
        assert!(content.contains(".deepseek/"));
        // The entries should be on different lines.
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines.len() >= 2);
    }

    #[test]
    fn ensure_deepseek_gitignored_detects_variant_without_slash() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        // Write .deepseek without trailing slash.
        std::fs::write(tmpdir.path().join(".gitignore"), ".deepseek\n").unwrap();

        ensure_deepseek_gitignored(tmpdir.path());

        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        // Should NOT add a duplicate entry.
        assert_eq!(content.matches(".deepseek").count(), 1);
    }
}
