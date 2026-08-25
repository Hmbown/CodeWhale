//! Generate an outbox fixture with interleaved *cross-process* writers for
//! the outbox E2E verifier gate.
//!
//! Usage:
//!
//! ```text
//! interleaved_outbox_fixture <outbox-path> [children]
//! ```
//!
//! The parent spawns `children` (default 6) copies of itself in `--child`
//! mode, each writing one `turn_start`/`turn_end` pair to the same shared
//! outbox through the outbox's cross-process exclusive lock, with a
//! per-child sleep between the pair's two lines so the appends interleave.
//! The parent then appends a `turn_start` for a "killed" session (the
//! SIGKILL shape: a start with no end) and runs the boot reconciliation
//! for that session, which appends the synthetic `turn_end`.
//!
//! The resulting JSONL is the verifier fixture: seq must be strictly
//! monotonic in file order (B2 — writers raced otherwise) and every turn
//! must be 1:1 paired (G1/turn pairing — the reconciled end pairs the
//! killed session's start). Each writer contributes exactly one turn pair,
//! so the verifier's cadence check has no end→start gaps to measure.
//!
//! All appends use [`LifecycleOutbox::emit_blocking`], the synchronous
//! runtime-free primitive, so every process appends deterministically
//! before it exits.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use codewhale_hooks::{LifecycleEvent, LifecycleOutbox};
use serde_json::json;

/// Default number of child writer processes.
const DEFAULT_CHILDREN: usize = 6;

/// Thread id of the "killed" session whose unpaired start the parent
/// reconciles.
const KILLED_THREAD: &str = "sess_fixture_killed";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--child") {
        let index: usize = args
            .get(1)
            .expect("--child <index>")
            .parse()
            .expect("child index");
        let path = PathBuf::from(args.get(2).expect("--child <index> <path>"));
        child_writer(index, path);
        return;
    }

    let path = PathBuf::from(
        args.first()
            .expect("usage: interleaved_outbox_fixture <outbox-path> [children]"),
    );
    let children: usize = args
        .get(1)
        .map(|n| n.parse().expect("children"))
        .unwrap_or(DEFAULT_CHILDREN);

    // Remove any previous fixture so the seq sequence starts at 1.
    if path.exists() {
        std::fs::remove_file(&path).expect("remove previous fixture");
    }
    let exe = std::env::current_exe().expect("current exe");
    let mut spawned = Vec::with_capacity(children);
    for index in 0..children {
        spawned.push(
            Command::new(&exe)
                .arg("--child")
                .arg(index.to_string())
                .arg(&path)
                .spawn()
                .unwrap_or_else(|error| panic!("spawn child {index}: {error}")),
        );
    }

    // The killed-session start while children are still interleaving.
    let outbox = LifecycleOutbox::new(Some(path.clone()), None, None);
    let killed_turn = "turn-killed-0";
    outbox
        .emit_blocking(LifecycleEvent {
            event: "turn_start".to_string(),
            kind: "turn.started".to_string(),
            thread_id: KILLED_THREAD.to_string(),
            turn_id: Some(killed_turn.to_string()),
            item_id: None,
            payload: json!({ "workspace": "/tmp/fixture-killed" }),
        })
        .expect("killed turn_start");

    for (index, mut child) in spawned.drain(..).enumerate() {
        let status = child
            .wait()
            .unwrap_or_else(|error| panic!("wait child {index}: {error}"));
        assert!(status.success(), "child {index} exited with {status}");
    }

    // Boot reconciliation owns the killed session's open turn.
    let reconciled = outbox
        .reconcile_interrupted_turns(KILLED_THREAD, "boot_reconciliation")
        .expect("reconcile");
    assert_eq!(reconciled, 1, "exactly the killed session's start");

    let text = std::fs::read_to_string(&path).expect("read fixture");
    let lines: Vec<&str> = text.lines().collect();
    let seqs: Vec<u64> = lines
        .iter()
        .map(|line| {
            serde_json::from_str::<serde_json::Value>(line).expect("json")["seq"]
                .as_u64()
                .expect("seq")
        })
        .collect();
    assert!(
        seqs.windows(2).all(|pair| pair[0] < pair[1]),
        "fixture seqs must be strictly monotonic in file order: {seqs:?}"
    );
    println!(
        "fixture: {} — {children} child writers, {} lines, seqs 1..={}, killed turn reconciled",
        path.display(),
        lines.len(),
        seqs.len(),
    );
}

/// One child writer: a single `turn_start`/`turn_end` pair, with a sleep
/// between the two appends so concurrent children interleave their lines.
fn child_writer(index: usize, path: PathBuf) {
    let thread_id = format!("sess_fixture_{index}");
    let turn_id = format!("turn-{index}");
    let outbox = LifecycleOutbox::new(Some(path), None, None);

    outbox
        .emit_blocking(LifecycleEvent {
            event: "turn_start".to_string(),
            kind: "turn.started".to_string(),
            thread_id: thread_id.clone(),
            turn_id: Some(turn_id.clone()),
            item_id: None,
            payload: json!({ "workspace": format!("/tmp/fixture-{index}") }),
        })
        .expect("turn_start");
    // Deliberate interleave window: each child appends its pair's end after
    // a different delay, so other children's appends land in between.
    std::thread::sleep(Duration::from_millis(10 + index as u64 * 7));
    outbox
        .emit_blocking(LifecycleEvent {
            event: "turn_end".to_string(),
            kind: "turn.completed".to_string(),
            thread_id,
            turn_id: Some(turn_id),
            item_id: None,
            payload: json!({ "status": "completed", "workspace": format!("/tmp/fixture-{index}") }),
        })
        .expect("turn_end");
}
