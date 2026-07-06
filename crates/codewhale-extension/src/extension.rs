//! Extension API for agent lifecycle hooks.
//!
//! Mirrors the Pi Coding Agent's `ExtensionAPI` pattern:
//! extensions register callbacks for lifecycle events (session start,
//! tool call, turn end, etc.) and the runtime dispatches to them.

use async_trait::async_trait;
use serde_json::Value;

/// Data available at each hook point.
#[derive(Debug, Clone)]
pub enum HookEvent {
    /// A new session has started.
    SessionStart {
        session_id: String,
        reason: String,
    },
    /// Before the agent sends a prompt to the LLM.
    BeforeAgentStart {
        system_prompt: String,
        prompt: String,
        is_first_turn: bool,
    },
    /// A tool is about to be called.
    ToolCall {
        tool_name: String,
        args: Value,
        turn_id: String,
    },
    /// A tool has returned a result.
    ToolResult {
        tool_name: String,
        args: Value,
        result: Value,
        is_error: bool,
        turn_id: String,
    },
    /// The current turn has ended.
    TurnEnd {
        turn_id: String,
        message_count: usize,
        tool_count: usize,
    },
    /// User input is being processed (for @memory:key expansion).
    Input {
        text: String,
    },
    /// User ran a shell command.
    UserBash {
        command: String,
    },
    /// Model was selected.
    ModelSelect {
        model: String,
        provider: String,
    },
    /// Before session compaction.
    SessionBeforeCompact {
        session_id: String,
        entry_count: usize,
    },
    /// After session compaction.
    SessionCompact {
        session_id: String,
        summary: String,
    },
    /// Session is shutting down.
    SessionShutdown {
        session_id: String,
    },
}

/// Result returned from a hook, possibly modifying the event.
#[derive(Debug, Clone, Default)]
pub struct HookResult {
    /// Modified system prompt (for BeforeAgentStart).
    pub system_prompt: Option<String>,
    /// Modified user prompt (for Input / BeforeAgentStart).
    pub prompt: Option<String>,
    /// Whether to suppress the default behavior.
    pub suppress: bool,
}

/// A single extension that hooks into the agent lifecycle.
#[async_trait]
pub trait Extension: Send + Sync {
    /// Name of this extension (e.g. "example").
    fn name(&self) -> &str;

    /// Called for each lifecycle event.
    async fn on_event(&self, event: &HookEvent) -> HookResult {
        let _ = event;
        HookResult::default()
    }

    /// Register MCP servers this extension provides.
    fn mcp_servers(&self) -> Vec<(String, McpServerDef)> {
        vec![]
    }

    /// Register slash commands this extension provides.
    fn slash_commands(&self) -> Vec<SlashCommandDef> {
        vec![]
    }

    /// Register keyboard shortcuts this extension provides.
    fn shortcuts(&self) -> Vec<ShortcutDef> {
        vec![]
    }

    /// Custom tool definitions this extension provides.
    fn tools(&self) -> Vec<ToolDef> {
        vec![]
    }
}

#[derive(Debug, Clone)]
pub struct McpServerDef {
    pub command: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
pub struct SlashCommandDef {
    pub name: String,
    pub description: String,
    pub handler: String, // e.g. "mcp_example_my_tool"
}

#[derive(Debug, Clone)]
pub struct ShortcutDef {
    pub keys: String,     // e.g. "ctrl+shift+m"
    pub description: String,
    pub action: String,   // e.g. "tn-memory-store"
}

#[derive(Debug, Clone)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    pub parameters: Option<Value>,
}

/// Manages a collection of extensions and dispatches events to all of them.
#[derive(Default)]
pub struct ExtensionManager {
    extensions: Vec<Box<dyn Extension>>,
}

impl ExtensionManager {
    pub fn new() -> Self {
        Self { extensions: Vec::new() }
    }

    pub fn register(&mut self, ext: Box<dyn Extension>) {
        self.extensions.push(ext);
    }

    pub fn extensions(&self) -> &[Box<dyn Extension>] {
        &self.extensions
    }

    /// Dispatch an event to all registered extensions and aggregate results.
    pub async fn dispatch(&self, event: &HookEvent) -> Vec<HookResult> {
        let mut results = Vec::new();
        for ext in &self.extensions {
            results.push(ext.on_event(event).await);
        }
        results
    }

    /// Collect all MCP server definitions from all extensions.
    pub fn collect_mcp_servers(&self) -> Vec<(String, McpServerDef)> {
        let mut servers = Vec::new();
        for ext in &self.extensions {
            servers.extend(ext.mcp_servers());
        }
        servers
    }

    /// Collect all slash commands from all extensions.
    pub fn collect_slash_commands(&self) -> Vec<SlashCommandDef> {
        let mut cmds = Vec::new();
        for ext in &self.extensions {
            cmds.extend(ext.slash_commands());
        }
        cmds
    }

    /// Collect all shortcuts from all extensions.
    pub fn collect_shortcuts(&self) -> Vec<ShortcutDef> {
        let mut shortcuts = Vec::new();
        for ext in &self.extensions {
            shortcuts.extend(ext.shortcuts());
        }
        shortcuts
    }

    /// Collect all tools from all extensions.
    pub fn collect_tools(&self) -> Vec<ToolDef> {
        let mut tools = Vec::new();
        for ext in &self.extensions {
            tools.extend(ext.tools());
        }
        tools
    }
}

