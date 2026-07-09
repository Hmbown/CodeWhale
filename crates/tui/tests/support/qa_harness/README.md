# PTY/frame-capture TUI QA harness

Tiny helper for integration tests that need to drive `deepseek-tui` like a real
user typing in a real terminal — keys and paste, plus assertions over the
parsed terminal frame and the workspace filesystem.

## When to use this

Reach for this harness when a bug only shows up in the **interactive**
terminal: paste behaviour, slash menus, mode switching, viewport rendering,
onboarding flow, mouse capture. Anything where a `TestBackend` or a
unit test on the underlying state machine is too divorced from what the user
actually sees.

For pure logic tests on `App`, `SkillRegistry`, the engine's `Op` / `Event`
plumbing, etc., keep using `crates/tui/src/.../tests` style unit tests. Don't
spin up a PTY just to assert a function returns the right value.

## Anatomy

- `pty.rs` — `PtySession`. Spawns a binary in a real PTY (via `portable-pty`),
  pumps the child's stdout into a buffer on a background thread, exposes
  `write_bytes`, `drain`, `shutdown`.
- `frame.rs` — `Frame`. Wraps `vt100::Parser`. Feed bytes in, ask questions
  out: `text()`, `row(y)`, `contains(s)`, `cursor()`, `debug_dump()`.
- `keys.rs` — byte-sequence builders for keys (`key::ch('/')`,
  `key::enter()`, `key::text("hello")`) and for paste (`paste::bracketed(s)`,
  `paste::unbracketed(s)`).
- `harness.rs` — `Harness`. Composes the two. Has `wait_for`, `wait_for_text`,
  `wait_for_idle`, plus `make_sealed_workspace()` for a tempdir HOME.

## Adding a new scenario

1. Pick the smallest set of inputs that reproduce the user-visible behaviour.
   If you can't reproduce it without a real LLM turn, the scenario probably
   belongs in a unit test (or a `wiremock`-driven turn test) instead.

2. Build a sealed workspace so the scenario doesn't see the developer's real
   `~/.deepseek/` or API keys:

   ```rust
   let ws = qa_harness::harness::make_sealed_workspace()?;
   std::fs::write(ws.user_skills_dir().join("foo/SKILL.md"), "...")?;
   ```

3. Spawn:

   ```rust
   let mut h = Harness::builder(Harness::cargo_bin("codewhale-tui"))
       .cwd(ws.workspace())
       .seal_home(ws.home())
       .env("DEEPSEEK_API_KEY", "ci-test-key")
       .args(["--workspace", ws.workspace().to_str().unwrap(),
              "--no-project-config", "--skip-onboarding"])
       .size(40, 120)
       .spawn()?;
   ```

4. Drive it:

   ```rust
   h.wait_for_text("Composer", Duration::from_secs(10))?;
   h.send(keys::key::ch('/'))?;
   h.wait_for_text("/skills", Duration::from_secs(2))?;
   ```

5. Assert:

   ```rust
   let f = h.frame();
   assert!(f.contains("local-skill"), "frame:\n{}", f.debug_dump());
   ```

6. Always shut down cleanly at the end so the PTY cleanup runs even on a
   failing assertion:

   ```rust
   let _ = h.shutdown();
   ```

## Conventions

- **Sealed env always.** No scenario should be able to see the real
  `$HOME/.deepseek/` or contact `api.deepseek.com`. If a scenario *has* to do a
  real model turn, route through a local `wiremock` or `tiny_http` fake
  provider and pass `DEEPSEEK_BASE_URL=<localhost>`.
- **Fail noisily.** When an assertion fails, print `frame.debug_dump()` so the
  CI log shows the rendered screen, not just `assertion failed`.
- **Prefer `wait_for_text` over `sleep`.** A scenario that sleeps 500ms before
  asserting will flake under CI load. A scenario that polls with a 10s
  timeout is robust.
- **Expect output to be slow on first launch.** The TUI does config probing,
  skill installation, and snapshot cleanup before showing the composer.
  Give startup at least 10–15 seconds before timing out.

## Platforms

`portable-pty` works on macOS, Linux, and Windows (ConPTY). Today the
scenarios target Unix only — the test binary is gated with
`#![cfg(unix)]` until the Windows-specific input plumbing has been audited
under the same harness.
