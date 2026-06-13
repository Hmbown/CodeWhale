//! Meeting manager for multi-agent discussions

// WIP collaboration surface — narrow harvest. See `tab/mod.rs` for the
// PR #2753 context.
#![allow(dead_code)]

use super::TabId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Status of a meeting
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeetingStatus {
    Active,
    Paused,
    Ended,
}

/// Type of meeting message
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeetingMessageType {
    Regular,
    Question,
    Answer,
    Proposal,
    Agreement,
    Objection,
    Summary,
}

/// A message in a meeting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingMessage {
    pub id: u64,
    pub sender: TabId,
    pub content: String,
    pub message_type: MeetingMessageType,
    pub timestamp: DateTime<Utc>,
}

impl MeetingMessage {
    pub fn new(id: u64, sender: TabId, content: String) -> Self {
        Self {
            id,
            sender,
            content,
            message_type: MeetingMessageType::Regular,
            timestamp: Utc::now(),
        }
    }

    pub fn question(id: u64, sender: TabId, content: String) -> Self {
        Self {
            id,
            sender,
            content,
            message_type: MeetingMessageType::Question,
            timestamp: Utc::now(),
        }
    }

    pub fn proposal(id: u64, sender: TabId, content: String) -> Self {
        Self {
            id,
            sender,
            content,
            message_type: MeetingMessageType::Proposal,
            timestamp: Utc::now(),
        }
    }
}

/// A decision made in a meeting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingDecision {
    pub id: u64,
    pub description: String,
    pub proposer: TabId,
    pub supporters: Vec<TabId>,
    pub timestamp: DateTime<Utc>,
}

impl MeetingDecision {
    pub fn new(id: u64, description: String, proposer: TabId) -> Self {
        Self {
            id,
            description,
            proposer,
            supporters: vec![proposer],
            timestamp: Utc::now(),
        }
    }

    pub fn add_supporter(&mut self, tab_id: TabId) {
        if !self.supporters.contains(&tab_id) {
            self.supporters.push(tab_id);
        }
    }

    pub fn support_count(&self) -> usize {
        self.supporters.len()
    }
}

/// A meeting session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Meeting {
    pub id: String,
    pub topic: String,
    pub participants: Vec<TabId>,
    pub messages: Vec<MeetingMessage>,
    pub decisions: Vec<MeetingDecision>,
    pub status: MeetingStatus,
    pub created_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

impl Meeting {
    pub fn new(id: String, topic: String, participants: Vec<TabId>) -> Self {
        Self {
            id,
            topic,
            participants,
            messages: Vec::new(),
            decisions: Vec::new(),
            status: MeetingStatus::Active,
            created_at: Utc::now(),
            ended_at: None,
        }
    }

    pub fn add_message(&mut self, msg: MeetingMessage) {
        self.messages.push(msg);
    }

    pub fn add_decision(&mut self, decision: MeetingDecision) {
        self.decisions.push(decision);
    }

    pub fn end(&mut self) {
        self.status = MeetingStatus::Ended;
        self.ended_at = Some(Utc::now());
    }

    pub fn duration(&self) -> chrono::Duration {
        let end = self.ended_at.unwrap_or_else(Utc::now);
        end - self.created_at
    }

    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    pub fn decision_count(&self) -> usize {
        self.decisions.len()
    }
}

/// Summary of a completed meeting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingSummary {
    pub id: String,
    pub topic: String,
    pub participant_count: usize,
    pub message_count: usize,
    pub decision_count: usize,
    pub duration_seconds: i64,
    pub decisions: Vec<String>,
}

/// Meeting history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingHistory {
    pub summary: MeetingSummary,
    pub created_at: DateTime<Utc>,
}

/// Manager for meeting sessions
pub struct MeetingManager {
    active_meetings: HashMap<String, Meeting>,
    history: Vec<MeetingHistory>,
    next_meeting_id: u64,
    next_message_id: u64,
    next_decision_id: u64,
}

impl MeetingManager {
    pub fn new() -> Self {
        Self {
            active_meetings: HashMap::new(),
            history: Vec::new(),
            next_meeting_id: 1,
            next_message_id: 1,
            next_decision_id: 1,
        }
    }

    /// Start a new meeting
    pub fn start_meeting(&mut self, topic: String, participants: Vec<TabId>) -> Option<String> {
        if participants.len() < 2 {
            return None;
        }

        let meeting_id = self.generate_meeting_id();
        let meeting = Meeting::new(meeting_id.clone(), topic, participants);
        self.active_meetings.insert(meeting_id.clone(), meeting);
        Some(meeting_id)
    }

    /// Get an active meeting by ID
    pub fn get_meeting(&self, meeting_id: &str) -> Option<&Meeting> {
        self.active_meetings.get(meeting_id)
    }

    /// Get a mutable meeting by ID
    pub fn get_meeting_mut(&mut self, meeting_id: &str) -> Option<&mut Meeting> {
        self.active_meetings.get_mut(meeting_id)
    }

    /// Add a message to a meeting
    pub fn add_message(&mut self, meeting_id: &str, msg: MeetingMessage) {
        if let Some(meeting) = self.active_meetings.get_mut(meeting_id) {
            meeting.add_message(msg);
        }
    }

    /// Create and add a new message
    pub fn create_message(
        &mut self,
        meeting_id: &str,
        sender: TabId,
        content: String,
    ) -> Option<u64> {
        let msg_id = self.next_message_id;
        self.next_message_id += 1;

        if let Some(meeting) = self.active_meetings.get_mut(meeting_id) {
            let msg = MeetingMessage::new(msg_id, sender, content);
            meeting.add_message(msg);
            Some(msg_id)
        } else {
            None
        }
    }

    /// Add a decision to a meeting
    pub fn add_decision(&mut self, meeting_id: &str, decision: MeetingDecision) {
        if let Some(meeting) = self.active_meetings.get_mut(meeting_id) {
            meeting.add_decision(decision);
        }
    }

    /// Create and add a new decision
    pub fn create_decision(
        &mut self,
        meeting_id: &str,
        description: String,
        proposer: TabId,
    ) -> Option<u64> {
        let decision_id = self.next_decision_id;
        self.next_decision_id += 1;

        if let Some(meeting) = self.active_meetings.get_mut(meeting_id) {
            let decision = MeetingDecision::new(decision_id, description, proposer);
            meeting.add_decision(decision);
            Some(decision_id)
        } else {
            None
        }
    }

    /// End a meeting
    pub fn end_meeting(&mut self, meeting_id: &str) -> Option<MeetingSummary> {
        if let Some(mut meeting) = self.active_meetings.remove(meeting_id) {
            meeting.end();
            let duration = meeting.duration();
            let summary = MeetingSummary {
                id: meeting.id.clone(),
                topic: meeting.topic.clone(),
                participant_count: meeting.participants.len(),
                message_count: meeting.message_count(),
                decision_count: meeting.decision_count(),
                duration_seconds: duration.num_seconds(),
                decisions: meeting
                    .decisions
                    .iter()
                    .map(|d| d.description.clone())
                    .collect(),
            };
            self.history.push(MeetingHistory {
                summary: summary.clone(),
                created_at: meeting.created_at,
            });
            Some(summary)
        } else {
            None
        }
    }

    /// Get active meeting for a specific tab
    pub fn active_meeting_for(&self, tab_id: TabId) -> Option<&Meeting> {
        self.active_meetings
            .values()
            .find(|m| m.participants.contains(&tab_id))
    }

    /// Get all active meetings
    pub fn active_meetings(&self) -> Vec<&Meeting> {
        self.active_meetings.values().collect()
    }

    /// Get meeting history
    pub fn history(&self) -> &[MeetingHistory] {
        &self.history
    }

    /// Get recent meetings
    pub fn recent_meetings(&self, limit: usize) -> Vec<&MeetingHistory> {
        let mut history: Vec<&MeetingHistory> = self.history.iter().collect();
        history.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        history.into_iter().take(limit).collect()
    }

    /// Check if a tab is in any active meeting
    pub fn is_in_meeting(&self, tab_id: TabId) -> bool {
        self.active_meetings
            .values()
            .any(|m| m.participants.contains(&tab_id))
    }

    fn generate_meeting_id(&mut self) -> String {
        let id = self.next_meeting_id;
        self.next_meeting_id += 1;
        format!("meeting_{}", id)
    }
}

impl Default for MeetingManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_meeting_lifecycle() {
        let mut manager = MeetingManager::new();
        let tab1 = TabId::new(1);
        let tab2 = TabId::new(2);

        let meeting_id = manager
            .start_meeting("Discuss design".to_string(), vec![tab1, tab2])
            .unwrap();

        assert!(manager.is_in_meeting(tab1));
        assert!(manager.is_in_meeting(tab2));
        assert!(!manager.is_in_meeting(TabId::new(3)));

        manager.create_message(&meeting_id, tab1, "Let's start".to_string());
        manager.create_message(&meeting_id, tab2, "Agreed".to_string());

        let meeting = manager.get_meeting(&meeting_id).unwrap();
        assert_eq!(meeting.message_count(), 2);

        manager.create_decision(&meeting_id, "Use component pattern".to_string(), tab1);

        let summary = manager.end_meeting(&meeting_id).unwrap();
        assert_eq!(summary.topic, "Discuss design");
        assert_eq!(summary.participant_count, 2);
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.decision_count, 1);
    }
}
