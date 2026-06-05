1|# codewhale
2|
3|Install and run CodeWhale, the agentic terminal for open-source and open-weight coding
4|models, from GitHub release artifacts.
5|
6|> Previously published as `deepseek-tui`. See `docs/REBRAND.md` in the upstream
7|> repository for the migration notes; the legacy `deepseek-tui` npm package is
8|> deprecated and receives no further releases.
9|
10|## Install
11|
12|```bash
13|npm install -g codewhale
14|# or
15|pnpm add -g codewhale
16|```
17|
18|For project-local usage:
19|
20|```bash
21|npm install codewhale
22|npx codewhale --help
23|```
24|
25|`postinstall` tries to download platform binaries into `bin/downloads/` and
26|exposes `codewhale` and `codewhale-tui` commands. If GitHub release assets are
27|temporarily unreachable, install continues and the wrapper retries the download
28|on first run.
29|
30|## First run
31|
32|```bash
33|codewhale login --api-key "YOUR_DEEPSEEK_API_KEY"
34|codewhale doctor
35|codewhale
36|```
37|
38|The `codewhale` facade and `codewhale-tui` binary share
39|`~/.codewhale/config.toml` for DeepSeek auth and default model settings. Legacy
40|`~/.deepseek/config.toml` installs are still read as a compatibility fallback.
41|Common TUI commands are available directly through the facade, including
42|`codewhale doctor`, `codewhale models`, `codewhale sessions`, and
43|`codewhale resume --last`.
44|
45|The app talks to DeepSeek's documented OpenAI-compatible Chat Completions API.
46|Set `DEEPSEEK_BASE_URL` only if you need the China endpoint or DeepSeek beta
47|features such as strict tool mode, chat prefix completion, or FIM completion.
48|
49|NVIDIA NIM-hosted DeepSeek V4 Pro is also supported:
50|
51|```bash
52|codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
53|codewhale --provider nvidia-nim
54|```
55|
56|For a single process, set `DEEPSEEK_PROVIDER=nvidia-nim` and `NVIDIA_API_KEY`
57|or `NVIDIA_NIM_API_KEY` (with `DEEPSEEK_API_KEY` as a compatibility fallback).
58|The NIM default model is `deepseek-ai/deepseek-v4-pro` and the default base URL
59|is `https://integrate.api.nvidia.com/v1`. With `--provider nvidia-nim`,
60|`--model deepseek-v4-flash` maps to `deepseek-ai/deepseek-v4-flash`.
61|
62|## Supported platforms
63|
64|Prebuilt binaries for the GitHub release are downloaded automatically:
65|
66|- Linux x64
67|- Linux arm64 (v0.8.8+)
68|- macOS x64 / arm64
69|- Windows x64
70|
71|Other platform/architecture combinations (musl, riscv64, FreeBSD, …) aren't
72|shipped as prebuilts. Unsupported platforms, checksum failures, and glibc
73|compatibility problems still fail with a clear error pointing you at
74|`cargo install codewhale-cli codewhale-tui --locked` and the full
75|[docs/INSTALL.md](https://github.com/Hmbown/CodeWhale/blob/main/docs/INSTALL.md)
76|build-from-source guide.
77|
78|## Configuration
79|
80|- Default binary version comes from `codewhaleBinaryVersion` in `package.json`
81| (with `deepseekBinaryVersion` as a backward-compat fallback).
82|- Set `DEEPSEEK_TUI_VERSION` or `DEEPSEEK_VERSION` to override the release version.
83|- Set `DEEPSEEK_TUI_GITHUB_REPO` or `DEEPSEEK_GITHUB_REPO` to override the source repo (defaults to `Hmbown/CodeWhale`).
84|- Set `DEEPSEEK_TUI_RELEASE_BASE_URL` to use an internal or mirrored
85| release-asset directory when GitHub Releases is unavailable. The directory
86|  must contain `codewhale-artifacts-sha256.txt` and the platform binaries.
87|- Set `DEEPSEEK_TUI_FORCE_DOWNLOAD=1` to force download even when the cached binary is already present.
88|- Set `DEEPSEEK_TUI_DISABLE_INSTALL=1` to skip install-time download.
89|- Set `DEEPSEEK_TUI_OPTIONAL_INSTALL=1` to make install-time retryable download
90|  failures warn and exit `0` instead of failing `npm install`.
91|
92|## Release integrity
93|
94|- `npm publish` runs a release-asset check to ensure all required binary assets
95|  exist for the target GitHub release before publishing.
96|- Install-time downloads are verified against the release checksum manifest before
97|  the wrapper marks them executable.
98|