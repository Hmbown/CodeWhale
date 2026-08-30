import type { ChromeDict } from "../types";

/**
 * Korean chrome dictionary — a native rewrite mirroring the current English
 * direction (bring your own model, runs on your machine); the old
 * "local-first" wordmark tag is intentionally gone.
 *
 * Terminology follows the TUI locale pack (`crates/tui/locales/ko.json`):
 * mode and permission names stay literal (Plan / Work / Operate, Ask /
 * Auto-Review / Full Access), 프로바이더 is "provider", 저장소 is
 * "repository", 추론 is "reasoning", 권한 is "permission". Commands, package
 * names, and GitHub are left as-is per docs/VOICE.md.
 *
 * The secondary nav labels pair the Korean primary with a short English
 * label — the Han pair (文档 / 指引 / …) is the English edition's own
 * editorial device and is never hardcoded at a call site.
 *
 * Nav labels are kept to two–four syllables: they sit in one horizontal
 * masthead row. "FAQ" therefore renders as 질의응답 rather than the longer
 * 자주 묻는 질문, in both the nav and the footer so the two agree.
 */
export const chrome: ChromeDict = {
  navDocs: "문서",
  navStart: "시작하기",
  navInstall: "설치",
  navFaq: "질의응답",
  navCommunity: "커뮤니티",
  navContribute: "기여",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "본문으로 건너뛰기",


  navPrimaryAria: "기본 탐색",
  navHomeAria: "Codewhale 홈",

  installCta: "설치 →",

  authSignIn: "로그인",
  authRegister: "회원가입",
  authGroupAria: "계정",

  wordmarkSeal: "深",
  wordmarkTag: "어떤 모델이든, 당신의 머신에서",

  // Newspaper dating: Korean papers write the issue date with the 자 suffix
  // (8월 3일자), so the label is the date itself rather than "제N호".
  issueLabel: "{date}자",
  dateLocale: "ko-KR",

  starsAria: "GitHub 스타 수",
  githubFallback: "GitHub",

  tickerLiveLabel: "실시간",
  tickerLiveTag: "LIVE",
  tickerMerged: "병합됨",
  tickerOpened: "열림",
  tickerClosed: "닫힘",
  tickerReleased: "릴리스",
  tickerFirstContribution: "첫 기여",
  tickerBy: "{handle} 님",
  tickerAria: "저장소 최근 활동",

  traceLabel: "추론 기록",
  traceTabsAria: "세션 발췌",

  menuOpen: "메뉴 열기",
  menuClose: "메뉴 닫기",

  themeAuto: "자동",
  themeLight: "밝게",
  themeDark: "어둡게",
  themeAria: "문서 테마: {mode} (클릭하면 전환)",
  themeTitle: "문서 테마 · 자동 / 밝게 / 어둡게",

  footerTagline:
    "Codewhale이 깊은 곳으로 대신 잠수하니 당신은 잠수하지 않아도 됩니다 — 오픈 소스 런타임의 문서, 소스, 커뮤니티.",
  footerProduct: "제품",
  footerProject: "프로젝트",
  footerDocs: "문서",
  footerGuide: "시작 가이드",
  footerInstall: "설치",
  footerModels: "모델",
  footerRuntime: "런타임",
  footerFaq: "질의응답",
  footerIssues: "이슈",
  footerContribute: "기여",
  footerLicense: "MIT 라이선스",
  footerPricing: "가격",
  footerTerms: "이용약관",
  footerPrivacy: "개인정보처리방침",
  footerChangelog: "변경 로그",
  footerCanonicalSource: "공식 소스: ",
  footerReleases: " · 릴리스: ",
  footerReleasesLink: "GitHub 릴리스",
  footerSecurity: "보안",

  switcherLabel: "언어",
  // "(으)로" is the standard Korean UI hedge when the interpolated noun's
  // final consonant is unknown at write time.
  switcherSwitchTo: "{label}(으)로 전환",
  partialBadge: "(일부 번역)",
};
