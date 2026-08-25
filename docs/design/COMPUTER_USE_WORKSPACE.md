# Computer use, phase 3 (future): the agent has a computer

Status: north star (2026-08-24). Not scheduled. Phase 2
([COMPUTER_USE_BACKGROUND.md](COMPUTER_USE_BACKGROUND.md)) ships the **host**
environment first; this document fixes the target shape so everything built
before it stays forward-compatible.

## The conceptual move

Computer use should not primarily mean "the agent controls my computer." It
should mean **"the agent has a computer."** For autonomous work the default
target is a persistent workspace machine owned by the agent; the user's real
desktop is an optional, higher-trust capability granted app by app.

## Three environments, one tool surface

- `workspace` — Codewhale's own persistent computer. Default for autonomous
  work. Persistent state, browser, repo, credentials, processes that stay
  alive, screenshots/recordings as artifacts.
- `host` — the user's real desktop. Explicitly granted, app by app
  (phase 2's consent model is this gate). Only for things that exist locally.
- `device` — Android / HarmonyOS, physical or emulated.

All higher-level tools (`computer_app_state`, `computer_element`,
click/type/key/scroll, future `computer_record`, `computer_files`) are
environment-independent. The agent never cares whether they become macOS AX,
Windows UIA, AT-SPI, ADB, or a remote VM call; that is the plugin's job. The
phase-2 `AppSelector`-keyed tool surface and the `ElementDriver` trait are
the seam: a workspace is a new *target*, not a new toolset.

## Architecture rules

1. **Never silently fall back from workspace → host.** If no workspace
   backend can start, say so and offer host mode explicitly; the security
   expectations are completely different.
2. **Abstract `WorkspaceBackend`, not "Docker".** Docker is the v1 backend;
   Lima/VM, Daytona, Modal, E2B, and a Shannon-managed cloud VM are later
   implementations of the same trait. Code thinks "workspace", never
   "container". Cloud is then a deployment of the same image + workspace API
   — local Codewhale and managed Codewhale are two deployments of the same
   thing (the Devin-shaped product).
3. **Secrets are a brokered capability, not a file copied into the box.**
   `codewhale secrets add github` + `codewhale workspace grant my-app github`
   → the workspace receives the minimum credential at runtime, scoped to that
   workspace/session, revocable. No host-env inheritance, no secrets in
   images or snapshots, and the grant disappears with the workspace.
4. **Inspect/control is first-class product surface, not debugging.** Live
   desktop view (VNC/noVNC), action/event log, pause/take-over/resume,
   downloadable artifacts (screenshots, `verification.mp4`, test results).
5. v1 is Docker + Ubuntu + Xvfb + WM + Chromium + VNC. VMs are architectured
   for (persistent disks, system services, stronger isolation) but not
   implemented until Docker hits a hard wall.

## Why the host lane comes first anyway

Phase 2's consent model (per-app allowlist, hard exclusions,
`needs_app_approval`) and the element-level tool surface are the pieces that
transfer directly: the workspace lane passes the consent gate trivially
(everything inside the box is allowed), and the same tools operate there
against the virtual display. Building them on the host first proves the
consent and verification semantics where they are hardest.

## North star test

Every computer-use architecture decision is judged by: *"give Codewhale a
task and it goes away for 40 minutes on its own computer and comes back with
a PR and a video proving it works."*
