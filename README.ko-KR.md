<!-- source: README.md sha256:ddc9cd7b5aea -->
# Codewhale

터미널에서 쓰는 코딩 에이전트입니다. 어떤 모델과도 동작하며, 오픈 모델을
우선합니다.

프로바이더, 모델, 작업을 지정하면 코드를 읽고, 파일을 편집하고, 명령을
실행하고, 결과를 확인하며, 작업이 끝나거나 사용자의 판단이 필요해질
때까지 계속 진행합니다. 대화형 작업에는 TUI를, 스크립트와 CI에는
`codewhale exec`를 사용합니다. Rust로 작성되었고, MIT 라이선스이며,
전부 사용자의 컴퓨터에서 실행됩니다.

이 프로젝트는 `deepseek-tui`로 시작했습니다. 그 주위에 형성된
커뮤니티에 더 많은 프로바이더가 필요했고, 지금은 DeepSeek, Claude, GPT,
Kimi, GLM과 그 밖의 30개 이상이 같은 런타임과 같은 도구를 통해
실행됩니다.

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

![터미널에서 실행 중인 Codewhale](assets/screenshot.png)

## 설치

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, 사전 빌드 아카이브, Android/Termux, 그리고
GitHub에 접근할 수 없는 사용자를 위한 CNB 미러는
[docs/INSTALL.md](docs/INSTALL.md)에서 다룹니다. `deepseek-tui`에서
넘어오나요? 설정과 세션은 그대로 이어집니다 —
[docs/REBRAND.md](docs/REBRAND.md)를 참고하세요.

## 사용

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
```

TUI 안에서: `/model`은 프로바이더와 모델을 함께 전환하고, `/fleet`은
워커 팀을 실행하며, `/restore`는 한 턴을 되돌립니다. `Tab`은
Plan / Act / Operate 모드를 순환하고, `Shift+Tab`은
Ask / Auto-Review / Full Access 승인 태세를 순환하며, `!`는 일반 승인
경로를 거쳐 셸 명령을 실행합니다.

## 하는 일

- 선택한 프로바이더 + 모델을 구체적인 라우트로 해석합니다: 엔드포인트,
  와이어 프로토콜, 컨텍스트 한도, 가격. 컨텍스트 예산과 비용 표시는
  실제 라우트에서 나오며, 알 수 없는 가격은 $0가 아니라 알 수 없음으로
  표시됩니다. ([docs/PROVIDERS.md](docs/PROVIDERS.md))
- 호스팅형 오픈 모델 프로바이더(`deepseek`, `openrouter`, `moonshot`,
  `zai`, `minimax`, `nvidia-nim`, …)와 통신하고, 키 없이 자체
  `vllm` / `sglang` / `ollama`에 연결하며, Anthropic에는 thinking과
  프롬프트 캐싱을 갖춘 Messages API로 네이티브 연결합니다.
- 여러 워커를 내구성 있게 실행합니다: Fleet은 작업을 추가 전용 원장에
  기록하므로 실행은 재시작에도 살아남고, `fleet resume`은 멈춘
  지점부터 이어서 진행합니다. Workflow는 더 큰 작업을 재개 가능하고
  검증 가능한 레인으로 계획합니다. ([docs/FLEET.md](docs/FLEET.md))
- 위험을 감이 아니라 코드로 통제합니다: 세 가지 모드(Plan은 읽기 전용),
  별도의 승인 태세, OS 샌드박싱(Seatbelt, Landlock + seccomp, bwrap),
  도구 호출마다 허용/거부/질문할 수 있는 훅, 그리고 `/restore`가 실제
  히스토리를 결코 건드리지 않게 하는 side-git 스냅샷.
- 저장소가 자체 법을 선언할 수 있습니다:
  `.codewhale/constitution.json`의 불변 조건은 Full Access조차 건너뛸
  수 없는 쓰기 보류로 컴파일됩니다.
  ([docs/CONFIGURATION.md](docs/CONFIGURATION.md))
- 양방향으로 MCP를 지원하고, 재사용 가능한 스킬을 불러오며, HTTP/SSE 및
  ACP 런타임 API를 노출하고, 커뮤니티
  [VS Code GUI](https://github.com/HengQuWorld/CodeWhale-VSCode)를
  뒷받침합니다.
- TUI는 작업을 점검할 수 있는 리시트로 보여 주고, 움직이는 라이브 행은
  하나로 유지하며, 실제 컨텍스트 인스펙터, 12가지 테마, 모션 축소
  모드와 ASCII 안전 모드를 갖추고 있습니다. UI 언어는 영어, 중국어
  간체, 일본어, 베트남어, 스페인어, 포르투갈어, 한국어를 지원하며,
  중국어 번체는 부분 지원입니다.

그 밖의 모든 것 — 설정, 키 바인딩, 샌드박스 세부 사항, 아키텍처 — 은
[docs](docs)와 [codewhale.net](https://codewhale.net/)에 있습니다.

## 기여

모든 피드백은 선물입니다. 이슈, PR, 재현 절차, 로그, 기능 요청, 첫
기여는 모두 이곳에서 실제 프로젝트 작업입니다. PR을 그대로 병합할 수
없을 때는 메인테이너가 작동하는 부분을 거두어 반영하고, 작성자의
크레딧은 커밋, 변경 로그,
[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md)에 그대로 남습니다.
사용하는 모델이나 프로바이더가 빠져 있거나 무언가가 여러분의 컴퓨터에서
깨진다면, 그것을 알려 주는 일이 할 수 있는 가장 유용한 일입니다.

- [열려 있는 이슈](https://github.com/Hmbown/CodeWhale/issues) — 처음
  기여하기 좋은 작업이 여기에 있습니다
- [CONTRIBUTING.md](CONTRIBUTING.md) — 개발 환경 설정과 PR 흐름
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — 이 프로젝트를 빚어 온
  모든 사람
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

프로젝트를 시작하게 해 준 모델과 지원을 제공한
[DeepSeek](https://github.com/deepseek-ai), Whale Brother family로
맞이해 준 [DataWhale](https://github.com/datawhalechina) 🐋, 그리고
터미널 에이전트 경험에 함께 협력해 준
[OpenWarp](https://github.com/zerx-lab/warp)와
[Open Design](https://github.com/nexu-io/open-design)에 감사드립니다.

## 라이선스

[MIT](LICENSE). 독립 커뮤니티 프로젝트이며, 어떤 모델 프로바이더와도
제휴 관계가 없습니다.

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
