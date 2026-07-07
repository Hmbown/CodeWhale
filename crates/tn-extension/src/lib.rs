//! TormentNexus extension adapter for CodeWhale — full Pi extension parity.
//!
//! Implements the [`codewhale_extension::Extension`] trait, calling
//! TormentNexus's local API (port 7778) at each lifecycle hook point.
//! Mirrors all Pi extension v4 functionality:
//! - L2 memory integration (store, search, vector search)
//! - MCP tool discovery via TN sidecar
//! - Session / skill / code search
//! - Context harvesting and scratchpad
//! - RBAC / audit logging
//! - @memory:key expansion
//! - Session compaction preservation
//! - Slash commands, shortcuts, custom tool registration

use async_trait::async_trait;
use codewhale_extension::{
    Extension, ExtensionManager, HookEvent, HookResult, McpServerDef, ShortcutDef,
    SlashCommandDef, ToolDef,
};
use serde_json::{json, Value};

const TN_BASE: &str = "http://127.0.0.1:7778";

/// Dangerous operation patterns checked against enterprise RBAC.
const DANGEROUS_PATTERNS: &[&str] = &[
    "rm -rf", "sudo ", "chmod -R 777",
    "DROP TABLE", "DROP DATABASE", "git push --force",
];

/// Tools whose results are auto-stored to L2 memory.
const STORE_TOOLS: &[&str] = &[
    "bash", "read", "grep", "tn_code_search", "tn_tool_search", "exec_shell",
];

/// TormentNexus extension adapter — full Pi extension v4 parity.
pub struct TormentNexusExtension {
    client: reqwest::Client,
}

impl TormentNexusExtension {
    pub fn new() -> Self {
        Self { client: reqwest::Client::new() }
    }

    pub fn register(manager: &mut ExtensionManager) {
        manager.register(Box::new(Self::new()));
    }

    async fn tn_post(&self, path: &str, body: Value) {
        let _ = self.client
            .post(&format!("{TN_BASE}{path}"))
            .json(&body)
            .timeout(std::time::Duration::from_secs(3))
            .send().await;
    }

    async fn tn_get(&self, path: &str) -> Option<Value> {
        self.client
            .get(&format!("{TN_BASE}{path}"))
            .timeout(std::time::Duration::from_secs(3))
            .send().await.ok()?.json().await.ok()
    }
}

#[async_trait]
impl Extension for TormentNexusExtension {
    fn name(&self) -> &str { "tormentnexus" }

    async fn on_event(&self, event: &HookEvent) -> HookResult {
        match event {
            HookEvent::SessionStart { session_id, reason } => {
                self.tn_post("/api/memory/add", json!({
                    "content": serde_json::to_string(&json!({
                        "content": format!("Session {reason}: {session_id}"),
                        "tags": ["system:session", format!("reason:{reason}")],
                        "category": "session",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    })).unwrap_or_default(),
                })).await;
                HookResult::default()
            }

            HookEvent::BeforeAgentStart { system_prompt, prompt, .. } => {
                let mut result = HookResult::default();

                let guidance = format!(
                    "{}\n\n## TormentNexus Integration\n\n\
                    TN is a local AI control plane on port 7778 with L2 vector memory, \
                    tool discovery, sessions, and skill registry.\n\n\
                    ### Memory\n- `mcp_tormentnexus_memory_scratchpad_get/set/append` — L1 working memory\n\
                    - REST: `POST /api/memory/add` — store to L2\n\
                    - REST: `GET /api/memory/search?q=...` — search L2\n\n\
                    ### Discovery\n- `mcp_tormentnexus_mcp_list_tools` / `mcp_call_tool` — MCP routing\n\
                    - REST: `GET /api/mcp/native/search?query=...` — semantic tool search\n\
                    - REST: `GET /api/skills/search?q=...` — skill registry\n\
                    - REST: `GET /api/memory/search?q=...&type=session` — session search\n\n\
                    ### Slash Commands\n- `/tn-store` — store memory | `/tn-search` — search\n\
                    - `/tn-status` — system health | `/tn-plan` — plan management\n\
                    - `/tn-summary` — summarize | `/tn-purge` — remove stale\n\n\
                    ### Best Practices\n1. Check scratchpad before significant work\n\
                    2. Store patterns after key moments\n3. Search L2 before complex tasks\n\
                    4. Use `@memory:key` inline for auto-expansion",
                    system_prompt
                );
                result.system_prompt = Some(guidance);

                // Per-turn L2 context harvesting
                if !prompt.is_empty() {
                    let q = if prompt.len() > 100 { &prompt[..100] } else { prompt.as_str() };
                    if let Some(resp) = self.tn_get(&format!("/api/memory/search?q={}&limit=3", urlencode(q))).await {
                        if let Some(memories) = resp.get("data").and_then(|d| d.as_array()) {
                            let ctx: Vec<String> = memories.iter().filter_map(|m| {
                                let c = m.get("content").or_else(|| m.get("text"))
                                    .and_then(|v| v.as_str()).unwrap_or("");
                                if c.is_empty() { return None; }
                                let s = if c.len() > 200 { &c[..200] } else { c };
                                Some(format!("  • {}", s))
                            }).collect();
                            if !ctx.is_empty() {
                                result.system_prompt = Some(format!(
                                    "{}\n\n## Relevant Context from TN L2\n{}",
                                    result.system_prompt.as_deref().unwrap_or(""),
                                    ctx.join("\n")
                                ));
                            }
                        }
                    }
                }
                result
            }

            HookEvent::ToolCall { tool_name, args, .. } => {
                self.tn_post("/api/memory/add", json!({
                    "content": serde_json::to_string(&json!({
                        "content": format!("Tool call: {tool_name}"),
                        "tags": ["system:tool_call", format!("tool:{tool_name}")],
                        "data": args, "category": "tool",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    })).unwrap_or_default(),
                })).await;

                // RBAC: check dangerous patterns
                let args_str = serde_json::to_string(args).unwrap_or_default().to_lowercase();
                for pattern in DANGEROUS_PATTERNS {
                    if args_str.contains(pattern) {
                        self.tn_post("/api/enterprise/authorize", json!({
                            "tool": tool_name, "action": pattern, "args": args,
                            "timestamp": chrono::Utc::now().to_rfc3339(),
                        })).await;
                        self.tn_post("/api/enterprise/audit/log", json!({
                            "tool": tool_name, "action": pattern, "args": args,
                            "timestamp": chrono::Utc::now().to_rfc3339(),
                            "userId": "codewhale-agent",
                        })).await;
                    }
                }
                HookResult::default()
            }

            HookEvent::ToolResult { tool_name, result, is_error, .. } => {
                if !is_error && STORE_TOOLS.contains(&tool_name.as_str()) {
                    let rt = serde_json::to_string(result).unwrap_or_default();
                    if rt.len() >= 100 && rt.len() < 2000 {
                        let s = &rt[..rt.len().min(300)];
                        self.tn_post("/api/memory/add", json!({
                            "content": serde_json::to_string(&json!({
                                "content": format!("[{}] {}", tool_name, s),
                                "tags": ["system:tool_result", format!("tool:{}", tool_name)],
                                "category": "tool_result",
                            })).unwrap_or_default(),
                        })).await;
                    }
                }
                HookResult::default()
            }

            HookEvent::TurnEnd { turn_id, tool_count, .. } => {
                if *tool_count > 0 {
                    self.tn_post("/api/memory/add", json!({
                        "content": serde_json::to_string(&json!({
                            "content": format!("Turn {turn_id}: {tool_count} tools"),
                            "tags": ["system:turn_end"],
                            "category": "turn",
                            "timestamp": chrono::Utc::now().to_rfc3339(),
                        })).unwrap_or_default(),
                    })).await;
                }
                HookResult::default()
            }

            HookEvent::Input { text } => {
                if text.contains("@memory:") {
                    if let Some(key) = text.split("@memory:").nth(1)
                        .and_then(|s| s.split_whitespace().next())
                    {
                        if !key.is_empty() {
                            if let Some(resp) = self.tn_get(&format!("/api/memory/search?q={}", urlencode(key))).await {
                                if let Some(memories) = resp.get("data").and_then(|d| d.as_array()) {
                                    if let Some(first) = memories.first() {
                                        let val = first.get("text").or_else(|| first.get("content"))
                                            .and_then(|v| v.as_str()).unwrap_or("<not found>");
                                        let expanded = text.replace(&format!("@memory:{key}"), val);
                                        if expanded != *text {
                                            return HookResult { prompt: Some(expanded), ..Default::default() };
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                HookResult::default()
            }

            HookEvent::UserBash { command } => {
                self.tn_post("/api/enterprise/audit/log", json!({
                    "tool": "user_bash", "command": command,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                })).await;
                HookResult::default()
            }

            HookEvent::ModelSelect { model, provider } => {
                self.tn_post("/api/memory/add", json!({
                    "content": serde_json::to_string(&json!({
                        "content": format!("Model: {model} ({provider})"),
                        "tags": ["system:model_select", format!("model:{model}")],
                        "category": "model",
                    })).unwrap_or_default(),
                })).await;
                HookResult::default()
            }

            HookEvent::SessionBeforeCompact { session_id, entry_count } => {
                self.tn_post("/api/memory/add", json!({
                    "content": serde_json::to_string(&json!({
                        "content": format!("Compacting: {session_id} ({entry_count} entries)"),
                        "tags": ["system:compaction", format!("session:{session_id}")],
                        "category": "system",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    })).unwrap_or_default(),
                })).await;
                HookResult::default()
            }

            HookEvent::SessionCompact { session_id, summary } => {
                self.tn_post("/api/memory/add", json!({
                    "content": serde_json::to_string(&json!({
                        "content": format!("Compacted: {session_id}\n{summary}"),
                        "tags": ["system:compacted", format!("session:{session_id}")],
                        "category": "system",
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    })).unwrap_or_default(),
                })).await;
                HookResult::default()
            }

            HookEvent::SessionShutdown { .. } => HookResult::default(),
        }
    }

    fn mcp_servers(&self) -> Vec<(String, McpServerDef)> {
        vec![("tormentnexus".into(), McpServerDef {
            command: r#"C:\Users\hyper\workspace\tormentnexus\tormentnexus.exe"#.into(),
            args: vec!["mcp".into()],
            env: vec![("TORMENTNEXUS_WORKSPACE_ROOT".into(), r#"C:\Users\hyper\workspace\tormentnexus"#.into())],
        })]
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef { name: "tn_memory_store".into(),
                description: "Store a memory in TormentNexus L2 vault. Params: content (required), tags (optional), category (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"content":{"type":"string"},"tags":{"type":"array","items":{"type":"string"}},"category":{"type":"string"}},"required":["content"]})) },
            ToolDef { name: "tn_memory_search".into(),
                description: "Search L2 vault by keyword. Params: query (required), tag (optional), category (optional), limit (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"tag":{"type":"string"},"category":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]})) },
            ToolDef { name: "tn_memory_vector_search".into(),
                description: "Semantic vector search L2. Params: query (required), limit (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]})) },
            ToolDef { name: "tn_tool_search".into(),
                description: "Semantic MCP tool search. Params: query (required), limit (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]})) },
            ToolDef { name: "tn_session_search".into(),
                description: "Search imported sessions. Params: query (required), limit (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer"}},"required":["query"]})) },
            ToolDef { name: "tn_skill_manage".into(),
                description: "Access skill registry. Params: query (required), action (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"action":{"type":"string"}},"required":["query"]})) },
            ToolDef { name: "tn_code_search".into(),
                description: "Multi-engine code search. Params: query (required), scope (optional), path (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"scope":{"type":"string"},"path":{"type":"string"}},"required":["query"]})) },
            ToolDef { name: "tn_context_harvest".into(),
                description: "Harvest L2 context + skills. Params: query (required), harvest_memory (optional), harvest_skills (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"query":{"type":"string"},"harvest_memory":{"type":"boolean"},"harvest_skills":{"type":"boolean"}},"required":["query"]})) },
            ToolDef { name: "tn_scratchpad".into(),
                description: "Read/write L1 scratchpad. Params: action (required: get/set), content (optional)".into(),
                parameters: Some(json!({"type":"object","properties":{"action":{"type":"string"},"content":{"type":"string"}},"required":["action"]})) },
        ]
    }

    fn slash_commands(&self) -> Vec<SlashCommandDef> {
        vec![
            SlashCommandDef { name: "tn-store".into(), description: "Store a memory in TN L2 vault".into(), handler: "tn:memory-store".into() },
            SlashCommandDef { name: "tn-search".into(), description: "Search TN L2 memory".into(), handler: "tn:memory-search".into() },
            SlashCommandDef { name: "tn-status".into(), description: "Show TN system status".into(), handler: "tn:system-status".into() },
            SlashCommandDef { name: "tn-plan".into(), description: "Manage project plans in L2".into(), handler: "tn:plan-manage".into() },
            SlashCommandDef { name: "tn-summary".into(), description: "Summarize session using TN".into(), handler: "tn:session-summary".into() },
            SlashCommandDef { name: "tn-purge".into(), description: "Remove stale memories".into(), handler: "tn:memory-purge".into() },
        ]
    }

    fn shortcuts(&self) -> Vec<ShortcutDef> {
        vec![
            ShortcutDef { keys: "ctrl+shift+m".into(), description: "TN memory search".into(), action: "tn:memory-search".into() },
            ShortcutDef { keys: "ctrl+shift+t".into(), description: "TN tool search".into(), action: "tn:tool-search".into() },
            ShortcutDef { keys: "ctrl+shift+p".into(), description: "TN system status".into(), action: "tn:system-status".into() },
        ]
    }
}

fn urlencode(s: &str) -> String {
    let mut result = String::new();
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => result.push(byte as char),
            b' ' => result.push_str("%20"),
            _ => result.push_str(&format!("%{:02X}", byte)),
        }
    }
    result
}
