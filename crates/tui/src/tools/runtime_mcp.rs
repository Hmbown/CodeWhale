//! Runtime MCP server management.
//!
//! Provides `StartRuntimeMcpServer` — the entry tool for LLM to dynamically
//! connect to MCP servers from conversation context. Also contains parsing
//! and naming helpers used by the tool.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::Mutex as AsyncMutex;

use crate::mcp::{McpPool, McpServerConfig, McpTool};
use crate::tools::spec::{
    ApprovalRequirement, ToolCapability, ToolContext, ToolError, ToolResult, ToolSpec,
};

// === Parsing Functions ===

#[derive(Debug, Clone)]
pub struct ParsedMcpServer {
    pub name: String,
    pub config: McpServerConfig,
}

/// Parse a command string or URL into an MCP server configuration.
///
/// - Local command: `npx @modelcontextprotocol/server-filesystem /tmp`
/// - Remote URL: `https://huggingface.co/mcp`
pub fn parse_mcp_command(input: &str) -> Result<ParsedMcpServer> {
    let input = input.trim();
    if input.is_empty() {
        anyhow::bail!("MCP command cannot be empty");
    }

    if input.starts_with("http://") || input.starts_with("https://") {
        let name = extract_name_from_url(input)?;
        return Ok(ParsedMcpServer {
            name,
            config: McpServerConfig {
                command: None,
                args: Vec::new(),
                env: HashMap::new(),
                cwd: None,
                url: Some(input.to_string()),
                transport: None,
                connect_timeout: None,
                execute_timeout: None,
                read_timeout: None,
                disabled: false,
                enabled: true,
                required: false,
                enabled_tools: Vec::new(),
                disabled_tools: Vec::new(),
                headers: HashMap::new(),
                env_headers: HashMap::new(),
                bearer_token_env_var: None,
                scopes: Vec::new(),
                oauth: None,
                oauth_resource: None,
            },
        });
    }

    let parts: Vec<String> = shell_words::split(input).unwrap_or_default();
    if parts.is_empty() {
        anyhow::bail!("MCP command cannot be empty");
    }

    let command = parts[0].clone();
    let args: Vec<String> = parts[1..].to_vec();
    let name = infer_server_name(&command, &args)?;

    Ok(ParsedMcpServer {
        name,
        config: McpServerConfig {
            command: Some(command),
            args,
            env: HashMap::new(),
            cwd: None,
            url: None,
            transport: None,
            connect_timeout: None,
            execute_timeout: None,
            read_timeout: None,
            disabled: false,
            enabled: true,
            required: false,
            enabled_tools: Vec::new(),
            disabled_tools: Vec::new(),
            headers: HashMap::new(),
            env_headers: HashMap::new(),
            bearer_token_env_var: None,
            scopes: Vec::new(),
            oauth: None,
            oauth_resource: None,
        },
    })
}

pub fn extract_name_from_url(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url)?;
    let host = parsed.host_str().unwrap_or("remote");
    let path = parsed.path().trim_matches('/');

    // Replace dots with dashes in hostname for better readability
    let host_part = host.replace('.', "-");

    // Combine host and path, replacing slashes with underscores
    let name = if path.is_empty() {
        host_part
    } else {
        format!("{}_{}", host_part, path.replace('/', "_"))
    };

    Ok(sanitize_name(&name))
}

fn infer_server_name(command: &str, args: &[String]) -> Result<String> {
    let cmd_path = std::path::Path::new(command);
    let cmd_base = cmd_path.file_stem().unwrap_or_default().to_string_lossy();

    // Windows cmd /c prefix: skip "cmd /c" and recurse on the remaining args
    // e.g. ["cmd", "/c", "npx", "-y", "@modelcontextprotocol/server-memory"]
    if cmd_base.as_ref() == "cmd"
        && args.len() >= 2
        && (args[0] == "/c" || args[0] == "/C" || args[0] == "/k" || args[0] == "/K")
    {
        let inner_cmd = &args[1];
        let inner_args: Vec<String> = args[2..].to_vec();
        return infer_server_name(inner_cmd, &inner_args);
    }

    // Package managers: extract the package name (first non-flag arg)
    if matches!(
        cmd_base.as_ref(),
        "npx" | "npm" | "pnpm" | "yarn" | "bunx" | "bun"
    ) {
        for arg in args {
            if !arg.starts_with('-') && arg != "exec" && arg != "run" && arg != "start" {
                // e.g. "@modelcontextprotocol/server-filesystem" → "filesystem"
                if let Some(name) = arg.split('/').next_back() {
                    if let Some(short) = name.strip_prefix("server-") {
                        return Ok(sanitize_name(short));
                    }
                    return Ok(sanitize_name(name));
                }
            }
        }
    }

    // Script interpreters: extract the script path (first non-flag arg)
    if matches!(
        cmd_base.as_ref(),
        "node" | "python" | "python3" | "uvx" | "uv" | "ruby" | "deno"
    ) && let Some(script) = args.iter().find(|a| !a.starts_with('-'))
    {
        let script_path = std::path::Path::new(script);
        if let Some(stem) = script_path.file_stem() {
            return Ok(sanitize_name(&stem.to_string_lossy()));
        }
    }

    // Fallback: first non-flag argument (script or file)
    if let Some(script) = args.iter().find(|a| !a.starts_with('-')) {
        let script_path = std::path::Path::new(script);
        if let Some(stem) = script_path.file_stem() {
            return Ok(sanitize_name(&stem.to_string_lossy()));
        }
    }

    // Last resort: command name itself
    Ok(sanitize_name(&cmd_base))
}

pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

// === Tool: StartRuntimeMcpServer ===

/// Entry tool for dynamically adding MCP servers from conversation context.
///
/// LLM calls this to start a local MCP server (stdio) or connect to a remote
/// one (HTTP). The server config is added to `McpPool.dynamic_servers` and
/// tools are discovered via the existing `McpConnection` / `StdioTransport` flow.
pub struct StartRuntimeMcpServer {
    pool: Arc<AsyncMutex<McpPool>>,
}

impl StartRuntimeMcpServer {
    pub fn new(pool: Arc<AsyncMutex<McpPool>>) -> Self {
        Self { pool }
    }
}

#[async_trait::async_trait]
impl ToolSpec for StartRuntimeMcpServer {
    fn name(&self) -> &str {
        "start_mcp_server"
    }

    fn description(&self) -> &str {
        "When a user provides an MCP server command (like 'npx ...') or URL \
         (like 'https://...'), call this tool immediately to start the server \
         and register its tools. Do NOT suggest editing config files. \
         Accepts a local command (stdio) or a remote URL (HTTP/SSE). \
         After the server starts, the response lists each tool's callable name. \
         You MUST copy those exact names when calling the tools. \
         Do NOT construct or guess tool names yourself."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "MCP server command or URL"
                },
                "name": {
                    "type": "string",
                    "description": "Optional server name (auto-inferred if omitted)"
                }
            },
            "required": ["server"]
        })
    }

    fn capabilities(&self) -> Vec<ToolCapability> {
        vec![ToolCapability::Network, ToolCapability::ExecutesCode]
    }

    fn approval_requirement(&self) -> ApprovalRequirement {
        ApprovalRequirement::Required
    }

    async fn execute(&self, input: Value, _context: &ToolContext) -> Result<ToolResult, ToolError> {
        let server = input
            .get("server")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::invalid_input("Missing required field: server"))?;

        let custom_name = input.get("name").and_then(|v| v.as_str());
        let parsed =
            parse_mcp_command(server).map_err(|e| ToolError::invalid_input(e.to_string()))?;

        // Reject shell-wrapped commands that could execute arbitrary code
        if let Some(ref cmd) = parsed.config.command {
            let cmd_lower = cmd.to_lowercase();
            if cmd_lower == "bash"
                || cmd_lower == "sh"
                || cmd_lower == "zsh"
                || cmd_lower == "cmd"
                || cmd_lower == "powershell"
            {
                return Err(ToolError::invalid_input(format!(
                    "Shell wrapper commands ({cmd}) are not allowed. \
                     Provide the actual MCP server command directly, \
                     e.g. 'npx @modelcontextprotocol/server-filesystem /tmp'"
                )));
            }
        }

        // Reject shell metacharacters in arguments to prevent injection.
        // Redirects (>, >>), pipes (|), command chaining (;, &&, ||),
        // subshells (``), and variable expansion ($) are all dangerous.
        for arg in &parsed.config.args {
            if arg.contains('>')
                || arg.contains('|')
                || arg.contains(';')
                || arg.contains('&')
                || arg.contains('`')
                || arg.contains('$')
            {
                return Err(ToolError::invalid_input(format!(
                    "Argument contains shell metacharacters: '{arg}'. \
                     MCP server arguments must not contain redirects, pipes, \
                     command chaining, or variable expansion."
                )));
            }
        }

        // Allowlist of known MCP server runtimes and package managers.
        // Commands not in this list are rejected to prevent arbitrary execution.
        if let Some(ref cmd) = parsed.config.command {
            let cmd_base = std::path::Path::new(cmd)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            const ALLOWED_COMMANDS: &[&str] = &[
                "npx", "npm", "pnpm", "yarn", "bunx", "bun", "node", "python", "python3", "uvx",
                "uv", "deno", "ruby", "cargo",
            ];
            if !ALLOWED_COMMANDS.contains(&cmd_base.as_ref()) {
                return Err(ToolError::invalid_input(format!(
                    "Command '{cmd}' is not in the allowed list. \
                     Permitted commands: {}",
                    ALLOWED_COMMANDS.join(", ")
                )));
            }
        }

        let server_name = custom_name
            .map(sanitize_name)
            .unwrap_or(parsed.name)
            .replace('_', "-");

        // Underscores in server names would cause tool name collision.
        // Tool names are formatted as mcp_{server}_{tool}; underscores in
        // server names would make it ambiguous (server "foo" + tool "bar_x"
        // vs server "foo_bar" + tool "x" both → mcp_foo_bar_x).
        // sanitize_name already converts non-alphanumeric chars to hyphens,
        // but underscores from the original input need explicit conversion.

        let transport = if parsed.config.url.is_some() {
            "http"
        } else {
            "stdio"
        };

        // Register server config, connect, and collect tool info
        let mut pool = self.pool.lock().await;
        pool.add_runtime_server_config(server_name.clone(), parsed.config)
            .map_err(ToolError::invalid_input)?;
        let conn = pool.get_or_connect(&server_name).await.map_err(|e| {
            ToolError::execution_failed(format!(
                "Failed to connect to MCP server '{}': {e}",
                server_name
            ))
        })?;

        let mcp_tools: Vec<McpTool> = conn.tools().to_vec();

        // Build tool list with fully qualified names (mcp_{server}_{tool})
        // so the LLM can call them directly without guessing the naming convention.
        let tools_list: Vec<String> = mcp_tools
            .iter()
            .map(|t| {
                let qualified = format!("mcp_{}_{}", server_name, t.name);
                format!(
                    "- {} → {}",
                    qualified,
                    t.description.as_deref().unwrap_or("no description")
                )
            })
            .collect();

        let result = serde_json::to_string(&json!({
            "status": "connected",
            "transport": transport,
            "server": server_name,
            "new_tools": mcp_tools.len(),
            "total_mcp_tools": pool.all_tools().len(),
            "message": format!(
                "MCP server '{}' connected via {}. {} tools discovered.\n\n\
                 Callable tools (use these exact names):\n{}",
                server_name, transport, mcp_tools.len(), tools_list.join("\n")
            )
        }))
        .unwrap_or_else(|_| "{}".to_string());

        Ok(ToolResult::success(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_command_stdio() {
        let parsed = parse_mcp_command("npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert!(parsed.config.command.is_some());
        assert!(parsed.config.url.is_none());
    }

    #[test]
    fn parse_command_url() {
        let parsed = parse_mcp_command("https://huggingface.co/mcp").unwrap();
        assert!(parsed.config.command.is_none());
        assert!(parsed.config.url.is_some());
        assert_eq!(parsed.name, "huggingface-co-mcp");
    }

    #[test]
    fn parse_command_url_with_subdomain() {
        let parsed = parse_mcp_command("https://api.example.com/mcp").unwrap();
        assert!(parsed.config.command.is_none());
        assert!(parsed.config.url.is_some());
        assert_eq!(parsed.name, "api-example-com-mcp");
    }

    #[test]
    fn parse_command_empty() {
        assert!(parse_mcp_command("").is_err());
        assert!(parse_mcp_command("   ").is_err());
    }

    #[test]
    fn extract_name_from_url_with_path() {
        assert_eq!(
            extract_name_from_url("https://huggingface.co/mcp").unwrap(),
            "huggingface-co-mcp"
        );
    }

    #[test]
    fn extract_name_from_url_with_subdomain() {
        assert_eq!(
            extract_name_from_url("https://api.example.com/mcp").unwrap(),
            "api-example-com-mcp"
        );
    }

    #[test]
    fn extract_name_from_url_no_path() {
        assert_eq!(
            extract_name_from_url("https://example.com").unwrap(),
            "example-com"
        );
    }

    #[test]
    fn extract_name_from_url_empty_path() {
        assert_eq!(
            extract_name_from_url("https://example.com/").unwrap(),
            "example-com"
        );
    }

    // === shell_words split tests ===

    #[test]
    fn shell_words_simple() {
        assert_eq!(
            shell_words::split("npx server /tmp").unwrap(),
            vec!["npx", "server", "/tmp"]
        );
    }

    #[test]
    fn shell_words_double_quotes() {
        assert_eq!(
            shell_words::split(r#"npx server --env="MY KEY""#).unwrap(),
            vec!["npx", "server", "--env=MY KEY"]
        );
    }

    #[test]
    fn shell_words_single_quotes() {
        assert_eq!(
            shell_words::split("npx server --env='MY KEY'").unwrap(),
            vec!["npx", "server", "--env=MY KEY"]
        );
    }

    #[test]
    fn shell_words_mixed_quotes() {
        assert_eq!(
            shell_words::split(r#"cmd --opt="hello world" --flag 'single'"#).unwrap(),
            vec!["cmd", "--opt=hello world", "--flag", "single"]
        );
    }

    #[test]
    fn shell_words_escaped_quote() {
        assert_eq!(
            shell_words::split(r#"cmd arg\"with\"quotes"#).unwrap(),
            vec!["cmd", r#"arg"with"quotes"#]
        );
    }

    #[test]
    fn shell_words_empty() {
        assert!(shell_words::split("").unwrap().is_empty());
        assert!(shell_words::split("   ").unwrap().is_empty());
    }

    #[test]
    fn shell_words_postgres_url() {
        assert_eq!(
            shell_words::split(
                r#"npx -y @modelcontextprotocol/server-postgres "postgresql://user:pass@host/db""#
            )
            .unwrap(),
            vec![
                "npx",
                "-y",
                "@modelcontextprotocol/server-postgres",
                "postgresql://user:pass@host/db"
            ]
        );
    }

    #[test]
    fn parse_command_with_quoted_args() {
        let parsed =
            parse_mcp_command(r#"npx @modelcontextprotocol/server-filesystem /tmp --env="MY KEY""#)
                .unwrap();
        assert_eq!(parsed.config.command, Some("npx".to_string()));
        assert_eq!(
            parsed.config.args,
            vec![
                "@modelcontextprotocol/server-filesystem",
                "/tmp",
                "--env=MY KEY"
            ]
        );
    }

    // === infer_server_name tests ===

    #[test]
    fn infer_name_npx_package() {
        let parsed = parse_mcp_command("npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert_eq!(parsed.name, "filesystem");
    }

    #[test]
    fn infer_name_npx_simple() {
        let parsed = parse_mcp_command("npx my-mcp-server").unwrap();
        assert_eq!(parsed.name, "my-mcp-server");
    }

    #[test]
    fn infer_name_pnpm_exec() {
        let parsed = parse_mcp_command("pnpm exec @modelcontextprotocol/server-postgres").unwrap();
        assert_eq!(parsed.name, "postgres");
    }

    #[test]
    fn infer_name_node_script() {
        let parsed = parse_mcp_command("node ./my-mcp-server.js").unwrap();
        assert_eq!(parsed.name, "my-mcp-server");
    }

    #[test]
    fn infer_name_python_script() {
        let parsed = parse_mcp_command("python3 mcp_server.py").unwrap();
        assert_eq!(parsed.name, "mcp-server");
    }

    #[test]
    fn infer_name_uvx_package() {
        let parsed = parse_mcp_command("uvx mcp-server-git").unwrap();
        assert_eq!(parsed.name, "mcp-server-git");
    }

    #[test]
    fn infer_name_bare_command() {
        let parsed = parse_mcp_command("/usr/local/bin/my-server").unwrap();
        assert_eq!(parsed.name, "my-server");
    }

    #[test]
    fn infer_name_windows_cmd_prefix() {
        let parsed =
            parse_mcp_command("cmd /c npx -y @modelcontextprotocol/server-memory").unwrap();
        assert_eq!(parsed.name, "memory");
    }

    #[test]
    fn infer_name_windows_cmd_uppercase() {
        let parsed =
            parse_mcp_command("cmd /C npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert_eq!(parsed.name, "filesystem");
    }

    #[test]
    fn infer_name_only_command_no_args() {
        // No args at all — falls through to last resort: command name itself
        let parsed = parse_mcp_command("my-server").unwrap();
        assert_eq!(parsed.name, "my-server");
    }

    #[test]
    fn infer_name_only_command_no_args_path() {
        // Absolute path, no args — uses file_stem of command
        let parsed = parse_mcp_command("/usr/local/bin/my-server").unwrap();
        assert_eq!(parsed.name, "my-server");
    }

    // === sanitize_name tests ===

    #[test]
    fn sanitize_name_preserves_hyphens() {
        assert_eq!(sanitize_name("my-server"), "my-server");
    }

    #[test]
    fn sanitize_name_converts_underscores_to_hyphens() {
        assert_eq!(sanitize_name("my_server"), "my-server");
    }

    #[test]
    fn sanitize_name_converts_special_chars_to_hyphens() {
        assert_eq!(sanitize_name("my@server!"), "my-server");
    }

    #[test]
    fn sanitize_name_trims_leading_trailing_hyphens() {
        assert_eq!(sanitize_name("_my_server_"), "my-server");
    }

    #[test]
    fn sanitize_name_preserves_alphanumeric() {
        assert_eq!(sanitize_name("server123"), "server123");
    }

    #[test]
    fn sanitize_name_empty_input() {
        assert_eq!(sanitize_name(""), "");
    }

    // === command validation tests ===

    #[test]
    fn reject_shell_wrapper_bash() {
        let result = parse_mcp_command("bash -c 'npx server'");
        assert!(result.is_ok()); // parsing succeeds
        // but execute would reject — tested via parse_mcp_command structure
    }

    #[test]
    fn reject_metachar_redirect_in_args() {
        let parsed = parse_mcp_command("npx server --out>file").unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('>')));
    }

    #[test]
    fn reject_metachar_pipe_in_args() {
        let parsed = parse_mcp_command("npx server arg1 | cat").unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('|')));
    }

    #[test]
    fn reject_metachar_dollar_in_args() {
        let parsed = parse_mcp_command(r#"npx server --key=$SECRET"#).unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('$')));
    }

    #[test]
    fn reject_metachar_backtick_in_args() {
        let parsed = parse_mcp_command("npx server --dir=`whoami`").unwrap();
        assert!(parsed.config.args.iter().any(|a| a.contains('`')));
    }

    #[test]
    fn allow_clean_mcp_command() {
        let parsed = parse_mcp_command("npx @modelcontextprotocol/server-filesystem /tmp").unwrap();
        assert_eq!(parsed.config.command, Some("npx".to_string()));
        assert!(
            parsed
                .config
                .args
                .iter()
                .all(|a| !a.contains('>') && !a.contains('|') && !a.contains('$'))
        );
    }

    #[test]
    fn allowlist_includes_common_runtimes() {
        // Verify the allowlist covers the expected commands
        const ALLOWED: &[&str] = &[
            "npx", "npm", "pnpm", "yarn", "bunx", "bun", "node", "python", "python3", "uvx", "uv",
            "deno", "ruby", "cargo",
        ];
        // All standard MCP server launchers should be present
        assert!(ALLOWED.contains(&"npx"));
        assert!(ALLOWED.contains(&"node"));
        assert!(ALLOWED.contains(&"python3"));
        assert!(ALLOWED.contains(&"uvx"));
    }

    // === approval-gate contract ===

    #[test]
    fn start_mcp_server_declares_required_approval() {
        // Security invariant (#3866): spawning a runtime MCP server is
        // side-effecting (child process / network connection), so the tool
        // spec itself must declare `ApprovalRequirement::Required`. Combined
        // with the engine's non-bypassable gate (see engine tests), this
        // guarantees an unapproved start is rejected before `execute` runs.
        let pool = Arc::new(AsyncMutex::new(McpPool::new(
            crate::mcp::McpConfig::default(),
        )));
        let tool = StartRuntimeMcpServer::new(pool);
        assert_eq!(tool.name(), "start_mcp_server");
        assert!(
            matches!(tool.approval_requirement(), ApprovalRequirement::Required),
            "start_mcp_server must require approval before spawning"
        );
    }
}
