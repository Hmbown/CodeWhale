import type { ChromeDict } from "../types";

/**
 * Japanese chrome dictionary.
 *
 * Native rewrite mirroring the current English direction — "any model, on
 * your machine", not the retired "local-first" positioning.
 *
 * Terminology follows the TUI locale pack (`crates/tui/locales/ja.json`):
 * modes and permission names stay literal (Plan / Work / Operate, Ask /
 * Auto-Review / Full Access), 権限 is "permissions", 推論 is "reasoning",
 * レシート is "receipt". The pack renders "posture" as 姿勢/権限 rather than
 * the katakana calque ポスチャ, so the website matches it.
 *
 * The secondary nav labels pair the Japanese primary with a short English
 * companion — the Han pair (文档 / 指引 / …) is the English edition's own
 * editorial device and is not reused here.
 */
export const chrome: ChromeDict = {
  navDocs: "ドキュメント",
  navStart: "はじめに",
  navInstall: "インストール",
  navFaq: "よくある質問",
  navCommunity: "コミュニティ",
  navContribute: "貢献",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "メインコンテンツへスキップ",


  navPrimaryAria: "メインナビゲーション",
  navHomeAria: "Codewhale ホーム",

  installCta: "インストール →",

  authSignIn: "ログイン",
  authRegister: "新規登録",
  authGroupAria: "アカウント",

  wordmarkSeal: "深",
  wordmarkTag: "どんなモデルでも、あなたのマシンで",

  issueLabel: "{date} 号",
  dateLocale: "ja-JP",

  starsAria: "GitHub のスター数",
  githubFallback: "GitHub",

  tickerLiveLabel: "速 報",
  tickerLiveTag: "LIVE",
  tickerMerged: "マージ",
  tickerOpened: "オープン",
  tickerClosed: "クローズ",
  tickerReleased: "リリース",
  tickerFirstContribution: "初コントリビュート",
  tickerBy: "{handle} さん",
  tickerAria: "リポジトリの最近の動き",

  traceLabel: "推論トレース",
  traceTabsAria: "セッションの抜粋",

  menuOpen: "メニューを開く",
  menuClose: "メニューを閉じる",

  themeAuto: "自動",
  themeLight: "ライト",
  themeDark: "ダーク",
  themeAria: "ドキュメントのテーマ：{mode}（クリックで切り替え）",
  themeTitle: "ドキュメントのテーマ · 自動 / ライト / ダーク",

  footerTagline:
    "深く潜るのはこちら。あなたは潜らなくていい — オープンソースランタイムのドキュメント、ソース、コミュニティ。",
  footerProduct: "製品",
  footerProject: "プロジェクト",
  footerDocs: "ドキュメント",
  footerGuide: "はじめかた",
  footerInstall: "インストール",
  footerModels: "モデル",
  footerRuntime: "ランタイム",
  footerFaq: "よくある質問",
  footerIssues: "Issues",
  footerContribute: "貢献",
  footerLicense: "MIT ライセンス",
  footerPricing: "料金",
  footerTerms: "利用規約",
  footerPrivacy: "プライバシーポリシー",
  footerChangelog: "変更履歴",
  footerCanonicalSource: "正規ソース：",
  footerReleases: " · リリース：",
  footerReleasesLink: "GitHub リリース",
  footerSecurity: "セキュリティ連絡先",

  switcherLabel: "言語",
  switcherSwitchTo: "{label} に切り替え",
  partialBadge: "（一部翻訳）",
};
