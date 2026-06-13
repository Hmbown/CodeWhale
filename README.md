# CodeWhale

> Local-first agent harness for DeepSeek V4 and open models: operating identity,
> nested authority, and a local evidence loop.

[简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[DeepWiki project index](https://deepwiki.com/Hmbown/CodeWhale)

![codewhale screenshot](assets/screenshot.png)

## The Idea

Most coding agents start by adding power: more tools, more context, more
autonomy. CodeWhale starts by assigning responsibility.

Before an agent edits a repo, it should have an address: this terminal, this
user, this branch, this session. That is the ego layer. Not swagger; continuity.
Not a persona mask; the place where responsibility attaches.

Then it needs law. A real workspace is a conflict stack: current user intent,
repo instructions, shell output, stale memory, previous handoffs, safety policy,
and half-finished work all compete inside the same turn. CodeWhale gives those
sources an order through the CodeWhale Constitution:

- **Ego is addressable.** The agent is an instance in this terminal and this
  workspace, not a model card or leaderboard score.
- **Evidence outranks narration.** Tool output beats a guess. A failed command
  is reported as a failed command. Verification is part of the task.
- **User intent stays sovereign.** The current request outranks stale repo
  guidance, memory, previous handoffs, and personality overlays.
- **Local law is explicit.** Repositories can add `.codewhale/constitution.json`
  for durable project authority, protected invariants, branch policy, and
  verification rules.
- **Runtime policy is enforced.** Modes, approval gates, sandboxing, rollback,
  and tool schemas are code, not advice the model has to remember.

The product is the ordering layer around the model: who is acting, whose law
applies, what evidence exists, and how the next human or agent can continue.

## What Ships

CodeWhale turns that thesis into plain runtime surfaces:

- approval-gated file, shell, git, web, MCP, RLM, and sub-agent tools;
- side-git snapshots and `/restore` rollback outside your repo's `.git`;
- live diagnostics after edits from language servers where available;
- concurrent sub-agents for parallel investigation and implementation;
- durable sessions, forks, relay handoffs, and runtime APIs for editor/GUI work;
- explicit provider/model routing with DeepSeek V4 first-class and other
  OpenAI-compatible routes kept separate.

DeepSeek is first-class, not exclusive. CodeWhale also carries provider paths for
OpenRouter, NVIDIA NIM, Xiaomi MiMo, Arcee, SiliconFlow, Fireworks, Novita,
OpenAI-compatible gateways, self-hosted SGLang/vLLM, Ollama, and Hugging Face
surfaces as they land.

## Install

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
codewhale --model auto
```

On first launch, CodeWhale prompts for a DeepSeek API key and stores it in
`~/.codewhale/config.toml`; legacy `~/.deepseek/` config is still read for
compatibility.

Other install paths are supported:

```bash
# Platform archives are attached to GitHub Releases.
# https://github.com/Hmbown/CodeWhale/releases

# CNB mirror path for users who cannot reliably reach GitHub:
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-tui --locked --force

# Legacy Homebrew compatibility while the formula is renamed
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

The `codewhale` npm wrapper is also available via `npm install -g codewhale`.

For Docker, direct downloads, China mirrors, Windows/Scoop, Nix, checksums, and
troubleshooting, use [docs/INSTALL.md](docs/INSTALL.md) or the website install
page.

## Upgrading from deepseek-tui

If you installed the legacy `deepseek-tui` package, run the commands for your
install method below. Your existing config, sessions, skills, and MCP settings
are preserved — see [docs/REBRAND.md](docs/REBRAND.md) for the full migration
guide.

**npm**

```bash
npm uninstall -g deepseek-tui
npm install -g codewhale
```

**Cargo**

```bash
cargo uninstall deepseek-tui-cli deepseek-tui 2>/dev/null || true
cargo install codewhale-cli codewhale-tui --locked
```

**Homebrew** — keep using `brew upgrade deepseek-tui` for now; the formula
rename is in progress.

**GitHub Releases** — download the `codewhale-*` archive for your platform
from the [Releases page](https://github.com/Hmbown/CodeWhale/releases) and
replace the old binaries on your `PATH`.

After upgrading, run `codewhale doctor` to confirm the migration succeeded.

## First Run

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

Useful in-session commands:

- `/provider` and `/model` choose the route and model.
- `/config` edits runtime settings.
- `/statusline` shows the current route, cost, and session state.
- `/skills` loads reusable workflows from `~/.codewhale/skills/`.
- `/restore` rolls back a prior turn from side-git snapshots.
- `! cargo test -p codewhale-tui` runs a shell command through the normal
  approval and sandbox path.

## Where Details Live

The README carries the idea and the first path. The details live in docs and on
[codewhale.net](https://codewhale.net/):

- [User guide](docs/GUIDE.md) — first hour with CodeWhale.
- [Install guide](docs/INSTALL.md) — every package path and troubleshooting.
- [Upgrading from deepseek-tui](docs/REBRAND.md) — migration guide for users on the legacy package.
- [Configuration](docs/CONFIGURATION.md) — config files, repo constitution, and
  provider settings.
- [Provider registry](docs/PROVIDERS.md) — model routes, credentials, base URLs,
  and capability boundaries.
- [Sub-agents](docs/SUBAGENTS.md) — roles, lifecycle, output contract, and
  recovery behavior.
- [Runtime API](docs/RUNTIME_API.md) — HTTP/SSE, ACP, mobile, and GUI/editor
  integration contracts.
- [Model Lab](docs/MODEL_LAB.md) — open-model discovery and evaluation roadmap.
- [Architecture](docs/ARCHITECTURE.md) — crate layout, runtime flow, tool system,
  extension points, and security model.
- [v0.9.0 release acceptance](docs/V0_9_0_RELEASE_ACCEPTANCE.md) — current
  integration checks before release tagging.

## v0.9.0 Track

v0.9.0 is the current integration lane, not a published release until the tag,
GitHub Release, npm package, Cargo crates, and release artifacts are actually
cut and verified.

The release line is gathering work around:

- stronger relay and handoff surfaces;
- calmer transcripts for dense tool runs;
- command and provider architecture cleanup;
- runtime APIs for VS Code and GUI clients;
- typed HarnessProfile posture and model routing;
- WhaleFlow branch/leaf workflow orchestration;
- contributor credit hygiene for harvested and direct community PRs.

Release-specific details belong in [CHANGELOG.md](CHANGELOG.md) and the v0.9.0
acceptance docs, not in this README.

## Thanks

- **[DeepSeek](https://github.com/deepseek-ai)** — thank you for the models and support that power every turn. 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — thank you for your support and for welcoming us into the Whale Brother family. 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — thank you for prioritizing codewhale support and for collaborating on a better terminal-agent experience.
- **[Open Design](https://github.com/nexu-io/open-design)** — thank you for support and collaboration around design-forward agent workflows.

This project ships with help from a growing community of contributors. The
maintainer rule is simple: reports and PRs are real project work, even when the
final patch has to be narrowed, delayed, or harvested into a maintainer branch.

For the v0.9 track, harvested PRs should keep visible credit in the commit or
PR body, changelog or release notes, and relevant issue/PR comments. Contributor
credit should use mappable GitHub identities from `.github/AUTHOR_MAP` or
numeric noreply addresses, not placeholder local emails. The contribution gate
is kept in dry-run mode unless a maintainer deliberately enables enforcement;
when it comments, the tone should be warm and practical rather than treating
the reporter as the problem. Recurring contributors should be recognized so the
automation gets out of their way and the public record shows their repeated
help.

Current v0.9 track credits:

- **[xyuai](https://github.com/xyuai)** — canonical CodeWhale settings path,
  provider persistence, provider picker, logout-scope, and MiMo auth cleanup
  work (#2730, #2714, #2715, #2717, #2718)
- **[shenjackyuanjie](https://github.com/shenjackyuanjie)** — HarmonyOS /
  OpenHarmony porting work and MatePad Edge validation trail (#2634)
- **[ousamabenyounes](https://github.com/ousamabenyounes)** — AZERTY/AltGr
  composer shortcut fix for Windows keyboard layouts (#2863, #2867)
- **[reidliu41](https://github.com/reidliu41)** — hotbar action-registry
  foundation and Ollama model-completion cleanup for the v0.9 track (#2866,
  #2742)
- **[ljm3790865](https://github.com/ljm3790865)** — multi-tab
  core/persistence foundation and broader tab collaboration direction (#2864,
  #2753)
- **[sximelon](https://github.com/sximelon)** — saved-session resume footer
  hint work plus provider-trait metadata registry direction reviewed and
  harvested for the v0.9 track (#2758, #2760, #2479)
- **[aboimpinto](https://github.com/aboimpinto)** — sidebar command polish and
  pausable custom-command lifecycle direction harvested into the v0.9 track,
  plus the directly merged command-support boundary cleanup and broader command
  layer design direction (#2788, #2732, #2871, #2851, #2791)
- **[AdityaVG13](https://github.com/AdityaVG13)** — WhaleFlow orchestration and
  cost-tracking drafts that shaped the maintained v0.9 WhaleFlow IR and
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
  direction harvested into the v0.9 prompt/cache path (#2687);
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

Current and recurring contributors include:

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
- **[gordonlu](https://github.com/gordonlu)** — Windows Enter / CSI-u input fix, status picker localization (7 MessageIds), and approval dialog localization across 7 locales (#1612, #2896, #2891)
- **[mdrkrg](https://github.com/mdrkrg)** — first-run onboarding crash fix when the API key is missing (#1598)
- **[Aitensa](https://github.com/Aitensa)** — CJK wrapping propagation for diff and pager output (#1622)
- **[qiyan233](https://github.com/qiyan233)** — legacy DeepSeek CN provider alias compatibility (#1645)
- **[zlh124](https://github.com/zlh124)** — WSL2/headless startup report, clipboard-init fix, CodeWhale tab-title polish, localized context-menu labels, and approval-dialog fixes (#1772, #1773, #2319, #2320, #2325)
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
- **[cy2311](https://github.com/cy2311)** — Windows `.bat` launcher for CodeWhale (#1861)
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
- **[AiurArtanis](https://github.com/AiurArtanis)** — Solarized Light theme (#2270)
- **[Lee-take](https://github.com/Lee-take)** — task migration and session environment isolation fixes (#2272)
- **[LeoAlex0](https://github.com/LeoAlex0)** — session persistence fixes for message counts and tool-output cache preservation (#2388, #2395)
- **[jimmyzhuu](https://github.com/jimmyzhuu)** — Baidu AI Search backend for `web_search` (#2371)
- **[rockyzhang](https://github.com/rockyzhang)** — RISC-V prebuilt binary support (#2383)
- **[mo-vic](https://github.com/mo-vic)** — `/purge` slash command for agent-driven context pruning (#2387)
- **[hufanexplore](https://github.com/hufanexplore)** — Java and Vue language-server defaults (#2367)
- **[hoclaptrinh33](https://github.com/hoclaptrinh33)** — Vietnamese localization support (#2358)
- **[AccMoment](https://github.com/AccMoment)** — proxy option for the update command (#2281)
- **[idling11](https://github.com/idling11)** — durable SlopLedger and `/hunt` rename/trophy-card work (#2161, #2306)
- **[cyq1017](https://github.com/cyq1017)** — runtime event envelope, render-diff debug logging, and deterministic composer history flushing (#2252, #2332, #2375)
- **[hongqitai](https://github.com/hongqitai)** — state schema parent-entry support and clippy/fmt cleanup (#2308, #2432)
- **[BryonGo](https://github.com/BryonGo)** — effective-model compaction budgeting fix (#2437)
- **[xyuai](https://github.com/xyuai)** — provider persistence to config, /logout scope clarification, provider picker key replacement shortcut, MiMo auth state cleanup (#2714, #2715, #2717, #2718)
- **[RefuseOdd](https://github.com/RefuseOdd)** — configurable `path_suffix` for OpenAI-compatible endpoints (#2558)

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

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Pull requests welcome — check the [open issues](https://github.com/Hmbown/CodeWhale/issues) for good first contributions.

CodeWhale gets a lot of good reports and PRs. The maintainer posture is to keep
that door open while protecting release quality:

- Issues should stay human-readable and actionable. Intake automation is
  advisory unless a maintainer deliberately enables enforcement.
- PRs are reviewed from code, tests, linked issues, and runtime behavior, not
  from title alone.
- If a PR is too broad to merge directly, maintainers may harvest the safe part
  into a narrower branch, then credit the author and explain what landed.
- Co-author trailers should use mappable GitHub noreply identities from
  `.github/AUTHOR_MAP`; reporters and repro authors should be thanked in
  changelogs, release notes, and closure comments.
- Recurring contributors can be added to `.github/APPROVED_CONTRIBUTORS` so
  dry-run gates stay out of their way.

Support: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

> [!Note]
> *Not affiliated with DeepSeek Inc.*

## License

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
