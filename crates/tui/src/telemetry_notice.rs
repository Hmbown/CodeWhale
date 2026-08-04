//! The first-run telemetry notice, on the interactive startup path.
//!
//! Shown once, before the terminal enters raw mode, on the same TTY the user
//! launched on. It is deliberately *not* hung off the setup wizard's deferral
//! machinery: `defer_update_checkpoint_for_app` persists a completed
//! constitution checkpoint without ever showing the user anything, and a
//! telemetry decision recorded that way would be a decision nobody made.
//!
//! Every path that does not render and answer the notice leaves
//! `telemetry_notice_decided_for` as `None`, and `None` means nothing is ever
//! collected. Silence is a supported outcome, not a degraded one:
//!
//! - `--skip-onboarding`: no notice, no decision, no emission.
//! - non-TTY (a pipe, CI, a container): no notice, no decision, no emission.
//! - answered "no": off, and not asked again until the notice content itself
//!   changes.

use std::io::{BufRead, IsTerminal, Write};

use codewhale_config::{SetupState, TELEMETRY_NOTICE_VERSION};
use codewhale_telemetry::notice;

/// Show the notice and record the answer, if and only if one is owed and this
/// process is on a terminal that can ask.
///
/// Returns `true` when a decision was recorded. Never returns an error: a
/// notice that cannot be shown is a notice that was not answered, which is the
/// off state, which is the default.
pub fn prompt_if_due(skip_onboarding: bool, config_path: Option<std::path::PathBuf>) -> bool {
    if skip_onboarding {
        return false;
    }
    if !(std::io::stdin().is_terminal() && std::io::stderr().is_terminal()) {
        return false;
    }
    let mut state = match SetupState::load() {
        Ok(Some(state)) => state,
        // A missing record is a first run, which is exactly when the notice is
        // owed. An *unreadable* record is not: overwriting it would be the one
        // failure mode that costs a user their constitution checkpoint.
        Ok(None) => SetupState::default(),
        Err(error) => {
            tracing::debug!("telemetry notice skipped; setup state unreadable: {error}");
            return false;
        }
    };
    if !state.needs_telemetry_notice(TELEMETRY_NOTICE_VERSION) {
        return false;
    }

    let opt_in = ask(&mut std::io::stderr(), &mut std::io::stdin().lock());

    // Enabling writes *both* halves. They are independent AND conditions at
    // emit time, so neither alone does anything, and that is what makes a
    // stale pre-existing `telemetry = true` — a key that has been settable and
    // inert for a long time — stay inert.
    //
    // Config first, decision second. Either order fails closed: a config write
    // without a decision is `ForcedOff` for want of consent, and a decision
    // without the config value is `ForcedOff` for want of the switch.
    if opt_in && let Err(error) = write_config_opt_in(config_path) {
        tracing::warn!("telemetry opt-in was not saved to config: {error}");
        let _ = writeln!(
            std::io::stderr(),
            "  Could not save that setting; telemetry stays off.\n"
        );
        return false;
    }

    state.record_telemetry_notice(TELEMETRY_NOTICE_VERSION, opt_in);
    if let Err(error) = state.save() {
        // Nothing was recorded, so the notice is still owed and will be asked
        // again. Emitting on the strength of an answer we failed to store
        // would be collection without a record of consent.
        tracing::warn!("telemetry decision was not saved: {error}");
        return false;
    }
    let _ = writeln!(std::io::stderr(), "{}\n", notice::decision_receipt(opt_in));
    opt_in
}

/// Set `telemetry = true` in the same config file this process was launched
/// with.
fn write_config_opt_in(config_path: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    let mut store = codewhale_config::ConfigStore::load(config_path)?;
    store.config.set_value("telemetry", "true")?;
    store.save()
}

/// Render the notice to `out` and read one answer from `input`.
///
/// Split out so the wording, the default, and the parsing are testable without
/// a terminal. Enter — an empty line — declines, and so does EOF.
fn ask(out: &mut impl Write, input: &mut impl BufRead) -> bool {
    let _ = writeln!(
        out,
        "\n  {}\n\n{}\n\n  [ Enable ]      [ No thanks ]\n\n  Selected: No thanks — press Enter to keep telemetry off.\n",
        notice::NOTICE_HEADLINE,
        indent(notice::NOTICE_BODY),
    );
    let _ = write!(out, "  {} ", notice::NOTICE_PROMPT);
    let _ = out.flush();

    let mut answer = String::new();
    if input.read_line(&mut answer).is_err() {
        return false;
    }
    notice::answer_is_yes(&answer)
}

fn indent(body: &str) -> String {
    body.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask_with(answer: &str) -> (bool, String) {
        let mut out: Vec<u8> = Vec::new();
        let mut input = answer.as_bytes();
        let decision = ask(&mut out, &mut input);
        (decision, String::from_utf8(out).expect("utf8"))
    }

    #[test]
    fn enter_declines() {
        // The declining option is pre-selected and Enter takes it. Enabling
        // costs a deliberate keystroke; declining costs none.
        assert!(!ask_with("\n").0);
        assert!(!ask_with("").0);
        assert!(!ask_with("  \n").0);
    }

    #[test]
    fn only_an_affirmative_answer_enables() {
        assert!(ask_with("y\n").0);
        assert!(ask_with("Y\n").0);
        assert!(ask_with("yes\n").0);
        assert!(!ask_with("n\n").0);
        assert!(!ask_with("no\n").0);
        assert!(!ask_with("sure\n").0);
        assert!(!ask_with("1\n").0);
    }

    #[test]
    fn the_notice_states_the_red_lines_and_the_way_out() {
        let (_, rendered) = ask_with("\n");
        for claim in [
            "never sends prompts",
            "Not sampled, not hashed",
            "random ID stored on this machine",
            "every 90 days",
            "docs/TELEMETRY.md",
            "codewhale config set telemetry false",
            "CODEWHALE_TELEMETRY=0",
            "press Enter to keep telemetry off",
        ] {
            assert!(rendered.contains(claim), "notice is missing: {claim}");
        }
        assert!(
            !rendered.contains("anonymized"),
            "the notice must not imply anonymization it does not perform"
        );
    }

    #[test]
    fn skip_onboarding_records_no_decision() {
        // Not a decision, and not a deferral that pretends to be one. The
        // constitution checkpoint records `Deferred` on this path; telemetry
        // deliberately does not mirror it.
        assert!(!prompt_if_due(true, None));
    }
}
