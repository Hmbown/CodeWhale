//! `/hf` — Hugging Face Hub and MCP integration helpers (#2709).

use crate::tui::app::App;

use super::CommandResult;

use serde_json;

/// Explainer shown by `/hf concepts` — distinguishes the three Hugging Face
/// integration surfaces so users understand which one they're configuring.
const HF_CONCEPTS: &str = "\
CodeWhale has three distinct Hugging Face integration surfaces:

1. HF Provider Route — chat inference
   Switch your LLM backend to Hugging Face Inference Providers.
   Use: /provider huggingface
   Config: [providers.huggingface] in config.toml
   Needs: HF_TOKEN or HUGGINGFACE_API_KEY

2. HF MCP — Hub search, resources, community tools
   Connect to Hugging Face's MCP server for model-card/docs search,
   dataset discovery, and community tooling.
   Use: /hf mcp status | /hf mcp setup
   Config: {\"huggingface\": {...}} in mcp.json
   Needs: HF_TOKEN (passed as Authorization header)

3. HF Hub — upload / export workflows
   Publish models, datasets, or Spaces to the Hub.
   This always requires explicit user action — CodeWhale never
   uploads to the Hub without your approval.
   Use: huggingface_hub Python package or git-based workflow";

// ── /hf command ──────────────────────────────────────────────────

/// `/hf` — Hugging Face Hub, MCP, and Inference integration helpers (#2709).
///
/// Commands:
///   `/hf mcp status`   — check whether HF MCP server is configured
///   `/hf mcp setup`    — print a safe config skeleton (`${HF_TOKEN}`)
///   `/hf docs`         — links to HF MCP and Hub documentation
///   `/hf concepts`     — explain HF provider vs MCP vs Hub
pub fn hf(app: &mut App, args: Option<&str>) -> CommandResult {
    let raw = args.unwrap_or("").trim();
    if raw.is_empty() {
        return CommandResult::message(
            "Usage: /hf mcp <status|setup>\n\
             /hf search <query> — search Hugging Face Hub\n\
             /hf docs           — HF MCP documentation\n\
             /hf concepts       — HF provider vs MCP vs Hub",
        );
    }

    let mut parts = raw.split_whitespace();
    let sub = parts.next().unwrap_or("").to_ascii_lowercase();

    match sub.as_str() {
        "mcp" => hf_mcp(app, parts.next()),
        "search" | "find" => hf_search(&raw[sub.len()..].trim()),
        "concepts" | "explain" => CommandResult::message(HF_CONCEPTS),
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

/// `/hf search <query>` — query the Hugging Face Hub API for models.
/// Falls back gracefully when the network is unavailable.
fn hf_search(query: &str) -> CommandResult {
    if query.is_empty() {
        return CommandResult::error("Usage: /hf search <query>");
    }
    match hf_hub_search(query) {
        Ok(results) => CommandResult::message(results),
        Err(e) => CommandResult::message(format!(
            "HF Hub API unavailable: {e}\n\
             Tip: use /hf mcp status to check whether HF MCP is configured,\n\
             or /hf docs for manual search links."
        )),
    }
}

/// Call the Hugging Face Hub models API and format the top 5 results.
fn hf_hub_search(query: &str) -> Result<String, String> {
    let url = format!(
        "https://huggingface.co/api/models?search={}&limit=5&full=false",
        urlencoding(query)
    );
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .map_err(|e| format!("client error: {e}"))?;
    let resp = client
        .get(&url)
        .header("User-Agent", "CodeWhale/0.9.0 HF MCP integration")
        .send()
        .map_err(|e| format!("request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let body: serde_json::Value = resp.json().map_err(|e| format!("parse error: {e}"))?;
    let models = body.as_array().ok_or("unexpected response format")?;
    if models.is_empty() {
        return Ok("No models found on Hugging Face Hub.".into());
    }
    let mut lines = vec![format!(
        "HF Hub search: \"{query}\" — top {} of {} results:\n",
        models.len().min(5),
        models.len()
    )];
    for (i, m) in models.iter().take(5).enumerate() {
        let id = m["id"].as_str().unwrap_or("?");
        let likes = m["likes"].as_u64().unwrap_or(0);
        let downloads = m["downloads"].as_u64().unwrap_or(0);
        let author = m["author"].as_str().unwrap_or("");
        let pipeline = m["pipeline_tag"].as_str().unwrap_or("");
        let url = format!("https://huggingface.co/{id}");
        lines.push(format!("{}. {id}", i + 1));
        if !author.is_empty() {
            lines.push(format!("   by {author}"));
        }
        let mut tags = Vec::new();
        if likes > 0 {
            tags.push(format!("♥ {likes}"));
        }
        if downloads > 0 {
            tags.push(format!("↓ {downloads}"));
        }
        if !pipeline.is_empty() {
            tags.push(pipeline.to_string());
        }
        if !tags.is_empty() {
            lines.push(format!("   {}", tags.join(" · ")));
        }
        lines.push(format!("   {url}"));
    }
    Ok(lines.join("\n"))
}

/// Simple URL encoding for search queries.
fn urlencoding(s: &str) -> String {
    s.replace(' ', "%20")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('?', "%3F")
        .replace('&', "%26")
        .replace('=', "%3D")
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

    #[test]
    fn hf_concepts_explains_three_surfaces() {
        assert!(HF_CONCEPTS.contains("HF Provider Route"));
        assert!(HF_CONCEPTS.contains("HF MCP"));
        assert!(HF_CONCEPTS.contains("HF Hub"));
        assert!(HF_CONCEPTS.contains("/provider huggingface"));
        assert!(HF_CONCEPTS.contains("/hf mcp"));
        assert!(HF_CONCEPTS.contains("HF_TOKEN"));
    }
}
