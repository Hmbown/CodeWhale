//! `/preview-request` — offline, redacted preview of the next outbound
//! request (#1004), plus typed base-prompt provenance (#3928).
//!
//! This module is a thin dispatcher. It owns argument parsing and nothing
//! else: the manifest is built by the **engine**
//! (`core::engine::preview`), which is the only authority that can rebuild
//! the exact next-turn tool catalog, active subset, MCP state, mode, gates,
//! permission posture, tool choice, and resolved route, and then run them
//! through the shared prepared-request seam every provider dialect uses.
//!
//! What this command will never do:
//!
//! - print effective system text, project instructions, memory, or skill text;
//!   the explicit `base-prompt` mode prints only the base layer;
//! - print message content, tool results, or attachment payloads;
//! - print credentials, URL paths, or absolute workspace paths;
//! - export the request body.
//!
//! It is an inspectability slice: typed counts, hashes, enums, and short
//! provenance labels. Human command only — deliberately not a model-visible
//! tool.
//!
//! Two things are worth knowing before reading the output:
//!
//! - **`--prompt <text>` is necessary for an exact manifest, and not always
//!   sufficient.** The next user message is part of the request, and under
//!   auto model routing it also decides the route; without it the route and
//!   body sections report a typed unavailable state instead of describing the
//!   previous turn. With it, a section is still typed unavailable whenever a
//!   real turn would do something an inspection may not — run `message_submit`
//!   hooks, connect MCP servers, auto-compact, recover from a context
//!   overflow, or consume queued sub-agent completions and LSP diagnostics.
//!   Exactness is conditional and the manifest says which condition failed.
//! - **Flags come before `--prompt`, which takes the rest of the line.** See
//!   [`parse_args`] for the grammar and why it is not "any order".
//! - **Preview never calls a provider or model.** Auto routing therefore
//!   reports a typed unavailable state even with `--prompt`: production must
//!   run the classifier before that route can be known exactly.
//!
//! The `dryrun` concept — preview the next request from the real
//! request-building seam rather than a hand-rolled summary — is harvested
//! from PR #1099 by TaoMu (GTC2080); no code from that PR is reused.

use super::CommandResult;
use crate::tui::app::{App, AppAction};

/// Usage line, kept in one place so the error path and the docs agree.
///
/// `--prompt` is terminal by construction: everything after it is prompt text.
/// That is what makes flag placement unambiguous instead of merely documented.
const USAGE: &str = "Usage: /preview-request [json] [--prompt <text>] | base-prompt  \
    (flags first; --prompt takes the rest)";

/// Entry point for `/preview-request` (aliases `/dryrun`, `/preview_request`).
pub fn preview_request(_app: &mut App, arg: Option<&str>) -> CommandResult {
    match parse_args(arg.unwrap_or_default()) {
        Ok(PreviewArgs {
            json,
            base_prompt_only,
            hypothetical_prompt,
        }) => CommandResult::action(AppAction::PreviewOutboundRequest {
            json,
            base_prompt_only,
            hypothetical_prompt,
        }),
        Err(message) => CommandResult::message(message),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PreviewArgs {
    json: bool,
    base_prompt_only: bool,
    hypothetical_prompt: Option<String>,
}

/// Parse `/preview-request` arguments.
///
/// # Grammar
///
/// ```text
/// args    := flag* [ "--prompt" WS+ prompt ]
/// flag    := "json" | "--json" | "manifest" | "--manifest"
///          | "prompt" | "base-prompt" | "--base-prompt"
/// prompt  := <every remaining byte, verbatim>
/// ```
///
/// Two properties this grammar exists to guarantee, both of which the first
/// implementation claimed and did not have:
///
/// - **Flag placement is truthful.** Flags come *before* `--prompt`; `--prompt`
///   is terminal. The old parser advertised "any order" while consuming every
///   trailing token — including a trailing `json` — into the prompt, so
///   `--prompt fix it json` silently previewed the prompt *"fix it json"* as a
///   human table. There is now exactly one reading of any input.
/// - **The prompt is byte-preserving.** The old parser did
///   `split_whitespace().join(" ")`, which collapsed every run of whitespace
///   and every newline. The hypothetical prompt is part of the request being
///   hashed, so collapsing it described a body that differed from the real one
///   in the one field the user typed. Only one whitespace codepoint that
///   *delimits* `--prompt` from its text is removed; everything after it —
///   additional leading whitespace, interior runs, newlines, trailing bytes —
///   survives exactly.
///
/// Anything before `--prompt` that is not a known flag is rejected rather than
/// guessed at, and because `--prompt` swallows the remainder there is no
/// trailing-argument position left to be ambiguous.
///
/// `base-prompt` / `--base-prompt` explicitly render only the exact effective
/// base prompt. They cannot be combined with JSON or a hypothetical prompt.
/// The effective system prompt remains protected: the ordinary manifest shows
/// only its canonical JSON size and hash because it may contain project
/// instructions, skills, and memory. `prompt` remains a compatibility alias
/// for the ordinary manifest and never dumps effective system text.
fn parse_args(raw: &str) -> Result<PreviewArgs, String> {
    const PROMPT_FLAG: &str = "--prompt";
    let mut json = false;
    let mut base_prompt_only = false;
    let mut rest = raw;

    loop {
        let trimmed = rest.trim_start();
        if trimmed.is_empty() {
            return Ok(PreviewArgs {
                json,
                base_prompt_only,
                hypothetical_prompt: None,
            });
        }
        let token_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        let (token, remainder) = trimmed.split_at(token_end);

        if token == PROMPT_FLAG {
            if base_prompt_only {
                return Err(format!(
                    "`base-prompt` cannot be combined with `--prompt`. {USAGE}"
                ));
            }
            // Consume exactly one whitespace codepoint as syntax. Any further
            // leading whitespace belongs to the prompt, just like trailing
            // whitespace and newlines do. The command dispatcher deliberately
            // preserves this raw remainder.
            let Some(delimiter) = remainder.chars().next().filter(|ch| ch.is_whitespace()) else {
                return Err(format!("`--prompt` needs text after it. {USAGE}"));
            };
            let prompt = &remainder[delimiter.len_utf8()..];
            if prompt.trim().is_empty() {
                return Err(format!("`--prompt` needs text after it. {USAGE}"));
            }
            return Ok(PreviewArgs {
                json,
                base_prompt_only,
                hypothetical_prompt: Some(prompt.to_string()),
            });
        }

        match token {
            "json" | "--json" => {
                if base_prompt_only {
                    return Err(format!(
                        "`base-prompt` cannot be combined with JSON. {USAGE}"
                    ));
                }
                json = true;
            }
            "manifest" | "--manifest" => json = false,
            "prompt" => {}
            "base-prompt" | "--base-prompt" => {
                if json {
                    return Err(format!(
                        "`base-prompt` cannot be combined with JSON. {USAGE}"
                    ));
                }
                base_prompt_only = true;
            }
            _ => {
                return Err(format!(
                    "Unknown argument. Flags come before `--prompt`, which takes the rest of the line as prompt text. {USAGE}"
                ));
            }
        }
        rest = remainder;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::models::Role;

    fn args(raw: &str) -> Result<PreviewArgs, String> {
        parse_args(raw)
    }

    #[test]
    fn default_invocation_requests_the_human_manifest() {
        assert_eq!(
            args("").unwrap(),
            PreviewArgs {
                json: false,
                base_prompt_only: false,
                hypothetical_prompt: None
            }
        );
        assert_eq!(args("manifest").unwrap(), args("").unwrap());
    }

    #[test]
    fn json_flag_is_accepted_in_both_spellings() {
        assert!(args("json").unwrap().json);
        assert!(args("--json").unwrap().json);
    }

    #[test]
    fn base_prompt_mode_is_explicit_and_cannot_mix_with_body_preview() {
        assert!(!args("prompt").unwrap().base_prompt_only);
        for alias in ["base-prompt", "--base-prompt"] {
            let parsed = args(alias).expect("base-prompt mode parses");
            assert!(parsed.base_prompt_only, "{alias}");
            assert_eq!(parsed.hypothetical_prompt, None, "{alias}");
            assert!(!parsed.json, "{alias}");
        }
        for invalid in [
            "json base-prompt",
            "base-prompt json",
            "base-prompt --prompt hi",
        ] {
            assert!(args(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn hypothetical_prompt_is_captured_verbatim_for_auto_resolution() {
        assert_eq!(
            args("--prompt refactor the parser").unwrap(),
            PreviewArgs {
                json: false,
                base_prompt_only: false,
                hypothetical_prompt: Some("refactor the parser".to_string()),
            }
        );
        assert_eq!(
            args("json --prompt fix the failing test").unwrap(),
            PreviewArgs {
                json: true,
                base_prompt_only: false,
                hypothetical_prompt: Some("fix the failing test".to_string()),
            }
        );
    }

    /// The prompt is hashed into the previewed body, so collapsing its bytes
    /// described a request that differed from the real one in exactly the
    /// field the user typed. `split_whitespace().join(" ")` did that.
    #[test]
    fn the_prompt_keeps_the_users_bytes() {
        for prompt in [
            "keep  two  spaces",
            "line one\nline two",
            "tabs\tand\tmore",
            "trailing space ",
        ] {
            let raw = format!("--prompt {prompt}");
            let parsed = args(&raw).expect("prompt parses");
            assert_eq!(
                parsed.hypothetical_prompt.as_deref(),
                Some(prompt),
                "`{prompt:?}` must survive the parser byte for byte"
            );
        }
        // Only one codepoint delimits the flag from its text. The other three
        // spaces are prompt bytes.
        assert_eq!(
            args("--prompt    padded start")
                .unwrap()
                .hypothetical_prompt
                .as_deref(),
            Some("   padded start")
        );
    }

    /// The old parser advertised "any order" and then swallowed every trailing
    /// token into the prompt, so a trailing `json` silently became prompt text.
    /// Flags are now unambiguously *before* `--prompt`.
    #[test]
    fn flags_after_the_prompt_are_prompt_text_not_flags() {
        let parsed = args("--prompt fix it json").expect("parses");
        assert!(
            !parsed.json,
            "a trailing `json` is part of the prompt, and the manifest stays human"
        );
        assert_eq!(parsed.hypothetical_prompt.as_deref(), Some("fix it json"));

        // The truthful spelling puts the flag first, and it works.
        let parsed = args("json --prompt fix it").expect("parses");
        assert!(parsed.json);
        assert_eq!(parsed.hypothetical_prompt.as_deref(), Some("fix it"));
    }

    #[test]
    fn unknown_arguments_before_the_prompt_are_rejected_not_guessed() {
        for raw in ["nope", "json nope", "--nope --prompt hi", "manifest -x"] {
            let err = args(raw).expect_err("an unknown argument must not parse");
            assert!(err.contains("Unknown argument"), "{raw}: {err}");
            assert!(err.contains("--prompt"), "{raw}: {err}");
        }
    }

    #[test]
    fn unknown_argument_diagnostic_is_bounded_and_never_echoes_input() {
        let hostile = format!(
            "sk-live-{}-/Users/alice/private/config\nsecond-line",
            "a".repeat(10_000)
        );
        let err = args(&hostile).expect_err("hostile input must be rejected");
        assert!(err.contains("Unknown argument"), "{err}");
        assert!(err.contains("--prompt"), "{err}");
        assert!(err.len() < 256, "diagnostic was not bounded: {}", err.len());
        for forbidden in ["sk-live", "/Users/alice", "second-line"] {
            assert!(!err.contains(forbidden), "{forbidden} leaked in {err}");
        }
    }

    #[test]
    fn empty_hypothetical_prompt_is_rejected() {
        for raw in ["--prompt", "--prompt   ", "json --prompt"] {
            let err = args(raw).expect_err("bare --prompt is an error");
            assert!(err.contains("needs text"), "{raw}: {err}");
            assert!(err.contains("/preview-request"), "{raw}: {err}");
        }
    }

    #[test]
    fn leading_and_repeated_whitespace_between_flags_is_ignored() {
        assert_eq!(args("   json   --manifest  ").unwrap(), args("").unwrap());
    }

    #[test]
    fn unknown_argument_is_rejected_without_touching_state() {
        let options = crate::test_support::test_tui_options(std::path::PathBuf::from(
            "/tmp/test-workspace-preview-request",
        ));
        let mut app = App::new(options, &Config::default());
        let messages_before = app.api_messages.len();
        let history_before = app.history.len();

        let result = preview_request(&mut app, Some("nope"));

        assert!(!result.is_error);
        assert!(
            result
                .message
                .as_deref()
                .is_some_and(|message| message.contains("/preview-request")),
            "{result:?}"
        );
        assert!(result.action.is_none());
        assert_eq!(app.api_messages.len(), messages_before);
        assert_eq!(app.history.len(), history_before);
    }

    #[test]
    fn command_delegates_to_the_engine_and_mutates_nothing() {
        let options = crate::test_support::test_tui_options(std::path::PathBuf::from(
            "/tmp/test-workspace-preview-request-pure",
        ));
        let mut app = App::new(options, &Config::default());
        app.api_messages.push(crate::models::Message {
            role: Role::User,
            content: vec![crate::models::ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        });

        let result = preview_request(&mut app, Some("json"));

        // The command itself renders nothing: the engine is the authority.
        assert!(result.message.is_none(), "{result:?}");
        assert!(matches!(
            result.action,
            Some(AppAction::PreviewOutboundRequest { json: true, .. })
        ));
        assert_eq!(app.api_messages.len(), 1);
        assert!(app.history.is_empty());
    }

    #[test]
    fn base_prompt_provenance_is_runtime_not_a_source_path() {
        let label = crate::prompts::base_prompt_origin().label();
        assert!(!label.contains("crates/"), "{label}");
        assert!(!label.contains(".rs"), "{label}");
        assert!(
            label.contains("bundled") || label.contains("override"),
            "{label}"
        );
    }

    #[test]
    fn this_command_contains_no_prompt_dumping_path() {
        // Guard against the removed disclosure being reintroduced here: the
        // source of this module must not reference the prompt-text helpers.
        let source = include_str!("preview_request.rs");
        for forbidden in [
            "effective_base_prompt_text",
            "system_prompt_text",
            "compose_default_static_layers",
        ] {
            assert!(
                !source.contains(&format!("{forbidden}(")),
                "`{forbidden}` must not be callable from the command layer"
            );
        }
    }
}
