//! Plugin tool system — scripts and commands as first-class tools.
//!
//! Users can drop self-describing scripts in `~/.codewhale/tools/` and they
//! are auto-discovered, parsed for frontmatter, and registered as model-visible
//! tools alongside built-in implementations.
//!
//! # Script frontmatter format
//!
//! Every plugin script must have a frontmatter header in its first 20 lines:
//!
//! ```sh
//! # name: my-tool
//! # description: Does something useful
//! # schema: {"type":"object","properties":{"input":{"type":"string"}}}
//! # approval: auto
//! ```
//!
//! The script receives the tool's JSON input on **stdin** and must return
//! a JSON `ToolResult` (`{"content": "...", "success": true}`) on **stdout**.
//! Non-JSON output is wrapped in a `ToolResult` with `success: false`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;
use tokio::io::AsyncWriteExt;

use super::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

use crate::config::ToolOverride;

/// Timeout for plugin script execution (120 seconds).
const PLUGIN_EXECUTION_TIMEOUT: Duration = Duration::from_secs(120);

/// Metadata extracted from a plugin script's frontmatter header.
#[derive(Debug, Clone)]
pub struct PluginMetadata {
    /// Tool name (from `# name:`).
    pub name: String,
    /// Human-readable description (from `# description:`).
    pub description: String,
    /// JSON Schema for the tool's input (from `# schema:`).
    /// Defaults to a permissive `{"type": "object"}` when absent.
    pub input_schema: Value,
    /// Approval requirement (from `# approval:`).
    /// Defaults to `Suggest`.
    pub approval: ApprovalRequirement,
}

/// A tool backed by an external script or executable dropped into the
/// plugins directory. The script receives JSON input on stdin and writes
/// a JSON `ToolResult` to stdout.
struct ScriptPluginTool {
    metadata: PluginMetadata,
    /// Absolute path to the script.
    script_path: PathBuf,
    /// Optional static arguments passed before the JSON input.
    args: Vec<String>,
}

impl std::fmt::Debug for ScriptPluginTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptPluginTool")
            .field("name", &self.metadata.name)
            .field("script_path", &self.script_path)
            .finish()
    }
}

#[async_trait]
impl ToolSpec for ScriptPluginTool {
    fn name(&self) -> &str {
        &self.metadata.name
    }

    fn description(&self) -> &str {
        &self.metadata.description
    }

    fn input_schema(&self) -> Value {
        self.metadata.input_schema.clone()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        // Unknown plugin — conservative: mark as requiring execution + approval.
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        self.metadata.approval
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let (interpreter, script_args) = script_command_parts(&self.script_path, &self.args);
        let label = self.script_path.display().to_string();
        run_plugin_child(&interpreter, &script_args, &label, input).await
    }
}

/// A tool backed by an arbitrary shell command from config.toml overrides.
/// Behaves like `ScriptPluginTool` but uses the user-specified command string.
struct CommandPluginTool {
    name: String,
    description: String,
    input_schema: Value,
    command: String,
    args: Vec<String>,
    approval: ApprovalRequirement,
}

impl std::fmt::Debug for CommandPluginTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandPluginTool")
            .field("name", &self.name)
            .field("command", &self.command)
            .finish()
    }
}

#[async_trait]
impl ToolSpec for CommandPluginTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> Value {
        self.input_schema.clone()
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![
            ToolCapability::ExecutesCode,
            ToolCapability::RequiresApproval,
        ]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        self.approval
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        // On Windows, if the command doesn't have an extension, try wrapping
        // in `cmd /c` or use `powershell` for `.ps1` files. For portability
        // we let tokio::process::Command resolve via PATH.
        let mut cmd = if cfg!(windows) && !self.command.contains('.') {
            let mut c = tokio::process::Command::new("cmd");
            c.arg("/c").arg(&self.command);
            c
        } else {
            tokio::process::Command::new(&self.command)
        };
        cmd.args(&self.args);
        let label = format!("command '{}'", self.command);
        run_plugin_child_raw(&mut cmd, &label, input).await
    }
}

// ---------------------------------------------------------------------------
// Script interpreter resolution
// ---------------------------------------------------------------------------

/// Parse a shebang line (`#!/usr/bin/env node`) to extract the interpreter.
fn parse_shebang(path: &Path) -> Option<(String, Vec<String>)> {
    let mut file = std::fs::File::open(path).ok()?;
    let content = read_prefix_to_string(&mut file, 256)?;
    let first_line = content.lines().next()?;
    let rest = first_line.strip_prefix("#!")?;
    let parts: Vec<&str> = rest.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }
    let interpreter = parts[0].to_string();
    let args: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
    Some((interpreter, args))
}

/// Resolve the interpreter binary and pre-args for a script file.
///
/// Priority:
/// 1. Shebang line from the script itself (`#!/usr/bin/env node`)
/// 2. Extension-based fallback for known script types
/// 3. Direct execution (assumes the OS knows how to run it)
fn resolve_interpreter(path: &Path) -> (String, Vec<String>) {
    // 1. Try shebang
    if let Some((interp, shebang_args)) = parse_shebang(path) {
        let bin_name = interp.rsplit('/').next().unwrap_or(&interp);
        // `env` is a special case: `#!/usr/bin/env node` → `node`
        // On Windows, `env` is not available, so extract the intended binary.
        if bin_name == "env" && !shebang_args.is_empty() {
            return (shebang_args[0].clone(), shebang_args[1..].to_vec());
        }
        if cfg!(windows) {
            return (bin_name.to_string(), shebang_args);
        }
        return (interp, shebang_args);
    }

    // 2. Extension-based fallback for common script types
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "ps1" => ("powershell".into(), vec!["-File".into()]),
        "py" => ("python".into(), vec![]),
        "js" | "mjs" => ("node".into(), vec![]),
        "ts" => ("npx".into(), vec!["tsx".into()]),
        "rb" => ("ruby".into(), vec![]),
        "sh" | "bash" | "zsh" => {
            // On Windows, route shell scripts through sh if available
            if cfg!(windows) {
                ("sh".into(), vec![])
            } else {
                (path.to_string_lossy().into(), vec![])
            }
        }
        _ => (path.to_string_lossy().into(), vec![]),
    }
}

fn script_command_parts(script_path: &Path, args: &[String]) -> (String, Vec<String>) {
    let (interpreter, mut script_args) = resolve_interpreter(script_path);
    let script_path_arg = script_path.to_string_lossy().to_string();
    if interpreter != script_path_arg {
        script_args.push(script_path_arg);
    }
    script_args.extend(args.iter().cloned());
    (interpreter, script_args)
}

fn read_prefix_to_string(reader: impl std::io::Read, max_bytes: u64) -> Option<String> {
    use std::io::Read;

    let mut buf = Vec::new();
    reader.take(max_bytes).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

// ---------------------------------------------------------------------------
// Shared child process helpers
// ---------------------------------------------------------------------------

/// Spawn a command, pipe JSON input to stdin, collect ToolResult from stdout.
async fn run_plugin_child(
    command: &str,
    args: &[String],
    label: &str,
    input: Value,
) -> Result<ToolResult, ToolError> {
    let mut cmd = tokio::process::Command::new(command);
    cmd.args(args);
    run_plugin_child_raw(&mut cmd, label, input).await
}

/// Run a pre-configured tokio Command, pipe JSON input, collect ToolResult.
async fn run_plugin_child_raw(
    cmd: &mut tokio::process::Command,
    label: &str,
    input: Value,
) -> Result<ToolResult, ToolError> {
    let input_bytes = serde_json::to_vec(&input)
        .map_err(|e| ToolError::invalid_input(format!("failed to serialize input: {e}")))?;

    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| ToolError::execution_failed(format!("failed to spawn {label}: {e}")))?;

    let stdin_writer = child.stdin.take().map(|mut stdin| {
        tokio::spawn(async move {
            if stdin.write_all(&input_bytes).await.is_ok() {
                let _ = stdin.shutdown().await;
            }
        })
    });

    let output = tokio::time::timeout(PLUGIN_EXECUTION_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ToolError::Timeout {
            seconds: PLUGIN_EXECUTION_TIMEOUT.as_secs(),
        })?
        .map_err(|e| ToolError::execution_failed(format!("process error: {e}")))?;

    if let Some(stdin_writer) = stdin_writer {
        let _ = stdin_writer.await;
    }

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        if let Ok(parsed) = serde_json::from_str::<ToolResult>(&stdout) {
            Ok(parsed)
        } else {
            Ok(ToolResult::success(stdout))
        }
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let combined = if stderr.is_empty() {
            stdout
        } else if stdout.is_empty() {
            stderr
        } else {
            format!("{stdout}\n{stderr}")
        };
        Err(ToolError::execution_failed(combined))
    }
}

// ---------------------------------------------------------------------------
// Frontmatter parsing
// ---------------------------------------------------------------------------

/// Parse frontmatter header from the first `max_lines` lines of a text file.
///
/// Expected format (one `# key: value` per line):
/// ```text
/// # name: my-tool
/// # description: Does something
/// # schema: {"type":"object"}
/// # approval: auto
/// ```
///
/// Also supports `// ` prefix for JavaScript/TypeScript scripts and `-- ` for Lua.
pub fn parse_frontmatter(content: &str) -> PluginMetadata {
    let mut name = String::new();
    let mut description = String::new();
    let mut schema_str = String::new();
    let mut approval_str = String::new();

    for line in content.lines().take(20) {
        let line = line.trim();
        // Strip leading comment markers: `#`, `//`, `--`.
        let rest = line
            .strip_prefix('#')
            .or_else(|| line.strip_prefix("//"))
            .or_else(|| line.strip_prefix("--"));
        let Some(rest) = rest else { continue };
        if let Some((key, value)) = rest.trim_start().split_once(':') {
            let key = key.trim().to_lowercase();
            let value = value.trim();
            match key.as_str() {
                "name" => name = value.to_string(),
                "description" => description = value.to_string(),
                "schema" => schema_str = value.to_string(),
                "approval" => approval_str = value.to_string(),
                _ => {}
            }
        }
    }

    let input_schema = if schema_str.is_empty() {
        // Default: accept any object payload
        serde_json::json!({"type": "object"})
    } else {
        serde_json::from_str(&schema_str).unwrap_or_else(|_| serde_json::json!({"type": "object"}))
    };

    let approval = match approval_str.to_lowercase().as_str() {
        "auto" => ApprovalRequirement::Auto,
        "required" => ApprovalRequirement::Required,
        _ => ApprovalRequirement::Suggest,
    };

    PluginMetadata {
        name: if name.is_empty() {
            "unnamed-plugin".to_string()
        } else {
            name
        },
        description: if description.is_empty() {
            "User-provided plugin tool".to_string()
        } else {
            description
        },
        input_schema,
        approval,
    }
}

/// Read the first 4 KB of a file and parse its frontmatter.
fn read_script_metadata(path: &Path) -> Option<PluginMetadata> {
    let mut file = std::fs::File::open(path).ok()?;
    let content = read_prefix_to_string(&mut file, 4096)?;
    let meta = parse_frontmatter(&content);
    // Require at least the `name` field to consider it a valid plugin.
    if meta.name == "unnamed-plugin" {
        return None;
    }
    Some(meta)
}

// ---------------------------------------------------------------------------
// Directory scanning
// ---------------------------------------------------------------------------

/// Scan a directory for plugin script files with frontmatter headers.
///
/// Files are considered eligible when:
/// - They are regular files (not directories, not symlinks)
/// - They don't start with `.` (hidden files)
/// - They are not `README.md`
/// - Their first 20 lines contain `# name:` frontmatter
pub fn scan_plugin_dir(dir: &Path) -> Vec<(PathBuf, PluginMetadata)> {
    let mut results = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            tracing::warn!("Failed to read plugin directory {}: {e}", dir.display());
            return results;
        }
    };

    let mut entries: Vec<_> = entries.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();

        // Skip directories and hidden files
        if path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.starts_with('.') || name == "README.md" {
                continue;
            }
        }

        // Try to parse frontmatter
        if let Some(meta) = read_script_metadata(&path) {
            results.push((path, meta));
        }
    }

    results
}

/// Load all plugin tools from a directory. Each eligible script becomes
/// a registered `ScriptPluginTool`.
pub fn load_plugin_tools(plugin_dir: &Path) -> Vec<Arc<dyn ToolSpec>> {
    let discovered = scan_plugin_dir(plugin_dir);
    let mut tools: Vec<Arc<dyn ToolSpec>> = Vec::with_capacity(discovered.len());

    for (path, meta) in discovered {
        tracing::info!(
            "Discovered plugin tool '{}' at {}",
            meta.name,
            path.display()
        );
        tools.push(Arc::new(ScriptPluginTool {
            metadata: meta,
            script_path: path,
            args: Vec::new(),
        }));
    }

    tools
}

/// Create a single tool from a `ToolOverride` config entry.
///
/// Returns `None` for `Disabled` (the caller handles removal separately).
pub fn tool_from_override(
    tool_name: &str,
    override_cfg: &ToolOverride,
    plugin_dir: &Path,
) -> Option<Arc<dyn ToolSpec>> {
    match override_cfg {
        ToolOverride::Disabled => None,
        ToolOverride::Script { path, args } => {
            let script_path = if Path::new(path).is_absolute() {
                PathBuf::from(path)
            } else {
                // Relative paths resolve relative to the plugin directory.
                plugin_dir.join(path)
            };

            if !script_path.exists() {
                tracing::warn!(
                    "Override script for '{}' not found at {}",
                    tool_name,
                    script_path.display()
                );
                return None;
            }

            // Read the script's own frontmatter for metadata, or provide
            // defaults if it has none.
            let meta = read_script_metadata(&script_path).unwrap_or_else(|| PluginMetadata {
                name: tool_name.to_string(),
                description: format!("Override for built-in tool '{tool_name}'"),
                input_schema: serde_json::json!({"type": "object"}),
                approval: ApprovalRequirement::Suggest,
            });

            Some(Arc::new(ScriptPluginTool {
                metadata: meta,
                script_path,
                args: args.clone().unwrap_or_default(),
            }) as Arc<dyn ToolSpec>)
        }
        ToolOverride::Command { command, args } => {
            // Build a description that includes the command.
            let description = format!("Override for '{tool_name}' — runs: {command}");
            let cmd_args = args.clone().unwrap_or_default();

            Some(Arc::new(CommandPluginTool {
                name: tool_name.to_string(),
                description,
                input_schema: serde_json::json!({"type": "object"}),
                command: command.clone(),
                args: cmd_args,
                approval: ApprovalRequirement::Suggest,
            }) as Arc<dyn ToolSpec>)
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    const DEADLOCK_CHILD_ENV: &str = "CODEWHALE_PLUGIN_DEADLOCK_CHILD";

    #[test]
    fn test_parse_frontmatter_full() {
        let content = "\
#!/usr/bin/env sh
# name: my-tool
# description: A useful custom tool
# schema: {\"type\":\"object\",\"properties\":{\"input\":{\"type\":\"string\"}}}
# approval: required
echo hello
";
        let meta = parse_frontmatter(content);
        assert_eq!(meta.name, "my-tool");
        assert_eq!(meta.description, "A useful custom tool");
        assert_eq!(meta.approval, ApprovalRequirement::Required);
        assert_eq!(
            meta.input_schema,
            serde_json::json!({"type":"object","properties":{"input":{"type":"string"}}})
        );
    }

    #[test]
    fn test_parse_frontmatter_accepts_compact_and_spaced_markers() {
        let content = "\
#!/usr/bin/env node
#name:compact-name
//  description:  spaced description
-- schema : {\"type\":\"object\",\"properties\":{\"ok\":{\"type\":\"boolean\"}}}
# approval: auto
";

        let meta = parse_frontmatter(content);

        assert_eq!(meta.name, "compact-name");
        assert_eq!(meta.description, "spaced description");
        assert_eq!(meta.approval, ApprovalRequirement::Auto);
        assert_eq!(
            meta.input_schema,
            serde_json::json!({"type":"object","properties":{"ok":{"type":"boolean"}}})
        );
    }

    #[test]
    fn test_parse_frontmatter_minimal() {
        let content = "# name: mini";
        let meta = parse_frontmatter(content);
        assert_eq!(meta.name, "mini");
        assert_eq!(meta.description, "User-provided plugin tool");
        assert_eq!(meta.approval, ApprovalRequirement::Suggest);
    }

    #[test]
    fn test_parse_frontmatter_missing_name() {
        let content = "# description: no name here";
        let meta = parse_frontmatter(content);
        assert_eq!(meta.name, "unnamed-plugin");
        // read_script_metadata would return None for this.
    }

    #[test]
    fn test_read_prefix_collects_multiple_short_reads() {
        struct OneByteReader {
            bytes: Vec<u8>,
            pos: usize,
        }

        impl std::io::Read for OneByteReader {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.pos >= self.bytes.len() {
                    return Ok(0);
                }
                buf[0] = self.bytes[self.pos];
                self.pos += 1;
                Ok(1)
            }
        }

        let reader = OneByteReader {
            bytes: b"# name: short-read\n# description: ok\n".to_vec(),
            pos: 0,
        };

        assert_eq!(
            read_prefix_to_string(reader, 4096).as_deref(),
            Some("# name: short-read\n# description: ok\n")
        );
    }

    #[test]
    fn test_resolve_interpreter_handles_absolute_shebang_by_platform() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("tool");
        std::fs::write(
            &script,
            "#!/opt/custom/bin/tool-runner --safe\n# name: tool\n",
        )
        .unwrap();

        let (interpreter, args) = resolve_interpreter(&script);

        if cfg!(windows) {
            assert_eq!(interpreter, "tool-runner");
        } else {
            assert_eq!(interpreter, "/opt/custom/bin/tool-runner");
        }
        assert_eq!(args, vec!["--safe"]);
    }

    #[test]
    fn test_script_command_parts_does_not_pass_direct_script_as_own_arg() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("direct-tool");
        std::fs::write(&script, "# name: direct\n").unwrap();

        let (interpreter, args) =
            script_command_parts(&script, &["--flag".to_string(), "value".to_string()]);

        assert_eq!(interpreter, script.to_string_lossy());
        assert_eq!(args, vec!["--flag", "value"]);
    }

    #[test]
    fn test_script_command_parts_passes_script_to_external_interpreter() {
        let dir = TempDir::new().unwrap();
        let script = dir.path().join("script.py");
        std::fs::write(&script, "# name: py\n").unwrap();

        let (interpreter, args) = script_command_parts(&script, &["--flag".to_string()]);

        assert_eq!(interpreter, "python");
        assert_eq!(
            args,
            vec![script.to_string_lossy().to_string(), "--flag".to_string()]
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_run_plugin_child_drains_stdout_while_writing_large_stdin() {
        let mut cmd = tokio::process::Command::new(std::env::current_exe().unwrap());
        cmd.arg("plugin_deadlock_child_process")
            .arg("--nocapture")
            .env(DEADLOCK_CHILD_ENV, "1");

        let input = serde_json::json!({ "payload": "y".repeat(1024 * 1024) });
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            run_plugin_child_raw(&mut cmd, "deadlock child", input),
        )
        .await
        .expect("plugin execution should not deadlock")
        .expect("plugin child should succeed");

        assert!(result.success);
        assert!(result.content.len() > 64 * 1024);
    }

    #[test]
    fn plugin_deadlock_child_process() {
        if std::env::var_os(DEADLOCK_CHILD_ENV).is_none() {
            return;
        }

        use std::io::{Read, Write};

        let mut stdout = std::io::stdout();
        stdout.write_all(&vec![b'x'; 1024 * 1024]).unwrap();
        stdout.flush().unwrap();

        let mut stdin = Vec::new();
        std::io::stdin().read_to_end(&mut stdin).unwrap();
        writeln!(
            stdout,
            "{{\"content\":\"read {} bytes\",\"success\":true}}",
            stdin.len()
        )
        .unwrap();
        std::process::exit(0);
    }

    #[test]
    fn test_scan_plugin_dir_finds_scripts() {
        let dir = TempDir::new().unwrap();

        // Valid plugin
        std::fs::write(
            dir.path().join("my-plugin.sh"),
            "# name: my-plugin\n# description: test\n",
        )
        .unwrap();

        // Hidden file — should be skipped
        std::fs::write(
            dir.path().join(".hidden.sh"),
            "# name: hidden\n# description: should skip\n",
        )
        .unwrap();

        // README — should be skipped
        std::fs::write(dir.path().join("README.md"), "# Tools\n").unwrap();

        // No frontmatter — should be skipped
        std::fs::write(dir.path().join("random.sh"), "echo hi\n").unwrap();

        let discovered = scan_plugin_dir(dir.path());
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].1.name, "my-plugin");
    }

    #[test]
    fn test_scan_plugin_dir_returns_files_sorted_by_name() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("z-plugin.sh"),
            "# name: z-plugin\n# description: z\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("a-plugin.sh"),
            "# name: a-plugin\n# description: a\n",
        )
        .unwrap();

        let discovered = scan_plugin_dir(dir.path());

        let names: Vec<_> = discovered
            .iter()
            .map(|(_, meta)| meta.name.as_str())
            .collect();
        assert_eq!(names, vec!["a-plugin", "z-plugin"]);
    }

    #[test]
    fn test_load_plugin_tools_creates_tools() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("greet.sh"),
            "# name: greet\n# description: Say hello\n# schema: {\"type\":\"object\",\"properties\":{\"name\":{\"type\":\"string\"}},\"required\":[\"name\"]}\n",
        )
        .unwrap();

        let tools = load_plugin_tools(dir.path());
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name(), "greet");
        assert_eq!(tools[0].description(), "Say hello");
    }

    #[test]
    fn test_tool_from_override_script() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("wrapper.sh"),
            "# name: exec_shell\n# description: Audit wrapper for exec_shell\n",
        )
        .unwrap();

        let override_cfg = ToolOverride::Script {
            path: "wrapper.sh".to_string(),
            args: None,
        };

        let tool = tool_from_override("exec_shell", &override_cfg, dir.path());
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "exec_shell");
    }

    #[test]
    fn test_tool_from_override_disabled() {
        let dir = TempDir::new().unwrap();
        let override_cfg = ToolOverride::Disabled;
        let tool = tool_from_override("code_execution", &override_cfg, dir.path());
        assert!(tool.is_none());
    }

    #[test]
    fn test_tool_from_override_command() {
        let dir = TempDir::new().unwrap();
        let override_cfg = ToolOverride::Command {
            command: "my-custom-reader".to_string(),
            args: Some(vec!["--format".to_string(), "json".to_string()]),
        };
        let tool = tool_from_override("read_file", &override_cfg, dir.path());
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "read_file");
    }

    #[test]
    fn test_tool_from_override_script_absolute_path() {
        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("audit.sh");
        std::fs::write(&script_path, "# name: exec_shell\n# description: Audit\n").unwrap();

        let override_cfg = ToolOverride::Script {
            path: script_path.to_str().unwrap().to_string(),
            args: None,
        };

        let tool = tool_from_override("exec_shell", &override_cfg, dir.path());
        assert!(tool.is_some());
    }

    #[test]
    fn test_approval_variants() {
        let check = |content: &str, expected: ApprovalRequirement| {
            assert_eq!(parse_frontmatter(content).approval, expected);
        };

        check("# name: x\n# approval: auto", ApprovalRequirement::Auto);
        check(
            "# name: x\n# approval: required",
            ApprovalRequirement::Required,
        );
        check(
            "# name: x\n# approval: suggest",
            ApprovalRequirement::Suggest,
        );
        check(
            "# name: x\n# approval: unknown",
            ApprovalRequirement::Suggest,
        );
        check("# name: x", ApprovalRequirement::Suggest);
    }
}
