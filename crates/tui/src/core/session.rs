//! Session state management for the core engine.
//!
//! Tracks conversation history, token usage, and session metadata.

use crate::models::{Message, SystemPrompt, Usage};
use crate::prefix_cache::PrefixStabilityManager;
use crate::project_context::{ProjectContext, load_project_context_with_parents};
use crate::prompt_zones::{AppendLog, FrozenPrefix};
use crate::tui::approval::ApprovalMode;
use crate::working_set::WorkingSet;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;

/// Maximum number of deferred schemas a conversation may keep in its active
/// toolbox. The permanent `read`/`write`/`edit`/`bash`/`agent`/`tool_search`
/// router surface is not counted here.
pub(crate) const TOOL_ACTIVATION_CACHE_MAX_NAMES: usize = 8;
/// Maximum serialized bytes added to requests by cached deferred schemas.
pub(crate) const TOOL_ACTIVATION_CACHE_MAX_SCHEMA_BYTES: usize = 16 * 1024;

/// Bounded, process-local conversation cache for tools activated by
/// `tool_search`.
///
/// Only names are retained. Every turn revalidates them against the currently
/// filtered catalog, so a disconnected MCP server, changed allow/deny rule, or
/// mode switch cannot resurrect a tool from an older authority posture.
#[derive(Debug, Clone, Default)]
pub(crate) struct ToolActivationCache {
    /// Least-recently used at the front, most-recently used at the back.
    names: VecDeque<String>,
}

#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct ToolActivationDelta {
    pub(crate) admitted: Vec<String>,
    pub(crate) evicted: Vec<String>,
    pub(crate) rejected: Vec<String>,
}

impl ToolActivationCache {
    /// Forget every deferred tool activated by the current conversation.
    ///
    /// `Op::SyncSession` calls this before installing another conversation's
    /// identity, history, and workspace. The cache is intentionally
    /// process-local, but it must still be conversation-local.
    pub(crate) fn clear(&mut self) {
        self.names.clear();
    }

    fn catalog_tool<'a>(
        catalog: &'a [crate::models::Tool],
        name: &str,
    ) -> Option<&'a crate::models::Tool> {
        catalog
            .iter()
            .find(|tool| tool.name == name && tool.defer_loading.unwrap_or(false))
    }

    fn serialized_bytes(tool: &crate::models::Tool) -> usize {
        serde_json::to_vec(tool).map_or(usize::MAX, |bytes| bytes.len())
    }

    fn total_serialized_bytes(&self, catalog: &[crate::models::Tool]) -> usize {
        self.names
            .iter()
            .filter_map(|name| Self::catalog_tool(catalog, name))
            .map(Self::serialized_bytes)
            .fold(0usize, usize::saturating_add)
    }

    /// Drop entries that are no longer deferred members of this turn's
    /// filtered catalog and enforce both cache bounds.
    pub(crate) fn revalidate(&mut self, catalog: &[crate::models::Tool]) -> Vec<String> {
        let mut evicted = Vec::new();
        self.names.retain(|name| {
            let keep = Self::catalog_tool(catalog, name).is_some_and(|tool| {
                Self::serialized_bytes(tool) <= TOOL_ACTIVATION_CACHE_MAX_SCHEMA_BYTES
            });
            if !keep {
                evicted.push(name.clone());
            }
            keep
        });
        while self.names.len() > TOOL_ACTIVATION_CACHE_MAX_NAMES
            || self.total_serialized_bytes(catalog) > TOOL_ACTIVATION_CACHE_MAX_SCHEMA_BYTES
        {
            if let Some(name) = self.names.pop_front() {
                evicted.push(name);
            } else {
                break;
            }
        }
        evicted
    }

    /// Touch requested deferred tools in search-result order. An oversized
    /// schema is rejected; otherwise least-recently-used entries are evicted
    /// until both bounds hold.
    pub(crate) fn activate(
        &mut self,
        catalog: &[crate::models::Tool],
        requested: &[String],
    ) -> ToolActivationDelta {
        let mut delta = ToolActivationDelta {
            evicted: self.revalidate(catalog),
            ..ToolActivationDelta::default()
        };
        let mut seen = HashSet::new();
        for name in requested {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Some(tool) = Self::catalog_tool(catalog, name) else {
                delta.rejected.push(name.clone());
                continue;
            };
            if Self::serialized_bytes(tool) > TOOL_ACTIVATION_CACHE_MAX_SCHEMA_BYTES {
                delta.rejected.push(name.clone());
                continue;
            }
            if let Some(index) = self.names.iter().position(|cached| cached == name) {
                self.names.remove(index);
            }
            self.names.push_back(name.clone());
            while self.names.len() > TOOL_ACTIVATION_CACHE_MAX_NAMES
                || self.total_serialized_bytes(catalog) > TOOL_ACTIVATION_CACHE_MAX_SCHEMA_BYTES
            {
                if let Some(evicted) = self.names.pop_front() {
                    delta.evicted.push(evicted);
                }
            }
        }

        let retained = self.names.iter().collect::<HashSet<_>>();
        delta.admitted = requested
            .iter()
            .filter(|name| retained.contains(name))
            .cloned()
            .collect();
        delta.evicted.sort();
        delta.evicted.dedup();
        delta.rejected.sort();
        delta.rejected.dedup();
        delta
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }
}

/// Session state for the engine.
#[derive(Debug, Clone)]
pub struct Session {
    /// Model being used
    pub model: String,

    /// Reasoning-effort tier for DeepSeek thinking mode:
    /// `"off" | "low" | "medium" | "high" | "max"`. `None` lets the provider
    /// apply its own defaults.
    pub reasoning_effort: Option<String>,
    /// Whether the user selected automatic reasoning effort.
    pub reasoning_effort_auto: bool,

    /// Whether the user selected automatic model routing.
    pub auto_model: bool,

    /// Workspace directory
    pub workspace: PathBuf,

    /// System prompt (optional)
    pub system_prompt: Option<SystemPrompt>,
    /// True when `system_prompt` is a persisted/runtime-supplied prefix that
    /// should not be replaced by mode/context refreshes.
    pub system_prompt_override: bool,
    /// Hash of the last assembled stable system prompt. Used to avoid
    /// replacing `system_prompt` when unchanged.
    pub last_system_prompt_hash: Option<u64>,
    /// Reason the pinned prefix will move on the next model request, set by
    /// an explicit header-change op (`/model`, mode change, goal edit, session
    /// sync) when it actually alters the system-prompt bytes. Consumed by the
    /// turn loop's prefix check so a declared change re-pins under a logged
    /// reason while an undeclared mid-loop change is reported as drift and the
    /// pin holds. `None` means "no declared change since the last request".
    pub pending_prefix_change_reason: Option<String>,
    /// The explicit prompt inputs (model, mode, goal, route, translation,
    /// verbosity) the pinned system prompt was composed from. At each new user
    /// turn the engine recomposes: if these inputs are unchanged but the
    /// composed bytes differ, that is workspace/instruction/skills/memory
    /// drift and is delivered to the model as a `<context_update>` message,
    /// never by moving the pinned header.
    pub(crate) pinned_prompt_context: Option<crate::core::engine::NextTurnPromptContext>,
    /// Flat text of the session context the model has last been shown, either
    /// as the pinned header or through `<context_update>` messages. New-turn
    /// deltas are computed against this so an update is delivered exactly once.
    pub(crate) context_update_baseline: Option<String>,
    /// Host-persistence copy of the history checkpoint generated by context
    /// compaction. This is never part of the standing system prompt.
    pub compaction_summary_prompt: Option<SystemPrompt>,

    /// Conversation history (API format), backed by AppendLog (#2264).
    pub messages: AppendLog,

    /// Total tokens used in this session
    pub total_usage: SessionUsage,

    /// Whether shell execution is allowed
    pub allow_shell: bool,

    /// Whether to trust paths outside workspace
    pub trust_mode: bool,

    /// Whether the current session should auto-approve tool safety checks.
    pub auto_approve: bool,

    /// Live UI approval policy used to steer the system prompt.
    pub approval_mode: ApprovalMode,

    /// Notes file path
    pub notes_path: PathBuf,

    /// MCP config path
    pub mcp_config_path: PathBuf,

    /// Session ID (for tracking)
    pub id: String,

    /// Project context loaded from AGENTS.md, etc.
    pub project_context: Option<ProjectContext>,

    /// Repo-aware working set for context management.
    pub working_set: WorkingSet,

    /// Prefix-cache stability monitor (inspired by Reasonix's Pillar 1).
    /// Tracks the immutable prefix fingerprint and detects drift across turns.
    /// Set during engine construction; None until the first system prompt assembly.
    pub prefix_stability: Option<PrefixStabilityManager>,

    /// Three-zone immutable prefix baseline (#2264). Frozen on the first
    /// request of the session; verified against the current system+tool
    /// state before every subsequent request. None until the first turn.
    pub frozen_prefix: Option<FrozenPrefix>,

    /// Deferred tools explicitly discovered during this conversation. Names
    /// are revalidated against the live catalog before each request.
    pub(super) tool_activation_cache: ToolActivationCache,

    /// Monotonic counter bumped on every direct mutation of `messages`.
    /// Consumed by the engine token-estimate cache
    /// to memoize the per-turn token estimate without re-walking the message
    /// list. Defaults to 0; bumped in [`Session::add_message`],
    /// [`Session::replace_messages`], and at other mutation sites in
    /// `core/engine.rs`.
    pub messages_revision: u64,
}

/// Cumulative usage statistics for a session.
#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_field_names)]
pub struct SessionUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cache creation (write) tokens. `None` when never observed by the API —
    /// do NOT display as 0, which would be indistinguishable from "no writes".
    pub cache_creation_input_tokens: Option<u64>,
    /// Cache read (hit) tokens. `None` when never observed by the API —
    /// do NOT display as 0, which would be indistinguishable from "no hits".
    pub cache_read_input_tokens: Option<u64>,
}

impl SessionUsage {
    /// Add usage from a turn
    pub fn add(&mut self, usage: &Usage) {
        self.input_tokens += u64::from(usage.input_tokens);
        self.output_tokens += u64::from(usage.output_tokens);
        if let Some(tokens) = usage.prompt_cache_write_tokens {
            self.cache_creation_input_tokens =
                Some(self.cache_creation_input_tokens.unwrap_or(0) + u64::from(tokens));
        }
        if let Some(tokens) = usage.prompt_cache_hit_tokens {
            self.cache_read_input_tokens =
                Some(self.cache_read_input_tokens.unwrap_or(0) + u64::from(tokens));
        }
    }
}

impl Session {
    /// Create a new session
    pub fn new(
        model: String,
        workspace: PathBuf,
        allow_shell: bool,
        trust_mode: bool,
        notes_path: PathBuf,
        mcp_config_path: PathBuf,
    ) -> Self {
        // Load project context from AGENTS.md, CLAUDE.md, etc.
        let project_context = load_project_context_with_parents(&workspace);
        let has_context = project_context.has_instructions();

        Self {
            model,
            reasoning_effort: None,
            reasoning_effort_auto: false,
            auto_model: false,
            workspace,
            system_prompt: None,
            system_prompt_override: false,
            compaction_summary_prompt: None,
            messages: AppendLog::new(),
            total_usage: SessionUsage::default(),
            allow_shell,
            trust_mode,
            auto_approve: false,
            approval_mode: ApprovalMode::Suggest,
            notes_path,
            mcp_config_path,
            id: uuid::Uuid::new_v4().to_string(),
            project_context: if has_context {
                Some(project_context)
            } else {
                None
            },
            last_system_prompt_hash: None,
            pending_prefix_change_reason: None,
            pinned_prompt_context: None,
            context_update_baseline: None,
            working_set: WorkingSet::default(),
            prefix_stability: None,
            frozen_prefix: None,
            tool_activation_cache: ToolActivationCache::default(),
            messages_revision: 0,
        }
    }

    /// Add a message to the conversation
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.messages_revision = self.messages_revision.saturating_add(1);
    }

    /// Replace the entire message history. Used by session resume and
    /// compaction. Bumps `messages_revision` exactly once even when the new
    /// history has a different length, so downstream caches invalidate
    /// atomically.
    #[allow(dead_code)]
    pub fn replace_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages.into();
        self.messages_revision = self.messages_revision.saturating_add(1);
    }

    /// Bump `messages_revision` without otherwise mutating the message list.
    /// Reserved for sites that mutate the message list in place (e.g. an
    /// in-place rewrite of a content block). Most call sites do not need
    /// this — prefer [`add_message`](Self::add_message) and
    /// [`replace_messages`](Self::replace_messages).
    pub fn bump_messages_revision(&mut self) {
        self.messages_revision = self.messages_revision.saturating_add(1);
    }

    /// Rebuild the working set from current messages (best effort).
    pub fn rebuild_working_set(&mut self) {
        self.working_set
            .rebuild_from_messages(&self.messages, &self.workspace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deferred_tool(name: &str, description_bytes: usize) -> crate::models::Tool {
        crate::models::Tool {
            tool_type: None,
            name: name.to_string(),
            description: "x".repeat(description_bytes),
            input_schema: json!({"type": "object", "properties": {}}),
            allowed_callers: None,
            defer_loading: Some(true),
            input_examples: None,
            strict: None,
            cache_control: None,
        }
    }

    #[test]
    fn session_usage_cache_starts_none() {
        let usage = SessionUsage::default();
        assert!(usage.cache_creation_input_tokens.is_none());
        assert!(usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn session_usage_cache_remains_none_when_api_omits_cache() {
        let mut usage = SessionUsage::default();
        let api_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            prompt_cache_hit_tokens: None,
            prompt_cache_miss_tokens: None,
            prompt_cache_write_tokens: None,
            reasoning_tokens: None,
            reasoning_replay_tokens: None,
            server_tool_use: None,
        };
        usage.add(&api_usage);
        assert!(usage.cache_creation_input_tokens.is_none());
        assert!(usage.cache_read_input_tokens.is_none());
    }

    #[test]
    fn session_usage_cache_accumulates_when_reported() {
        let mut usage = SessionUsage::default();
        let api_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            prompt_cache_hit_tokens: Some(30),
            prompt_cache_miss_tokens: Some(50),
            prompt_cache_write_tokens: Some(20),
            reasoning_tokens: None,
            reasoning_replay_tokens: None,
            server_tool_use: None,
        };
        usage.add(&api_usage);
        assert_eq!(usage.cache_read_input_tokens, Some(30));
        assert_eq!(usage.cache_creation_input_tokens, Some(20));
        usage.add(&api_usage);
        assert_eq!(usage.cache_read_input_tokens, Some(60));
        assert_eq!(usage.cache_creation_input_tokens, Some(40));
    }

    #[test]
    fn session_usage_cache_preserves_explicit_zero() {
        let mut usage = SessionUsage::default();
        let api_usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            prompt_cache_hit_tokens: Some(0), // explicit zero from provider
            prompt_cache_miss_tokens: Some(50),
            prompt_cache_write_tokens: Some(1234),
            reasoning_tokens: None,
            reasoning_replay_tokens: None,
            server_tool_use: None,
        };
        usage.add(&api_usage);
        // 0 is a valid observed value, must NOT be converted to None
        assert_eq!(usage.cache_read_input_tokens, Some(0));
        assert_eq!(usage.cache_creation_input_tokens, Some(1234));
    }

    #[test]
    fn tool_activation_cache_is_lru_bounded_to_eight_names() {
        let catalog = (0..10)
            .map(|index| deferred_tool(&format!("tool_{index}"), 8))
            .collect::<Vec<_>>();
        let requested = catalog
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let mut cache = ToolActivationCache::default();
        let delta = cache.activate(&catalog, &requested);

        assert_eq!(cache.names().count(), TOOL_ACTIVATION_CACHE_MAX_NAMES);
        assert_eq!(
            cache.names().collect::<Vec<_>>(),
            vec![
                "tool_2", "tool_3", "tool_4", "tool_5", "tool_6", "tool_7", "tool_8", "tool_9"
            ]
        );
        assert_eq!(delta.admitted.len(), TOOL_ACTIVATION_CACHE_MAX_NAMES);
        assert!(delta.evicted.contains(&"tool_0".to_string()));
        assert!(delta.evicted.contains(&"tool_1".to_string()));
    }

    #[test]
    fn touching_a_cached_tool_makes_it_most_recent() {
        let catalog = (0..9)
            .map(|index| deferred_tool(&format!("tool_{index}"), 8))
            .collect::<Vec<_>>();
        let first_eight = catalog[..8]
            .iter()
            .map(|tool| tool.name.clone())
            .collect::<Vec<_>>();
        let mut cache = ToolActivationCache::default();
        cache.activate(&catalog, &first_eight);
        cache.activate(&catalog, &["tool_0".to_string()]);
        cache.activate(&catalog, &["tool_8".to_string()]);

        let names = cache.names().collect::<Vec<_>>();
        assert!(names.contains(&"tool_0"));
        assert!(!names.contains(&"tool_1"));
        assert_eq!(names.last().copied(), Some("tool_8"));
    }

    #[test]
    fn oversized_schema_is_never_admitted() {
        let catalog = vec![deferred_tool(
            "huge",
            TOOL_ACTIVATION_CACHE_MAX_SCHEMA_BYTES + 1,
        )];
        let mut cache = ToolActivationCache::default();
        let delta = cache.activate(&catalog, &["huge".to_string()]);

        assert_eq!(cache.names().count(), 0);
        assert_eq!(delta.rejected, vec!["huge"]);
    }

    #[test]
    fn revalidate_drops_removed_denied_or_eager_tools() {
        let catalog = vec![deferred_tool("kept", 8), deferred_tool("gone", 8)];
        let mut cache = ToolActivationCache::default();
        cache.activate(&catalog, &["kept".to_string(), "gone".to_string()]);
        let mut next_catalog = vec![deferred_tool("kept", 8), deferred_tool("gone", 8)];
        next_catalog[1].defer_loading = Some(false);

        let evicted = cache.revalidate(&next_catalog);
        assert_eq!(cache.names().collect::<Vec<_>>(), vec!["kept"]);
        assert_eq!(evicted, vec!["gone"]);
    }

    #[test]
    fn clearing_for_session_sync_forgets_all_activated_tools() {
        let catalog = vec![deferred_tool("one", 8), deferred_tool("two", 8)];
        let mut cache = ToolActivationCache::default();
        cache.activate(&catalog, &["one".to_string(), "two".to_string()]);

        cache.clear();

        assert_eq!(cache.names().count(), 0);
    }
}
