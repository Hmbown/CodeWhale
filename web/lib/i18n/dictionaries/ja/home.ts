import type { HomeDict } from "../types";

/**
 * Japanese home dictionary for the newspaper-ocean landing page.
 *
 * Native rewrite mirroring the current English direction: bring your own
 * model, runs on your machine — no trace of the old "local-first" or
 * "LLM leverage for everyone" positioning.
 *
 * Product vocabulary stays literal and matches the TUI locale pack:
 * Plan / Work / Operate, Ask / Auto-Review / Full Access, Codewhale, TUI,
 * `codewhale exec`, Runtime API + MCP, fleet, Node 18+, Rust, MIT.
 * "Permission posture" renders as 権限 (the TUI's own wording), not ポスチャ.
 *
 * `sealCommunity` uses the Japanese form 衆 rather than the English
 * edition's simplified 众; the other seals are kanji shared with English.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — 深く潜るのはこちら。あなたは潜らなくていい。",
  metaDescription:
    "Codewhale が深く潜るので、あなたは潜らなくて済みます — オープンソースのターミナルコーディングエージェント。モデルは自分のものを。あなたのマシンで動く。Rust 製、MIT。",

  kicker: "オープンソース · モデルは自分で選ぶ · ターミナルで動く",
  heroTitleA: "Codewhale は深く潜る。",
  heroTitleB: "あなたが潜る必要はない。",
  heroIntro:
    "{brand} は、ターミナルで動くオープンソースのコーディングエージェントです。モデルとタスクを渡せば、コードを読み、ファイルを編集し、自分でチェックを回して、仕事が終わるか、あなたの判断が必要になったところで止まります。モデルは何でも持ち込めます。混ぜてもいい — 役割ごとに別のモデルを固定できます。",
  install: "インストール",
  docs: "ドキュメント",
  copy: "コピー",
  copied: "コピー済み ✓",

  installEyebrow: "1 行でインストール",
  installRequirement: "Node 18+ が必要 — Rust ツールチェーンは不要",
  installOtherWays: "その他の方法 →",

  latestRelease: "最新リリース {tag}",
  releaseUnavailable: "リリース情報を取得できません",
  currentSource: "ソース",
  sourceCandidate: "未リリース",
  providerRoutes: "{count} プロバイダー",
  publishedRelease: "リリース済み",
  figcaptionSourceCandidate: "未リリース",

  shotSession: "現在のセッション",
  screenshotAlt:
    "Operate モード、クジラ、入力欄、フッターが写った現在の Codewhale ターミナルセッション",
  figcaption: "現在の Codewhale セッション · Operate モード · 権限は Ask",

  proofHeading: "水中のターミナルシェル。どんなモデルでも。あなたのマシンで。",
  proofBody:
    "すでに使っているモデルをそのまま持ち込めます — ホスト型、ゲートウェイ、ローカル。Plan / Work / Operate と明示的な権限で、どこまで潜るかはあなたの管理下に。",

  sealDecides: "法",
  decidesEyebrow: "判断の過程を見る",
  decidesHeading: "トレースの中で確かめられる規範",
  decidesLede:
    "実際のセッションからの抜粋 — 優先順位づけされた規範は、ランディングページの主張ではなく、モデルの推論の中で見えます。",

  sealWorkflow: "行",
  workflowHeading: "タスクから、検証済みの変更へ。",
  workflow: [
    ["調査", "リポジトリと、その指示と、タスクを読みます。"],
    ["実行", "明示的な承認の境界を通してファイルを編集します。"],
    ["検証", "チェックを実行し、結果を確認します。"],
    ["報告", "簡潔で、あとから辿れるレシートを残します。"],
  ],
  receiptAria: "作業レシートの例",
  receiptInspect: "リポジトリと指示",
  receiptAct: "選択した権限の範囲で編集",
  receiptReport: "チェック通過 · レシート保存済み",

  sealStart: "起",
  startHeading: "Codewhale は初めてですか？ 4 ステップで最後まで。",
  startLede:
    "インストール → キー不要の最初のセッション → プロバイダー接続 → 最初の fleet ワークフロー。用語は用語集ページに。",
  startGuideLink: "はじめかたガイドを読む →",
  startVocabularyLink: "製品用語を見る →",

  sealBoundaries: "界",
  boundariesHeadingA: "あなたのモデル。",
  boundariesHeadingB: "あなたの境界。",
  boundariesBody:
    "モデル、作業モード、権限は、いずれも明示的に選びます。不明なコストは不明なままとし、プレビュー段階の画面にはその表示を残します。",
  hostedGatewayLocal: "ホスト型、ゲートウェイ、ローカルのモデル",
  planActOperateDesc: "読み取り専用の計画から自律実行まで",
  askAutoReviewDesc: "作業に合わせて権限を選ぶ",
  tuiExecWebDesc: "対話型とヘッドレス、両方のランタイム画面",

  sealSurfaces: "面",
  surfacesHeading: "作業のある場所で、そのままランタイムを使う。",
  surfaces: [
    ["TUI", "対話型のターミナル作業"],
    ["codewhale exec", "スクリプトと CI"],
    ["Web クライアント", "ループバック限定のブラウザクライアント"],
    ["Runtime API + MCP", "ローカル連携"],
    ["fleet", "永続的なマルチエージェント作業"],
  ],
  runtimeLink: "ランタイムの各画面と安定性の注記を見る →",

  installBandHeading: "コマンド 1 つで始める。",
  binaries: "バイナリ",
  chinaMirrors: "中国ミラー",
  installGuideLink: "インストールガイドを読む →",

  sealCommunity: "衆",
  communityHeading: "公開の場でつくる",
  communityBody:
    "MIT ライセンス。ランタイム、プロバイダー、プラットフォーム、ドキュメント、テストにまたがる貢献者たちの手で形づくられています。",
  communityLinksAria: "コミュニティリンク",
  contribute: "貢献する",
};
