<!-- source: README.md sha256:4fb18fffb0fe -->
# Codewhale

터미널에서 쓰는 오픈소스 코딩 에이전트 — 모델은 당신이 가져옵니다.

Codewhale은 DeepSeek을 위한 네이티브 경험으로 시작했습니다. 이후 커뮤니티가 이끄는 프로젝트로 성장했습니다. 점점 커지는 국제 커뮤니티에 맞고, 가능한 한 많은 모델과 프로바이더를 지원하는 하나의 코딩 하네스입니다 — 오픈 모델을 가장 먼저, 호스팅이든 로컬이든, 어느 하나를 특별 대우하지 않습니다.

프로바이더, 모델, 작업을 지정하면 코드를 읽고, 파일을 편집하고, 명령을 실행하고, 스스로 작업을 확인하며, 작업이 끝나거나 사용자의 판단이 필요해지면 멈춥니다. 작업 도중에도 `/model`로 모델을 바꿀 수 있습니다. 대화형 작업에는 TUI를, 스크립트와 CI에는 `codewhale exec`를 사용합니다. Rust로 작성했고, MIT 라이선스이며, 당신의 컴퓨터에서 실행됩니다.

다른 하네스와 다른 점은 이것입니다. **역할마다 어떤 모델을 쓸지 당신이 고르고, 서로 같을 필요가 없습니다.** Fleet은 역할별로 프로바이더, 모델, 추론 등급을 각각 고정합니다. 그래서 빠르고 저렴한 모델이 값비싼 추론 모델을 지휘할 수도 있고, GLM builder와 Kimi reviewer가 같은 작업을 함께 처리할 수도 있습니다. 자신의 역할과 자신의 constitution을 쓰면, 그 하네스는 우리 것이 아니라 당신 것이 됩니다.

우리는 항상 기여자와 개선할 방법을 찾고 있습니다. 사용하는 모델이나 프로바이더가 빠져 있거나 무언가가 깨진다면, 그것을 알려 주는 일이 할 수 있는 가장 유용한 일 중 하나입니다 — [기여](#기여)를 참고하세요.

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![터미널에서 실행 중인 Codewhale](assets/screenshot.png)

## 설치

```bash
npm install -g codewhale
```

Cargo, Docker, Nix, Scoop, 사전 빌드 아카이브, Android/Termux, 그리고 GitHub에 접근할 수 없는 사용자를 위한 CNB 미러는 [docs/INSTALL.md](docs/INSTALL.md)에서 다룹니다. `deepseek-tui`에서 넘어오나요? 설정과 세션은 그대로 이어집니다 — [docs/REBRAND.md](docs/REBRAND.md)를 참고하세요.

## 사용

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```


TUI 안에서: `/model`은 프로바이더와 모델을 함께 전환하고, `/fleet`은 팀을 구성하고 실행하며(한 번에 한 역할씩, 각자 자기 모델을 가집니다), `/undo`는 직전 턴을 되돌리고, `/restore <N>`은 워크스페이스를 이전 스냅샷으로 되돌립니다(인자 없는 `/restore`는 스냅샷 목록만 보여줍니다). 입력창이 비어 있을 때 `Tab`은 Plan / Work / Operate 모드를 순환하고, 입력창에 내용이 있으면 `Tab`은 슬래시 명령과 `@` 멘션을 자동 완성합니다. `Shift+Tab`은 언제든지 Ask / Auto-Review / Full Access 권한 태세를 순환합니다. `!`는 일반 승인 경로를 거쳐 셸 명령을 실행합니다.

## 기능

- **어떤 모델이든, 어떤 프로바이더든, 그리고 어떤 조합이든.** DeepSeek, Claude, GPT, Kimi, GLM 등 30개 이상의 프로바이더와 키 없이 쓰는 자체 vLLM, SGLang, Ollama가 모두 하나의 런타임과 하나의 도구 세트를 통해 동작합니다. 저장된 역할은 `provider`, `model`, 추론 등급을 명시적으로 기록하므로 하나의 실행 안에서 Fleet이 여러 벤더에 걸칠 수 있고, 역할의 라우트는 그때 활성화된 프로바이더에 좌우되지 않습니다. 컨텍스트 예산과 가격은 실제 라우트에서 가져오며, 알 수 없는 가격은 $0이 아니라 알 수 없음으로 표시됩니다.
- **당신이 직접 쓰는 하네스.** 역할은 읽고 수정할 수 있는 파일입니다. 역할마다 모델, 도구 태세, 상시 지시를 담아 팀과 공유하려면 프로젝트에, 저장소를 옮겨 다니며 쓰려면 개인 설정 옆에 둡니다. constitution은 모든 세션에서 에이전트가 어떻게 행동하기를 바라는지 기록해, 하네스가 우리 방식이 아니라 당신의 방식에 맞도록 합니다.
- **허용하기 전까지는 읽기 전용.** Plan 모드는 파일을 바꾸지 않고, 위험한 명령은 승인을 거칩니다. OS 샌드박스가 실제로 명령을 래핑할 때 Codewhale은 이를 그대로 표시합니다. macOS에서는 사용 가능한 Seatbelt, Linux에서는 옵트인 bubblewrap입니다. 저장소의 `constitution.json`은 Full Access조차 건너뛸 수 없는 쓰기 홀드로 컴파일됩니다.
- **이어서 할 수 있는 작업.** Fleet은 모든 단계를 추가 전용 원장에 기록하므로, `fleet resume`으로 멈춘 지점부터 이어갈 수 있습니다.

## 더 알아보기

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — 호스팅·게이트웨이·로컬까지 모든
  프로바이더 라우트
- [docs/FLEET.md](docs/FLEET.md) — Fleet, 원장, 재개
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — Workflow
  안의 동결된, 프로바이더 중립 실험 검색
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`, 훅,
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — 모드, 훅, 권한
  규칙, 안전 기준선, 저장소 규칙, 승인, 샌드박스가 함께 적용되는 방식
- [docs/HOOKS.md](docs/HOOKS.md) — 11개의 TUI 수명 주기 훅 이벤트, 해당
  페이로드, 턴을 조정할 수 있는 3개 이벤트 (`codewhale exec`와 CLI 하위
  명령은 훅을 실행하지 않음)
- [docs/WEB.md](docs/WEB.md) — 루프백 전용 내장 브라우저 클라이언트와 일회성
  인증 경계

나머지 — 모드, 키 바인딩, 샌드박스 세부 사항, MCP, 런타임 API, 아키텍처 —
는 [docs](docs)와 [codewhale.net](https://codewhale.net/)에 있습니다.

## 기여

이슈, PR, 재현 절차, 로그, 기능 요청은 모두 이곳에서 실제 프로젝트 작업이며, 첫 기여도 환영합니다. PR을 그대로 병합할 수 없을 때는 메인테이너가 작동하는 부분을 거두어 반영하고, 작성자의 크레딧은 커밋, 변경 로그, [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md)에 그대로 남습니다.

- [열려 있는 이슈](https://github.com/Hmbown/CodeWhale/issues) — 처음
  기여하기 좋은 작업이 여기에 있습니다
- [CONTRIBUTING.md](CONTRIBUTING.md) — 개발 환경 설정과 PR 흐름
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — 이 프로젝트를 빚어 온
  모든 사람
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

프로젝트를 시작하게 해 준 모델과 지원을 제공한 [DeepSeek](https://github.com/deepseek-ai), Whale Brother family로 맞이해 준 [DataWhale](https://github.com/datawhalechina) 🐋, 그리고 터미널 에이전트 경험에 함께 협력해 준 [OpenWarp](https://github.com/zerx-lab/warp)와 [Open Design](https://github.com/nexu-io/open-design)에 감사드립니다.

## 라이선스

[MIT](LICENSE). 독립 커뮤니티 프로젝트이며, 어떤 모델 프로바이더와도 제휴 관계가 없습니다.

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
