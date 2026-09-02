<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale은 Rust로 만든 터미널용 오픈 소스 코딩 에이전트로, 사용자들과 함께 공개적으로 개선해 나갑니다.

![터미널에서 실행 중인 Codewhale](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## 설치

```bash
npm install -g codewhale
codewhale
```

처음 실행하면 공급자 연결 과정을 안내하며, 오프라인 상태로 계속 사용할 수도 있습니다. Codewhale은 Cargo, Docker, Nix, Scoop, 사전 빌드 아카이브, Android/Termux, CNB 미러도 지원합니다. [설치 안내서](docs/INSTALL.md)를 참조하세요.

각 셸에서 Tab 자동 완성은 명령 한 줄로 설정할 수 있습니다 — `codewhale completion bash|zsh|fish|powershell|elvish`. [셸 자동 완성](docs/INSTALL.md#8-shell-completions)을 참조하세요.

## 사용법

팀원에게 말하듯 Codewhale에 요청하세요:

```text
Fix the failing tests and explain what changed.
```

TUI를 열지 않고 작업을 실행할 수도 있습니다:

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale은 저장소를 읽고, 파일을 편집하고, 명령을 실행하고, 결과를 확인하며 목표를 향해 계속 작업할 수 있습니다. 어느 정도의 접근 권한을 줄지는 사용자가 결정합니다.

## Codewhale을 선택하는 이유

- **원하는 모델을 사용하세요.** 호스팅 공급자에 연결하거나 Ollama, vLLM, SGLang을 통해 로컬 모델을 사용할 수 있습니다. `/model`로 공급자와 모델을 전환하세요.
- **계속 주도권을 가지세요.** Plan은 읽기 전용입니다. Ask, Auto-Review, Full Access는 승인 동작을 명확하게 보여 줍니다. `/undo`는 마지막 턴을 되돌리고 `/restore`는 작업 공간을 이전 스냅샷으로 복원합니다.
- **긴 작업도 체계적으로 관리하세요.** 세션을 저장하고, 지속되는 `/goal`을 설정하고, 워크플로 실행 전에 검토하며, 에이전트의 내부 지시가 대화 기록에 섞이지 않도록 여러 에이전트를 조율할 수 있습니다.
- **이미 사용 중인 에이전트를 확장하세요.** MCP 서버와 스킬을 연결하고, 훅을 구성하고, 에이전트 역할을 프로젝트나 개인 설정에 읽기 쉬운 파일로 보관할 수 있습니다.

명령과 키보드 단축키를 보려면 TUI에서 `/help`를 실행하세요.

## 안전

Codewhale은 사용자가 허용한 접근 권한으로 사용자의 컴퓨터에서 실행됩니다. 승인 모드와 저장소 규칙은 에이전트가 할 수 있는 일을 제한하며, 지원되는 환경에서는 선택적 OS 샌드박싱으로 더 강력한 실행 경계를 추가할 수 있습니다. 가격이 알려지지 않은 모델은 무료로 표시하지 않고 미확인 상태로 둡니다.

정확한 정책 적용 순서는 [권한 부여 순서](docs/AUTHORIZATION_ORDER.md)에서, 로컬 설정은 [구성](docs/CONFIGURATION.md)에서 확인하세요.

## 문서

- [공급자와 로컬 모델](docs/PROVIDERS.md)
- [에이전트 팀](docs/FLEET.md)
- [MCP](docs/MCP.md), [훅](docs/HOOKS.md), [구성](docs/CONFIGURATION.md)
- [로컬 웹 클라이언트](docs/WEB.md)
- [전체 문서](docs)

## 커뮤니티 참여

사람들이 Codewhale을 사용하고, 불편한 점을 알리고, 수정에 힘을 보탤 때 Codewhale은 더 좋아집니다. 필요한 공급자가 없거나 워크플로가 불편하거나 터미널 UI가 작업을 방해한다면 [issue를 등록](https://github.com/Hmbown/CodeWhale/issues)해 주세요. 개선 방법을 알고 있다면 [pull request를 등록](CONTRIBUTING.md)해 주세요. 첫 기여도 환영하며, 반영된 작업에는 기여자의 이름을 남깁니다.

[Discord](https://discord.gg/37gfS3ksug)에 참여하거나 WeChat에서 Hunter(`hunterbown`)를 추가한 뒤 Whale Brothers 그룹 참여를 요청하세요.

## 프로젝트 역사

Codewhale은 `deepseek-tui`로 시작했으며 해당 구성 및 세션과의 호환성을 계속 유지합니다. 현재는 특정 공급자에 종속되지 않고 독립적으로 관리되며, 어떤 모델 공급자와도 제휴하지 않습니다.

모든 기여자와 프로젝트의 성장을 도운 오픈 소스 커뮤니티에 감사드립니다. [기여자 기록](docs/CONTRIBUTORS.md)을 확인하세요.

## 라이선스

[MIT](LICENSE). 다른 오픈 소스 프로젝트를 바탕으로 수정한 부분은 [타사 고지](docs/THIRD_PARTY_NOTICES.md)에 기록되어 있습니다.
