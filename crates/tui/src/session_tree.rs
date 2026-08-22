use crate::models::{ContentBlock, Message, Role};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
pub const CURRENT_JOURNAL_SCHEMA_VERSION: u32 = 1;
pub type EntryId = String;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SpawnDepth(pub u32);
impl SpawnDepth {
    pub fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEntryKind {
    Message {
        message: Message,
    },
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Compaction {
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens_before: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tokens_after: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    BranchSummary {
        branch_id: String,
        summary: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_branch_id: Option<String>,
    },
    System {
        content: String,
    },
}
impl SessionEntryKind {
    pub fn is_contextual(&self) -> bool {
        matches!(
            self,
            Self::Message { .. } | Self::User { .. } | Self::Assistant { .. }
        )
    }
    pub fn as_message(&self) -> Option<Message> {
        match self {
            Self::Message { message } => Some(message.clone()),
            Self::User { text } => Some(Message {
                role: Role::User,
                content: vec![ContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                }],
            }),
            Self::Assistant { text } => Some(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: text.clone(),
                    cache_control: None,
                }],
            }),
            Self::Compaction { summary, .. } => Some(Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: format!("[compaction summary] {summary}"),
                    cache_control: None,
                }],
            }),
            Self::BranchSummary { summary, .. } => Some(Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: format!("[branch summary] {summary}"),
                    cache_control: None,
                }],
            }),
            Self::System { content } => Some(Message {
                role: Role::System,
                content: vec![ContentBlock::Text {
                    text: content.clone(),
                    cache_control: None,
                }],
            }),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEntry {
    pub id: EntryId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<EntryId>,
    #[serde(flatten)]
    pub kind: SessionEntryKind,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub spawn_depth: u32,
}
impl SessionEntry {
    pub fn new(kind: SessionEntryKind, parent_id: Option<EntryId>, spawn_depth: u32) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            parent_id,
            kind,
            created_at: Utc::now(),
            spawn_depth,
        }
    }
    pub fn short_id(&self) -> &str {
        if self.id.len() >= 8 {
            &self.id[..8]
        } else {
            &self.id
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct SessionJournal {
    #[serde(default)]
    pub entries: Vec<SessionEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_id: Option<EntryId>,
    #[serde(default = "default_journal_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub spawn_depth: u32,
}
fn default_journal_schema_version() -> u32 {
    CURRENT_JOURNAL_SCHEMA_VERSION
}
impl SessionJournal {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            leaf_id: None,
            schema_version: CURRENT_JOURNAL_SCHEMA_VERSION,
            spawn_depth: 0,
        }
    }
    pub fn with_spawn_depth(depth: u32) -> Self {
        Self {
            spawn_depth: depth,
            ..Self::new()
        }
    }
    pub fn append(&mut self, kind: SessionEntryKind) -> EntryId {
        let entry = SessionEntry::new(kind, self.leaf_id.clone(), self.spawn_depth);
        let id = entry.id.clone();
        self.entries.push(entry);
        self.leaf_id = Some(id.clone());
        id
    }
    pub fn append_message(&mut self, message: Message) -> EntryId {
        self.append(SessionEntryKind::Message { message })
    }
    pub fn append_compaction(
        &mut self,
        summary: String,
        tokens_before: Option<u64>,
        tokens_after: Option<u64>,
        model: Option<String>,
    ) -> EntryId {
        self.append(SessionEntryKind::Compaction {
            summary,
            tokens_before,
            tokens_after,
            model,
        })
    }
    pub fn append_branch_summary(
        &mut self,
        branch_id: String,
        summary: String,
        parent_branch_id: Option<String>,
    ) -> EntryId {
        self.append(SessionEntryKind::BranchSummary {
            branch_id,
            summary,
            parent_branch_id,
        })
    }
    pub fn branch_to(&mut self, entry_id: &str) -> Result<(), String> {
        if self.entries.iter().any(|e| e.id == entry_id) {
            self.leaf_id = Some(entry_id.to_string());
            Ok(())
        } else {
            Err(format!("entry {entry_id} not found"))
        }
    }
    pub fn fork_from(&self, from_entry_id: Option<&str>) -> Result<Self, String> {
        let leaf = if let Some(id) = from_entry_id {
            if !self.entries.iter().any(|e| e.id == id) {
                return Err(format!("fork source {id} not found"));
            }
            Some(id.to_string())
        } else {
            self.leaf_id.clone()
        };
        Ok(Self {
            entries: self.entries.clone(),
            leaf_id: leaf,
            schema_version: self.schema_version,
            spawn_depth: self.spawn_depth.saturating_add(1),
        })
    }
    pub fn index(&self) -> HashMap<&str, &SessionEntry> {
        self.entries.iter().map(|e| (e.id.as_str(), e)).collect()
    }
    pub fn children_of(&self, parent_id: Option<&str>) -> Vec<&SessionEntry> {
        self.entries
            .iter()
            .filter(|e| e.parent_id.as_deref() == parent_id)
            .collect()
    }
    pub fn contains(&self, entry_id: &str) -> bool {
        self.entries.iter().any(|e| e.id == entry_id)
    }
    pub fn leaf(&self) -> Option<&SessionEntry> {
        self.leaf_id
            .as_deref()
            .and_then(|id| self.entries.iter().find(|e| e.id == id))
    }
    pub fn root_to_leaf(&self) -> Vec<&SessionEntry> {
        let index: HashMap<&str, &SessionEntry> =
            self.entries.iter().map(|e| (e.id.as_str(), e)).collect();
        let mut path = Vec::new();
        let mut cur = self.leaf_id.as_deref();
        let mut seen = HashSet::new();
        while let Some(id) = cur {
            if !seen.insert(id) {
                break;
            }
            if let Some(entry) = index.get(id) {
                path.push(*entry);
                cur = entry.parent_id.as_deref();
            } else {
                break;
            }
        }
        path.reverse();
        path
    }
    pub fn active_messages(&self, include_system: bool) -> Vec<Message> {
        self.root_to_leaf()
            .into_iter()
            .filter_map(|e| {
                if !include_system && !e.kind.is_contextual() {
                    return None;
                }
                e.kind.as_message()
            })
            .collect()
    }
    pub fn leaves(&self) -> Vec<&SessionEntry> {
        let parents: HashSet<&str> = self
            .entries
            .iter()
            .filter_map(|e| e.parent_id.as_deref())
            .collect();
        self.entries
            .iter()
            .filter(|e| !parents.contains(e.id.as_str()))
            .collect()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn validate(&self) -> Result<(), String> {
        let ids: HashSet<&str> = self.entries.iter().map(|e| e.id.as_str()).collect();
        for entry in &self.entries {
            if let Some(parent) = entry.parent_id.as_deref()
                && !ids.contains(parent)
            {
                return Err(format!("entry {} missing parent {}", entry.id, parent));
            }
        }
        if let Some(leaf) = self.leaf_id.as_deref()
            && !ids.contains(leaf)
        {
            return Err(format!("leaf {leaf} not found"));
        }
        Ok(())
    }
    pub fn from_messages(messages: Vec<Message>, spawn_depth: u32) -> Self {
        let mut j = Self::with_spawn_depth(spawn_depth);
        for msg in messages {
            j.append(SessionEntryKind::Message { message: msg });
        }
        j
    }
    pub fn to_messages(&self) -> Vec<Message> {
        self.active_messages(true)
    }

    /// Make `messages` the active projection without rewriting the journal.
    ///
    /// The existing active branch remains as evidence. We reuse its longest
    /// unchanged prefix, then append the repaired suffix as a sibling branch.
    pub fn rebranch_active_messages(&mut self, messages: &[Message]) {
        let active_path = self.root_to_leaf();
        let shared_prefix = active_path
            .iter()
            .zip(messages)
            .take_while(|(entry, message)| entry.kind.as_message().as_ref() == Some(*message))
            .count();
        self.leaf_id = shared_prefix
            .checked_sub(1)
            .map(|index| active_path[index].id.clone());
        for message in &messages[shared_prefix..] {
            self.append_message(message.clone());
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionImportContainer {
    pub format_version: u32,
    pub source: String,
    pub metadata: Option<serde_json::Value>,
    pub entries: Vec<SessionEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leaf_id: Option<EntryId>,
    pub exported_at: DateTime<Utc>,
    #[serde(default)]
    pub spawn_depth: u32,
}
impl SessionImportContainer {
    pub fn new(
        source: String,
        journal: &SessionJournal,
        metadata: Option<serde_json::Value>,
    ) -> Self {
        Self {
            format_version: CURRENT_JOURNAL_SCHEMA_VERSION,
            source,
            metadata,
            entries: journal.entries.clone(),
            leaf_id: journal.leaf_id.clone(),
            exported_at: Utc::now(),
            spawn_depth: journal.spawn_depth,
        }
    }
    pub fn into_journal(self) -> Result<SessionJournal, String> {
        let j = SessionJournal {
            entries: self.entries,
            leaf_id: self.leaf_id,
            schema_version: self.format_version,
            spawn_depth: self.spawn_depth,
        };
        j.validate()?;
        Ok(j)
    }
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}
pub fn render_tree(journal: &SessionJournal) -> String {
    if journal.entries.is_empty() {
        return "(empty session — no entries yet)".to_string();
    }
    let index = journal.index();
    let mut out = String::new();
    let mut children: HashMap<Option<&str>, Vec<&SessionEntry>> = HashMap::new();
    for entry in &journal.entries {
        children
            .entry(entry.parent_id.as_deref())
            .or_default()
            .push(entry);
    }
    let active_ids: HashSet<&str> = journal
        .root_to_leaf()
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    let leaf = journal.leaf_id.as_deref();
    fn render_node(
        out: &mut String,
        children: &HashMap<Option<&str>, Vec<&SessionEntry>>,
        active_ids: &HashSet<&str>,
        leaf: Option<&str>,
        parent: Option<&str>,
        depth: usize,
    ) {
        let Some(nodes) = children.get(&parent) else {
            return;
        };
        for (idx, entry) in nodes.iter().enumerate() {
            let is_last = idx + 1 == nodes.len();
            let prefix = if depth == 0 {
                "".to_string()
            } else {
                let mut p = String::new();
                for _ in 0..depth - 1 {
                    p.push_str("│  ");
                }
                if is_last {
                    p.push_str("└─ ");
                } else {
                    p.push_str("├─ ");
                }
                p
            };
            let marker = if Some(entry.id.as_str()) == leaf {
                "*"
            } else if active_ids.contains(entry.id.as_str()) {
                "●"
            } else {
                "○"
            };
            let kind_label = match &entry.kind {
                SessionEntryKind::Message { message } => {
                    let role = &message.role;
                    let snippet: String = message
                        .content
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let short: String = snippet.chars().take(60).collect();
                    format!("{role}: {short}")
                }
                SessionEntryKind::User { text } => {
                    let short: String = text.chars().take(60).collect();
                    format!("user: {short}")
                }
                SessionEntryKind::Assistant { text } => {
                    let short: String = text.chars().take(60).collect();
                    format!("assistant: {short}")
                }
                SessionEntryKind::Compaction { summary, .. } => {
                    let short: String = summary.chars().take(60).collect();
                    format!("compaction: {short}")
                }
                SessionEntryKind::BranchSummary {
                    branch_id, summary, ..
                } => {
                    let short: String = summary.chars().take(60).collect();
                    format!("branch:{} {short}", &branch_id[..branch_id.len().min(8)])
                }
                SessionEntryKind::System { content } => {
                    let short: String = content.chars().take(60).collect();
                    format!("system: {short}")
                }
            };
            out.push_str(&format!(
                "{prefix}{marker} {} [{}] {kind_label}\n",
                entry.short_id(),
                entry.id
            ));
            render_node(out, children, active_ids, leaf, Some(&entry.id), depth + 1);
        }
    }
    render_node(&mut out, &children, &active_ids, leaf, None, 0);
    let _ = index;
    if let Some(leaf_id) = leaf {
        out.push_str(&format!(
            "\nleaf: {leaf_id} (active, {} entries)\n",
            journal.entries.len()
        ));
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ContentBlock, Message, Role};
    fn msg(role: &str, text: &str) -> Message {
        Message {
            role: Role::from(role),
            content: vec![ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
        }
    }
    #[test]
    fn append_creates_child_of_leaf() {
        let mut j = SessionJournal::new();
        let a = j.append(SessionEntryKind::User {
            text: "hello".into(),
        });
        let b = j.append(SessionEntryKind::Assistant {
            text: "world".into(),
        });
        assert_eq!(j.leaf_id.as_deref(), Some(b.as_str()));
        let be = j.entries.iter().find(|e| e.id == b).unwrap();
        assert_eq!(be.parent_id.as_deref(), Some(a.as_str()));
    }
    #[test]
    fn branch_moves_leaf_only() {
        let mut j = SessionJournal::new();
        let a = j.append(SessionEntryKind::User { text: "a".into() });
        let b = j.append(SessionEntryKind::User { text: "b".into() });
        let _c = j.append(SessionEntryKind::User { text: "c".into() });
        j.branch_to(&a).unwrap();
        let d = j.append(SessionEntryKind::User { text: "d".into() });
        assert_eq!(j.entries.len(), 4);
        assert_eq!(j.leaf_id.as_deref(), Some(d.as_str()));
        let path: Vec<String> = j.root_to_leaf().iter().map(|e| e.id.clone()).collect();
        assert_eq!(path, vec![a.clone(), d.clone()]);
        assert!(j.entries.iter().any(|e| e.id == b));
    }
    #[test]
    fn from_messages_migrates() {
        let msgs = vec![msg("user", "hi"), msg("assistant", "hello")];
        let j = SessionJournal::from_messages(msgs, 0);
        assert_eq!(j.entries.len(), 2);
        assert!(j.validate().is_ok());
        assert_eq!(j.root_to_leaf().len(), 2);
    }
    #[test]
    fn repaired_messages_form_an_append_only_sibling_branch() {
        let original = vec![
            msg("user", "shared"),
            msg("assistant", "broken"),
            msg("user", "old tail"),
        ];
        let mut journal = SessionJournal::from_messages(original, 0);
        let old_leaf = journal.leaf_id.clone().expect("old leaf");
        let repaired = vec![
            msg("user", "shared"),
            msg("assistant", "repaired"),
            msg("user", "new tail"),
        ];

        journal.rebranch_active_messages(&repaired);

        assert_eq!(journal.to_messages(), repaired);
        assert!(journal.contains(&old_leaf), "old evidence must remain");
        assert_eq!(
            journal.entries.len(),
            5,
            "one shared entry plus two branches"
        );
    }
    #[test]
    fn compaction_fits() {
        let mut j = SessionJournal::new();
        let id = j.append_compaction("summary".into(), Some(1000), Some(100), None);
        assert!(j.contains(&id));
        assert!(matches!(
            j.leaf().unwrap().kind,
            SessionEntryKind::Compaction { .. }
        ));
    }
    #[test]
    fn branch_summary_fits() {
        let mut j = SessionJournal::new();
        let a = j.append(SessionEntryKind::User {
            text: "root".into(),
        });
        let b = j.append_branch_summary(a.clone(), "branch summary".into(), None);
        assert!(j.contains(&b));
    }
    #[test]
    fn spawn_depth_fork() {
        let mut j = SessionJournal::with_spawn_depth(1);
        let forked = j.fork_from(None).unwrap();
        assert_eq!(forked.spawn_depth, 2);
        let a = j.append(SessionEntryKind::User {
            text: "root".into(),
        });
        let fork2 = j.fork_from(Some(&a)).unwrap();
        assert_eq!(fork2.spawn_depth, 2);
    }
    #[test]
    fn foreign_roundtrip() {
        let mut j = SessionJournal::new();
        j.append(SessionEntryKind::User {
            text: "hello".into(),
        });
        let c = SessionImportContainer::new("codewhale".into(), &j, None);
        let json = c.to_json().unwrap();
        let back = SessionImportContainer::from_json(&json).unwrap();
        let j2 = back.into_journal().unwrap();
        assert_eq!(j.entries.len(), j2.entries.len());
    }
    #[test]
    fn render_marks_active() {
        let mut j = SessionJournal::new();
        j.append(SessionEntryKind::User {
            text: "root".into(),
        });
        let tree = render_tree(&j);
        assert!(tree.contains('*'));
    }
    #[test]
    fn active_messages_root_to_leaf() {
        let mut j = SessionJournal::new();
        j.append(SessionEntryKind::User { text: "a".into() });
        j.append(SessionEntryKind::User { text: "b".into() });
        let msgs = j.active_messages(false);
        assert_eq!(msgs.len(), 2);
        j.branch_to(&j.entries[0].id.clone()).unwrap();
        j.append(SessionEntryKind::User { text: "c".into() });
        let msgs2 = j.active_messages(false);
        assert_eq!(msgs2.len(), 2);
    }
}
