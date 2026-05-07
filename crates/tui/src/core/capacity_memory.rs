//! Persistent memory snapshots for capacity controller interventions.

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Canonical compact state persisted by interventions.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CanonicalState {
    pub goal: String,
    pub constraints: Vec<String>,
    pub confirmed_facts: Vec<String>,
    pub open_loops: Vec<String>,
    pub pending_actions: Vec<String>,
    pub critical_refs: Vec<String>,
}

/// Replay verification metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayInfo {
    pub tool_id: String,
    pub tool_name: String,
    pub pass: bool,
    pub diff_summary: String,
}

/// JSONL record written for each intervention.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacityMemoryRecord {
    pub id: String,
    pub ts: String,
    pub turn_index: u64,
    pub action_trigger: String,
    pub h_hat: f64,
    pub c_hat: f64,
    pub slack: f64,
    pub risk_band: String,
    pub canonical_state: CanonicalState,
    pub source_message_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay_info: Option<ReplayInfo>,
}

fn capacity_memory_dirs() -> Vec<PathBuf> {
    if let Ok(raw) = std::env::var("DEEPSEEK_CAPACITY_MEMORY_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return vec![PathBuf::from(shellexpand::tilde(trimmed).as_ref())];
        }
    }

    let mut dirs = Vec::new();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".deepseek").join("memory"));
    }

    let cwd = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".deepseek")
        .join("memory");
    dirs.push(cwd);

    dirs.dedup();
    dirs
}

pub fn append_capacity_record(session_id: &str, record: &CapacityMemoryRecord) -> Result<PathBuf> {
    let candidates = candidate_session_memory_paths(session_id);
    append_capacity_record_to_candidates(&candidates, record)
}

pub fn append_capacity_record_to_path(path: &Path, record: &CapacityMemoryRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create memory directory {}", parent.display()))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open memory log {}", path.display()))?;
    let line =
        serde_json::to_string(record).context("Failed to serialize capacity memory record")?;
    writeln!(file, "{line}")
        .with_context(|| format!("Failed to write memory record {}", path.display()))?;
    Ok(())
}

pub fn load_last_k_capacity_records(
    session_id: &str,
    k: usize,
) -> Result<Vec<CapacityMemoryRecord>> {
    let candidates = candidate_session_memory_paths(session_id);
    load_last_k_capacity_records_from_candidates(&candidates, k)
}

pub(super) fn load_last_k_capacity_records_from_path(
    path: &Path,
    k: usize,
) -> Result<Vec<CapacityMemoryRecord>> {
    if k == 0 || !path.exists() {
        return Ok(Vec::new());
    }

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("Failed to open memory log {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();

    for line in reader.lines() {
        let line = line.with_context(|| format!("Failed reading {}", path.display()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<CapacityMemoryRecord>(&line) {
            records.push(record);
        }
    }

    if records.len() > k {
        Ok(records.split_off(records.len() - k))
    } else {
        Ok(records)
    }
}

fn candidate_session_memory_paths(session_id: &str) -> Vec<PathBuf> {
    capacity_memory_dirs()
        .into_iter()
        .map(|dir| dir.join(format!("{session_id}.jsonl")))
        .collect()
}

fn append_capacity_record_to_candidates(
    paths: &[PathBuf],
    record: &CapacityMemoryRecord,
) -> Result<PathBuf> {
    let mut last_err: Option<anyhow::Error> = None;
    for path in paths {
        match append_capacity_record_to_path(path, record) {
            Ok(()) => return Ok(path.clone()),
            Err(err) => last_err = Some(err),
        }
    }

    Err(last_err.unwrap_or_else(|| anyhow!("No capacity memory path candidates available")))
}

fn load_last_k_capacity_records_from_candidates(
    paths: &[PathBuf],
    k: usize,
) -> Result<Vec<CapacityMemoryRecord>> {
    if k == 0 {
        return Ok(Vec::new());
    }

    let mut newest: Option<(SystemTime, Vec<CapacityMemoryRecord>)> = None;
    let mut last_err: Option<anyhow::Error> = None;

    for path in paths {
        if !path.exists() {
            continue;
        }

        match load_last_k_capacity_records_from_path(path, k) {
            Ok(records) => {
                if records.is_empty() {
                    continue;
                }
                let modified = fs::metadata(path)
                    .and_then(|meta| meta.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let should_replace = newest
                    .as_ref()
                    .map(|(current, _)| modified >= *current)
                    .unwrap_or(true);
                if should_replace {
                    newest = Some((modified, records));
                }
            }
            Err(err) => last_err = Some(err),
        }
    }

    if let Some((_, records)) = newest {
        return Ok(records);
    }
    if let Some(err) = last_err {
        return Err(err);
    }
    Ok(Vec::new())
}

#[must_use]
pub fn new_record_id() -> String {
    format!("cap_{}", &uuid::Uuid::new_v4().to_string()[..8])
}

#[must_use]
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// Scan all memory directories for the newest record across ALL sessions.
/// Returns (session_id, record) if any records exist.
pub fn find_latest_cross_session() -> Option<(String, CapacityMemoryRecord)> {
    let dirs = capacity_memory_dirs();
    let mut newest: Option<(SystemTime, String, CapacityMemoryRecord)> = None;

    for dir in &dirs {
        if !dir.exists() {
            continue;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Some(session_id) = path.file_stem().and_then(|s| s.to_str()).map(String::from)
            else {
                continue;
            };
            let Ok(records) = load_last_k_capacity_records_from_path(&path, 1) else {
                continue;
            };
            let Some(record) = records.into_iter().last() else {
                continue;
            };
            let Ok(meta) = fs::metadata(&path) else {
                continue;
            };
            let Ok(modified) = meta.modified() else {
                continue;
            };
            if newest
                .as_ref()
                .map(|(m, _, _)| modified >= *m)
                .unwrap_or(true)
            {
                newest = Some((modified, session_id, record));
            }
        }
    }

    newest.map(|(_, sid, rec)| (sid, rec))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn memory_jsonl_round_trip() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("session.jsonl");

        let record = CapacityMemoryRecord {
            id: "cap_1".to_string(),
            ts: now_rfc3339(),
            turn_index: 2,
            action_trigger: "targeted_context_refresh".to_string(),
            h_hat: 1.2,
            c_hat: 3.8,
            slack: 2.6,
            risk_band: "medium".to_string(),
            canonical_state: CanonicalState {
                goal: "Ship feature".to_string(),
                ..CanonicalState::default()
            },
            source_message_ids: vec!["m1".to_string()],
            replay_info: None,
        };

        append_capacity_record_to_path(&path, &record).expect("append");
        let records = load_last_k_capacity_records_from_path(&path, 1).expect("load");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].canonical_state.goal, "Ship feature");
    }

    #[test]
    fn append_falls_back_to_next_candidate_path() {
        let tmp = tempdir().expect("tempdir");
        let blocked_root = tmp.path().join("blocked");
        fs::write(&blocked_root, "file").expect("create blocking file");
        let blocked_path = blocked_root.join("session.jsonl");
        let fallback_path = tmp.path().join("fallback").join("session.jsonl");

        let record = CapacityMemoryRecord {
            id: "cap_fallback".to_string(),
            ts: now_rfc3339(),
            turn_index: 1,
            action_trigger: "targeted_context_refresh".to_string(),
            h_hat: 1.0,
            c_hat: 3.8,
            slack: 2.8,
            risk_band: "medium".to_string(),
            canonical_state: CanonicalState::default(),
            source_message_ids: vec!["m1".to_string()],
            replay_info: None,
        };

        let chosen = append_capacity_record_to_candidates(
            &[blocked_path.clone(), fallback_path.clone()],
            &record,
        )
        .expect("append with fallback");
        assert_eq!(chosen, fallback_path);
        assert!(chosen.exists());
    }

    #[test]
    fn load_prefers_newest_candidate_records() {
        let tmp = tempdir().expect("tempdir");
        let older = tmp.path().join("older.jsonl");
        let newer = tmp.path().join("newer.jsonl");

        let old_record = CapacityMemoryRecord {
            id: "cap_old".to_string(),
            ts: now_rfc3339(),
            turn_index: 1,
            action_trigger: "targeted_context_refresh".to_string(),
            h_hat: 1.0,
            c_hat: 3.8,
            slack: 2.8,
            risk_band: "medium".to_string(),
            canonical_state: CanonicalState {
                goal: "old".to_string(),
                ..CanonicalState::default()
            },
            source_message_ids: vec!["m1".to_string()],
            replay_info: None,
        };
        let new_record = CapacityMemoryRecord {
            id: "cap_new".to_string(),
            ts: now_rfc3339(),
            turn_index: 2,
            action_trigger: "verify_and_replan".to_string(),
            h_hat: 1.4,
            c_hat: 3.8,
            slack: 2.4,
            risk_band: "high".to_string(),
            canonical_state: CanonicalState {
                goal: "new".to_string(),
                ..CanonicalState::default()
            },
            source_message_ids: vec!["m2".to_string()],
            replay_info: None,
        };

        append_capacity_record_to_path(&older, &old_record).expect("write older");
        std::thread::sleep(std::time::Duration::from_millis(10));
        append_capacity_record_to_path(&newer, &new_record).expect("write newer");

        let records = load_last_k_capacity_records_from_candidates(&[older, newer], 1)
            .expect("load newest records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].canonical_state.goal, "new");
    }
}
