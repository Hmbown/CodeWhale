import type { HomeDict } from "../types";

/**
 * Korean home dictionary — a native rewrite mirroring the current English
 * direction: open-source terminal coding agent, bring your own model, runs
 * on your machine. The old "local-first" / "LLM leverage for ordinary
 * people" framing is intentionally gone.
 *
 * Product terms stay literal, matching `crates/tui/locales/ko.json`:
 * Plan / Work / Operate, Ask / Auto-Review / Full Access, Codewhale, fleet,
 * `codewhale exec`. "permission posture" renders as 권한 상태 everywhere;
 * "provider route" as 프로바이더 경로 (the TUI pack uses 경로 for route);
 * "receipt" as 기록, the reading the TUI pack uses in prose.
 *
 * The hero headline is one sentence split across two lines: heroTitleA ends
 * on the -고 connective so the <br/> break reads as a pause, not as two
 * sentences.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — 깊은 곳으로 대신 잠수하니, 당신은 잠수하지 않아도 됩니다.",
  metaDescription:
    "Codewhale이 깊은 곳으로 대신 잠수하니 당신은 잠수하지 않아도 됩니다 — 오픈 소스 터미널 코딩 에이전트. 원하는 모델을 가져오세요. 당신의 머신에서 실행됩니다. Rust, MIT.",

  kicker: "오픈 소스 · 원하는 모델을 그대로 · 터미널에서 실행",
  heroTitleA: "깊은 곳으로 대신 잠수하고,",
  heroTitleB: "당신은 잠수하지 않아도 됩니다.",
  heroIntro:
    "{brand}은 터미널을 위한 오픈 소스 코딩 에이전트입니다. 모델과 작업을 주면 코드를 읽고, 파일을 고치고, 스스로 검사를 실행한 뒤 작업이 끝났거나 당신이 필요할 때 멈춥니다. 어떤 모델이든 가져올 수 있고 섞어 쓸 수도 있습니다 — 역할마다 다른 모델을 지정하세요.",
  install: "설치",
  docs: "문서",
  copy: "복사",
  copied: "복사됨 ✓",

  installEyebrow: "한 줄 설치",
  installRequirement: "Node 18+ 필요 — Rust 툴체인은 필요 없음",
  installOtherWays: "다른 방법 →",

  latestRelease: "최신 릴리스 {tag}",
  releaseUnavailable: "릴리스 상태를 확인할 수 없음",
  currentSource: "소스",
  sourceCandidate: "미공개",
  providerRoutes: "프로바이더 {count}개",
  publishedRelease: "공개됨",
  figcaptionSourceCandidate: "미공개",

  shotSession: "현재 세션",
  screenshotAlt:
    "Operate 모드, 고래, 입력창, 하단 바가 보이는 현재 Codewhale 터미널 세션",
  figcaption: "현재 Codewhale 세션 · Operate 모드 · Ask 권한 상태",

  proofHeading: "수중 터미널 셸. 어떤 모델이든. 당신의 머신에서.",
  proofBody:
    "이미 쓰고 있는 모델을 그대로 가져오세요 — 호스팅형이든, 게이트웨이든, 로컬이든. Plan / Work / Operate와 명시적인 권한 상태가 잠수를 당신의 통제 아래에 둡니다.",

  sealDecides: "法",
  decidesEyebrow: "판단 과정 보기",
  decidesHeading: "추론 기록에서 확인되는 규칙",
  decidesLede:
    "실제 세션 발췌 — 우선순위가 매겨진 프로젝트 규칙이 랜딩 페이지 주장이 아니라 모델 추론 속에서 드러납니다.",

  sealWorkflow: "行",
  workflowHeading: "작업에서 검증된 변경까지.",
  workflow: [
    ["조사", "저장소와 지침, 그리고 작업 내용을 읽습니다."],
    ["실행", "명시적인 승인 경계 안에서 파일을 수정합니다."],
    ["검증", "검사를 실행하고 결과를 확인합니다."],
    ["보고", "간결하고 오래 남는 기록을 남깁니다."],
  ],
  receiptAria: "작업 기록 예시",
  receiptInspect: "저장소와 지침",
  receiptAct: "선택한 권한 상태 안에서 수정",
  receiptReport: "검사 통과 · 기록 저장됨",

  sealStart: "起",
  startHeading: "Codewhale이 처음인가요? 네 단계면 끝입니다.",
  startLede:
    "설치 → 키 없는 첫 세션 → 프로바이더 연결 → 첫 fleet 워크플로. 용어는 용어 페이지에 정의되어 있습니다.",
  startGuideLink: "시작 가이드 읽기 →",
  startVocabularyLink: "제품 용어 보기 →",

  sealBoundaries: "界",
  boundariesHeadingA: "당신의 모델.",
  boundariesHeadingB: "당신의 경계.",
  boundariesBody:
    "모델과 작업 모드, 권한 상태를 직접 고르세요. 알 수 없는 비용은 알 수 없다고 밝히고, 미리보기 단계의 기능은 그대로 미리보기라고 표시합니다.",
  hostedGatewayLocal: "호스팅형, 게이트웨이, 로컬 모델",
  planActOperateDesc: "읽기 전용 계획부터 자율 실행까지",
  askAutoReviewDesc: "작업에 맞는 권한 상태 선택",
  tuiExecWebDesc: "대화형과 헤드리스 런타임 인터페이스",

  sealSurfaces: "面",
  surfacesHeading: "작업이 일어나는 자리에서 런타임을 사용하세요.",
  surfaces: [
    ["TUI", "대화형 터미널 작업"],
    ["codewhale exec", "스크립트와 CI"],
    ["웹 클라이언트", "루프백 전용 브라우저 클라이언트"],
    ["Runtime API + MCP", "로컬 통합"],
    ["fleet", "지속형 멀티 에이전트 작업"],
  ],
  runtimeLink: "런타임 인터페이스와 안정성 노트 보기 →",

  installBandHeading: "명령 하나로 시작하세요.",
  binaries: "바이너리",
  chinaMirrors: "중국 미러",
  installGuideLink: "설치 가이드 읽기 →",

  sealCommunity: "众",
  communityHeading: "공개적으로 개발합니다",
  communityBody:
    "MIT 라이선스로 공개되어 있으며, 런타임과 프로바이더, 플랫폼, 문서, 테스트 전반의 기여자들이 함께 만들어 갑니다.",
  communityLinksAria: "커뮤니티 링크",
  contribute: "기여하기",
};
