# Contributors

Codewhale is built in the open with a growing community of contributors. Every
issue report and pull request is real project work — welcome at any experience
level. This is the full per-PR contributor record, grouped into release/time
bands with the newest band first so it stays scannable. Expand any band to see
everyone.

For the live list, see the
[GitHub contributors page](https://github.com/Hmbown/CodeWhale/graphs/contributors),
[`AUTHOR_MAP`](https://github.com/Hmbown/CodeWhale/blob/main/.github/AUTHOR_MAP),
and [CHANGELOG.md](../CHANGELOG.md).

## Organizational thanks

- **[DeepSeek](https://github.com/deepseek-ai)** — the models and support that got this project started. 感谢 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — for the support and for welcoming us into the Whale Brother family. 感谢 DataWhale 的支持。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — for prioritizing Codewhale support and collaborating on a better terminal-agent experience.
- **[Open Design](https://github.com/nexu-io/open-design)** — for support and collaboration around design-forward agent workflows.

The maintainer rule: reports and PRs are real project work, even when the final
patch has to be narrowed, delayed, or harvested into a maintainer branch.
Harvested PRs keep visible credit in the commit/PR body, changelog or release
notes, and relevant issue/PR comments.

---

## Contributors by time

<details open>
<summary><strong>v0.9.12 — native search, live token truth, and runtime surfaces</strong></summary>

**Merged or adapted contributions**

- **[hexin / h3c-hexin](https://github.com/h3c-hexin)** — provider-native web
  search across four routes: the DeepSeek adapter (#5683), Z.AI and BigModel
  (#5685), and Xiaomi MiMo (#5687), each fail-closed for undocumented models,
  plus the constraint enforcement that runs before any fallback (#5682); also
  authoritative edit-last-turn boundaries (#5621), Kimi Code k3-256k support
  (#5622), post-compaction input-token reporting (#5623), and preserving the
  scheduled model selection in automations (#5650)
- **[Isabel Wu / wuisabel-gif](https://github.com/wuisabel-gif)** — live
  session token totals (#5624), persisted context-pressure warnings (#5629),
  discoverable Fleet roster editing (#5604), the capability-gated cursor
  accent (#5599), and `/copy` for the latest completed response (#5692)
- **[Paulo Aboim Pinto / aboimpinto](https://github.com/aboimpinto)** —
  preserved Windows verbatim-path operands through POSIX word splitting
  (#5610) and moved the plugins group onto the command shapes (#5657)
- **[Alex Musichen / musichen](https://github.com/musichen)** — fixed the
  configured-view DeepSeek model picker so it shows a stable DeepSeek heading
  and keeps every official catalog model for the active provider visible
  (#5689)
- **[gaord](https://github.com/gaord)** — added the `GET /v1/fleet/profiles`
  runtime API endpoint, reusing the FleetManager validation path (#5688)
- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — corrected
  English documentation inaccuracies and contributed the first zh_hans
  translations for the Tier-2 docs (#5613)
- **[M-Maciej](https://github.com/M-Maciej)** — goal continuation cadence fix
  (#5591)
- **[Serephus / serephus](https://github.com/serephus)** — nixpkgs update
  (#5669)

</details>

<details>
<summary><strong>v0.9.11 — portable command surfaces and turn-loop structure</strong></summary>

**Merged or adapted contributions**

- **[Sun Zhenyuan / bistack](https://github.com/bistack)** — extracted tool-call
  planning, execution, and result processing from the turn loop while preserving
  the existing sandbox-policy, hook, budget, and batch-execution behavior
  (#5523)
- **[Isabel Wu / wuisabel-gif](https://github.com/wuisabel-gif)** — added
  bounded, workspace-relative multi-file diagnostics to the existing model-
  visible `lsp` tool, reusing the shared transport pool without expanding the
  75-tool catalog (#5524)
- **[Paulo Aboim Pinto / aboimpinto](https://github.com/aboimpinto)** — added
  portable presentation, media-attachment, and operation-digest facets to the
  command contract, then moved all seven utility command handlers onto the
  contract-backed dispatch path (#5525)

**Reviewed overlap**

- **[Lstarsky0](https://github.com/Lstarsky0)** — reviewed #5524 and identified
  the closed-channel, severity-filtering, and truncation-receipt gaps covered by
  the maintainer hardening pass.
- Isabel Wu's PR #5530 independently routed the legacy plural completion
  command through the public CLI generator. Equivalent behavior, including the
  published `codew` alias registration, had already landed in the integration
  lane, so the PR is recorded here as reviewed project work rather than counted
  as a second merged contribution.

**Reports and reproductions**

- **[RepentStar](https://github.com/RepentStar)** — reported the stale shell-
  completion generator and missing `codew` registration that led to the
  completion fixes and upgrade guidance in #5526.

</details>

<details>
<summary><strong>v0.9.10 — retention, identity, and durable approvals</strong></summary>

**Merged or adapted contributions**

- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — restored
  `/title` as an independent, persistent terminal-window title in PR #5509,
  and contributed the Tier 1 Chinese and Indonesian documentation move to
  `docs/zh_hans/` and `docs/id/` with one-release redirect stubs (#5507,
  epic #5482)
- **[Sun Zhenyuan / bistack](https://github.com/bistack)** — extracted the
  turn-loop stream processor in PR #5514 while preserving retry,
  cancellation, usage, TTFT, steering, and partial-response behavior
- **[OctoBored](https://github.com/OctoBored)** — supplied the working
  no-token Star History mirror used by the localized README set after the
  canonical chart endpoint began returning a restricted placeholder (#5510)
- **[cacdcaecawae](https://github.com/cacdcaecawae)** — added provider-neutral
  typed MCP image forwarding in PR #5515; the adapted version also emits a
  visible omission receipt for malformed image fields
- **[cyq1017](https://github.com/cyq1017)** — approval outcomes are persisted
  before execution can proceed: receipts commit to a session-owned log first,
  unpersistable evidence blocks the tool, stale decisions are rejected, and
  resume reconstructs closed and interrupted approvals (#5491, closes #5360)
- **[aboimpinto](https://github.com/aboimpinto)** — the TUI-owned
  dependency-injection and migration infrastructure behind safe slash-command
  extraction: seven capability facets, a dual-path dispatch seam, and
  source-aware CI enforcement so a command slice cannot claim migration while
  it still accepts concrete `App` (#5506, EPIC-005/FEAT-015 under the #5316
  umbrella they filed)
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — the TUI header now
  names the active repository or linked worktree before the branch and dirty
  marker, derived from Git's common directory and capped by shell density
  tier (#5511, the repo/worktree slice of #5437)
- **[Lstarsky0](https://github.com/Lstarsky0)** — `docs/hooks` and
  `docs/troubleshooting` move onto the dictionary spine, retiring their inline
  bilingual literals in favor of locale dictionaries with token-aware code
  spans (#5504, closes #5337 they filed)

**Reports, reproductions, and verification**

- **[DingYong4223](https://github.com/DingYong4223)** — reported the
  narrow-terminal completion truncation closed by the bounded hover reveal
  (#998)
- **[AiurArtanis](https://github.com/AiurArtanis)** and
  **[formp3](https://github.com/formp3)** — identified the completion surfaces
  affected by narrow-terminal truncation (#998)
- **[sky-sun-moon](https://github.com/sky-sun-moon)** — reported the missing
  full per-turn input, reasoning, tool, and assistant pages that shaped Turn
  Inspector navigation (#1682)
- **[cy2311](https://github.com/cy2311)** — reported the Windows launch path
  now covered by the Windows Terminal-aware batch launcher (#1854)
- **[demian-welt](https://github.com/demian-welt)** — provided the reproducible
  pre-header SSE transport failure behind the bounded HTTP/1.1 retry (#4683)
- **[BrathonBai](https://github.com/BrathonBai)** — reported the Tabby/CJK IME
  candidate-window jump that led to the cursor hide-diff-position-show
  transaction (#5023)
- **[M-Maciej](https://github.com/M-Maciej)** — provided the organization-
  coordinator use case and 5-30 minute cadence requirement behind cancellable
  cross-turn goal delays (#5508)
- **[thejayjetson](https://github.com/thejayjetson)** — the header
  status-indicator report that pinned the regression to a specific setting,
  having already ruled out every value, both themes, `fancy_animations`, the
  terminal auto-off rules, and a clean `codewhale doctor` (#5512)
- **[hxfhd](https://github.com/hxfhd)** — reported the deterministic
  cross-session contamination class behind this release's session-ownership
  boundary, plus route budgeting evidence (#5518)
- **[sfdzhmr](https://github.com/sfdzhmr)** — reported the route budgeting
  root causes that exact route-limit propagation now closes (#5516)

</details>

<details>
<summary><strong>v0.9.9 — truth and resilience</strong></summary>

**Merged or adapted contributions**

- **[h3c-hexin](https://github.com/h3c-hexin)** — a concrete route/offering
  output limit now outranks the conservative 8,192-token compatibility guess
  for an uncatalogued model; routes that publish no limit stay fail-closed
  (#5461, closes #5460); every web tool surface uses the noisy-result soft
  limit (#5474); lowercase saved selectors resolve against the owning
  Z.ai/DeepSeek catalog row (#5475); configured-skill prompt paths remain
  stable across ephemeral roots and operating systems (#5492, #5473)
- **[Gabriel-Degret](https://github.com/Gabriel-Degret)** — configurable
  auto-router classifier timeout (#5494; first contribution)

**Reports, reproductions, and verification**

- **[asto18089](https://github.com/asto18089)** — diagnosed the Z.ai `glm-5.2`
  casing collision and wrote the first provider-scoped fix downstream
  (Pinvou/CodeWhale#14, carried upstream in #5475)

- **[hardy922](https://github.com/hardy922)** — the context-window honesty
  report that became the every-surface labeling pass (#5239)
- **[redstar](https://github.com/redstar)** — bwrap container essentials and
  configurable extra roots (#5410)
- **[all-lopezg](https://github.com/all-lopezg)** — SSE UTF-8 garbling on
  DeepSeek Flash with screenshots, fixed fail-closed in every dialect (#5374)
- **[alitvak69](https://github.com/alitvak69)** — the unverified-live-pricing
  report behind the bundled fallback rates and DeepSeek V4 tiers (#5241)
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — bounded reproduction
  attempts for the macOS filtered-suite hang note on #5056

</details>

<details>
<summary><strong>v0.9.8 — marketplace, Google, Ollama Cloud</strong></summary>

**Merged or adapted contributions**

- **[EvanProgramming](https://github.com/EvanProgramming)** — webhook hook
  sink no longer panics when its HTTP client fails to build (#5381); session-index
  JSONL writes are serialized so concurrent stores cannot drop an append
  (#5382)
- **[Lstarsky0](https://github.com/Lstarsky0)** — session peek hides internal
  runtime events (#5376); thinking-ladder tests re-pinned (#5378);
  provider-count follow-ups (#5383/#5384); macOS agy fixture
  canonicalization (#5392); website zh-Hans 宪章 terminology (#5397);
  regenerated website facts harvested and corrected so Google and Ollama
  Cloud are listed and Antigravity is not advertised as a route (#5398)
- **[mvanhorn](https://github.com/mvanhorn)** — read-only, repository-fact
  driven settings preview on the website models page (#5411, fixes #5370)
- **[buiducnhat](https://github.com/buiducnhat)** — canonical `ultra`
  reasoning effort mapped to every provider's maximum tier (#5409); session
  titles truncated by character count instead of byte offset so CJK and
  emoji names cut at the intended width (#5415)
- **[SparkofSpike](https://github.com/SparkofSpike)** — `/title` and the
  saved session name in the terminal tab/window title, plus the fix for the
  deadlock when the title changed during an active turn; landed as a
  discoverable alias of `/rename` on the one persisted session name (#5419)
- **[redstar](https://github.com/redstar)** — Eden AI registered as a named
  OpenAI-compatible provider with `EDENAI_API_KEY`, `eden-ai`/`eden_ai`
  aliases, and the global and EU endpoints (#5422)
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — background verifier
  test isolated from the rustup `rustc` shim and `$HOME`, one narrow
  reliability slice of #5056 (#5423)

</details>

<details>
<summary><strong>v0.9.7 — the catalog stays ordinary</strong></summary>

**Merged or adapted contributions**

- **[XhesicaFrost](https://github.com/XhesicaFrost)** — copy from the message
  context menu now takes canonical content, keeping authored Unicode,
  Markdown, and hard line breaks in the clipboard while role glyphs,
  continuation rails, and visual wrapping stay out of it (PR #5319)
- **[h3c-hexin](https://github.com/h3c-hexin)** — split durable-session
  snapshot reads from crash recovery so an embedding host inspecting a
  session while a tool is still running no longer gets a spurious crash
  repair (PR #5320)
- **[XiaoHuo888-hue](https://github.com/XiaoHuo888-hue)** — OrcaRouter as a
  first-class named provider with `ORCAROUTER_API_KEY`, auto-routing,
  CLI `--provider` selection, and TUI picker entries (PR #5321)

**Reports and verification**

- **[USTHzhanglu](https://github.com/USTHzhanglu)** — reported the Auto-Review
  shell/write regression with a Windows reproduction and audit-log evidence
  (issue #5323)
- **[Lstarsky0](https://github.com/Lstarsky0)** — traced the Auto-Review
  regression to the exact permission-posture change and documented the safe
  recovery choices (issue #5323)

</details>

<details>
<summary><strong>v0.9.6 — fewer bumpers, truthful endings</strong></summary>

**Merged or adapted contributions**

- **[Xavier Pestel / xavierpestel-ai](https://github.com/xavierpestel-ai)** —
  Mistral AI (la Plateforme) as a first-class provider route, including the
  adjustable-reasoning wire contract and its multi-turn thinking replay
  (PR #5295)
- **[Ben Younes / ousamabenyounes](https://github.com/ousamabenyounes)** —
  inherited nested-agent depth cap, keeping child agents from widening the
  budget provided by their parent (PR #5317)
- **[Rafael Cavalheri / rafaelcavalheri](https://github.com/rafaelcavalheri)** —
  multi-round ACP file, search, Git, patch, and explicitly enabled shell tool
  execution with cancellation and fail-closed shell safety (PR #5225)

</details>

<details>
<summary><strong>v0.9.5 — one runtime, managed controls, and uninterrupted turns</strong></summary>

**Merged or adapted contributions**

- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — stale cached
  session-title recovery so `New Session` yields to the first real prompt
- **[Paulo Aboim Pinto / aboimpinto](https://github.com/aboimpinto)** — the
  shared alias-aware command discovery contract across the palette, slash
  completion, dispatch, and acceptance harness
- **[Sun Zhenyuan / bistack](https://github.com/bistack)** — background
  incremental MCP Registry refreshes that return the local snapshot immediately
- **[SKY ZHAO / skyzhao1223](https://github.com/skyzhao1223)** — prompt-based
  `model = "auto"` tier selection in PR #5257

</details>

<details>
<summary><strong>v0.9.4 — calmer output, recovery, and release hardening</strong></summary>

**Merged or adapted contributions**

- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — LaTeX
  rendering in PR #4981, circled-digit and keycap width handling in PR #5001,
  and actionable File-edit recovery in PR #5008; sub-agent checkpoint resume
  in PR #5242, real shell wait elapsed time in PR #5240, and keeping
  alternate-scroll off while mouse capture is active in PR #5234
- **[XhesicaFrost](https://github.com/XhesicaFrost)** — long Windows user-PATH
  preservation in PR #5006
- **[Paulo Aboim Pinto / aboimpinto](https://github.com/aboimpinto)** — the
  user-command dispatch acceptance contract in PR #4992
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — Windows
  OpenHarmony linker re-quoting for spaced SDK paths in PR #5095
- **[bistack](https://github.com/bistack)** — MCP Registry discovery with
  Registry-first tool selection in PR #5238
- **[vFONGv](https://github.com/vFONGv)** — the zh-CN Windows beginner guide
  with screenshots in PR #5229
- **[mky](https://github.com/mky)** — FreeBSD build fix (`rquickjs` `bindgen` on FreeBSD, PR #5254)
- **[cacdcaecawae](https://github.com/cacdcaecawae)** — embedder-owned sub-agent state roots (PR #5252)

**Reports, reproductions, and verification**

- **[DracheTek](https://github.com/DracheTek)** — the multilingual, CRLF-heavy
  File-edit failure report in issue #5003
- **[An Ziwu / MuRongMoQing](https://github.com/MuRongMoQing)** — the Windows
  PATH-overwrite defect report in issue #4685

</details>

<details>
<summary><strong>v0.9.3 — DeepSeek Responses, canonical tools, and runtime hardening</strong></summary>

- **[greyfreedom](https://github.com/greyfreedom)** (Turisla) — the documented
  and tested authorization-order contract from PR #4980
- **[nightt5879](https://github.com/nightt5879)** (Nightt) — foreground shell
  detachment before steering from PR #4979
- **[Watcher24](https://github.com/Watcher24)** — the macOS File Provider
  sandbox report and reproduction in #4085
- **[Fred Leitz / fleitz](https://github.com/fleitz)** — required
  source-candidate credit for the canonical `Bash` no-`cwd` workspace fix and
  regression in PR #4673 (issue #4674)

</details>

<details>
<summary><strong>v0.9.2 — routing, workflow, localization, and inspectability</strong></summary>

- **[greyfreedom](https://github.com/greyfreedom)** — exact repository-scoped
  allow grants and the cross-platform path-semantics repairs from PR #4761
- **[nightt5879](https://github.com/nightt5879)** — off-event-loop terminal
  clipboard writes (PR #4896), complete locale exposure in settings (PR #4856),
  CRLF-safe edits (PR #4942), and automatic-routing reasoning preservation
  (PR #4961)
- **[XhesicaFrost](https://github.com/XhesicaFrost)** — the configurable
  session-token header from PR #4610 and context-menu hover alignment in PR #4897
- **[cyq1017](https://github.com/cyq1017)** — the hooks configuration/executor
  split from PR #4087
- **[snail-vs](https://github.com/snail-vs)** (snailoniu) — OpenCode Zen's
  model-aware route, authentication, documentation, and test-isolation work
  from closed PR #4467, whose contributor commits are preserved in the
  candidate
- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — the zh-Hans
  translation quality review harvested from PR #4908, duplicate-session fix in
  PR #4963, LaTeX implementation from PR #4973 landed through #4974, and the
  context-menu reproduction in #4803
- **[GTC2080](https://github.com/GTC2080)** (TaoMu) — the `dryrun` concept
  behind `/preview-request`: preview the next outbound request from the real
  request-building seam rather than a hand-rolled summary (PR #1099,
  harvested)
- **[h3c-hexin](https://github.com/h3c-hexin)** — the non-UTF-8 `fetch_url`
  report and implementation direction harvested from PR #4909
- **[Fred Leitz / fleitz](https://github.com/fleitz)** — required
  source-candidate credit for the canonical `Bash` no-`cwd` workspace fix and
  regression in PR #4673 (issue #4674)
- **[LmeSzinc](https://github.com/LmeSzinc)** — the Windows CRLF `edit_file`
  reproduction, root-cause analysis, and affected-code anchors in issue #4764
- **[atmosuwiryo](https://github.com/atmosuwiryo)** — ACP numeric-id
  compatibility (PR #4929) and the Indonesian documentation and website locale
  (PRs #4962 and #4972)
- **[M-Maciej](https://github.com/M-Maciej)** — the expanded-by-default
  reasoning setting and report (PR #4928, issue #4925)
- **[raphamorim](https://github.com/raphamorim)** — migration of the QA PTY
  frame parser to `rio-vt` (PR #4931)
- **[LI-Jialu](https://github.com/LI-Jialu)** — truthful finalization of stale
  restored shell cells (PR #4937)
- **[kobihikri](https://github.com/kobihikri)** — release-container SBOM and
  explicit provenance mode (PR #4958)
- **[EvanProgramming](https://github.com/EvanProgramming)**,
  **[adity982](https://github.com/adity982)**, and
  **[vibecoding-skills](https://github.com/vibecoding-skills)** — equivalent
  fix direction for effective-home resolution (#4760), MCP call-once behavior
  (#4756), and streaming/non-streaming timeout separation (#4743)
- **[antarikshraya](https://github.com/antarikshraya)** — the LaTeX transcript
  rendering report in #4957
- **[eugenicum](https://github.com/eugenicum)** — the token-header request and
  output-presentation measurements in #4520 and #4468
- **[whp233](https://github.com/whp233)** — the StepFun/OpenCode subscription
  route request in #4526
- **[redjade75723](https://github.com/redjade75723)** — stale resumed-workspace
  diagnosis input from #4100
- **[hardy922](https://github.com/hardy922)** — the native DeepSeek tool-token
  visibility report in #3880
- **[JayBeest](https://github.com/JayBeest)** — contributor-onboarding product
  direction in #4227
- **[elijahchan2019](https://github.com/elijahchan2019)** — the optional
  reasoning-highlight request in #4089
- **[cy2311](https://github.com/cy2311)** — sessions rail, archive, and
  auto-resume direction in #2934
- **[wywsoor](https://github.com/wywsoor)** — export/restore correlation
  direction in #2494

</details>

<details open>
<summary><strong>v0.9.1 — provider and runtime hardening</strong></summary>

- **[h3c-hexin](https://github.com/h3c-hexin)** — calendar-anchored hourly
  automation recurrence across DST and lifecycle transitions (PR #4381),
  explicit limits for unknown local models (PR #4656 / #4655), and
  idle-timeout progress telemetry (PR #4657)
- **[gaord](https://github.com/gaord)** — Runtime API provider registry and
  atomic provider-switch endpoints (PR #4658)
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — the unified
  `/skills` root catalog, audit/provenance model, validated mutations, manager
  UI, and acceptance coverage (PR #4679), plus Enter-send lag diagnosis and
  fix direction for #4605 (PR #4654; landed through the release-lane
  async-dispatch split)
- **[Paulo Aboim Pinto / aboimpinto](https://github.com/aboimpinto)** — the
  Layer 5.1 user-command registry boundary from PR #3278; the exact authored
  evidence commit from PR #4046, preserved intact in the integration graph;
  and the #2870 follow-up audit whose metadata and malformed-sibling gaps
  shaped the final corrections. Paulo also provided the structured, redacted
  Agent Details and `current_activity` design direction preserved from
  #2694/#2889 and the real-PTY lifecycle acceptance direction from #2886
- **[baendlorel](https://github.com/baendlorel)** — TelecomJS TokenHub provider
  and key-scoped live-catalog direction, source-partitioned refresh behavior,
  and both refresh-order regressions from PR #4370, harvested into the current
  provider architecture with co-authorship preserved
- **[zhangweiii](https://github.com/zhangweiii)** and
  **[Sterne Lee / sternelee](https://github.com/sternelee)** — the original
  first-class OpenCode Go implementations in PRs #773 and #1050, harvested
  into the current provider architecture with co-authorship preserved in the
  landing commit
- **[Sean Tse / seanthefuturegorilla](https://github.com/seanthefuturegorilla)**
  — the canonical OpenCode Go/Zen provider request and acceptance direction in
  #1481
- **[nightt5879](https://github.com/nightt5879)** — `/debt` compatibility
  aliases with dispatch-consistent user-command shadowing across help and
  slash discovery (PR #4680), plus the Solarized Light background preservation
  fix for the underwater shell (PR #4471)
- **[AiurArtanis](https://github.com/AiurArtanis)** — the Solarized Light
  v0.9.0 regression report and reproduction (#4457)
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — the HarmonyOS
  workflow-js bindgen, portable-pty gating, and SDK environment work
  (PR #4470)
- **[shenyongqing](https://github.com/shenyongqing)** — the original HarmonyOS
  workflow-js bindgen approach (PR #4384), carried into the landed
  implementation with credit
- **[Luis Mateus Vargas / luismateusvargas](https://github.com/luismateusvargas)**
  — the Windows hook-process leak reproduction, process-tree analysis, and EOF
  fix direction (#4489)
- **[redjade75723](https://github.com/redjade75723)** — the persistent Windows
  PTY failure report that exposed lossy high-bit exit-status handling (#4100)
- **[w1w218](https://github.com/w1w218)** — the Windows ARM64 release request
  and cross-compilation report that led to native release, npm, updater, and
  archive support (#4267)
- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — the Windows
  Ctrl+O reproduction that exposed pre-pager result truncation and conflicting
  composer shortcut routing (#4482)
- **[Lu Shihan / Angel-Hair](https://github.com/Angel-Hair)** — session-owned
  read-before-edit tracking and the explicit, backwards-compatible
  `apply_patch` replacement contract (PRs #4475 and #4476)
- **[dmitri-0](https://github.com/dmitri-0)** — configurable cache-hit
  visibility in the phase strip (PR #4474)
- **[Fred Leitz / fleitz](https://github.com/fleitz)** — canonical `Bash`
  no-`cwd` workspace resolution and its regression test, keeping isolated
  sub-agent commands inside their selected worktree (PR #4673; issue #4674)
- **[Sh1Zuku / SparkofSpike](https://github.com/SparkofSpike)** — exact
  Vim-space regression reproduction and verification that the v0.9.1 input
  path already contains the needed global binding (PR #4477)

</details>

<details open>
<summary><strong>v0.9.0 — underwater shell, runtime persistence &amp; release evidence</strong></summary>

The v0.9.0 lane grew from a maintenance sweep into the underwater shell,
message-first Operate, broader Fleet and Workflow behavior, runtime-thread
persistence, terminal selection, UTF-8 handling, internationalization, and
release evidence. The reports and pull requests below materially shaped it.

- **[Amuthan / amuthantamil](https://github.com/amuthantamil)** — approval-time
  transcript review report that led to live Page Up/Down, modified-arrow,
  Home/End, and mouse-wheel navigation while the decision card stays active
  (#4371)
- **[Angel-Hair](https://github.com/Angel-Hair)** — reports that restored the
  opt-in `remember` tool to the first-turn catalog, made required user
  confirmation a real goal blocker, and added a clear recovery notice for
  cached approval denials (#4373, #4374, #4375)
- **[Bruce / bruce6135](https://github.com/bruce6135)** — Kimi Coding Plan
  reproduction that exposed the false 1K emergency-compaction budget (#4368)
- **[Matt Van Horn / mvanhorn](https://github.com/mvanhorn)** — first-turn
  `remember` regression coverage and the Kimi output-budget correction that
  prevents false emergency compaction (#4377, #4378)
- **[郝某人BH / hmr-BH](https://github.com/hmr-BH)** — native Simplified
  Chinese review that replaced literal legal/doctrinal metaphors with clear
  collaboration terminology across setup and `/constitution` (#4369)
- **[WavesMan](https://github.com/WavesMan)** — large-tree `@` completion
  reproduction that led to bounded background discovery and exact-path
  resolution on send (#4365)
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — input-budget-aware
  compaction work harvested from PR #4293 with co-authorship preserved
- **[idling11](https://github.com/idling11)** — keyboard-driven keyword search
  for the docs and FAQ surfaces (PR #4364)
- **[LeoLin990405](https://github.com/LeoLin990405)** — initial bounded
  workspace-walk approach and root-cause analysis for large-tree `@` mention
  discovery (PR #4367), harvested into the final fail-soft background
  implementation for #4365
- **[octo-patch](https://github.com/octo-patch)** — MiniMax Messages provider
  support for MiniMax-M3 and MiniMax-M2.7 across OpenAI-compatible and Messages
  routes, regional endpoints, catalog metadata, pricing, and request coverage
  (PR #4354)
- **[Wenshan Deng / findshan](https://github.com/findshan)** — original offline
  token/cache/cost scorecard and regression gate (#3388), extended with
  provider-aware provenance in #4335
- **[hongqitai](https://github.com/hongqitai)** — localization extraction and
  English-locale enforcement that keep UI copy on the typed translation path
  (#4225, #4194)
- **[nsfoxer](https://github.com/nsfoxer)** — bounded, fail-soft MCP capability
  discovery with advertised/legacy `tools/list` handling and Unicode-safe
  description formatting (#4308, harvested with co-authorship)
- **[maple / yekern](https://github.com/yekern)** — root-cause analysis and fix
  direction for age-evicting terminal worker records from long-lived
  sub-agent state ledgers (#4217)
- **[moduvoice](https://github.com/moduvoice)** — Korean (ko) UI locale with
  full key parity and onboarding/setup wiring (PR #4347)
- **[qinlinwang](https://github.com/qinlinwang)** — Anthropic tool-schema
  sanitization for top-level oneOf/anyOf/allOf failures (PR #4346)
- **[knqiufan](https://github.com/knqiufan)** — Anthropic cache-write token
  pricing at published rates (PR #4348, #4318)
- **[Chavdar Ivanov / ci4ic4](https://github.com/ci4ic4)** — NetBSD QuickJS
  bindgen support for codewhale-workflow-js (PR #4349)
- **[eugenicum](https://github.com/eugenicum)** — copy-paste rail-pollution
  report with code-aware fix direction (#4208)
- **[JayBeest](https://github.com/JayBeest)** — parent
  `--disallowed-tools` inheritance through sub-agents and Fleet workers,
  harvested from PR #4096 with authorship preserved (#4042)
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — move runtime-thread
  coordination onto `parking_lot::Mutex` to remove async lock contention
  (#4243, #4149)
- **[cyq1017](https://github.com/cyq1017)** — Windows Python stdio UTF-8
  handling and richer active-tool summaries (#4281/#4202, #3818)

- **[Jeffrey Luna / Mr-Moon121](https://github.com/Mr-Moon121)** — anti-polling
  constitution for sub-agent waiting (harvested into #4097 / PR #4229 from
  PR #4098)

- **[MXAntian](https://github.com/MXAntian)** — persist compaction summaries
  into thread records so `/v1` engine reloads keep compacted context (#4091)
- **[nightt5879](https://github.com/nightt5879)** — keep native terminal
  selection usable when mouse capture is disabled, and advance fuzzy edit
  matches on UTF-8 character boundaries; preserve inline skill task text in
  PR #4372 (#4088, #4045, #3915)
- **[gaord](https://github.com/gaord)** — add the community-maintained
  Codewhale for VS Code GUI frontend to the English and Chinese READMEs (#4035)
- **[Darrell Thomas](https://github.com/DarrellThomas)** — remove the unused
  whale route taxonomy module and its dead tests (#4041)
- **[Taixin Guo](https://github.com/taixinguo)** — CJK fuzzy-edit panic report
  and fix direction credited in the UTF-8 boundary patch (#3971, #4045)
- **[CCChisato](https://github.com/CCChisato)** — preserve task text when
  sending dollar, unified-slash, and explicit skill invocations (#3915,
  co-authored)
- **[Sun Zhenyuan / bistack](https://github.com/bistack)** — dynamic MCP server
  infrastructure and approval-gated model-started MCP servers from chat
  context (#3869, #3866, harvested with authorship preserved)

</details>

<details>
<summary><strong>v0.8.66 — release readiness, provider intake &amp; UI hardening</strong></summary>

The v0.8.66 release prepared the 0.8.66 package lane, hardened provider/model
routing and modal surfaces, advanced Hotbar/sub-agent UI reliability, and pulled
in several community provider and bridge contributions with release credit.

- **[lerugray](https://github.com/lerugray)** — Sakana AI Fugu provider
  support across config, CLI, TUI provider picker, docs, and model completions
  (#3748, harvested)
- **[greyfreedom](https://github.com/greyfreedom)** — read-only `/config
  ask-rules` view for resolved ask-rule paths, status, and configured
  tool/command/path rules (#3569, merged)
- **[noaft](https://github.com/noaft)** — `/links` provider docs fallback
  update, including the current Codewhale docs URL and a Baidu Qianfan docs
  link (#3621, harvested)
- **[noaft](https://github.com/noaft)** — OpenModel provider support across
  config, CLI, TUI provider picker, docs, and registry checks (#3585,
  harvested)
- **[pkeging](https://github.com/pkeging)** — original plugin manifest,
  discovery, CLI, and MCP foundation (#3708, #3709, #3710, merged), plus WeCom
  Bridge deployment and security documentation, including the approval-timeout
  configuration surface (#3640, harvested)
- **[codepgq](https://github.com/codepgq)** — cross-client plugin-workflow
  migration request that shaped the explicit compatibility and trust boundary
  (#1172)
- **[Wenshan Deng / findshan](https://github.com/findshan)** — original offline
  token/cache/cost scorecard and regression gate (#3388), extended with
  provider-aware provenance in #4335
- **[buko](https://github.com/buko)** — precise Ctrl+O external-editor freeze
  reproduction that shaped the terminal input-pump fix (#3657)
- **[cyq1017](https://github.com/cyq1017)** — sub-agent progress-event
  headroom report that shaped the fanout reliability hardening (#3783)

</details>

<details>
<summary><strong>v0.8.63 — sub-agent budgets, command extraction &amp; reliability</strong></summary>


The v0.8.63 release hardened sub-agent fanout with token-budget governance and
queue-and-drain admission, split the TUI command surface into focused modules,
and landed reliability fixes for app-server teardown, JavaScript-execution
proxying, and DeepSeek thinking tool calls — alongside community contributions.

- **[donglovejava](https://github.com/donglovejava)** — per-worker sub-agent
  token-budget enforcement, so a `token_budget`/`max_tokens` on an individual
  `agent` call bounds that worker mid-run with a clean `budget_exhausted` stop
  (#3321, harvested)
- **[cyq1017](https://github.com/cyq1017)** — `js_execution` proxy-environment
  handling (#3331), Hugging Face API-key env in the auth probe (#3329), and Codex
  Responses request retry (#3344) — harvested into the train
- **[aboimpinto](https://github.com/aboimpinto)** — FEAT-005 command extraction:
  core/session command groups split into focused modules via `RegisterCommand`,
  `/swarm` migration, and Gherkin acceptance coverage (#3330, merged literally
  with authorship preserved)
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — tear down the delegated
  serve/app-server child process when the dispatcher exits (#3259 / #3317)
- **[nightt5879](https://github.com/nightt5879)** — keep the onboarding marker in
  the codewhale home view (#3302) and branch-hygiene check hardening (#3348)
- **[gaord](https://github.com/gaord)** — preserve thinking/tool blocks when
  seeding a thread from a saved session, plus Hugging Face provider env (#3329)
- **[greyfreedom](https://github.com/greyfreedom)** — persist ask-permission rules
  from approvals and stabilize the CI verifier/provider-registry checks
- Reports that shaped fixes: **[lordwedggie](https://github.com/lordwedggie)**
  (#3331 proxy env), **[Final527](https://github.com/Final527)** (#3240 legacy
  state migration), **[dxfq](https://github.com/dxfq)** (#3228 sidebar default)

</details>

<details>
<summary><strong>v0.8.62 — provider/model routing, TOML comment preservation &amp; community closeout</strong></summary>


The v0.8.62 release retuned provider/model routing (GLM-5.2 as the default direct
Z.AI model, `type: "explore"` sub-agents defaulting to the cheaper same-family
sibling), added TOML comment preservation and the Codewhale-only skill discovery
gate, and shipped the static Linux x64 musl binary — alongside a broad community
closeout and a retroactive credit reconciliation pass.

- **[zlh124](https://github.com/zlh124)** — preserve user comments and formatting
  when rewriting `config.toml`/`settings.toml`/`tui.toml` (with a malformed-file
  fallback) and Linux build deps in the cargo install guides (#3270)
- **[idling11](https://github.com/idling11)** — Kimi `type:object` schema root for
  all parameter shapes (#3281), `approval_mode` restore on Plan→Agent with a
  wait-for-user guard (#3279), and workroom metadata draft types
- **[LeoLin990405](https://github.com/LeoLin990405)** — Poppler `pdftotext -v`
  detection (#1667), session persistence before stall/cancel recovery (#2739),
  and debounced thinking-stream re-renders (#1620)
- **[nightt5879](https://github.com/nightt5879)** — Codewhale-only skill discovery
  gate (`[skills].scan_codewhale_only`) ignoring cross-tool directories (#3296) and
  app-server no-auth loopback docs
- **[reidliu41](https://github.com/reidliu41)** — slash commands exposed as hotbar
  actions (#3269)
- **[wavezhang](https://github.com/wavezhang)** — static Linux x64 (musl) release
  binaries
- **[wuisabel-gif](https://github.com/wuisabel-gif)** — per-tool snapshot gate
  respecting `[snapshots].enabled` (#3292) and composer history written under
  `.codewhale`
- **[gaord](https://github.com/gaord)** — `workspace_follow_symlinks` setting for
  symlink-aware tool operations with hardened path handling
- **[greyfreedom](https://github.com/greyfreedom)** — ask-permission rules honored
  at runtime (#3295)
- **[aboimpinto](https://github.com/aboimpinto)** — EPIC-001 command-boundary
  replay and user-registry review feedback
- **[h3c-hexin](https://github.com/h3c-hexin)** — volatile workspace path moved
  out of the static system prefix (prefix-cache hygiene)
- **[hongchen1993](https://github.com/hongchen1993)** — heuristic-only auto routing
  when the flash router is unavailable
- **[lucaszhu-hue](https://github.com/lucaszhu-hue)** — Atlas Cloud provider setup
  docs
- Retroactive reconciliation (shipped earlier, credited now):
  **[manaskarra](https://github.com/manaskarra)** / **[xfy6238](https://github.com/xfy6238)** (#1157),
  **[djairjr](https://github.com/djairjr)** (#1309 alongside reidliu41),
  **[Geallier](https://github.com/Geallier)** (#1470),
  **[quentin-lian](https://github.com/quentin-lian)** / **[k0tran](https://github.com/k0tran)** (#1531 / #1992),
  **[F1LT3R](https://github.com/F1LT3R)** (#1656),
  **[cmyyy](https://github.com/cmyyy)** (#1842),
  **[Final527](https://github.com/Final527)** (#3058)

</details>

<details>
<summary><strong>v0.8.61 — runtime control plane &amp; community closeout</strong></summary>


The v0.8.61 release was a community closeout: the runtime control plane, provider
patches, and TUI fixes landed alongside first-time and returning contributor work.

- **[idling11](https://github.com/idling11)** — DeepInfra provider support with
  OpenAI-compatible routing and model registry entries (#3235, closes #3231)
- **[greyfreedom](https://github.com/greyfreedom)** — atomic ask-only
  permission-rule persistence so an execution-policy rule survives the write
  that triggered the prompt (#3233)
- **[VincentCorleone](https://github.com/VincentCorleone)** — WeChat bridge
  (`integrations/weixin-bridge`) leveraging Feishu + Tencent OpenClaw (#3206)
- **[nightt5879](https://github.com/nightt5879)** — whale-accent rename (#3197)
  and `$skillname` aliases for `/skill` activation (#3241)
- **[mvanhorn](https://github.com/mvanhorn)** — non-DeepSeek model pricing
  coverage (#3201)
- **[cyq1017](https://github.com/cyq1017)** — Telegram polling transport
  (#3195) and VS Code read-only API documentation (#3013)
- **[RobertEmprechtinger](https://github.com/RobertEmprechtinger)** — mobile
  event history (#3220)
- **[gaord](https://github.com/gaord)** — runtime-API session save (#3199)
- **[hongchen1993](https://github.com/hongchen1993)** — `DEEPSEEK_BASE_URL` /
  `MODEL` honored in `exec` (#3221)

</details>

<details>
<summary><strong>Forward track — recent v0.9 work (newest)</strong></summary>


- **[xyuai](https://github.com/xyuai)** — canonical Codewhale settings path,
  provider persistence, provider picker, logout-scope, and MiMo auth cleanup
  work (#2730, #2714, #2715, #2717, #2718)
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — HarmonyOS /
  OpenHarmony porting work and MatePad Edge validation trail (#2634)
- **[ousamabenyounes](https://github.com/ousamabenyounes)** — AZERTY/AltGr
  composer shortcut fix for Windows keyboard layouts (#2863, #2867)
- **[reidliu41](https://github.com/reidliu41)** — hotbar action-registry
  foundation and Ollama model-completion cleanup for the forward track (#2866,
  #2742)
- **[ljm3790865](https://github.com/ljm3790865)** — multi-tab
  core/persistence foundation and broader tab collaboration direction (#2864,
  #2753)
- **[sximelon](https://github.com/sximelon)** — saved-session resume footer
  hint work plus provider-trait metadata registry direction reviewed and
  harvested for the forward track (#2758, #2760, #2479)
- **[aboimpinto](https://github.com/aboimpinto)** — sidebar command polish and
  pausable custom-command lifecycle direction harvested into the forward track,
  plus the directly merged command-support boundary cleanup and broader command
  layer design direction (#2788, #2732, #2871, #2851, #2791)
- **[AdityaVG13](https://github.com/AdityaVG13)** — Workflow orchestration
  and cost-tracking drafts that shaped the maintained workflow IR and
  TraceStore foundation (#2482, #2486)
- **[lbcheng888](https://github.com/lbcheng888)**,
  **[AiurArtanis](https://github.com/AiurArtanis)**, and
  **[nasus9527](https://github.com/nasus9527)** — VS Code extension scaffold
  direction, Agent View request, and IDE plugin request that shaped the
  official Phase 0 extension (#1022, #1584, #2580)
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — `web_run` cache-state
  lock-splitting, turn-metadata prefix-cache stability, and project-context
  cache work (#2502, #2517, #2636)
- **[idling11](https://github.com/idling11)** — PlanArtifact continuity,
  dense tool-call transcript collapse, sidebar detail popovers, and
  HarnessPosture provider/model policy direction (#2733, #2738, #2734,
  #2741, #2692, #2694, #2693)
- **[h3c-hexin](https://github.com/h3c-hexin)** — sub-agent model inheritance,
  configured `skills_dir` discovery, prompt-environment stability, and static
  prompt composer direction (#2736, #2737, #2786)
- **[gaord](https://github.com/gaord)** — runtime thread workspace updates and
  completed-thread saved-session API work (#2640, #2639)
- **[cyq1017](https://github.com/cyq1017)** — trusted workspace MCP config,
  provider auth rollback, custom search endpoint, custom completion sound,
  restore-listing, and pending-input delivery-mode label work (#2751, #2755,
  #2510, #2512, #2513, #2532, #2054)
- **[yusufgurdogan](https://github.com/yusufgurdogan)** — Sofya search
  provider implementation harvested as a non-default search backend (#2790)
- **[LeoAlex0](https://github.com/LeoAlex0)** — runtime prompt metadata cache
  direction harvested into the maintained prompt/cache path (#2687);
  `allow_shell` prefix-cache decoupling and `visibility="internal"`
  explanation for mode-flip stability (#2949, #2951)
- **[hongchen1993](https://github.com/hongchen1993)** — Volcengine provider
  in TUI dispatcher and dispatcher API-key preference (#2923, #2928)
- **[NASLXTO](https://github.com/NASLXTO)** and
  **[wuxixing](https://github.com/wuxixing)** — large-workspace startup
  reports that shaped the bounded project-context fallback (#697, #1827)
- **[shuxiangxuebiancheng](https://github.com/shuxiangxuebiancheng)**,
  **[hongqitai](https://github.com/hongqitai)**, and
  **[cyq1017](https://github.com/cyq1017)** — third-party
  OpenAI-compatible path-suffix report and follow-up review trail (#1874,
  #2508, #2506)


</details>

<details>
<summary><strong>Recurring &amp; historical contributors</strong></summary>


- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — 28 PRs spanning features, fixes, and VS Code extension scaffolding (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — Markdown rendering for tables, bold/italic, and horizontal rules (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Windows + China install documentation (#578)
- **[20bytes](https://github.com/20bytes)** — User memory docs and help polish (#569)
- **[staryxchen](https://github.com/staryxchen)** — glibc compatibility preflight (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — glibc compatibility improvements and terminal restoration on SIGINT/SIGTERM (#565, #1586)
- **[shentoumengxin](https://github.com/shentoumengxin)** — Shell `cwd` boundary validation (#524)
- **[toi500](https://github.com/toi500)** — Windows paste fix report
- **[xsstomy](https://github.com/xsstomy)** — Terminal startup repaint report
- **[melody0709](https://github.com/melody0709)** — Slash-prefix Enter activation report
- **[lloydzhou](https://github.com/lloydzhou)** and **[jeoor](https://github.com/jeoor)** — Compaction cost reports; lloydzhou also contributed deterministic environment context (#813, #922) and KV prefix-cache stabilisation (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README clarity pass (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows install documentation (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — Pricing/discount info update (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — model ID case-sensitivity compatibility report (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — stale `working...` state bug report, Windows clipboard fallback, MCP Streamable HTTP session fixes, and Homebrew tap automation (#738, #850, #1643, #1631)
- **[reidliu41](https://github.com/reidliu41)** — resume hint, workspace trust persistence, Ollama provider support, thinking-block stream finalization, CI cache hardening, streaming wrap, and DeepSeek model completions (#863, #870, #921, #1078, #1603, #1628, #1601)
- **[xieshutao](https://github.com/xieshutao)** — plain Markdown skill fallback (#869)
- **[GK012](https://github.com/GK012)** — npm wrapper `--version` fallback (#885)
- **[y0sif](https://github.com/y0sif)** — parent turn-loop wakeup after direct child sub-agent completion (#901)
- **[mac119](https://github.com/mac119)** and **[leo119](https://github.com/leo119)** — `codewhale update` command documentation (#838, #917)
- **[dumbjack](https://github.com/dumbjack)** / **浩淼的mac** — command-safety null-byte hardening (#706, #918)
- **macworkers** — fork confirmation with the new session id (#600, #919)
- **zero** and **[zerx-lab](https://github.com/zerx-lab)** — notification condition config and richer OSC 9 notification body (#820, #920)
- **[chnjames](https://github.com/chnjames)** — cached @mention completions, config recovery polish, and Windows UTF-8 shell output (#849, #927, #982, #1018)
- **[angziii](https://github.com/angziii)** — config safety, async cleanup, Docker hardening, and command-safety fixes (#822, #824, #827, #831, #833, #835, #837)
- **[elowen53](https://github.com/elowen53)** — UTF-8 decoding and deterministic test coverage (#825, #840)
- **[wdw8276](https://github.com/wdw8276)** — `/rename` command for custom session titles (#836)
- **[banqii](https://github.com/banqii)** — `.cursor/skills` discovery path support (#817)
- **[junskyeed](https://github.com/junskyeed)** — dynamic `max_tokens` calculation for API requests (#826)
- **Hafeez Pizofreude** — SSRF protection in `fetch_url` and Star History chart
- **Unic (YuniqueUnic)** — Schema-driven config UI (TUI + web)
- **Jason** — SSRF security hardening
- **[axobase001](https://github.com/axobase001)** — snapshot orphan cleanup, npm install guards, session telemetry fixes, model-scope cache clear, symlinked skill support, npm mirror-escape-hatch guidance, proxy preservation for child tasks, mobile runtime control, Docker toolbox docs, large-output receipts, and activity detail context (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056, #1608, #1968, #2296, #2297, #2298)
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` command foundation and SSE gzip/brotli decompression (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Plan-mode read-only sandbox safety fix (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — precise paste-Enter auto-submit reproducer (#1073)
- **[Duducoco](https://github.com/Duducoco)** and **[AlphaGogoo](https://github.com/AlphaGogoo)** — skills slash-menu and `/skills` coverage fix (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — window-resize artifact fix for macOS Terminal.app and ConHost (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — OpenRouter and custom-endpoint model-ID preservation (#1066)
- **[Jefsky](https://github.com/Jefsky)** — DeepSeek endpoint correction report (#1079, #1084)
- **[wlon](https://github.com/wlon)** — NVIDIA NIM provider API-key preference diagnosis (#1081)
- **[Horace Liu](https://github.com/liuhq)** — Nix package support and install documentation (#1173)
- **[jieshu666](https://github.com/jieshu666)** — terminal repaint flicker reduction (#1563)
- **[gordonlu](https://github.com/gordonlu)** — Windows Enter / CSI-u input fix, status picker localization (7 MessageIds), approval dialog localization across 7 locales, and mode picker + composer Vim indicator localization across 7 locales (#1612, #2896, #2891, #2239)
- **[mdrkrg](https://github.com/mdrkrg)** — first-run onboarding crash fix when the API key is missing (#1598)
- **[Aitensa](https://github.com/Aitensa)** — CJK wrapping propagation for diff and pager output (#1622)
- **[qiyan233](https://github.com/qiyan233)** — legacy DeepSeek CN provider alias compatibility (#1645)
- **[zlh124](https://github.com/zlh124)** — WSL2/headless startup report, clipboard-init fix, Codewhale tab-title polish, localized context-menu labels, and approval-dialog fixes (#1772, #1773, #2319, #2320, #2325)
- **[aboimpinto](https://github.com/aboimpinto)** — Windows alt-screen
  logging, Home/End composer, runtime log follow-ups, sidebar command polish,
  and pausable command lifecycle work (#1774, #1776, #1748, #1749, #1782,
  #1783, #2788, #2732)
- **[LeoLin990405](https://github.com/LeoLin990405)** — provider model passthrough, reasoning replay, thinking-only turn, and Windows quoting fixes (#1740, #1743, #1742, #1744)
- **[nightt5879](https://github.com/nightt5879)** — Ctrl+C prompt restore, provider registry drift docs, tool-search defaults, footer git branch display, and startup prompt interactivity (#1764, #2274, #2344, #2347, #2373)
- **[donglovejava](https://github.com/donglovejava)** — paste @file consolidation, CJK panic fix, user feedback, RLM routing, edit_file retry, hidden-worktree discovery skip, IME composer routing, and eager shell companion tools (#2154-#2168, #2302, #2329, #2330, #2331)
- **[encyc](https://github.com/encyc)** — session token breakdown in footer and `/status` (#2152)
- **[saieswar237](https://github.com/saieswar237)** — review pipeline docs (#2178)
- **[sximelon](https://github.com/sximelon)** — paste Enter suppression, key handler extraction (#2174, #2042)
- **[nanookclaw](https://github.com/nanookclaw)** — search provider in doctor output (#2135)
- **[Sskift](https://github.com/Sskift)** — CLI default env override prevention and statusline footer clearing (#2119, #2248)
- **[xin1104](https://github.com/xin1104)** — Homebrew codewhale binary install (#2105)
- **[mrluanma](https://github.com/mrluanma)** — Metaso search provider (#2059)
- **[Lellansin](https://github.com/Lellansin)** — skip config merge at home dir (#2055)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — update release channels and legacy MCP SSE fixes (#2145, #2301)
- **[cy2311](https://github.com/cy2311)** — Windows `.bat` launcher for Codewhale (#1861)
- **[LING71671](https://github.com/LING71671)** — effective cost currency context, custom provider docs, and core tool taxonomy prompt block (#1902, #2287, #2292)
- **[dzyuan](https://github.com/dzyuan)** — Volcengine provider support with DeepSeek V4 Pro/Flash models (#1993)
- **[mvanhorn](https://github.com/mvanhorn)** — live request-shape test factories and global `~/.agents/AGENTS.md` fallback (#2107, #2236)
- **[malsony](https://github.com/malsony)** — Matrix-inspired theme and theme picker improvements (#2129)
- **[gaord](https://github.com/gaord)** — external GUI runtime event bridge, session detail serialization, and skills API discovery alignment (#2133, #2265, #2285)
- **[yuanchenglu](https://github.com/yuanchenglu)** — Feishu per-chat model switching (#2149)
- **[HUQIANTAO](https://github.com/HUQIANTAO)** — Xiaomi balance/status work, stalled-turn recovery, approval intent summaries, mobile smoke/QR support, Claude theme, and broad docs/test/CI coverage (#2257, #2267, #2283, #2384, #2385, #2389, #2403, #2440-#2458, #2460)
- **[h3c-hexin](https://github.com/h3c-hexin)** — web-search URL decoding, prompt/instructions override hooks, sub-agent guidance, SSRF fake-IP trust configuration, and prompt-cache-friendly environment placement (#2245, #2311, #2313, #2314, #2354, #2355, #2356)
- **[tdccccc](https://github.com/tdccccc)** — approval prompt key-detail and shell-preview work harvested into the maintained approval path (#1991, #2269)
- **[AresNing](https://github.com/AresNing)** — first-run guide, message-submit hook transform design, and turn-end observer hook work harvested into the maintained hooks path (#2278, #2318, #2434, #2578)
- **[Implementist](https://github.com/Implementist)** — Volcengine Ark search provider and reliability hardening (#2426, #2429, #2439)
- **[lihuan215](https://github.com/lihuan215)** — Unix socket hook sink design harvested into the opt-in hook event path (#2333, #2430)
- **[AdityaVG13](https://github.com/AdityaVG13)** — Xiaomi MiMo provider support (#2246)
- **[New2Niu](https://github.com/New2Niu)** — macOS display notifications (#2260)
- **[AiurArtanis](https://github.com/AiurArtanis)** — Solarized Light theme and
  canonical-background regression report (#2270, #4457)
- **[Lee-take](https://github.com/Lee-take)** — task migration and session environment isolation fixes (#2272)
- **[LeoAlex0](https://github.com/LeoAlex0)** — session persistence fixes for message counts and tool-output cache preservation (#2388, #2395)
- **[jimmyzhuu](https://github.com/jimmyzhuu)** — Baidu AI Search backend for `web_search` (#2371)
- **[rockyzhang](https://github.com/rockyzhang)** — RISC-V prebuilt binary support (#2383)
- **[mo-vic](https://github.com/mo-vic)** — `/purge` slash command for agent-driven context pruning (#2387)
- **[hufanexplore](https://github.com/hufanexplore)** — Java and Vue language-server defaults (#2367)
- **[hoclaptrinh33](https://github.com/hoclaptrinh33)** — Vietnamese localization support (#2358)
- **[AccMoment](https://github.com/AccMoment)** — proxy option for the update command (#2281)
- **[idling11](https://github.com/idling11)** — durable debt ledger and `/hunt` rename/trophy-card work (#2161, #2306)
- **[cyq1017](https://github.com/cyq1017)** — runtime event envelope, render-diff debug logging, and deterministic composer history flushing (#2252, #2332, #2375)
- **[hongqitai](https://github.com/hongqitai)** — state schema parent-entry support and clippy/fmt cleanup (#2308, #2432)
- **[BryonGo](https://github.com/BryonGo)** — effective-model compaction budgeting fix (#2437)
- **[xyuai](https://github.com/xyuai)** — provider persistence to config, /logout scope clarification, provider picker key replacement shortcut, MiMo auth state cleanup (#2714, #2715, #2717, #2718)
- **[RefuseOdd](https://github.com/RefuseOdd)** — configurable `path_suffix` for OpenAI-compatible endpoints (#2558)


</details>

<details>
<summary><strong>v0.8.48 — reports, repros &amp; verification (earliest listed)</strong></summary>

Reports, repros, and verification that shaped v0.8.48 also deserve visible
credit: **[@buko](https://github.com/buko)**, **[@yyyCode](https://github.com/yyyCode)**,
**[@gaslebinh-glitch](https://github.com/gaslebinh-glitch)**, **[@Dr3259](https://github.com/Dr3259)**,
**[@lpeng1711694086-lang](https://github.com/lpeng1711694086-lang)**, **[@VerrPower](https://github.com/VerrPower)**,
**[@yan-zay](https://github.com/yan-zay)**, **[@jretz](https://github.com/jretz)**,
**[@Neo-millunnium](https://github.com/Neo-millunnium)**, **[@caeserchen](https://github.com/caeserchen)**,
**[@T-Phuong-Nguyen](https://github.com/T-Phuong-Nguyen)**, **[@zhyuzhyu](https://github.com/zhyuzhyu)**,
**[@0gl20shk0sbt36](https://github.com/0gl20shk0sbt36)**, **[@hatakes](https://github.com/hatakes)**,
**[@goodvecn-dev](https://github.com/goodvecn-dev)**, **[@bevis-wong](https://github.com/bevis-wong)**,
**[@PurplePulse](https://github.com/PurplePulse)**, and **[@nbiish](https://github.com/nbiish)**.

---

</details>

<details>
<summary><strong>Reconciled credits — earlier contributors restored to the record</strong></summary>

A credit-reconciliation pass mapped every shipped commit author to a GitHub
handle and found these contributors whose merged work was not yet listed here or
in the changelog. Restoring them with thanks — every one shipped real code.

- **[MoriTang](https://github.com/MoriTang)** — account balance status-bar item, with a request timeout, reused HTTP client, stale-balance-on-failure handling, and DeepSeek-gated display
- **[mars-base](https://github.com/mars-base)** — session title shown in the composer border and `gh` discovery across common install paths (#836)
- **[Giggitycountless](https://github.com/Giggitycountless)** — auto-add `.deepseek/` to `.gitignore`, gitignore-check robustness, and `/clear` resetting the Todos panel
- **[Inference1](https://github.com/Inference1)** — vLLM provider support, README pricing/structure clarity, and the Model Studio reasoning route guard: a fail-closed host check so Alibaba's `enable_thinking` never reaches a custom gateway, plus catching that Codewhale was sending a disable switch to the always-thinking `qwen3.8-max` (#737, #776, #5233)
- **[membphis](https://github.com/membphis)** — bordered Markdown table rendering and Shift+Enter newline in the composer (#801)
- **[JasonOA888](https://github.com/JasonOA888)** — `web_run` network-policy enforcement and refusing to snapshot `$HOME` (#798, #800)
- **[tuohai666](https://github.com/tuohai666)** — recursive skills-directory reading plus hook-dispatch and approval-branch test coverage (#811)
- **[xuezhaoyu](https://github.com/xuezhaoyu)** — DEC 2026 synchronized-update flicker fix for GPU terminals, guaranteeing `END_SYNC_UPDATE` even when a draw fails
- **[manaskarra](https://github.com/manaskarra)** — global `~/.deepseek/AGENTS.md` fallback loading (#1157)
- **[gerryqi](https://github.com/gerryqi)** — wrap fire-and-forget `spawn_blocking` calls with a panic dump (#810)
- **[ccomma](https://github.com/ccomma)** — skip snapshots for dangerous workspaces (#804)
- **[AGSaturn](https://github.com/AGSaturn)** — preserve requested model-ID casing in registry resolution (#733)
- **[wucm667](https://github.com/wucm667)** — accept provider-prefixed DeepSeek model IDs (#794)
- **[quentin-lian](https://github.com/quentin-lian)** — portable-pty 0.9 upgrade for LoongArch64 support (#1992)
- **[Beltran12138](https://github.com/Beltran12138)** — treat `deepseek-chat` / `deepseek-reasoner` aliases as reasoning models
- **[chuntseevolving](https://github.com/chuntseevolving)** — send `TurnStarted` before snapshot to prevent WSL2 timeout
- **[lawrencewzen](https://github.com/lawrencewzen)** — preserve UTF-8 while stripping ANSI
- **[hhhaiai](https://github.com/hhhaiai)** — keep workspace skills visible when the prompt budget truncates
- **[khalid-hungerstation](https://github.com/khalid-hungerstation)** — bundle the delegate skill alongside skill-creator
- **[Anyexyz](https://github.com/Anyexyz)** — GitHub Actions workflow to sync with the CNB repo
- **[nightfallsad](https://github.com/nightfallsad)** — clearer `/continue` hint copy
- **[zxyasfas](https://github.com/zxyasfas)** — align Rust MSRV references with the workspace (#739)

_A follow-up audit of harvested commits (work reimplemented onto a maintainer
branch rather than merged) surfaced contributors whose machine-readable credit
was dropped. Restoring them here — every one shipped real code:_

- **[CrepuscularIRIS](https://github.com/CrepuscularIRIS)** — OpenHarmony→Linux npm binary mapping, O(1) job-panel refresh, file-mention UTF-8 boundary safety, Kitty keyboard protocol on Windows, and auto low-motion under Termius/SSH (#1479, #1483, #1494, #1495, #1499, #1475)
- **[MMMarcinho](https://github.com/MMMarcinho)** — `image_analyze` vision tool (#1467)
- **[MeAiRobot](https://github.com/MeAiRobot)** — toast-stack overlay z-order fix (#1485)
- **[NorethSea](https://github.com/NorethSea)** — `update` refreshing the sibling TUI binary (#1492)
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — bundled
  v4-best-practices skill (#1448) and input-budget-aware compaction trigger
  (#4293)
- **[YaYII](https://github.com/YaYII)** — opt-in `/translate` command (#1462)
- **[sandofree](https://github.com/sandofree)** — Tavily and Bocha `web_search` backends (#1294)
- **[tiger-dog](https://github.com/tiger-dog)** — approval one-line banner and Markdown underscore handling (#1455)
- **[Jianfengwu2024](https://github.com/Jianfengwu2024)** — preserving MSVC toolchain vars in the child environment (#1487)
- **[wplll](https://github.com/wplll)** — prompt-cache warmup keys, tool-catalog handling, a dedup test, and pack ordering (#2390, #2391, #2392, #2393)

Additional harvested PRs from the same audit, credited to contributors already
listed above: **[axobase001](https://github.com/axobase001)** (#2400, #2405, #2406, #2407, #2408, #2415), **[cyq1017](https://github.com/cyq1017)** (#2516, #2534, #2540), **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** (#1451, #1456), **[reidliu41](https://github.com/reidliu41)** (#1444, #1493), **[lucaszhu-hue](https://github.com/lucaszhu-hue)** (#1436, #2343), **[h3c-hexin](https://github.com/h3c-hexin)** (#1480), **[Duducoco](https://github.com/Duducoco)** (#1345), **[zhuangbiaowei](https://github.com/zhuangbiaowei)** (#1416), **[wdw8276](https://github.com/wdw8276)** (#1498), and **[buko](https://github.com/buko)** (#2377).

_A further machine-credit pass restored these contributors, missing from both the
list above and the contribution graph (AUTHOR_MAP entries added; logins/IDs
verified against the GitHub user API) — every one shipped real code:_

- **[1Git2Clone](https://github.com/1Git2Clone)** — `Ctrl+P`/`Ctrl+N` slash-menu navigation
- **[rockeverm3m](https://github.com/rockeverm3m)** — community ACP adapter reference in the docs
- **[hxy91819](https://github.com/hxy91819)** — stable MCP tool ordering for prefix-cache stability (#1319)
- **[heloanc](https://github.com/heloanc)** — Home/End keys moving the cursor in the input box (#1246)

</details>

---

Missed someone? Open an issue or PR — credit is kept current, and names are happily added. See [CONTRIBUTING.md](../CONTRIBUTING.md) to get started.
