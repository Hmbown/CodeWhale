//! Lightweight startup milestone tracing (#3757).
//!
//! Records named milestones against a single process-start instant and emits
//! one summary line to the runtime log when the TUI enters its event loop.
//! Milestones are buffered in memory because most of them occur before the
//! runtime log is initialized; the summary is the artifact, not the events.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static PROCESS_START: OnceLock<Instant> = OnceLock::new();
static MILESTONES: Mutex<Vec<(&'static str, u64)>> = Mutex::new(Vec::new());

/// Pin the process-start instant. First call wins; later calls are no-ops so
/// tests and alternate entry points cannot skew the timeline.
pub fn mark_process_start() {
    let _ = PROCESS_START.set(Instant::now());
}

/// Record `label` at the current elapsed time since process start. No-op if
/// [`mark_process_start`] was never called (e.g. non-interactive subcommands).
pub fn mark(label: &'static str) {
    let Some(start) = PROCESS_START.get() else {
        return;
    };
    let elapsed_ms = start.elapsed().as_millis() as u64;
    if let Ok(mut milestones) = MILESTONES.lock() {
        milestones.push((label, elapsed_ms));
    }
}

/// Emit the buffered milestones as one summary line and clear the buffer.
/// Called once the runtime log exists (just before the event loop starts).
pub fn log_summary() {
    let Some(start) = PROCESS_START.get() else {
        return;
    };
    let total_ms = start.elapsed().as_millis() as u64;
    let Ok(mut milestones) = MILESTONES.lock() else {
        return;
    };
    let line = milestones
        .iter()
        .map(|(label, ms)| format!("{label}={ms}ms"))
        .collect::<Vec<_>>()
        .join(" ");
    milestones.clear();
    tracing::info!(target: "startup", "startup {line} event_loop={total_ms}ms");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn milestones_accumulate_and_summary_drains() {
        mark_process_start();
        mark("alpha");
        mark("beta");
        {
            let milestones = MILESTONES.lock().unwrap();
            let labels: Vec<&str> = milestones.iter().map(|(l, _)| *l).collect();
            assert!(labels.contains(&"alpha"));
            assert!(labels.contains(&"beta"));
        }
        log_summary();
        assert!(MILESTONES.lock().unwrap().is_empty());
    }
}
