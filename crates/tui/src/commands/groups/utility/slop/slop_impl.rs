use crate::commands::CommandResult;
use crate::tui::app::App;

pub fn slop(_app: &mut App, arg: Option<&str>) -> CommandResult {
    let arg = arg.map(str::trim).unwrap_or("");
    let ledger = match crate::slop_ledger::SlopLedger::load() {
        Ok(l) => l,
        Err(e) => return CommandResult::error(format!("Failed to load slop ledger: {e}")),
    };

    match arg {
        "" => CommandResult::message(ledger.summary()),
        "query" | "q" => {
            if ledger.is_empty() {
                return CommandResult::message("Slop ledger is empty.");
            }
            let mut out = String::new();
            for entry in &ledger.query(&Default::default()) {
                use std::fmt::Write;
                let _ = writeln!(
                    out,
                    "[{}] {} ({:?} | {:?}) — {}",
                    crate::slop_ledger::short_id(&entry.id),
                    entry.bucket.as_str(),
                    entry.severity,
                    entry.status,
                    entry.title
                );
            }
            CommandResult::message(out)
        }
        "export" | "e" => {
            let md = ledger.export_markdown(None, None);
            CommandResult::message(md)
        }
        _ => CommandResult::error(format!(
            "Unknown /slop action '{arg}'. Use /slop, /slop query, or /slop export."
        )),
    }
}

