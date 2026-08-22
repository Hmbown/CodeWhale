mod models {
    pub use codewhale_core::request::{ContentBlock, Message};
    pub use codewhale_core::role::Role;
}
#[path = "../src/session_tree.rs"]
#[allow(dead_code)] // The probe intentionally exercises only the journal hot paths.
mod session_tree;

use models::Message;
use serde::{Deserialize, Serialize};
use session_tree::SessionJournal;
use std::io::Write;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Probe {
    #[serde(default)]
    schema_version: u32,
    metadata: serde_json::Value,
    messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    journal: Option<SessionJournal>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    leaf_id: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

fn main() {
    let path = std::env::args().nth(1).expect("path");
    let raw = std::fs::read_to_string(&path).unwrap();
    println!("file bytes: {}", raw.len());

    let t = Instant::now();
    let s: Probe = serde_json::from_str(&raw).unwrap();
    println!("typed parse: {:?}", t.elapsed());
    println!(
        "messages: {} journal entries: {}",
        s.messages.len(),
        s.journal.as_ref().map(|j| j.entries.len()).unwrap_or(0)
    );

    for _ in 0..3 {
        let t = Instant::now();
        let out = serde_json::to_string_pretty(&s).unwrap();
        println!(
            "to_string_pretty FULL: {:?} ({} bytes)",
            t.elapsed(),
            out.len()
        );
    }
    let mut dedup = s.clone();
    dedup.messages = Vec::new();
    for _ in 0..3 {
        let t = Instant::now();
        let out = serde_json::to_string_pretty(&dedup).unwrap();
        println!(
            "to_string_pretty JOURNAL-ONLY: {:?} ({} bytes)",
            t.elapsed(),
            out.len()
        );
    }
    for _ in 0..3 {
        let t = Instant::now();
        let c = s.clone();
        println!(
            "Probe::clone (== SavedSession::clone): {:?} ({} msgs)",
            t.elapsed(),
            c.messages.len()
        );
    }
    let j = s.journal.as_ref().unwrap();
    for _ in 0..3 {
        let t = Instant::now();
        let m = j.to_messages();
        println!(
            "journal.to_messages (deep clone): {:?} ({} msgs)",
            t.elapsed(),
            m.len()
        );
    }
    for _ in 0..3 {
        let t = Instant::now();
        let jj = SessionJournal::from_messages(s.messages.clone(), 0);
        println!(
            "from_messages(messages.to_vec()) [UI-thread snapshot build]: {:?} ({} entries)",
            t.elapsed(),
            jj.entries.len()
        );
    }
    for _ in 0..3 {
        let t = Instant::now();
        let eq = s.messages == j.to_messages();
        println!(
            "storage_compatible_copy compare: {:?} (eq={})",
            t.elapsed(),
            eq
        );
    }
    let content = serde_json::to_string_pretty(&s).unwrap();
    let dir = std::env::temp_dir();
    for _ in 0..3 {
        let t = Instant::now();
        let mut f = tempfile::NamedTempFile::new_in(&dir).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.as_file().sync_all().unwrap();
        let p = dir.join("zz_perf_probe_out.json");
        f.persist(&p).unwrap();
        println!(
            "atomic write+fsync {} bytes: {:?}",
            content.len(),
            t.elapsed()
        );
    }
    let dc = serde_json::to_string_pretty(&dedup).unwrap();
    for _ in 0..3 {
        let t = Instant::now();
        let mut f = tempfile::NamedTempFile::new_in(&dir).unwrap();
        f.write_all(dc.as_bytes()).unwrap();
        f.as_file().sync_all().unwrap();
        let p = dir.join("zz_perf_probe_out2.json");
        f.persist(&p).unwrap();
        println!(
            "atomic write+fsync DEDUP {} bytes: {:?}",
            dc.len(),
            t.elapsed()
        );
    }
}
