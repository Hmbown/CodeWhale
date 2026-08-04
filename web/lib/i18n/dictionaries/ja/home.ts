import type { HomeDict } from "../types";

/**
 * Japanese home dictionary for the newspaper-ocean landing page.
 *
 * Product vocabulary stays literal and matches the TUI locale pack:
 * Plan / Act / Operate, Ask / Auto-Review / Full Access, Codewhale, TUI,
 * `codewhale exec`, Runtime API + MCP, Fleet, Node 18+, Rust, MIT.
 * "Permission posture" renders as 権限 (the TUI's own wording), not ポスチャ.
 *
 * `sealCommunity` uses the Japanese form 衆 rather than the English
 * edition's simplified 众; the other seals are kanji shared with English.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — 深く潜るのはこちら。あなたは潜らなくていい。",
  metaDescription:
    "Codewhale が深く潜るので、あなたは潜らなくて済みます。LLM の力をふつうの人の手に届け、ものをつくれるようにするターミナルエージェント。あなたのマシンの上で動きます。Rust 製、MIT ライセンス。",

  kicker: "オープンソース · どんなモデルでも · ターミナルで動く",
  heroTitleA: "深く潜るのはこちら。",
  heroTitleB: "あなたは潜らなくていい。",
  heroIntro:
    "{brand} は、LLM の力をふつうの人の手に届け、ものをつくれるようにします。ターミナルの中でリポジトリを読み、ファイルを編集し、チェックを走らせ、レシートを残す — コードが読めることを前提にしません。動くのはあなたのマシンの上。モデルは製品ではなく、選択可能なコンポーネントです。",
  install: "インストール",
  docs: "ドキュメント",
  copy: "コピー",
  copied: "コピー済み ✓",

  installEyebrow: "1 行でインストール",
  installRequirement: "Node 18+ が必要 — Rust ツールチェーンは不要",
  installOtherWays: "その他の方法 →",

  latestRelease: "最新リリース {tag}",
  releaseUnavailable: "リリース情報を取得できません",
  currentSource: "現在のソース",
  sourceCandidate: "ソース候補",
  providerRoutes: "{count} 件のプロバイダールート",
  publishedRelease: "公開済みリリース",
  figcaptionSourceCandidate: "ソース候補",

  shotSession: "現在のセッション",
  screenshotAlt:
    "Operate モード、クジラ、入力欄、フッターが写った現在の Codewhale ターミナルセッション",
  figcaption: "現在の Codewhale セッション · Operate モード · 権限は Ask",

  proofHeading: "水中のターミナルシェル。モデル中立。ローカルファースト。",
  proofBody:
    "すでに使っているホスト型、ゲートウェイ、ローカルのモデルをそのまま持ち込めます。Codewhale はあなたのマシンで動き、モデルを製品ではなく選択可能なコンポーネントとして扱います。Plan / Act / Operate と明示的な権限の指定によって、どこまで潜るかは常にあなたの管理下にあります。",

  sealDecides: "法",
  decidesEyebrow: "判断の過程を見る",
  decidesHeading: "トレースの中で確かめられる規範",
  decidesLede:
    "実際のセッションからの忠実な抜粋です — 優先順位づけされたプロジェクトの規範は、ランディングページの主張ではなく、モデルの推論の中で観察できます。",

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
    "インストール → キー不要の最初のセッション → プロバイダー接続 → 最初の Fleet ワークフロー。ここで出てくる用語は、用語集ページに定義があります。",
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
    ["Fleet", "永続的なマルチエージェント作業"],
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
