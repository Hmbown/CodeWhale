//! Utility commands group barrel.
//!
//! Each command lives in its own file under this directory.
//! This module declares the submodules and provides the `CommandGroup`
//! implementation that collects them.

pub(crate) mod queue;
pub(crate) mod stash;
pub(crate) mod hooks;
pub(crate) mod anchor;
pub(crate) mod network;
pub(crate) mod mcp;
pub(crate) mod rlm;
pub(crate) mod task;
pub(crate) mod jobs;
pub(crate) mod slop;

use crate::commands::traits::{Command, CommandGroup};
use crate::tui::app::App;

use self::queue::Queue;
use self::stash::Stash;
use self::hooks::Hooks;
use self::anchor::Anchor;
use self::network::Network;
use self::mcp::Mcp;
use self::rlm::Rlm;
use self::task::Task;
use self::jobs::Jobs;
use self::slop::Slop;

pub struct UtilityCommands;
impl CommandGroup for UtilityCommands {
    fn commands(&self) -> Vec<Box<dyn Command>> {
        vec![
            Box::new(Queue),
            Box::new(Stash),
            Box::new(Hooks),
            Box::new(Anchor),
            Box::new(Network),
            Box::new(Mcp),
            Box::new(Rlm),
            Box::new(Task),
            Box::new(Jobs),
            Box::new(Slop),
        ]
    }
}


// ── Helpers ────────────────────────────────────────────────────────────────

fn parse_depth_prefixed_arg(
    arg: Option<&str>,
    default_depth: u32,
) -> Result<(u32, Option<&str>), String> {
    let Some(raw) = arg.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok((default_depth, None));
    };
    let mut parts = raw.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    if first.chars().all(|ch| ch.is_ascii_digit()) {
        let depth: u32 = first
            .parse()
            .map_err(|_| "Depth must be an integer from 0 to 3".to_string())?;
        if depth > 3 {
            return Err("Depth must be between 0 and 3".to_string());
        }
        Ok((depth, parts.next().map(str::trim)))
    } else {
        Ok((default_depth, Some(raw)))
    }
}

fn resolves_to_existing_file(app: &App, input: &str) -> bool {
    let path = std::path::Path::new(input);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        app.workspace.join(path)
    };
    candidate.is_file()
}
