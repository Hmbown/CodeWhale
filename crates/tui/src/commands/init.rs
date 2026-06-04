//! /init command - Generate AGENTS.md for project
//!
//! Gathers rich project context (directory structure, build system, git info, CI/CD,
//! test frameworks) and delegates AGENTS.md generation to the LLM agent via
//! `AppAction::SendMessage`. This mirrors Claude Code's `/init` behavior — the agent
//! reads key source files, understands the architecture, and produces a customized,
//! comprehensive project guide.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use crate::project_context;
use crate::tui::app::{App, AppAction};

use super::CommandResult;

/// Generate an AGENTS.md file for the current project by gathering context and
/// delegating content generation to the LLM agent.
pub fn init(app: &mut App) -> CommandResult {
    let workspace = &app.workspace;

    // Ensure .deepseek/ is gitignored if we're inside a git repo.
    ensure_deepseek_gitignored(workspace);

    // Check if AGENTS.md already exists — update it in place rather than refusing.
    let agents_path = workspace.join("AGENTS.md");
    let already_exists = agents_path.exists();

    // Gather rich project context for the agent.
    let context = gather_project_context(workspace);

    // Read existing AGENTS.md content if updating.
    let existing_content = if already_exists {
        read_existing_agents_md(workspace)
    } else {
        None
    };

    // Construct the prompt for the LLM agent.
    let prompt = build_init_prompt(&context, existing_content.as_deref(), already_exists);

    // Display message to user AND send the prompt to the agent.
    let verb = if already_exists {
        "Updating"
    } else {
        "Creating"
    };
    let msg = format!(
        "{verb} AGENTS.md at {}\n\nThe agent will analyze the codebase and generate a customized project guide.",
        agents_path.display()
    );

    CommandResult::with_message_and_action(msg, AppAction::SendMessage(prompt))
}

/// If `workspace` is inside a git repository, ensure workspace-local CodeWhale
/// state is listed in the nearest `.gitignore` so snapshots, auto-generated
/// instructions, and other runtime state are not accidentally committed — while
/// keeping the authored `.codewhale/constitution.json` repo authority policy
/// committable (a directory exclude cannot be overridden, so `.codewhale/*` plus
/// a negation is required).
fn ensure_deepseek_gitignored(workspace: &Path) {
    // Only act if this workspace is a git repo.
    if !workspace.join(".git").exists() {
        return;
    }

    let gitignore = workspace.join(".gitignore");
    let entries = [
        "**/.codewhale/*",
        "!**/.codewhale/constitution.json",
        ".deepseek/",
    ];

    // Read existing contents once.
    let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
    let mut missing: Vec<&str> = Vec::new();
    for entry in entries {
        let entry_no_slash = entry.trim_end_matches('/');
        let already_ignored = existing.lines().any(|line| {
            let trimmed = line.trim();
            trimmed == entry || trimmed == entry_no_slash
        });
        if !already_ignored {
            missing.push(entry);
        }
    }

    if missing.is_empty() {
        return;
    }

    // Append missing entries. If .gitignore doesn't exist yet, create it.
    use std::io::Write;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&gitignore)
    {
        // If the file is non-empty and doesn't end with a newline, add one first.
        if let Ok(meta) = file.metadata()
            && meta.len() > 0
            && let Ok(mut f) = std::fs::File::open(&gitignore)
        {
            use std::io::Seek;
            if f.seek(std::io::SeekFrom::End(-1)).is_ok() {
                let mut buf = [0u8; 1];
                if f.read_exact(&mut buf).is_ok() && buf[0] != b'\n' {
                    let _ = writeln!(file);
                }
            }
        }
        for entry in &missing {
            let _ = writeln!(file, "{entry}");
        }
    }
}

// ---------------------------------------------------------------------------
// Context gathering functions
// ---------------------------------------------------------------------------

/// Orchestrate all context gathering and return structured Markdown for the agent prompt.
fn gather_project_context(workspace: &Path) -> String {
    let mut ctx = String::new();

    // Project type summary (from existing utility).
    let summary = crate::utils::summarize_project(workspace);
    ctx.push_str("## Project Summary\n\n");
    ctx.push_str(&summary);
    ctx.push_str("\n\n");

    // Cargo.toml analysis.
    if let Some(info) = parse_cargo_toml(workspace) {
        ctx.push_str("## Rust / Cargo\n\n");
        ctx.push_str(&info);
        ctx.push_str("\n\n");
    }

    // package.json analysis.
    if let Some(info) = parse_package_json(workspace) {
        ctx.push_str("## Node.js / npm\n\n");
        ctx.push_str(&info);
        ctx.push_str("\n\n");
    }

    // Git repository info.
    if let Some(info) = gather_git_info(workspace) {
        ctx.push_str("## Git Repository\n\n");
        ctx.push_str(&info);
        ctx.push_str("\n\n");
    }

    // CI/CD systems.
    let ci = detect_ci_systems(workspace);
    if !ci.is_empty() {
        ctx.push_str("## CI/CD\n\n");
        for system in &ci {
            let _ = std::fmt::write(&mut ctx, format_args!("- {system}\n"));
        }
        ctx.push('\n');
    }

    // Build systems.
    let build = detect_build_systems(workspace);
    if !build.is_empty() {
        ctx.push_str("## Additional Build Systems\n\n");
        for system in &build {
            let _ = std::fmt::write(&mut ctx, format_args!("- {system}\n"));
        }
        ctx.push('\n');
    }

    // Test frameworks.
    let tests = detect_test_frameworks(workspace);
    if !tests.is_empty() {
        ctx.push_str("## Test Frameworks\n\n");
        for framework in &tests {
            let _ = std::fmt::write(&mut ctx, format_args!("- {framework}\n"));
        }
        ctx.push('\n');
    }

    // Directory tree (from existing utility).
    let tree = crate::utils::project_tree(workspace, 3);
    ctx.push_str("## Directory Structure (depth 3)\n\n```\n");
    ctx.push_str(&tree);
    ctx.push_str("\n```\n\n");

    // Structured project context pack (from existing utility).
    if let Some(pack) = project_context::generate_project_context_pack(workspace) {
        ctx.push_str("## Detailed Project Context\n\n```json\n");
        ctx.push_str(&pack);
        ctx.push_str("\n```\n\n");
    }

    ctx
}

/// Parse `Cargo.toml` and return a human-readable summary of the Rust project structure.
fn parse_cargo_toml(workspace: &Path) -> Option<String> {
    let cargo_path = workspace.join("Cargo.toml");
    let raw = std::fs::read_to_string(&cargo_path).ok()?;
    let doc: toml::Value = toml::from_str(&raw).ok()?;

    let mut lines: Vec<String> = Vec::new();

    // Package info.
    if let Some(package) = doc.get("package") {
        if let Some(name) = package.get("name").and_then(|v| v.as_str()) {
            lines.push(format!("- Package name: `{name}`"));
        }
        if let Some(version) = package.get("version").and_then(|v| v.as_str()) {
            lines.push(format!("- Version: {version}"));
        }
        if let Some(edition) = package.get("edition").and_then(|v| v.as_str()) {
            lines.push(format!("- Rust edition: {edition}"));
        }
    }

    // Workspace info.
    if let Some(workspace_section) = doc.get("workspace") {
        lines.push("- **This is a workspace root**".to_string());
        if let Some(members) = workspace_section.get("members").and_then(|v| v.as_array()) {
            let member_names: Vec<&str> = members.iter().filter_map(|m| m.as_str()).collect();
            if !member_names.is_empty() {
                lines.push(format!("- Workspace members: {}", member_names.join(", ")));
            }
        }
    }

    // Dependencies.
    if let Some(deps) = doc.get("dependencies").and_then(|v| v.as_table()) {
        let dep_names: Vec<&str> = deps.keys().map(|k| k.as_str()).collect();
        if !dep_names.is_empty() {
            lines.push(format!("- Key dependencies: {}", dep_names.join(", ")));
        }
    }

    // Dev dependencies — test frameworks.
    if let Some(dev_deps) = doc.get("dev-dependencies").and_then(|v| v.as_table()) {
        let dev_names: Vec<&str> = dev_deps.keys().map(|k| k.as_str()).collect();
        if !dev_names.is_empty() {
            lines.push(format!("- Dev dependencies: {}", dev_names.join(", ")));
        }
    }

    // Workspace-level dependencies (shared across workspace members).
    if let Some(ws_deps) = doc
        .get("workspace")
        .and_then(|w| w.get("dependencies"))
        .and_then(|v| v.as_table())
    {
        let ws_dep_names: Vec<&str> = ws_deps.keys().map(|k| k.as_str()).collect();
        if !ws_dep_names.is_empty() {
            lines.push(format!(
                "- Workspace dependencies: {}",
                ws_dep_names.join(", ")
            ));
        }
    }

    // Features.
    if let Some(features) = doc.get("features").and_then(|v| v.as_table()) {
        let feat_names: Vec<&str> = features.keys().map(|k| k.as_str()).collect();
        if !feat_names.is_empty() {
            lines.push(format!("- Features: {}", feat_names.join(", ")));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Parse `package.json` and return a human-readable summary of the Node.js project.
fn parse_package_json(workspace: &Path) -> Option<String> {
    let pkg_path = workspace.join("package.json");
    let raw = std::fs::read_to_string(&pkg_path).ok()?;
    let doc: serde_json::Value = serde_json::from_str(&raw).ok()?;

    let mut lines: Vec<String> = Vec::new();

    if let Some(name) = doc.get("name").and_then(|v| v.as_str()) {
        lines.push(format!("- Package name: `{name}`"));
    }

    // Scripts.
    if let Some(scripts) = doc.get("scripts").and_then(|v| v.as_object()) {
        let script_names: Vec<&str> = scripts.keys().map(|k| k.as_str()).collect();
        if !script_names.is_empty() {
            lines.push(format!("- Scripts: {}", script_names.join(", ")));
        }
    }

    // Dependencies.
    if let Some(deps) = doc.get("dependencies").and_then(|v| v.as_object()) {
        let dep_keys: Vec<&str> = deps.keys().map(|k| k.as_str()).collect();
        if !dep_keys.is_empty() {
            // Detect frameworks from deps.
            let frameworks = detect_js_frameworks(&dep_keys);
            if !frameworks.is_empty() {
                lines.push(format!("- Frameworks detected: {}", frameworks.join(", ")));
            }
            lines.push(format!("- Dependencies: {}", dep_keys.join(", ")));
        }
    }

    // Dev dependencies.
    if let Some(dev_deps) = doc.get("devDependencies").and_then(|v| v.as_object()) {
        let dev_keys: Vec<&str> = dev_deps.keys().map(|k| k.as_str()).collect();
        if !dev_keys.is_empty() {
            lines.push(format!("- Dev dependencies: {}", dev_keys.join(", ")));
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Detect JS frameworks from dependency names.
fn detect_js_frameworks(deps: &[&str]) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let candidates: &[(&str, &str)] = &[
        ("react", "React"),
        ("next", "Next.js"),
        ("vue", "Vue"),
        ("nuxt", "Nuxt"),
        ("svelte", "Svelte"),
        ("sveltekit", "SvelteKit"),
        ("astro", "Astro"),
        ("express", "Express"),
        ("fastify", "Fastify"),
        ("hono", "Hono"),
        ("vite", "Vite"),
        ("webpack", "Webpack"),
        ("esbuild", "esbuild"),
        ("turbo", "Turbopack"),
        ("tailwindcss", "Tailwind CSS"),
    ];
    for dep in deps {
        let lower = dep.to_lowercase();
        for (key, label) in candidates {
            if lower == *key && !found.contains(&label.to_string()) {
                found.push((*label).to_string());
            }
        }
    }
    found
}

/// Gather git repository information via subprocess calls.
fn gather_git_info(workspace: &Path) -> Option<String> {
    // Check if we're in a git repo.
    if !workspace.join(".git").exists() {
        return None;
    }

    let run = |args: &[&str]| -> Option<String> {
        Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .ok()
            .and_then(|out| {
                if out.status.success() {
                    String::from_utf8(out.stdout)
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                } else {
                    None
                }
            })
    };

    let mut lines: Vec<String> = Vec::new();

    // Remote URL.
    if let Some(url) = run(&["remote", "get-url", "origin"]) {
        lines.push(format!("- Remote: {url}"));
    }

    // Current branch.
    if let Some(branch) = run(&["rev-parse", "--abbrev-ref", "HEAD"]) {
        lines.push(format!("- Branch: {branch}"));
    }

    // Status summary.
    let status_output = Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=no"])
        .current_dir(workspace)
        .output()
        .ok();
    if let Some(out) = status_output
        && out.status.success()
    {
        let status_str = String::from_utf8_lossy(&out.stdout);
        let staged = status_str
            .lines()
            .filter(|l| {
                let b = l.as_bytes();
                b.len() >= 2 && b[0] != b' ' && b[0] != b'?'
            })
            .count();
        let unstaged = status_str
            .lines()
            .filter(|l| {
                let b = l.as_bytes();
                b.len() >= 2 && b[1] != b' ' && b[1] != b'?'
            })
            .count();
        let untracked = status_str.lines().filter(|l| l.starts_with("??")).count();
        if staged > 0 || unstaged > 0 || untracked > 0 {
            let mut parts = Vec::new();
            if staged > 0 {
                parts.push(format!("{staged} staged"));
            }
            if unstaged > 0 {
                parts.push(format!("{unstaged} modified"));
            }
            if untracked > 0 {
                parts.push(format!("{untracked} untracked"));
            }
            lines.push(format!("- Working tree: {}", parts.join(", ")));
        }
    }

    // Recent commits.
    if let Some(log) = run(&["log", "--oneline", "-5"]) {
        let commits: Vec<&str> = log.lines().collect();
        if !commits.is_empty() {
            lines.push("- Recent commits:".to_string());
            for c in commits {
                lines.push(format!("  - {c}"));
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

/// Detect CI/CD systems configured in the project.
fn detect_ci_systems(workspace: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();

    if workspace.join(".github").join("workflows").is_dir()
        && let Ok(entries) = std::fs::read_dir(workspace.join(".github").join("workflows"))
    {
            let files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".yml") || name.ends_with(".yaml") {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect();
            if files.is_empty() {
                found.push("GitHub Actions".to_string());
            } else {
                found.push(format!("GitHub Actions ({})", files.join(", ")));
            }
    }
    if workspace.join(".gitlab-ci.yml").exists() {
        found.push("GitLab CI".to_string());
    }
    if workspace.join("Jenkinsfile").exists() {
        found.push("Jenkins".to_string());
    }
    if workspace.join(".circleci").join("config.yml").exists() {
        found.push("CircleCI".to_string());
    }
    if workspace.join(".travis.yml").exists() {
        found.push("Travis CI".to_string());
    }
    if workspace.join("azure-pipelines.yml").exists() {
        found.push("Azure Pipelines".to_string());
    }

    found
}

/// Detect additional build systems beyond Cargo/npm.
fn detect_build_systems(workspace: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();

    if workspace.join("Makefile").exists() {
        found.push("Makefile".to_string());
    }
    if workspace.join("Justfile").exists() {
        found.push("Justfile".to_string());
    }
    if workspace.join("CMakeLists.txt").exists() {
        found.push("CMake".to_string());
    }
    if workspace.join("meson.build").exists() {
        found.push("Meson".to_string());
    }
    if workspace.join("BUILD.bazel").exists() || workspace.join("BUILD").exists() {
        found.push("Bazel".to_string());
    }
    if workspace.join("scripts").is_dir()
        && let Ok(entries) = std::fs::read_dir(workspace.join("scripts"))
    {
            let scripts: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    let path = e.path();
                    if (name.ends_with(".sh") || name.ends_with(".py") || name.ends_with(".js"))
                        && path.is_file()
                    {
                        Some(name)
                    } else {
                        None
                    }
                })
                .collect();
            if !scripts.is_empty() {
                found.push(format!("scripts/ ({})", scripts.join(", ")));
            }
    }

    found
}

/// Detect test frameworks from project configuration.
fn detect_test_frameworks(workspace: &Path) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();

    // Rust: check Cargo.toml dev-dependencies (both crate and workspace level).
    if let Ok(raw) = std::fs::read_to_string(workspace.join("Cargo.toml"))
        && let Ok(doc) = toml::from_str::<toml::Value>(&raw)
    {
        let mut dep_keys: Vec<&str> = Vec::new();
            if let Some(dev_deps) = doc.get("dev-dependencies").and_then(|v| v.as_table()) {
                dep_keys.extend(dev_deps.keys().map(|k| k.as_str()));
            }
            if let Some(ws_dev_deps) = doc
                .get("workspace")
                .and_then(|w| w.get("dev-dependencies"))
                .and_then(|v| v.as_table())
            {
                dep_keys.extend(ws_dev_deps.keys().map(|k| k.as_str()));
            }

            let rust_test_frameworks: &[(&str, &str)] = &[
                ("tokio-test", "tokio-test"),
                ("proptest", "proptest"),
                ("quickcheck", "quickcheck"),
                ("rstest", "rstest"),
                ("criterion", "criterion (benchmark)"),
                ("mockall", "mockall"),
                ("pretty_assertions", "pretty_assertions"),
            ];
            for (dep_key, label) in rust_test_frameworks {
                if dep_keys.contains(dep_key) {
                    found.push((*label).to_string());
                }
            }
    }

    // Node.js: check package.json devDependencies.
    if let Ok(raw) = std::fs::read_to_string(workspace.join("package.json"))
        && let Ok(doc) = serde_json::from_str::<serde_json::Value>(&raw)
        && let Some(dev_deps) = doc.get("devDependencies").and_then(|v| v.as_object())
    {
        let dev_keys: Vec<&str> = dev_deps.keys().map(|k| k.as_str()).collect();

                let js_test_frameworks: &[(&str, &str)] = &[
                    ("jest", "Jest"),
                    ("vitest", "Vitest"),
                    ("mocha", "Mocha"),
                    ("jasmine", "Jasmine"),
                    ("ava", "AVA"),
                    ("playwright", "Playwright"),
                    ("cypress", "Cypress"),
                    ("@testing-library/react", "Testing Library"),
                ];
                for (dep_key, label) in js_test_frameworks {
                    if dev_keys.contains(dep_key) {
                        found.push((*label).to_string());
                    }
                }
    }

    // Python: check common test config files.
    if workspace.join("pytest.ini").exists()
        || workspace.join("tox.ini").exists()
        || workspace.join("conftest.py").exists()
        || (workspace.join("pyproject.toml").exists()
            && std::fs::read_to_string(workspace.join("pyproject.toml"))
                .ok()
                .is_some_and(|raw| raw.contains("[tool.pytest")))
    {
        found.push("pytest".to_string());
    }

    found
}

/// Read existing AGENTS.md content (up to 100KB) for in-place update.
fn read_existing_agents_md(workspace: &Path) -> Option<String> {
    let path = workspace.join("AGENTS.md");
    let meta = std::fs::metadata(&path).ok()?;
    let limit = 100 * 1024;
    let len = meta.len() as usize;
    let content = if len > limit {
        let mut f = std::fs::File::open(&path).ok()?;
        let mut buf = vec![0u8; limit];
        f.read_exact(&mut buf).ok()?;
        String::from_utf8_lossy(&buf).into_owned()
    } else {
        std::fs::read_to_string(&path).ok()?
    };
    if content.trim().is_empty() {
        None
    } else {
        Some(content)
    }
}

// ---------------------------------------------------------------------------
// Prompt builder
// ---------------------------------------------------------------------------

/// Build the SendMessage prompt instructing the agent to analyze and generate AGENTS.md.
fn build_init_prompt(
    context: &str,
    existing_content: Option<&str>,
    already_exists: bool,
) -> String {
    let mut prompt = String::new();

    prompt.push_str(
        "You are generating a comprehensive AGENTS.md file for this project. \
         Your task is to deeply analyze the codebase and produce a customized, \
         actionable project guide that will help future AI agents work effectively here.\n\n",
    );

    prompt.push_str("## Project Context (pre-gathered)\n\n");
    prompt.push_str(context);
    prompt.push('\n');

    if let Some(existing) = existing_content {
        prompt.push_str("## Existing AGENTS.md\n\n");
        prompt.push_str("Below is the current AGENTS.md content. ");
        if already_exists {
            prompt.push_str(
                "Update it in place: preserve any custom sections that still apply, \
                replace stale or incorrect information with your fresh analysis. ",
            );
        }
        prompt.push_str("\n\n```markdown\n");
        prompt.push_str(existing);
        prompt.push_str("\n```\n\n");
    }

    prompt.push_str("## Instructions\n\n");

    prompt.push_str(
        "1. **Read key source files** to understand the architecture:\n\
           - Start with the main entry point(s) (e.g., main.rs, index.ts, app.py)\n\
           - Read the top-level module structure to understand component boundaries\n\
           - Read a few representative files from each major module or crate\n\
           - Read config files (config.example.toml, tsconfig.json, etc.) to understand settings\n\n\
         2. **Generate AGENTS.md** at the workspace root. Use `AGENTS.md` as the filename. \
           Include these sections as applicable:\n\n\
           ### Build / Test / Lint\n\
           - Exact commands for: build, test (all + single), lint, format, run, install deps\n\
           - Be specific — if there's a Justfile, use `just <target>`; if nextest, use `cargo nextest run`\n\n\
           ### Architecture\n\
           - High-level description of the project's purpose\n\
           - Component or module tree with 1-2 sentence descriptions each\n\
           - Data flow through the system (if determinable)\n\n\
           ### Key Files & Directories\n\
           - What each top-level directory contains\n\
           - Important config files and what they control\n\n\
           ### Coding Conventions\n\
           - What you observe from reading source files: naming, error handling patterns, \
             module organization, test patterns\n\
           - Code generation (build.rs, protobuf, etc.) if present\n\n\
           ### Git Workflow\n\
           - Branch naming conventions (if observable from recent commits)\n\
           - Commit message style\n\n\
           ### CI/CD\n\
           - How tests run in CI, what's checked on PRs\n\n\
           ### Tips for AI Agents\n\
           - Common pitfalls in the codebase structure\n\
           - Where to look for specific kinds of things\n\
           - Any gotchas in the build setup\n\n\
         3. **Style requirements**:\n\
           - Be concise and actionable. This is a reference document, not a tutorial.\n\
           - Use markdown headings, code blocks, and bullet lists.\n\
           - Keep the total under ~150 lines unless the project genuinely needs more.\n\
           - Write in English.\n\
           - Do NOT include placeholder HTML comments like \"<!-- add stuff here -->\".\n\
           - If you cannot determine something with confidence, omit that section rather than guessing.\n\n\
         4. **Write the file** using the file write tool. \
           The file should be named `AGENTS.md` at the workspace root.\n\n",
    );

    if already_exists {
        prompt.push_str(
            "The file already exists — update it in place, \
            preserving custom content that still applies but replacing stale information.\n\n",
        );
    }

    prompt.push_str(
        "5. After writing, briefly summarize what you learned and what you put into AGENTS.md.\n",
    );

    prompt
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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

    // --- init() integration tests ---

    #[test]
    fn init_returns_send_message_action() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        let result = init(&mut app);
        assert!(result.message.is_some());
        let msg = result.message.unwrap();
        assert!(msg.contains("Creating AGENTS.md"));
        assert!(
            matches!(result.action, Some(AppAction::SendMessage(_))),
            "expected SendMessage action"
        );
    }

    #[test]
    fn init_says_updating_when_agents_md_exists() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        std::fs::write(tmpdir.path().join("AGENTS.md"), "existing content").unwrap();
        let result = init(&mut app);
        assert!(result.message.unwrap().contains("Updating AGENTS.md"));
        assert!(matches!(result.action, Some(AppAction::SendMessage(_))));
    }

    #[test]
    fn init_includes_gitignore_handling() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        let result = init(&mut app);
        assert!(!result.is_error);
        // Should have added .deepseek/ to .gitignore.
        let gi = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(gi.contains(".deepseek/"));
    }

    #[test]
    fn init_prompt_includes_context_for_rust_project() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        std::fs::write(
            tmpdir.path().join("Cargo.toml"),
            "[package]\nname = \"test-crate\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let result = init(&mut app);
        let Some(AppAction::SendMessage(prompt)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(
            prompt.contains("test-crate"),
            "prompt should mention crate name"
        );
        assert!(
            prompt.contains("Read key source files"),
            "should have instructions"
        );
        assert!(
            prompt.contains("AGENTS.md"),
            "should mention AGENTS.md filename"
        );
    }

    #[test]
    fn init_prompt_includes_existing_content() {
        let tmpdir = TempDir::new().unwrap();
        let mut app = create_test_app_with_tmpdir(&tmpdir);
        std::fs::write(
            tmpdir.path().join("AGENTS.md"),
            "# My Project\n\nCustom instructions here.",
        )
        .unwrap();
        let result = init(&mut app);
        let Some(AppAction::SendMessage(prompt)) = result.action else {
            panic!("expected SendMessage action");
        };
        assert!(prompt.contains("Custom instructions here"));
        assert!(prompt.contains("update it in place"));
    }

    // --- parse_cargo_toml tests ---

    #[test]
    fn parse_cargo_toml_single_crate() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(
            tmpdir.path().join("Cargo.toml"),
            "[package]\nname = \"my-crate\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n\
             [dependencies]\ntokio = \"1\"\nserde = \"1\"\n",
        )
        .unwrap();
        let info = parse_cargo_toml(tmpdir.path()).unwrap();
        assert!(info.contains("my-crate"));
        assert!(info.contains("1.0.0"));
        assert!(info.contains("2021"));
        assert!(info.contains("tokio"));
        assert!(info.contains("serde"));
    }

    #[test]
    fn parse_cargo_toml_workspace() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(
            tmpdir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/cli\", \"crates/tui\"]\n\n\
             [workspace.dependencies]\nserde = \"1\"\n",
        )
        .unwrap();
        let info = parse_cargo_toml(tmpdir.path()).unwrap();
        assert!(info.contains("workspace root"));
        assert!(info.contains("crates/cli"));
        assert!(info.contains("crates/tui"));
    }

    #[test]
    fn parse_cargo_toml_missing() {
        let tmpdir = TempDir::new().unwrap();
        assert!(parse_cargo_toml(tmpdir.path()).is_none());
    }

    #[test]
    fn parse_cargo_toml_invalid() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("Cargo.toml"), "not valid toml {{{").unwrap();
        assert!(parse_cargo_toml(tmpdir.path()).is_none());
    }

    // --- parse_package_json tests ---

    #[test]
    fn parse_package_json_basic() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(
            tmpdir.path().join("package.json"),
            r#"{"name":"my-app","scripts":{"build":"tsc","test":"jest"},"dependencies":{"react":"^18"},"devDependencies":{"jest":"^29"}}"#,
        )
        .unwrap();
        let info = parse_package_json(tmpdir.path()).unwrap();
        assert!(info.contains("my-app"));
        assert!(info.contains("build"));
        assert!(info.contains("test"));
        assert!(info.contains("React"));
        assert!(info.contains("jest"));
    }

    #[test]
    fn parse_package_json_missing() {
        let tmpdir = TempDir::new().unwrap();
        assert!(parse_package_json(tmpdir.path()).is_none());
    }

    // --- gather_git_info tests ---

    #[test]
    fn gather_git_info_no_repo_returns_none() {
        let tmpdir = TempDir::new().unwrap();
        assert!(gather_git_info(tmpdir.path()).is_none());
    }

    #[test]
    fn gather_git_info_in_repo_returns_branch() {
        let tmpdir = TempDir::new().unwrap();
        // Init a real git repo.
        Command::new("git")
            .args(["init"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.email", "test@test.com"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["config", "user.name", "Test"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["checkout", "-b", "main"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();
        // Create a commit so rev-parse works.
        std::fs::write(tmpdir.path().join("hello.txt"), "hi").unwrap();
        Command::new("git")
            .args(["add", "."])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();
        Command::new("git")
            .args(["commit", "-m", "initial"])
            .current_dir(tmpdir.path())
            .output()
            .unwrap();

        let info = gather_git_info(tmpdir.path()).unwrap();
        assert!(
            info.contains("main") || info.contains("master"),
            "should show branch: {info}"
        );
    }

    // --- detect_ci_systems tests ---

    #[test]
    fn detect_ci_github_actions() {
        let tmpdir = TempDir::new().unwrap();
        let wf_dir = tmpdir.path().join(".github").join("workflows");
        std::fs::create_dir_all(&wf_dir).unwrap();
        std::fs::write(wf_dir.join("ci.yml"), "").unwrap();
        let ci = detect_ci_systems(tmpdir.path());
        assert!(ci.iter().any(|s| s.contains("GitHub Actions")));
    }

    #[test]
    fn detect_ci_none() {
        let tmpdir = TempDir::new().unwrap();
        assert!(detect_ci_systems(tmpdir.path()).is_empty());
    }

    // --- detect_build_systems tests ---

    #[test]
    fn detect_makefile() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("Makefile"), "").unwrap();
        let build = detect_build_systems(tmpdir.path());
        assert!(build.contains(&"Makefile".to_string()));
    }

    #[test]
    fn detect_justfile() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("Justfile"), "").unwrap();
        let build = detect_build_systems(tmpdir.path());
        assert!(build.contains(&"Justfile".to_string()));
    }

    // --- detect_test_frameworks tests ---

    #[test]
    fn detect_rust_test_frameworks_from_cargo() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(
            tmpdir.path().join("Cargo.toml"),
            "[dev-dependencies]\ntokio-test = \"1\"\nproptest = \"1\"\n",
        )
        .unwrap();
        let frameworks = detect_test_frameworks(tmpdir.path());
        assert!(frameworks.contains(&"tokio-test".to_string()));
        assert!(frameworks.contains(&"proptest".to_string()));
    }

    #[test]
    fn detect_js_test_frameworks_from_package_json() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(
            tmpdir.path().join("package.json"),
            r#"{"devDependencies":{"jest":"^29","vitest":"^1"}}"#,
        )
        .unwrap();
        let frameworks = detect_test_frameworks(tmpdir.path());
        assert!(frameworks.contains(&"Jest".to_string()));
        assert!(frameworks.contains(&"Vitest".to_string()));
    }

    // --- read_existing_agents_md tests ---

    #[test]
    fn read_existing_agents_md_present() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("AGENTS.md"), "hello world").unwrap();
        let content = read_existing_agents_md(tmpdir.path());
        assert_eq!(content, Some("hello world".to_string()));
    }

    #[test]
    fn read_existing_agents_md_missing() {
        let tmpdir = TempDir::new().unwrap();
        assert!(read_existing_agents_md(tmpdir.path()).is_none());
    }

    #[test]
    fn read_existing_agents_md_empty_file_returns_none() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::write(tmpdir.path().join("AGENTS.md"), "").unwrap();
        assert!(read_existing_agents_md(tmpdir.path()).is_none());
    }

    // --- build_init_prompt tests ---

    #[test]
    fn build_init_prompt_contains_all_sections() {
        let ctx = "## Project Summary\n\nA Rust project\n";
        let prompt = build_init_prompt(ctx, None, false);
        assert!(prompt.contains("Project Context"));
        assert!(prompt.contains("A Rust project"));
        assert!(prompt.contains("Read key source files"));
        assert!(prompt.contains("Build / Test / Lint"));
        assert!(prompt.contains("Architecture"));
        assert!(prompt.contains("AGENTS.md"));
    }

    #[test]
    fn build_init_prompt_with_existing_content() {
        let ctx = "## Project Summary\n\nA Rust project\n";
        let existing = "# Old AGENTS.md content";
        let prompt = build_init_prompt(ctx, Some(existing), true);
        assert!(prompt.contains("Old AGENTS.md content"));
        assert!(prompt.contains("Update it in place"));
    }

    #[test]
    fn build_init_prompt_new_file_no_update_instruction() {
        let ctx = "## Project Summary\n\nA Rust project\n";
        let prompt = build_init_prompt(ctx, None, false);
        assert!(!prompt.contains("The file already exists"));
    }

    // --- js framework detection ---

    #[test]
    fn detect_js_frameworks_react() {
        let deps = ["react", "react-dom", "vite"];
        let frameworks = detect_js_frameworks(&deps);
        assert!(frameworks.contains(&"React".to_string()));
        assert!(frameworks.contains(&"Vite".to_string()));
    }

    #[test]
    fn detect_js_frameworks_none() {
        let deps = ["lodash", "axios"];
        assert!(detect_js_frameworks(&deps).is_empty());
    }

    // --- ensure_deepseek_gitignored (preserved tests) ---

    #[test]
    fn ensure_deepseek_gitignored_creates_gitignore() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        ensure_deepseek_gitignored(tmpdir.path());
        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(content.contains(".deepseek/"));
        // .codewhale/ is ignored at any depth, but the committed
        // constitution.json is kept.
        assert!(content.contains("**/.codewhale/*"));
        assert!(content.contains("!**/.codewhale/constitution.json"));
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
        ensure_deepseek_gitignored(tmpdir.path());
        assert!(!tmpdir.path().join(".gitignore").exists());
    }

    #[test]
    fn ensure_deepseek_gitignored_handles_no_trailing_newline() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        std::fs::write(tmpdir.path().join(".gitignore"), "target/").unwrap();
        ensure_deepseek_gitignored(tmpdir.path());
        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert!(content.contains("target/"));
        assert!(content.contains(".deepseek/"));
        let lines: Vec<&str> = content.lines().collect();
        assert!(lines.len() >= 2);
    }

    #[test]
    fn ensure_deepseek_gitignored_detects_variant_without_slash() {
        let tmpdir = TempDir::new().unwrap();
        std::fs::create_dir_all(tmpdir.path().join(".git")).unwrap();
        std::fs::write(tmpdir.path().join(".gitignore"), ".deepseek\n").unwrap();
        ensure_deepseek_gitignored(tmpdir.path());
        let content = std::fs::read_to_string(tmpdir.path().join(".gitignore")).unwrap();
        assert_eq!(content.matches(".deepseek").count(), 1);
    }
}
