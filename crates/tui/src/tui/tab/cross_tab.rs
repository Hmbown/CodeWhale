//! Cross-tab collaboration events

// WIP collaboration surface — narrow harvest. See `tab/mod.rs` for the
// PR #2753 context.
#![allow(dead_code)]

use super::{Priority, TabId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A link between two tabs for collaboration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossTabLink {
    pub from: TabId,
    pub to: TabId,
    pub created_at: DateTime<Utc>,
}

/// Cross-tab event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CrossTabEvent {
    /// Task delegation request
    TaskDelegation {
        task_id: String,
        from_tab: TabId,
        to_tab: TabId,
        description: String,
        priority: Priority,
        created_at: DateTime<Utc>,
    },
    /// Review request
    ReviewRequest {
        request_id: String,
        from_tab: TabId,
        to_tab: TabId,
        content_ref: String,
        criteria: Vec<String>,
        created_at: DateTime<Utc>,
    },
    /// Meeting invitation
    MeetingInvite {
        meeting_id: String,
        from_tab: TabId,
        participants: Vec<TabId>,
        topic: String,
        created_at: DateTime<Utc>,
    },
    /// Result returned from delegation
    ResultReturn {
        task_id: String,
        from_tab: TabId,
        to_tab: TabId,
        result: String,
        created_at: DateTime<Utc>,
    },
    /// Context sync between tabs
    ContextSync {
        tab_ids: Vec<TabId>,
        changes: Vec<ContextChange>,
        created_at: DateTime<Utc>,
    },
}

impl CrossTabEvent {
    /// Get the sender tab ID
    #[allow(clippy::wrong_self_convention)] // getter, not a constructor
    pub fn from_tab(&self) -> TabId {
        match self {
            Self::TaskDelegation { from_tab, .. } => *from_tab,
            Self::ReviewRequest { from_tab, .. } => *from_tab,
            Self::MeetingInvite { from_tab, .. } => *from_tab,
            Self::ResultReturn { from_tab, .. } => *from_tab,
            // ContextSync has no single sender; return the first participant
            // if any, otherwise a sentinel `TabId(0)`. Callers that need to
            // distinguish "no sender" should match on the variant directly.
            Self::ContextSync { tab_ids, .. } => tab_ids.first().copied().unwrap_or(TabId(0)),
        }
    }

    /// Get the target tab ID (if applicable)
    pub fn to_tab(&self) -> Option<TabId> {
        match self {
            Self::TaskDelegation { to_tab, .. } => Some(*to_tab),
            Self::ReviewRequest { to_tab, .. } => Some(*to_tab),
            Self::MeetingInvite { participants, .. } => participants.first().copied(),
            Self::ResultReturn { to_tab, .. } => Some(*to_tab),
            Self::ContextSync { .. } => None,
        }
    }

    /// Get creation timestamp
    pub fn created_at(&self) -> DateTime<Utc> {
        match self {
            Self::TaskDelegation { created_at, .. } => *created_at,
            Self::ReviewRequest { created_at, .. } => *created_at,
            Self::MeetingInvite { created_at, .. } => *created_at,
            Self::ResultReturn { created_at, .. } => *created_at,
            Self::ContextSync { created_at, .. } => *created_at,
        }
    }
}

/// Type of context change
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContextChangeType {
    VariableSet(String),
    VariableRemoved(String),
    MessageAdded,
    FileModified(String),
    StateUpdate,
}

/// A change in shared context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextChange {
    pub change_type: ContextChangeType,
    pub value: Option<String>,
    pub timestamp: DateTime<Utc>,
}

impl ContextChange {
    pub fn variable_set(name: &str, value: &str) -> Self {
        Self {
            change_type: ContextChangeType::VariableSet(name.to_string()),
            value: Some(value.to_string()),
            timestamp: Utc::now(),
        }
    }

    pub fn file_modified(path: &str) -> Self {
        Self {
            change_type: ContextChangeType::FileModified(path.to_string()),
            value: None,
            timestamp: Utc::now(),
        }
    }
}

/// Shared context between linked tabs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedContext {
    pub participants: Vec<TabId>,
    pub shared_variables: HashMap<String, String>,
    pub shared_messages: Vec<SharedMessage>,
    pub meeting_notes: Vec<String>,
    pub created_at: DateTime<Utc>,
}

impl SharedContext {
    pub fn new(participants: Vec<TabId>) -> Self {
        Self {
            participants,
            shared_variables: HashMap::new(),
            shared_messages: Vec::new(),
            meeting_notes: Vec::new(),
            created_at: Utc::now(),
        }
    }

    pub fn add_variable(&mut self, name: &str, value: &str) {
        self.shared_variables
            .insert(name.to_string(), value.to_string());
    }

    pub fn get_variable(&self, name: &str) -> Option<&String> {
        self.shared_variables.get(name)
    }

    pub fn add_message(&mut self, sender: TabId, content: &str) {
        self.shared_messages.push(SharedMessage {
            sender,
            content: content.to_string(),
            timestamp: Utc::now(),
        });
    }

    pub fn add_meeting_note(&mut self, note: &str) {
        self.meeting_notes.push(note.to_string());
    }
}

/// A message in shared context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedMessage {
    pub sender: TabId,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}
