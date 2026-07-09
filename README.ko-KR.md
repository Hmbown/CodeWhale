# CodeWhale

> 어떤 모델에도 쓸 수 있는 터미널 코딩 에이전트 — 오픈 모델 우선.

CodeWhale은 터미널 코딩 에이전트입니다 — TUI와 CLI로 제공됩니다. 모델과
프로젝트를 지정하면 코드를 읽고, 편집하고, 명령을 실행하고, 결과를
확인하고, 여러 단계의 작업을 계획하며, 실패하면 스스로 수정하면서 일을
시작합니다.

오픈 소스(MIT, Rust)이고, 사용자의 컴퓨터에서 실행되며, 사람들이 실제로
사용하는 모델과 함께 동작합니다. DeepSeek과 오픈 웨이트 모델은
일급으로 지원되고, LAN 안의 로컬 vLLM/SGLang/Ollama 장비는 키가 전혀
필요 없습니다. 하지만 Claude, GPT, Kimi, GLM도 같은 런타임과 같은
도구를 쓰는 동등한 구성원입니다. 프로바이더와 모델을 고르면
CodeWhale이 실제 라우트를 해석해 실행합니다.

이 프로젝트는 DeepSeek 워크플로를 중심으로 만든 코딩 하네스인
`deepseek-tui`에서 시작했습니다. 상당수가 중국에 있던 개발자
커뮤니티가 이를 채택하고, 리포트를 제출하고, 수정 사항을 기여하면서 이
하네스가 하나의 모델보다 더 큰 범위를 다룬다는 점이 분명해졌습니다.
이후 멀티 프로바이더 지원이 이어졌고, 프로젝트도 그에 맞춰 CodeWhale이
되었습니다. 원하는 모델, 엔드포인트, 기능이 보이지 않는다면 이슈를
열어 주세요 — 프로젝트는 그렇게 성장합니다.

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md) · [codewhale.net](https://codewhale.net/) · [Install guide](docs/INSTALL.md) · [Provider registry](docs/PROVIDERS.md) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![터미널에서 실행 중인 CodeWhale](assets/screenshot.png)

## 설치

```bash
npm install -g codewhale
codewhale --version   # 0.8.67
```

npm 래퍼(Node 18+)는 GitHub Releases에서 SHA-256으로 검증된 바이너리를
다운로드하고 `codewhale`, `codew`, `codewhale-tui`를 설치합니다.
소스에서 빌드하는 쪽을 선호하나요? cargo(Rust 1.88+)를 사용하세요:

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

> **Linux 사용자:** 먼저 시스템 빌드 의존성을 설치하세요:
> `sudo apt-get install -y build-essential pkg-config libdbus-1-dev`.
> [INSTALL.md](docs/INSTALL.md#4-install-via-cargo-any-tier-1-rust-target)를 참고하세요.

그 밖의 설치 경로:

```bash
# Docker
docker pull ghcr.io/hmbown/codewhale:latest

# Nix
nix run github:Hmbown/CodeWhale

# Windows
scoop install codewhale        # or the NSIS installer from GitHub Releases

# CNB mirror for users who cannot reliably reach GitHub
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.67 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.67 codewhale-tui --locked --force

# Legacy Homebrew compatibility while the formula is renamed
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Linux x64/arm64, macOS x64/arm64, Windows x64용 사전 빌드 아카이브는
[GitHub Releases](https://github.com/Hmbown/CodeWhale/releases)에
첨부되어 있습니다. Linux riscv64 사전 빌드는 upstream QuickJS bindings
지원이 준비될 때까지 잠시 중단되었습니다. 체크섬, 중국 미러, Windows
관련 세부 사항, 문제 해결은 [docs/INSTALL.md](docs/INSTALL.md)에 있습니다.

**레거시 `deepseek-tui` 패키지에서 업그레이드하나요?** 설정, 세션,
스킬, MCP 설정은 보존됩니다. [docs/REBRAND.md](docs/REBRAND.md)를
확인한 다음 `codewhale doctor`를 실행해 확인하세요.

## 첫 실행

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

모든 프로바이더는 같은 한 줄 형태를 사용합니다: `--provider openrouter`,
`--provider moonshot`, `--provider openmodel`, 또는 `vllm`, `sglang`,
`ollama`를 키 없이도 자신의 localhost 런타임으로 지정할 수 있습니다.
대신 Claude 키가 있다면 `codewhale auth set --provider anthropic`을
실행하거나 `ANTHROPIC_API_KEY`만 내보내세요. 이후에는 네이티브 Messages
어댑터가 처리합니다.

키는 `~/.codewhale/config.toml`에 저장됩니다. 레거시 `~/.deepseek/`
설정도 호환성을 위해 계속 읽습니다.

세션 안에서 유용한 명령:

- `/provider`는 준비 상태 대시보드를 엽니다 — 프로바이더별 인증 상태,
  해석된 기본 라우트, 비용/사용량 미터를 보여 줍니다. `/model`은 모델과
  추론 강도를 선택합니다. 두 명령 모두 인수(`/provider nvidia-nim`,
  `/model auto`)를 받아 세션 중간에 전환할 수도 있습니다.
- `/restore`는 side-git 스냅샷에서 이전 턴을 롤백합니다.
- `/fleet`은 Fleet 설정 뷰를 엽니다 — 역할, 프로필, 로드아웃, 정책을
  다룹니다.
- `/skills`는 `~/.codewhale/skills/`에서 재사용 가능한 워크플로를
  불러옵니다.
- `/config`는 런타임 설정을 편집하고, `/statusline`은 라우트, 비용,
  세션 상태를 표시할 푸터 칩을 고릅니다.
- `! cargo test -p codewhale-tui`는 일반적인 승인 및 샌드박스 경로를
  통해 임의의 셸 명령을 실행합니다.

스크립트와 CI용 헤드리스 실행:

```bash
codewhale exec --allowed-tools read_file,exec_shell --max-turns 10 "fix the failing test"
```

## 프로바이더와 라우팅

프로바이더와 모델을 고르면 CodeWhale은 **실제 라우트**를 해석합니다.
이는 단순히 base URL만 바꾸는 것이 아니라 구체적인 엔드포인트, 와이어
프로토콜, 모델 ID, 컨텍스트 한도, 가격까지 확정하는 일입니다.
`RouteResolver`만이 해석된 라우트를 만들 수 있으므로 TUI 선택기, CLI,
헤드리스 실행이 모두 같은 선택 로직을 사용합니다. 그 뒤의 카탈로그는
Models.dev 형식으로 커밋되어 있는 네트워크 없는 스냅샷이며, 선택적으로
프로바이더의 실시간 `/models` 엔드포인트에서 새로 고칠 수 있습니다.

라우트가 해석되어 있기 때문에 나머지 하네스도 이를 정직하게 다룰 수
있습니다:

- **라우트 인식 컨텍스트 예산.** 압축 임계값과 사용 가능한 창은
  하드코딩된 추정치가 아니라 해석된 라우트의 실제 컨텍스트 한도에서
  나옵니다.
- **정직한 비용 표시.** 라우트는 정확히 하나의 비용 상태를 보고합니다:
  토큰당 가격, 구독/쿼터 미터, 계정 크레딧, *로컬 / 해당 없음*, 또는
  *알 수 없음 / 오래됨*. CodeWhale은 가지고 있지 않은 가격을 만들어내지
  않습니다. 일치하지 않는 모델은 $0가 아니라 알 수 없음으로 표시됩니다.
- **명시적인 와이어 프로토콜.** 라우트가 Chat Completions,
  OpenAI Responses API, 네이티브 Anthropic Messages 중 무엇을 말하는지는
  프롬프트에서 추론하지 않고 해석된 라우트에 실립니다. 추론 강도는
  각 프로바이더의 고유한 방언으로 변환됩니다.

`/provider`와 `/model`로 세션 중간에 라우트를 전환하세요. 전체 레지스트리
— 자격 증명, base URL, 기능 경계 — 는
[docs/PROVIDERS.md](docs/PROVIDERS.md)에 있습니다.

### 지원되는 프로바이더

모든 프로바이더는 같은 런타임과 같은 도구를 통해 라우팅됩니다. 원하는
대상이 여기에 없다면 이슈로 열기 좋은 내용입니다.

- **오픈 모델, 호스팅형:** `deepseek`(기본값), `openrouter`,
  `huggingface`(Inference Providers), `moonshot`(Kimi), `zai`(GLM),
  `minimax`, `volcengine`(Ark), `nvidia-nim`, `together`, `fireworks`,
  `novita`, `siliconflow` / `siliconflow-CN`, `arcee`, `xiaomi-mimo`,
  `openmodel`, `deepinfra`, `stepfun`, `atlascloud`, `qianfan`, `wanjie-ark`, 모든
  게이트웨이를 위한 일반 `openai` 호환 라우트도 포함됩니다.
- **오픈 모델, 셀프 호스팅형:** 자신의 localhost 엔드포인트에 연결하는
  `vllm`, `sglang`, `ollama` — 키가 필요 없습니다.
- **폐쇄형 프로바이더, 네이티브:** adaptive thinking, prompt-cache
  breakpoint, signed-thinking replay를 갖춘 전용 `/v1/messages` 어댑터를
  통해 연결되는 `anthropic`; DeepSeek의 옵트인 Messages-API 라우트인
  `deepseek-anthropic`; 그리고 API 키 대신 기존 ChatGPT/Codex CLI 로그인을
  재사용하는 `openai-codex`(실험적).

## Fleet

Fleet은 멀티 워커 실행을 위한 CodeWhale의 내구성 있는 제어 플레인입니다.
fleet 워커는 헤드리스 `codewhale exec` 실행이지만, Fleet은 이를
내구성 있게 시작하고 추적합니다. 작업은 추가 전용 원장
(`.codewhale/fleet.jsonl`)에 기록되므로 매니저 종료, 노트북 절전,
런타임 재시작이 있어도 실행은 살아남습니다.

```bash
codewhale fleet run tasks.json --max-workers 4
codewhale fleet status
codewhale fleet resume <run-id>
```

`fleet resume`은 원장을 재생하고, 하트비트가 멈춘 진행 중 작업을
조정하며(예산 안에서 재시도하고, 아니면 실패 및 에스컬레이션), 멱등적입니다.
따라서 매니저를 중단시킨 어떤 일이 있은 뒤에도 실행해도 안전합니다. 각
워커는 타입이 있는 리시트(`pass` / `fail` / `partial` / `skip` /
`timeout`)를 기록하므로 `fleet status`가 실제로 일어난 일을 보고할 수
있습니다.

워커는 설정의 `[fleet]` 아래에서 구성하거나 앱 안의 Fleet 설정 뷰에서
작성하는 **역할**, **프로필**, **로드아웃**, **슬롯**으로 형태가
정해집니다. 로드아웃은 모델 의도를 `strong`, `balanced`, `fast` 같은
클래스로 표현하고, 라우트 resolver가 이를 구체적인 프로바이더/모델로
바꿉니다. 이는 세션 안의 서브 에이전트를 뒷받침하는 것과 같은 헤드리스
런타임이며, Fleet은 그 위의 내구성 있는 레이어입니다.
[docs/FLEET.md](docs/FLEET.md)를 참고하세요.

## 안전

CodeWhale은 파일을 편집하고 명령을 실행하므로, 안전 태세는 부가 기능이
아니라 제품의 일부입니다.

- **세 가지 모드.** Plan(읽기 전용 조사), Agent(실행하되 동작마다 질문),
  YOLO(자동 승인). `Tab` 또는 `/mode`로 전환합니다.
- **승인 게이트가 있는 도구.** `.codewhale/hooks.toml` 훅 시스템은 모든
  도구 호출 전에 허용, 거부, 질문을 할 수 있고, exec 정책은 명령이
  실행되는지, 승인이 필요한지, 완전히 금지되는지를 결정합니다.
- **OS 샌드박싱.** macOS의 Seatbelt, Linux의 Landlock과 seccomp syscall
  filter, 그리고 사용 가능한 곳의 bubblewrap(bwrap).
- **롤백.** side-git 스냅샷은 저장소의 실제 `.git` 바깥에 있으므로
  `/restore`가 실제 히스토리를 건드리지 않고도 한 턴을 되돌릴 수 있습니다.

## 기능

- **지속 goal 루프.** `/goal`로 목표를 설정하면 에이전트는 작업이
  완료되거나, 막히거나, 사용자가 멈출 때까지 턴을 넘어 계속 작업합니다.
  코드를 읽고, 편집하고, 실행하고, 결과를 확인합니다. 턴 제한은 없습니다.
  `/task`는 백그라운드 작업을 추적하며, Work 사이드바는 실시간 계획과
  체크리스트 상태를 보여 줍니다.
- **내구성 있는 세션.** 재시작과 시스템 절전 이후에도 유지됩니다. 도구
  호출 마흔 번이 걸리는 작업은 마흔한 번째 호출까지 살아남습니다.
- **헤드리스 모드.** 스크립트와 CI를 위해 `--allowed-tools`,
  `--disallowed-tools`(거부가 우선), `--max-turns`,
  `--append-system-prompt`를 사용하는 `codewhale exec`.
- **양방향 MCP.** 외부 MCP 서버의 도구를 사용하거나, `codewhale mcp`를
  통해 CodeWhale 자체를 MCP 서버로 노출할 수 있습니다.
- **스킬.** `/skills`로 불러오는 `~/.codewhale/skills/`의 재사용 가능한
  워크플로입니다.
- **어디에나 임베드.** HTTP/SSE 및 ACP 런타임 API, VS Code 확장,
  Telegram/Feishu 브리지(Weixin은 실험적).

## 지시사항의 순위가 정해지는 방식

프로젝트가 발전하면 지시사항은 쌓이고 결국 서로 충돌합니다. 원래 스펙,
나중의 리팩터링에서 생긴 모순, 오래된 메모리, 이전 에이전트의 핸드오프,
현재 요청, 그리고 핸드오프가 주장한 내용과 맞지 않는 최신 테스트 출력이
함께 존재합니다. 평평한 시스템 프롬프트는 모델이 추측으로 이를 해결하게
만듭니다. CodeWhale은 **중첩 헌장**을 사용하므로 감이 아니라
정의된 순위가 있습니다.

시스템 프롬프트는 가장 정적인 것부터 계층화되며, 그 순서는 코드에서
강제됩니다(흐트러질 수 없다고 검증하는 테스트가 있습니다):

1. **전역 헌장** — 모든 바이너리에 컴파일되는 기본 법입니다. 그
   우선순위 조항은 모든 충돌에 대한 권한 순서를 고정합니다.
2. **사용자 전역 헌장** — `/constitution`과 `/setup`에서 관리하며,
   `$CODEWHALE_HOME/constitution.json`에 구조화된 데이터로 저장되고
   모델이 읽는 prose block으로 렌더링됩니다. 일반 설정 경로이며 원시
   프롬프트 편집기가 아닙니다.
3. **프로젝트의 법** — 저장소에 `.codewhale/constitution.json`을 두어
   `protected_invariants`, `branch_policy`, `verification_policy`,
   `escalate_when`을 선언합니다. 이는 메모리와 핸드오프보다 위에 있는
   독립 권한 블록으로 로드됩니다.
4. **현재 요청** — 이번 턴에서 작동하는 지시사항입니다.
5. **실시간 증거** — 도구가 실제로 반환한 내용입니다. 정답 기준이며,
   모델은 그 너머로 지시받을 수는 있지만 존재하지 않는 사실을 보고해서는
   안 됩니다.

두 지시사항이 충돌하면 각각은 위에 있는 것에 양보합니다. 이 법은 모델이
아니라 하네스 안에 있으므로, 모델을 바꿔도 구조는 그대로 유지됩니다.
헌장 텍스트는 선호를 표현할 수 있지만 승인, 샌드박스, 네트워크, 신뢰,
MCP 권한 같은 런타임 보안 설정을 조용히 바꾸지는 않습니다.

## 세부 정보 위치

README는 짧은 버전입니다. 나머지는 docs와
[codewhale.net](https://codewhale.net/)에 있습니다:

- [사용자 가이드](docs/GUIDE.md) · [설치 가이드](docs/INSTALL.md) ·
  [설정](docs/CONFIGURATION.md) · [프로바이더 레지스트리](docs/PROVIDERS.md)
- [모드](docs/MODES.md) — Agent, Plan, YOLO.
- [Fleet](docs/FLEET.md) · [Sub-agents](docs/SUBAGENTS.md) — 역할, 생명주기,
  출력 계약, 복구 동작.
- [아키텍처](docs/ARCHITECTURE.md) — 크레이트 구성, 런타임 흐름, 도구 시스템,
  확장 지점, 보안 모델.
- [Workflow 작성](docs/WORKFLOW_AUTHORING.md) · [MCP](docs/MCP.md) ·
  [Runtime API](docs/RUNTIME_API.md) · [Model Lab](docs/MODEL_LAB.md)
- [키 바인딩](docs/KEYBINDINGS.md) · [샌드박스와 승인](docs/SANDBOX.md)
  · [접근성](docs/ACCESSIBILITY.md) · [Docker](docs/DOCKER.md)
  · [Memory](docs/MEMORY.md)
- [전체 문서 색인](docs) — 그 밖의 모든 것.

## 프로젝트

CodeWhale은 한 사람의 DeepSeek 사이드 프로젝트로 시작했습니다. 전 세계
여러 나라의 개발자들이 지금의 모습을 만들었습니다. 모든 릴리스의 기여자
목록이 그 증거입니다. 이 프로젝트는 공개적으로 만들어지고, 이슈도
공개적으로 분류되며, 릴리스는 `main`에서 만들어집니다.

가르치던 초기에 배운 것이 있습니다. **모든 피드백은 선물입니다.** 이슈,
PR, 버그 리포트, 기능 아이디어, "첫 PR", 호기심 어린 질문은 모두 실제
프로젝트 작업으로 인정됩니다. 최종 패치가 좁혀지거나, 지연되거나,
메인테이너 커밋에 접혀 들어가야 하는 경우에도 메인테이너는 모든
리포트를 기여로 대합니다. 반복해서 기여하는 사람들은 공개 기록에 계속
크레딧이 남습니다. 작동하지 않는 것을 만났거나, 목록에 없는 모델을
원한다면, 그것이 이 프로젝트에 가장 유용하게 알려 줄 수 있는 내용입니다.

- [열려 있는 이슈](https://github.com/Hmbown/CodeWhale/issues) — 처음
  기여하기 좋은 작업이 여기에 있습니다.
- [CONTRIBUTING.md](CONTRIBUTING.md) — 개발 루프를 설정하고 PR을 여세요.
- [행동 강령](CODE_OF_CONDUCT.md) — 서로에게 훌륭하게 대하세요.
- [기여자](docs/CONTRIBUTORS.md) — CodeWhale을 빚어 온 사람들입니다.

후원: [Buy me a coffee](https://www.buymeacoffee.com/hmbown).

## 감사

CodeWhale은 이를 사용하고, 깨뜨리고, 고치는 사람들 덕분에 존재합니다.

- **[DeepSeek](https://github.com/deepseek-ai)** — 이 프로젝트가 시작될 수
  있게 해 준 모델과 지원에 감사드립니다.
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — 지원과 Whale
  Brother family에 맞이해 준 것에 감사드립니다.
- **[OpenWarp](https://github.com/zerx-lab/warp)** 및
  **[Open Design](https://github.com/nexu-io/open-design)** — 더 나은
  터미널 에이전트 경험을 위해 함께 협력해 주었습니다.
- **모든 기여자** — PR별 전체 기록은
  [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md)에 있습니다. 감사합니다.

## License

[MIT](LICENSE)

> *CodeWhale은 독립 커뮤니티 프로젝트이며 어떤 모델 프로바이더와도 제휴 관계가
> 없습니다.*

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
