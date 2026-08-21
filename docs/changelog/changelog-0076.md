# changelog-0076 — harden route_budget parallel test flake

Date: 2026-08-20

## Summary

Harden two unit tests in `crates/tui` whose outcome depended on
process-global environment state that sibling tests mutate concurrently:

- `route_budget::tests::v4_trigger_uses_window_percent_when_it_fits_spendable_input`
- `prompts::tests::system_prompt_prefix_never_leaks_private_content`

Both fixes are test-only environment isolation; no assertion was weakened or
changed, and no production code changed.

## Root cause

`route_context_budget(ApiProvider::Deepseek, "deepseek-v4-pro", None, 0)`
resolves the route output reservation through
`explicit_max_output_tokens_override()`, which reads the process-global
`CODEWHALE_MAX_OUTPUT_TOKENS` / `DEEPSEEK_MAX_OUTPUT_TOKENS` environment
variables. The V4 test asserts the no-override values (output cap 65 536,
input ceiling 933 440, trigger 800 000) but never took `lock_test_env()` and
never pinned those variables. Sibling tests in the same binary (this module,
`client`, `vision/tools`, `core/engine`) set those variables while holding
the env barrier; because the V4 test did not participate in that barrier, a
concurrent writer could flip the value between the test's reads — the
order-dependent flake seen once in a full parallel run (passes in isolation,
passes on reruns).

The same class of bug surfaced during `--test-threads=1` verification in
`system_prompt_prefix_never_leaks_private_content`: the prompt is built with
`effective_home_dir()`, which reads process-global `HOME`/`USERPROFILE`.
The test never isolated those, so on a machine with
`~/.codewhale/instructions.md` the global instructions block leaked its real
absolute path into the prompt and failed the absolute-path assertion. The
test only passed in parallel runs when a sibling test's temporary `HOME`
guard happened to be live at the same moment; serialized, it failed
deterministically.

## Fix

- `crates/tui/src/route_budget.rs`: the V4 trigger test now holds
  `lock_test_env()` and removes `CODEWHALE_MAX_OUTPUT_TOKENS` and
  `DEEPSEEK_MAX_OUTPUT_TOKENS` for its duration — the established pattern
  used by every sibling test with the same dependency. Assertions unchanged.
- `crates/tui/src/prompts.rs`: the prefix-leak test now holds
  `lock_test_env()` and pins `HOME`/`USERPROFILE` to a scratch directory
  (and removes `DEEPSEEK_SKILLS_DIR`), matching the neighboring
  byte-stability tests, so global instructions and home-resolved skills
  cannot leak real absolute paths. Assertions unchanged.

## Verification

Full suite, `cargo test -p codewhale-tui --lib` (10 904 tests):

- Run A, `--test-threads=1`: 10891 passed / 0 failed / 13 ignored
- Run B, `--test-threads=1`: 10891 passed / 0 failed / 13 ignored
- Run C, default threads: 10891 passed / 0 failed / 13 ignored
- Run D, default threads: 10891 passed / 0 failed / 13 ignored

Four consecutive full-suite runs green, covering both thread modes. Before
the prompts fix, a serialized run reproduced the second failure
deterministically (1 failed, `system_prompt_prefix_never_leaks_private_content`);
after both fixes that run mode is green.

- `cargo fmt --all -- --check`: clean.
- `cargo clippy -p codewhale-tui --lib`: clean (exit 0).
- Focused checks: `route_budget::tests::v4_trigger_uses_window_percent_when_it_fits_spendable_input`
  and the full `prompts::` module (109/109) green in isolation.
