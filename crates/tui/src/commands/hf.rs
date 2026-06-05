//! `/hf` — Hugging Face Hub and MCP integration helpers (#2709).

use crate::tui::app::App;

use super::CommandResult;

/// `/hf mcp status` — checks whether the Hugging Face MCP server is configured.
/// `/hf mcp setup` — prints a safe config skeleton (no secrets).
pub fn hf(app: &mut App, args: Option<&str>) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() {
        return CommandResult::message(
            "Usage: /hf mcp <status|setup>\n\
             /hf docs   — open Hugging Face MCP documentation",
        );
    }

    let mut parts = raw.split_whitespace();
    let sub = parts.next().unwrap_or("").to_ascii_lowercase();

    match sub.as_str() {
        "mcp" => hf_mcp(app, parts.next()),
        "docs" => CommandResult::message(
            "Hugging Face MCP server docs: https://huggingface.co/docs/hub/hf-mcp-server\n\
             Hugging Face Hub MCP client docs: https://huggingface.co/docs/huggingface_hub/main/package_reference/mcp",
        ),
        _ => CommandResult::error(format!(
            "Unknown subcommand: '{sub}'. Use: /hf mcp <status|setup>"
        )),
    }
}

fn hf_mcp(app: &mut App, action: Option<&str>) -> CommandResult {
    match action.unwrap_or("status") {
        "status" => {
            let configured = hf_mcp_configured(app);
            if configured {
                CommandResult::message(
                    "✅ Hugging Face MCP server is configured.\n\
                     Use /mcp status to see all configured MCP servers.",
                )
            } else {
                CommandResult::message(
                    "❌ Hugging Face MCP server is not configured.\n\
                     Run /hf mcp setup to see a config skeleton, or visit\n\
                     https://huggingface.co/docs/hub/hf-mcp-server for setup docs.",
                )
            }
        }
        "setup" => {
            let skeleton = hf_mcp_config_skeleton();
            CommandResult::message(format!(
                "Add this to your MCP config (mcp.json or CodeWhale MCP config):\n\n{skeleton}\n\n\
                 ⚠️  Replace ${{HF_TOKEN}} with your Hugging Face token.\n\
                 Never commit your token to version control."
            ))
        }
        other => CommandResult::error(format!(
            "Unknown /hf mcp subcommand: '{other}'. Use: status | setup"
        )),
    }
}

/// Check whether a Hugging Face MCP server is present in the current MCP config.
fn hf_mcp_configured(app: &App) -> bool {
    crate::mcp::load_config(&app.mcp_config_path)
        .map(|cfg| cfg.servers.contains_key("huggingface"))
        .unwrap_or(false)
}

/// Return a safe config skeleton for the Hugging Face MCP server with secrets
/// replaced by placeholder variables.
fn hf_mcp_config_skeleton() -> String {
    r#"```jsonc
{
  "servers": {
    "huggingface": {
      "url": "https://huggingface.co/api/mcp",
      "headers": {
        "Authorization": "Bearer ${HF_TOKEN}"
      }
    }
  }
}
```"#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hf_mcp_config_skeleton_does_not_contain_real_tokens() {
        let skeleton = hf_mcp_config_skeleton();
        // The skeleton must contain a placeholder, not a real token.
        assert!(skeleton.contains("${HF_TOKEN}"));
        assert!(!skeleton.contains("hf_"));
        assert!(!skeleton.contains("Bearer hf_"));
    }

    #[test]
    fn hf_mcp_config_skeleton_is_valid_jsonc_structure() {
        let skeleton = hf_mcp_config_skeleton();
        assert!(skeleton.contains("\"huggingface\""));
        assert!(skeleton.contains("\"url\""));
        assert!(skeleton.contains("\"headers\""));
        assert!(skeleton.contains("\"Authorization\""));
    }
}
