<!-- source: README.md sha256:1569156eb887 -->
# Codewhale

ターミナルで動くオープンソースのコーディングエージェント — モデルはあなたが持ち込む。

Codewhale は DeepSeek のためのネイティブ体験として始まりました。そこから、コミュニティ主導のプロジェクトへと成長しています。広がり続ける国際的なコミュニティに合い、できるだけ多くのモデルとプロバイダに対応する、ひとつのコーディングハーネスです — オープンモデルを最優先に、ホスト型でもローカルでも、どれかを特別扱いすることはありません。

プロバイダ、モデル、タスクを渡すと、コードを読み、ファイルを編集し、コマンドを実行し、自分の作業を確認して、タスクが完了するかあなたの手が必要になった時点で止まります。タスクの途中でも `/model` でモデルを切り替えられます。対話的な作業には TUI を、スクリプトと CI には `codewhale exec` を。Rust 製、MIT ライセンスで、あなたのマシン上で動きます。

他のハーネスと違うのはここです。**役割ごとにどのモデルを使うかはあなたが決められ、しかも揃える必要がありません。** Fleet は役割ごとにプロバイダ・モデル・推論ティアを個別に固定します。だから速くて安いモデルが高価な推論モデルを指揮することも、GLM の builder と Kimi の reviewer が同じ仕事に取り組むこともできます。自分の役割と自分の constitution を書けば、そのハーネスは私たちのものではなく、あなたのものになります。

私たちは常にコントリビューターと改善の方法を探しています。使っているモデルやプロバイダが見当たらないとき、あるいは何かが壊れたときは、それを知らせてもらえることが最も役に立つことのひとつです — [コントリビューション](#コントリビューション)を見てください。

[English](README.md) · [简体中文](README.zh-CN.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md) · [Discord](https://discord.gg/37gfS3ksug)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join%20the%20community-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

![ターミナルで動作する Codewhale](assets/screenshot.png)

## インストール

```bash
npm install -g codewhale
```

Cargo、Docker、Nix、Scoop、ビルド済みアーカイブ、Android/Termux、そして GitHub に到達できないユーザー向けの CNB ミラーについては [docs/INSTALL.md](docs/INSTALL.md) で扱っています。`deepseek-tui` からの移行なら、設定とセッションはそのまま引き継がれます — [docs/REBRAND.md](docs/REBRAND.md) を参照してください。

## 使い方

```bash
codewhale auth set --provider deepseek   # or export ANTHROPIC_API_KEY, etc.
codewhale                                # open the TUI
codewhale exec "fix the failing test"    # headless
codewhale web                            # local browser client on 127.0.0.1
```


TUI では、`/model` がプロバイダとモデルをまとめて切り替え、`/fleet` がチームを組み立てて走らせ(一度にひとつの役割、それぞれが自分のモデルを持ちます)、`/undo` が直前のターンを取り消し、`/restore <N>` がワークスペースを以前のスナップショットへ巻き戻します(引数なしの `/restore` は一覧を表示するだけです)。入力欄が空のとき、`Tab` は Plan / Work / Operate を順に切り替えます。入力欄に文字があるときの `Tab` はスラッシュコマンドと `@` メンションの補完になります。`Shift+Tab` はいつでも Ask / Auto-Review / Full Access の権限スタンスを順に切り替えます。`!` は Shell コマンドを通常の承認経路で実行します。

## できること

- **どのモデルでも、どのプロバイダでも、そしてどんな組み合わせでも。** DeepSeek、Claude、GPT、Kimi、GLM をはじめ 30 以上のプロバイダ、そしてキー不要のあなた自身の vLLM・SGLang・Ollama が、すべてひとつのランタイムとひとつのツール群を通って動きます。カタログは各プロバイダの最新ラインナップを追跡します——DeepSeek の V4 Pro バックエンド(ラベルは `DeepSeek-V4-Pro-0813`)は引き続き `deepseek-v4-pro` として呼び出せ、Grok 4.6 が xAI の直接デフォルト、OrcaRouter は `orcarouter/auto` でルーティングされます。保存された役割は `provider`・`model`・推論ティアを明示的に記録するので、ひとつの実行の中で Fleet が複数のベンダーにまたがることができ、役割のルートはそのとき有効なプロバイダに左右されません。コンテキスト予算と価格は実際のルートに由来し、不明な価格は $0 ではなく不明と表示されます。
- **あなたが書くハーネス。** 役割は読んで編集できるファイルです。役割ごとにモデル、ツールの姿勢、常設の指示を持ち、チームで共有するならプロジェクトに、リポジトリをまたいで持ち歩くなら個人設定の隣に置きます。constitution はすべてのセッションを通じてエージェントにどう振る舞ってほしいかを記録し、ハーネスを私たちのやり方ではなくあなたのやり方に合わせます。
- **許可するまでは読み取り専用。** Plan モードはファイルを変更せず、リスクのあるコマンドは承認でゲートされます。OS サンドボックスが実際にコマンドをラップするとき、Codewhale はそれを明示します。macOS では利用可能な Seatbelt、Linux ではオプトインの bubblewrap です。リポジトリの `constitution.json` は書き込みホールドへとコンパイルされ、Full Access でもスキップできません。
- **再開できる作業。** Fleet はすべてのステップを追記専用の台帳に記録するので、`fleet resume` で止めたところから再開できます。

## インテグレーション

- **DeepSeek Harness（dsh）— Codewhale 経由で接続。**
  `codewhale integrations dsh connect` は既存の `@deepseek-ai/dsh`
  インストールを Codewhale のプロバイダールート、権限、ワークスペースに
  接続し、`integrations dsh install-bundle` はオプトインの DSH プラグイン
  バンドルを追加して、`dsh --profile codewhale` が単独で同じ ID を持てる
  ようにします。権限とライフサイクルは Codewhale が管理し、dsh の
  セッション、プロファイル、認証情報は一切変更されません。
  [docs/INTEGRATIONS_DSH.md](docs/INTEGRATIONS_DSH.md) を参照。
- **VS Code。** 公式拡張機能の雛形（`extensions/vscode`）は Codewhale を
  統合ターミナルで開き、ローカルランタイム経由の読み取り専用 Agent View
  を提供します。現在はローカル開発プレビューであり、マーケットプレイス
  版ではありません。

## さらに詳しく

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — ホスト型・ゲートウェイ・ローカル
  まで、すべてのプロバイダルート
- [docs/FLEET.md](docs/FLEET.md) — Fleet、台帳、再開
- [docs/WORKFLOW_EXPERIMENTAL_SEARCH.md](docs/WORKFLOW_EXPERIMENTAL_SEARCH.md) — Workflow 内の凍結済み・プロバイダ中立の実験的検索
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`、フック、
  constitution
- [docs/AUTHORIZATION_ORDER.md](docs/AUTHORIZATION_ORDER.md) — モード、フック、
  権限ルール、安全フロア、リポジトリルール、承認、サンドボックスの組み合わせ方
- [docs/HOOKS.md](docs/HOOKS.md) — 11 個の TUI ライフサイクルフックイベント、
  そのペイロード、ターンを誘導できる 3 イベント（`codewhale exec` と CLI
  サブコマンドではフックは発火しません）
- [docs/WEB.md](docs/WEB.md) — ループバック専用の組み込みブラウザクライアントと
  ワンタイム認証境界

その他 — モード、キーバインド、サンドボックスの詳細、MCP、ランタイム API、
アーキテクチャ — は [docs](docs) と [codewhale.net](https://codewhale.net/)
にあります。

## コントリビューション

Issue、PR、再現手順、ログ、機能要望は、どれもここでは本物のプロジェクト作業です。初めてのコントリビューションも歓迎します。PR がそのままマージできない場合、メンテナは使える部分を harvest し、作者のクレジットは残ります — コミットにも、changelog にも、[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) にも。

- [Open issues](https://github.com/Hmbown/CodeWhale/issues) — 最初のコントリビューションに向くものはここにあります
- [CONTRIBUTING.md](CONTRIBUTING.md) — 開発環境のセットアップと PR の流れ
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — このプロジェクトを形づくってきた全員
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

プロジェクトの出発点となったモデルとサポートを提供してくれた [DeepSeek](https://github.com/deepseek-ai)、「鯨兄弟」ファミリーに迎え入れてくれた [DataWhale](https://github.com/datawhalechina) 🐋、そしてターミナルエージェント体験で協力してくれている [OpenWarp](https://github.com/zerx-lab/warp) と [Open Design](https://github.com/nexu-io/open-design) に感謝します。

## ライセンス

[MIT](LICENSE)。独立したコミュニティプロジェクトであり、いかなるモデルプロバイダとも提携していません。

![ターミナルで 3 つの読み取り専用 scout サブエージェントを並列起動する Codewhale](assets/fanout.gif)