# Contributors

CodeWhale is built in the open with a growing community of contributors. Every
issue report and pull request is real project work — welcome at any experience
level. This is the full per-PR contributor record in **chronological order**
(newest first), collapsed into time bands so it stays scannable. Expand any band
to see everyone.

For the live list, see the
[GitHub contributors page](https://github.com/Hmbown/CodeWhale/graphs/contributors),
[`AUTHOR_MAP`](https://github.com/Hmbown/CodeWhale/blob/main/.github/AUTHOR_MAP),
and [CHANGELOG.md](../CHANGELOG.md).

## Organizational thanks

- **[DeepSeek](https://github.com/deepseek-ai)** — the models and support that got this project started. 感谢 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — for the support and for welcoming us into the Whale Brother family. 感谢 DataWhale 的支持。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — for prioritizing codewhale support and collaborating on a better terminal-agent experience.
- **[Open Design](https://github.com/nexu-io/open-design)** — for support and collaboration around design-forward agent workflows.

The maintainer rule: reports and PRs are real project work, even when the final
patch has to be narrowed, delayed, or harvested into a maintainer branch.
Harvested PRs keep visible credit in the commit/PR body, changelog or release
notes, and relevant issue/PR comments.

---

## Contributors by time

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
sibling), added TOML comment preservation and the CodeWhale-only skill discovery
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
- **[nightt5879](https://github.com/nightt5879)** — CodeWhale-only skill discovery
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


- **[xyuai](https://github.com/xyuai)** — canonical CodeWhale settings path,
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
- **[AdityaVG13](https://github.com/AdityaVG13)** — WhaleFlow orchestration and
  cost-tracking drafts that shaped the maintained WhaleFlow IR and
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
- **[Inference1](https://github.com/Inference1)** — vLLM provider support and README pricing/structure clarity (#737, #776)
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
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — bundled v4-best-practices skill (#1448)
- **[YaYII](https://github.com/YaYII)** — opt-in `/translate` command (#1462)
- **[sandofree](https://github.com/sandofree)** — Tavily and Bocha `web_search` backends (#1294)
- **[tiger-dog](https://github.com/tiger-dog)** — approval one-line banner and Markdown underscore handling (#1455)
- **[Jianfengwu2024](https://github.com/Jianfengwu2024)** — preserving MSVC toolchain vars in the child environment (#1487)
- **[wplll](https://github.com/wplll)** — prompt-cache warmup keys, tool-catalog handling, a dedup test, and pack ordering (#2390, #2391, #2392, #2393)

Additional harvested PRs from the same audit, credited to contributors already
listed above: **[axobase001](https://github.com/axobase001)** (#2400, #2405, #2406, #2407, #2408, #2415), **[cyq1017](https://github.com/cyq1017)** (#2516, #2534, #2540), **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** (#1451, #1456), **[reidliu41](https://github.com/reidliu41)** (#1444, #1493), **[lucaszhu-hue](https://github.com/lucaszhu-hue)** (#1436, #2343), **[h3c-hexin](https://github.com/h3c-hexin)** (#1480), **[Duducoco](https://github.com/Duducoco)** (#1345), **[zhuangbiaowei](https://github.com/zhuangbiaowei)** (#1416), **[wdw8276](https://github.com/wdw8276)** (#1498), and **[buko](https://github.com/buko)** (#2377).

</details>

---

Missed someone? Open an issue or PR — credit is kept current, and names are happily added. See [CONTRIBUTING.md](../CONTRIBUTING.md) to get started.
