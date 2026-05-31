//! `deepseek metrics` — reads the audit log and session/task stores and prints
//! a human-readable usage rollup.
//!
//! Data sources:
//! - `~/.deepseek/audit.log`   — one JSON line per event (approvals, credentials)
//! - `~/.deepseek/sessions/`   — saved session JSON files (tool call history)
//! - `~/.deepseek/tasks/runtime/events/` — runtime thread JSONL event streams

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

// ──────────────────────────────────────────────────────────────────────────────
// Public entry-point
// ──────────────────────────────────────────────────────────────────────────────

/// Arguments accepted by `deepseek metrics`.
#[derive(Debug, Default)]
pub struct MetricsArgs {
    /// Emit machine-readable JSON instead of human text.
    pub json: bool,
    /// Restrict to events newer than this cutoff (inclusive).
    pub since: Option<DateTime<Utc>>,
}

pub fn run(args: MetricsArgs) -> Result<()> {
    let base = deepseek_home();

    // Collect data from every source; treat missing files as empty.
    let mut rollup = Rollup::default();
    read_audit_log(&base.join("audit.log"), args.since, &mut rollup);
    read_session_files(&base.join("sessions"), args.since, &mut rollup);
    read_runtime_events(
        &base.join("tasks").join("runtime").join("events"),
        args.since,
        &mut rollup,
    );

    if args.json {
        print_json(&rollup)?;
    } else {
        print_human(&rollup);
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────────────────────
// Duration-string parser  ("7d", "24h", "30m", "2h", "now-2h", "2h30m")
// ──────────────────────────────────────────────────────────────────────────────

/// Parse a loose humantime-ish duration string into an absolute `DateTime<Utc>`
/// cutoff (i.e. `Utc::now() - duration`).
///
/// Accepted forms:
/// - `7d` / `24h` / `30m` / `90s`
/// - `2h30m`, `1d12h`
/// - `now-2h` (leading `now-` is stripped before parsing)
pub fn parse_since(s: &str) -> Result<DateTime<Utc>> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.strip_prefix("now-").unwrap_or(&s);
    let secs = parse_duration_secs(s)?;
    Ok(Utc::now() - Duration::seconds(secs))
}

fn parse_duration_secs(s: &str) -> Result<i64> {
    // Walk through the string accumulating numbers and consuming unit suffixes.
    let mut total: i64 = 0;
    let mut num_buf = String::new();

    for ch in s.chars() {
        match ch {
            '0'..='9' => num_buf.push(ch),
            'd' | 'h' | 'm' | 's' => {
                let n: i64 = num_buf
                    .parse()
                    .map_err(|_| anyhow::anyhow!("invalid duration component: {num_buf:?}"))?;
                num_buf.clear();
                let factor = match ch {
                    'd' => 86_400,
                    'h' => 3_600,
                    'm' => 60,
                    's' => 1,
                    _ => unreachable!(),
                };
                total += n * factor;
            }
            _ => anyhow::bail!("unrecognised character {ch:?} in duration {s:?}"),
        }
    }

    if !num_buf.is_empty() {
        // Trailing bare number — treat as seconds.
        let n: i64 = num_buf.parse()?;
        total += n;
    }

    if total == 0 {
        anyhow::bail!("duration {s:?} resolved to zero seconds");
    }

    Ok(total)
}

// ──────────────────────────────────────────────────────────────────────────────
// Rollup data model
// ──────────────────────────────────────────────────────────────────────────────

/// Per-tool aggregated counters.
#[derive(Debug, Default, serde::Serialize)]
pub struct ToolStats {
    pub calls: u64,
    /// Calls that were auto-approved (no prompt required).
    pub auto_approved: u64,
    /// Calls that required a manual prompt.
    pub prompted: u64,
    /// Total elapsed ms (from events that carry this field).
    pub total_elapsed_ms: u64,
    /// Number of elapsed_ms samples included in `total_elapsed_ms`.
    pub elapsed_samples: u64,
    /// Successful calls (where we have result data).
    pub successes: u64,
    /// Failed calls.
    pub failures: u64,
}

impl ToolStats {
    fn success_rate_pct(&self) -> Option<f64> {
        let judged = self.successes + self.failures;
        if judged == 0 {
            None
        } else {
            Some(self.successes as f64 / judged as f64 * 100.0)
        }
    }

    fn avg_elapsed_ms(&self) -> Option<u64> {
        self.total_elapsed_ms.checked_div(self.elapsed_samples)
    }
}

/// Compaction event stats.
#[derive(Debug, Default, serde::Serialize)]
pub struct CompactionStats {
    pub events: u64,
    /// Sum of `reduction_ratio` from events that carry it (0.0–1.0 each).
    pub ratio_sum: f64,
    pub ratio_samples: u64,
}

impl CompactionStats {
    fn avg_reduction_pct(&self) -> Option<f64> {
        if self.ratio_samples == 0 {
            None
        } else {
            Some(self.ratio_sum / self.ratio_samples as f64 * 100.0)
        }
    }
}

/// Sub-agent spawn stats.
#[derive(Debug, Default, serde::Serialize)]
pub struct AgentStats {
    pub spawns: u64,
    pub successes: u64,
    pub failures: u64,
}

impl AgentStats {
    fn success_rate_pct(&self) -> Option<f64> {
        let judged = self.successes + self.failures;
        if judged == 0 {
            None
        } else {
            Some(self.successes as f64 / judged as f64 * 100.0)
        }
    }
}

/// Capacity-controller / rate-limit intervention stats.
#[derive(Debug, Default, serde::Serialize)]
pub struct CapacityStats {
    pub total: u64,
    pub by_category: HashMap<String, u64>,
}

/// Credential / session event stats (from audit log).
#[derive(Debug, Default, serde::Serialize)]
pub struct CredentialStats {
    pub saves: u64,
    pub clears: u64,
}

/// Top-level rollup.
#[derive(Debug, Default, serde::Serialize)]
pub struct Rollup {
    /// UTC timestamp of the earliest event we've seen.
    pub earliest_ts: Option<DateTime<Utc>>,
    /// UTC timestamp of the latest event we've seen.
    pub latest_ts: Option<DateTime<Utc>>,
    /// Per-tool stats keyed by tool name.
    pub tools: HashMap<String, ToolStats>,
    pub compaction: CompactionStats,
    pub agents: AgentStats,
    pub capacity: CapacityStats,
    pub credentials: CredentialStats,
    /// Total lines read across all sources.
    pub total_lines: u64,
    /// Lines successfully parsed.
    pub parsed_lines: u64,
}

impl Rollup {
    fn touch_ts(&mut self, ts: &DateTime<Utc>) {
        match self.earliest_ts {
            None => self.earliest_ts = Some(*ts),
            Some(ref cur) if ts < cur => self.earliest_ts = Some(*ts),
            _ => {}
        }
        match self.latest_ts {
            None => self.latest_ts = Some(*ts),
            Some(ref cur) if ts > cur => self.latest_ts = Some(*ts),
            _ => {}
        }
    }

    fn tool_mut(&mut self, name: &str) -> &mut ToolStats {
        self.tools.entry(name.to_string()).or_default()
    }

    fn total_tool_calls(&self) -> u64 {
        self.tools.values().map(|t| t.calls).sum()
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Source readers
// ──────────────────────────────────────────────────────────────────────────────

/// Read one-JSON-line-per-event audit log.
fn read_audit_log(path: &Path, since: Option<DateTime<Utc>>, rollup: &mut Rollup) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::trace!(
                "metrics: could not read audit log {}: {}",
                path.display(),
                e
            );
            return;
        }
    };

    for raw_line in content.lines() {
        rollup.total_lines += 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!("metrics: skipping malformed audit line: {e}");
                continue;
            }
        };

        // Parse timestamp — field is "ts" in audit log.
        let ts = parse_ts_field(&v, "ts");

        if let Some(cutoff) = since {
            match ts {
                Some(t) if t < cutoff => continue,
                _ => {}
            }
        }

        rollup.parsed_lines += 1;
        if let Some(t) = &ts {
            rollup.touch_ts(t);
        }

        let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");

        match event {
            "tool.approval.auto_approve" => {
                let tool_name = v
                    .pointer("/details/tool_name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown");
                let stats = rollup.tool_mut(tool_name);
                stats.calls += 1;
                stats.auto_approved += 1;
            }
            "tool.approval.prompted" => {
                let tool_name = v
                    .pointer("/details/tool_name")
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown");
                let stats = rollup.tool_mut(tool_name);
                stats.calls += 1;
                stats.prompted += 1;
            }
            "tool.completed" | "tool.result" => {
                let tool_name = v
                    .pointer("/details/tool_name")
                    .or_else(|| v.pointer("/payload/tool_name"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown");
                let stats = rollup.tool_mut(tool_name);
                stats.calls += 1;

                // Optional elapsed_ms
                if let Some(ms) = v
                    .pointer("/details/elapsed_ms")
                    .or_else(|| v.pointer("/payload/elapsed_ms"))
                    .and_then(|v| v.as_u64())
                {
                    stats.total_elapsed_ms += ms;
                    stats.elapsed_samples += 1;
                }

                // Success / failure
                let success = v
                    .pointer("/details/success")
                    .or_else(|| v.pointer("/payload/success"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(true);
                if success {
                    stats.successes += 1;
                } else {
                    stats.failures += 1;
                }
            }
            "compaction.completed" | "context.compaction" => {
                rollup.compaction.events += 1;
                if let Some(ratio) = v
                    .pointer("/details/reduction_ratio")
                    .or_else(|| v.pointer("/payload/reduction_ratio"))
                    .and_then(|r| r.as_f64())
                {
                    rollup.compaction.ratio_sum += ratio;
                    rollup.compaction.ratio_samples += 1;
                }
            }
            "agent.spawn" | "subagent.spawned" => {
                rollup.agents.spawns += 1;
            }
            "agent.completed" | "subagent.completed" => {
                let success = v
                    .pointer("/details/success")
                    .or_else(|| v.pointer("/payload/success"))
                    .and_then(|b| b.as_bool())
                    .unwrap_or(true);
                if success {
                    rollup.agents.successes += 1;
                } else {
                    rollup.agents.failures += 1;
                }
            }
            e if e.starts_with("capacity.") => {
                rollup.capacity.total += 1;
                let category = v
                    .pointer("/details/category")
                    .or_else(|| v.pointer("/payload/category"))
                    .and_then(|c| c.as_str())
                    .unwrap_or(e.trim_start_matches("capacity."));
                *rollup
                    .capacity
                    .by_category
                    .entry(category.to_string())
                    .or_insert(0) += 1;
            }
            "credential.save" => {
                rollup.credentials.saves += 1;
            }
            "credential.clear" => {
                rollup.credentials.clears += 1;
            }
            _ => {
                // Unknown event — tracked in parsed_lines but otherwise ignored.
            }
        }
    }
}

/// Read session JSON files under `sessions/` (one per session).
/// These carry tool call history with optional elapsed_ms and result data.
fn read_session_files(sessions_dir: &Path, since: Option<DateTime<Utc>>, rollup: &mut Rollup) {
    let rd = match std::fs::read_dir(sessions_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::trace!(
                "metrics: could not list sessions dir {}: {}",
                sessions_dir.display(),
                e
            );
            return;
        }
    };

    for entry in rd.flatten() {
        let path = entry.path();
        // Only look at .json files directly in sessions/; skip sub-dirs.
        if path.is_dir() || path.extension().map(|e| e != "json").unwrap_or(true) {
            continue;
        }
        read_session_file(&path, since, rollup);
    }
}

fn read_session_file(path: &Path, since: Option<DateTime<Utc>>, rollup: &mut Rollup) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::trace!(
                "metrics: could not read session file {}: {}",
                path.display(),
                e
            );
            return;
        }
    };

    rollup.total_lines += 1;

    let v: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            tracing::trace!(
                "metrics: skipping malformed session file {}: {}",
                path.display(),
                e
            );
            return;
        }
    };

    rollup.parsed_lines += 1;

    // Session-level timestamp filter (check metadata.created_at or updated_at).
    let session_ts = v
        .pointer("/metadata/updated_at")
        .or_else(|| v.pointer("/metadata/created_at"))
        .and_then(|t| t.as_str())
        .and_then(|s| s.parse::<DateTime<Utc>>().ok());

    if let Some(cutoff) = since
        && let Some(ts) = &session_ts
        && *ts < cutoff
    {
        return;
    }

    if let Some(ts) = session_ts {
        rollup.touch_ts(&ts);
    }

    // Walk messages looking for tool_use calls with associated results.
    let messages = match v.get("messages").and_then(|m| m.as_array()) {
        Some(m) => m,
        None => return,
    };

    // Build a map from tool_use_id → (tool_name, elapsed_ms_option, started_at_option).
    let mut pending: HashMap<String, (String, Option<u64>)> = HashMap::new();

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("");
        let content_arr = match msg.get("content").and_then(|c| c.as_array()) {
            Some(c) => c,
            None => continue,
        };

        for block in content_arr {
            let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
            match (role, block_type) {
                ("assistant", "tool_use") => {
                    let id = block.get("id").and_then(|i| i.as_str()).unwrap_or("");
                    let name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown");
                    let elapsed_ms = block.get("elapsed_ms").and_then(|e| e.as_u64());
                    if !id.is_empty() {
                        pending.insert(id.to_string(), (name.to_string(), elapsed_ms));
                    }
                }
                ("user", "tool_result") => {
                    let id = block
                        .get("tool_use_id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("");
                    if let Some((name, elapsed_ms)) = pending.remove(id) {
                        let stats = rollup.tool_mut(&name);
                        // Only count if not already counted via audit log (we don't de-dup, so
                        // session files may double-count approvals; that's acceptable — users who
                        // want precise counts should use --json and cross-reference).
                        stats.calls += 1;
                        if let Some(ms) = elapsed_ms {
                            stats.total_elapsed_ms += ms;
                            stats.elapsed_samples += 1;
                        }
                        // Tool result success: absence of "is_error": true
                        let is_error = block
                            .get("is_error")
                            .and_then(|e| e.as_bool())
                            .unwrap_or(false);
                        if is_error {
                            stats.failures += 1;
                        } else {
                            stats.successes += 1;
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // Walk messages for compaction events embedded as special user messages.
    for msg in messages {
        if let Some(compaction) = msg
            .get("compaction")
            .or_else(|| msg.pointer("/metadata/compaction"))
        {
            rollup.compaction.events += 1;
            if let Some(ratio) = compaction.get("reduction_ratio").and_then(|r| r.as_f64()) {
                rollup.compaction.ratio_sum += ratio;
                rollup.compaction.ratio_samples += 1;
            }
        }
    }
}

/// Read JSONL event streams from the tasks runtime events directory.
fn read_runtime_events(events_dir: &Path, since: Option<DateTime<Utc>>, rollup: &mut Rollup) {
    let rd = match std::fs::read_dir(events_dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            tracing::trace!(
                "metrics: could not list events dir {}: {}",
                events_dir.display(),
                e
            );
            return;
        }
    };

    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e != "jsonl").unwrap_or(true) {
            continue;
        }
        read_events_jsonl(&path, since, rollup);
    }
}

fn read_events_jsonl(path: &Path, since: Option<DateTime<Utc>>, rollup: &mut Rollup) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            tracing::trace!(
                "metrics: could not read events file {}: {}",
                path.display(),
                e
            );
            return;
        }
    };

    for raw_line in content.lines() {
        rollup.total_lines += 1;
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }

        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::trace!("metrics: skipping malformed event line: {e}");
                continue;
            }
        };

        let ts = parse_ts_field(&v, "timestamp");

        if let Some(cutoff) = since {
            match ts {
                Some(t) if t < cutoff => continue,
                _ => {}
            }
        }

        rollup.parsed_lines += 1;
        if let Some(t) = &ts {
            rollup.touch_ts(t);
        }

        let event = v.get("event").and_then(|e| e.as_str()).unwrap_or("");

        match event {
            "tool.started" | "tool.completed" | "tool.failed" => {
                let tool_name = v
                    .pointer("/payload/tool_name")
                    .or_else(|| v.pointer("/payload/name"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("unknown");
                let stats = rollup.tool_mut(tool_name);

                if event == "tool.started" {
                    stats.calls += 1;
                } else if event == "tool.completed" {
                    stats.successes += 1;
                    if let Some(ms) = v.pointer("/payload/elapsed_ms").and_then(|v| v.as_u64()) {
                        stats.total_elapsed_ms += ms;
                        stats.elapsed_samples += 1;
                    }
                } else {
                    // tool.failed
                    stats.failures += 1;
                }
            }
            "compaction.completed" => {
                rollup.compaction.events += 1;
                if let Some(ratio) = v
                    .pointer("/payload/reduction_ratio")
                    .and_then(|r| r.as_f64())
                {
                    rollup.compaction.ratio_sum += ratio;
                    rollup.compaction.ratio_samples += 1;
                }
            }
            "agent.spawned" | "subagent.spawned" => {
                rollup.agents.spawns += 1;
            }
            "agent.completed" | "subagent.completed" => {
                let success = v
                    .pointer("/payload/success")
                    .and_then(|b| b.as_bool())
                    .unwrap_or(true);
                if success {
                    rollup.agents.successes += 1;
                } else {
                    rollup.agents.failures += 1;
                }
            }
            e if e.starts_with("capacity.") => {
                rollup.capacity.total += 1;
                let category = v
                    .pointer("/payload/category")
                    .and_then(|c| c.as_str())
                    .unwrap_or(e.trim_start_matches("capacity."));
                *rollup
                    .capacity
                    .by_category
                    .entry(category.to_string())
                    .or_insert(0) += 1;
            }
            _ => {}
        }
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Output formatters
// ──────────────────────────────────────────────────────────────────────────────

fn print_json(rollup: &Rollup) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(rollup)?);
    Ok(())
}

fn print_human(rollup: &Rollup) {
    // Period header
    match (rollup.earliest_ts, rollup.latest_ts) {
        (Some(start), Some(end)) => {
            let days = (end - start).num_days();
            println!(
                "Period: {} → {} ({} days)",
                start.format("%Y-%m-%d"),
                end.format("%Y-%m-%d"),
                days
            );
        }
        (Some(start), None) | (None, Some(start)) => {
            println!("Period: {} → (unknown)", start.format("%Y-%m-%d"));
        }
        (None, None) => {
            println!("Period: (no data)");
        }
    }

    // ── Tools ──────────────────────────────────────────────────────────────
    let total_calls = rollup.total_tool_calls();
    if total_calls > 0 {
        // Overall success rate from session-file data (where we have result info).
        let total_ok: u64 = rollup.tools.values().map(|t| t.successes).sum();
        let total_judged: u64 = rollup
            .tools
            .values()
            .map(|t| t.successes + t.failures)
            .sum();
        let overall_rate = if total_judged > 0 {
            format!(
                "{:.1}% success",
                total_ok as f64 / total_judged as f64 * 100.0
            )
        } else {
            // Only approval events — show prompt breakdown.
            let auto: u64 = rollup.tools.values().map(|t| t.auto_approved).sum();
            let prompted: u64 = rollup.tools.values().map(|t| t.prompted).sum();
            format!("{auto} auto-approved, {prompted} prompted")
        };

        println!(
            "Tools: {:>6} calls ({})",
            fmt_num(total_calls),
            overall_rate
        );

        // Sort tools by call count descending, top 15.
        let mut tools: Vec<(&String, &ToolStats)> = rollup.tools.iter().collect();
        tools.sort_by_key(|b| std::cmp::Reverse(b.1.calls));
        for (name, stats) in tools.iter().take(15) {
            let rate_str = match stats.success_rate_pct() {
                Some(pct) => format!("{pct:5.1}%"),
                None => {
                    // Only approval data available — show auto/prompted breakdown.
                    let a = stats.auto_approved;
                    let p = stats.prompted;
                    if p == 0 {
                        format!("auto×{a}  ")
                    } else {
                        format!("auto×{a}/prompted×{p}")
                    }
                }
            };
            let avg_str = match stats.avg_elapsed_ms() {
                Some(ms) => format!("  avg {ms}ms"),
                None => String::new(),
            };
            println!(
                "  {name:<22} {:>6}  {rate_str}{avg_str}",
                fmt_num(stats.calls)
            );
        }
        if tools.len() > 15 {
            println!("  … and {} more tools", tools.len() - 15);
        }
    } else {
        println!("Tools: (no data)");
    }

    // ── Compaction ─────────────────────────────────────────────────────────
    if rollup.compaction.events > 0 {
        let avg_str = match rollup.compaction.avg_reduction_pct() {
            Some(pct) => format!(", avg {pct:.0}% size reduction"),
            None => String::new(),
        };
        println!(
            "Compaction: {} events{}",
            fmt_num(rollup.compaction.events),
            avg_str
        );
    } else {
        println!("Compaction: (no data)");
    }

    // ── Sub-agents ─────────────────────────────────────────────────────────
    if rollup.agents.spawns > 0 {
        let rate_str = match rollup.agents.success_rate_pct() {
            Some(pct) => format!(", {pct:.1}% success"),
            None => String::new(),
        };
        println!(
            "Sub-agents: {} spawns{}",
            fmt_num(rollup.agents.spawns),
            rate_str
        );
    } else {
        println!("Sub-agents: (no data)");
    }

    // ── Capacity interventions ─────────────────────────────────────────────
    if rollup.capacity.total > 0 {
        let cat_str: String = {
            let mut cats: Vec<(&String, &u64)> = rollup.capacity.by_category.iter().collect();
            cats.sort_by(|a, b| b.1.cmp(a.1));
            cats.iter()
                .map(|(k, v)| format!("{v} {k}"))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!(
            "Capacity interventions: {} ({})",
            fmt_num(rollup.capacity.total),
            cat_str
        );
    } else {
        println!("Capacity interventions: (no data)");
    }

    // ── Credentials ────────────────────────────────────────────────────────
    if rollup.credentials.saves > 0 || rollup.credentials.clears > 0 {
        println!(
            "Credentials: {} saves, {} clears",
            rollup.credentials.saves, rollup.credentials.clears
        );
    }
}

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

fn deepseek_home() -> PathBuf {
    // Respect DEEPSEEK_HOME env override; fall back to ~/.deepseek.
    if let Ok(v) = std::env::var("DEEPSEEK_HOME")
        && !v.is_empty()
    {
        return PathBuf::from(v);
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".deepseek")
}

/// Parse a timestamp from a JSON value field (tries RFC3339).
fn parse_ts_field(v: &Value, field: &str) -> Option<DateTime<Utc>> {
    v.get(field)?.as_str()?.parse::<DateTime<Utc>>().ok()
}

/// Format a number with thousands separators.
fn fmt_num(n: u64) -> String {
    let s = n.to_string();
    let mut result = String::with_capacity(s.len() + s.len() / 3);
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}

// ──────────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Duration parser ──

    #[test]
    fn parse_since_7d() {
        let cutoff = parse_since("7d").unwrap();
        let expected = Utc::now() - Duration::days(7);
        // Allow ±2s for test execution time.
        assert!((cutoff - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn parse_since_24h() {
        let cutoff = parse_since("24h").unwrap();
        let expected = Utc::now() - Duration::hours(24);
        assert!((cutoff - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn parse_since_30m() {
        let cutoff = parse_since("30m").unwrap();
        let expected = Utc::now() - Duration::minutes(30);
        assert!((cutoff - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn parse_since_now_prefix() {
        // "now-2h" should strip "now-" and parse "2h".
        let cutoff = parse_since("now-2h").unwrap();
        let expected = Utc::now() - Duration::hours(2);
        assert!((cutoff - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn parse_since_compound() {
        let cutoff = parse_since("2h30m").unwrap();
        let expected = Utc::now() - Duration::seconds(2 * 3600 + 30 * 60);
        assert!((cutoff - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn parse_since_compound_days_hours() {
        let cutoff = parse_since("1d12h").unwrap();
        let expected = Utc::now() - Duration::seconds(36 * 3600);
        assert!((cutoff - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn parse_since_error_on_invalid() {
        assert!(parse_since("xyz").is_err());
        assert!(parse_since("").is_err());
    }

    // ── fmt_num ──

    #[test]
    fn fmt_num_zero() {
        assert_eq!(fmt_num(0), "0");
    }

    #[test]
    fn fmt_num_thousands() {
        assert_eq!(fmt_num(1_000), "1,000");
        assert_eq!(fmt_num(12_453), "12,453");
        assert_eq!(fmt_num(1_000_000), "1,000,000");
    }

    // ── Rollup from audit log ──

    fn make_audit_line(event: &str, tool: &str, ts: &str) -> String {
        format!(
            r#"{{"details":{{"mode":"YOLO","session_id":null,"tool_name":"{tool}"}},"event":"{event}","ts":"{ts}"}}"#
        )
    }

    #[test]
    fn audit_log_empty_file() {
        let mut rollup = Rollup::default();
        // Non-existent path — should not panic, rollup stays empty.
        read_audit_log(Path::new("/nonexistent/audit.log"), None, &mut rollup);
        assert_eq!(rollup.total_lines, 0);
    }

    #[test]
    fn audit_log_parses_auto_approve() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let line1 = make_audit_line(
            "tool.approval.auto_approve",
            "exec_shell",
            "2026-04-01T10:00:00+00:00",
        );
        let line2 = make_audit_line(
            "tool.approval.auto_approve",
            "read_file",
            "2026-04-02T10:00:00+00:00",
        );
        writeln!(tmp, "{line1}").unwrap();
        writeln!(tmp, "{line2}").unwrap();

        let mut rollup = Rollup::default();
        read_audit_log(tmp.path(), None, &mut rollup);

        assert_eq!(rollup.parsed_lines, 2);
        assert_eq!(rollup.tools["exec_shell"].calls, 1);
        assert_eq!(rollup.tools["exec_shell"].auto_approved, 1);
        assert_eq!(rollup.tools["read_file"].calls, 1);
    }

    #[test]
    fn audit_log_skips_malformed_lines() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        writeln!(tmp, "not json at all").unwrap();
        writeln!(
            tmp,
            r#"{{"event":"credential.save","ts":"2026-04-01T10:00:00+00:00"}}"#
        )
        .unwrap();

        let mut rollup = Rollup::default();
        read_audit_log(tmp.path(), None, &mut rollup);

        // 2 lines total, 1 malformed skipped, 1 parsed.
        assert_eq!(rollup.total_lines, 2);
        assert_eq!(rollup.parsed_lines, 1);
        assert_eq!(rollup.credentials.saves, 1);
    }

    #[test]
    fn audit_log_since_filter() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        let line_old = make_audit_line(
            "tool.approval.auto_approve",
            "exec_shell",
            "2025-01-01T00:00:00+00:00",
        );
        let line_new = make_audit_line(
            "tool.approval.auto_approve",
            "read_file",
            "2026-04-01T00:00:00+00:00",
        );
        writeln!(tmp, "{line_old}").unwrap();
        writeln!(tmp, "{line_new}").unwrap();

        let cutoff: DateTime<Utc> = "2026-01-01T00:00:00Z".parse().unwrap();
        let mut rollup = Rollup::default();
        read_audit_log(tmp.path(), Some(cutoff), &mut rollup);

        // Only the newer line should be counted.
        assert_eq!(rollup.parsed_lines, 1);
        assert!(!rollup.tools.contains_key("exec_shell"));
        assert_eq!(rollup.tools["read_file"].calls, 1);
    }

    #[test]
    fn total_tool_calls_sums_across_tools() {
        let mut rollup = Rollup::default();
        rollup.tool_mut("read_file").calls = 4_012;
        rollup.tool_mut("exec_shell").calls = 1_118;
        assert_eq!(rollup.total_tool_calls(), 5_130);
    }
}
