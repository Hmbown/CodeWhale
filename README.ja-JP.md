<!-- source: README.md sha256:ddc9cd7b5aea -->
# Codewhale

ターミナルで動くコーディングエージェント。あらゆるモデルで動作し、オープンモデルを最優先します。

プロバイダ、モデル、タスクを渡すと、コードを読み、ファイルを編集し、コマンドを実行し、結果を確認して、タスクが完了するかあなたの手が必要になるまで作業を続けます。対話的な作業には TUI を、スクリプトと CI には `codewhale exec` を。Rust 製、MIT ライセンスで、すべて手元のマシン上で動きます。

このプロジェクトは `deepseek-tui` として始まりました。その周りに生まれたコミュニティがより多くのプロバイダを必要としたため、いまでは DeepSeek、Claude、GPT、Kimi、GLM ほか 30 以上のモデルが、同じランタイムと同じツール群を通って動いています。

[English](README.md) · [简体中文](README.zh-CN.md) · [Tiếng Việt](README.vi.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [codewhale.net](https://codewhale.net/) · [Docs](docs) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)

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
```

TUI では、`/model` がプロバイダとモデルをまとめて切り替え、`/fleet` がワーカーのチームを走らせ、`/restore` がターンを取り消します。`Tab` は Plan / Act / Operate を順に切り替え、`Shift+Tab` は Ask / Auto-Review / Full Access の承認スタンスを順に切り替え、`!` は Shell コマンドを通常の承認経路で実行します。

## できること

- プロバイダとモデルの選択を具体的なルートに解決します: エンドポイント、ワイヤプロトコル、コンテキスト上限、価格。コンテキスト予算とコスト表示は実際のルートに基づき、不明な価格は $0 ではなく不明として表示されます。([docs/PROVIDERS.md](docs/PROVIDERS.md))
- ホスト型のオープンモデルプロバイダ（`deepseek`、`openrouter`、`moonshot`、`zai`、`minimax`、`nvidia-nim` など）、キー不要で使える自前の `vllm` / `sglang` / `ollama`、そして thinking とプロンプトキャッシュに対応した Messages API 経由のネイティブな Anthropic と通信します。
- 複数のワーカーを耐久的に走らせます: Fleet は作業を追記専用の台帳（ledger）に記録するため、実行は再起動を生き延び、`fleet resume` が止まったところから再開します。Workflow は大きなジョブを、再開可能で検証可能なレーンへ計画します。([docs/FLEET.md](docs/FLEET.md))
- リスクは雰囲気ではなくコードでゲートします: 3 つのモード（Plan は読み取り専用）、独立した承認スタンス、OS サンドボックス（Seatbelt、Landlock + seccomp、bwrap）、ツール呼び出しごとに allow/deny/ask を判定できるフック、そして `/restore` が実際の履歴に決して触れないようにする side-git スナップショット。
- リポジトリが自らの法を宣言できます: `.codewhale/constitution.json` の不変条件は、Full Access でもスキップできない書き込みホールドにコンパイルされます。([docs/CONFIGURATION.md](docs/CONFIGURATION.md))
- MCP はクライアントとサーバーの両方向に対応し、再利用可能なスキルを読み込み、HTTP/SSE と ACP の Runtime API を公開し、コミュニティ製の [VS Code GUI](https://github.com/HengQuWorld/CodeWhale-VSCode) を支えています。
- TUI は作業を検査可能なレシートとして表示し、ライブに動く行は常に 1 行だけに保ち、本物のコンテキストインスペクタ、12 のテーマ、モーション低減モードと ASCII セーフモードを備えます。UI は英語、简体中文、日本語、Tiếng Việt、Español、Português、한국어で利用でき、繁體中文には部分的に対応しています。

それ以外のすべて — 設定、キーバインド、サンドボックスの詳細、アーキテクチャ — は [docs](docs) と [codewhale.net](https://codewhale.net/) にあります。

## コントリビューション

すべてのフィードバックは贈り物です。Issue、PR、再現手順、ログ、機能要望、初めてのコントリビューションは、どれもここでは本物のプロジェクト作業です。PR がそのままマージできない場合、メンテナは使える部分を収穫（harvest）し、作者のクレジットは残ります — コミットにも、changelog にも、[docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) にも。使っているモデルやプロバイダが見当たらないとき、あるいは手元のマシンで何かが壊れたとき、それを知らせてもらえることが何より役に立ちます。

- [Open issues](https://github.com/Hmbown/CodeWhale/issues) — 最初のコントリビューションに向くものはここにあります
- [CONTRIBUTING.md](CONTRIBUTING.md) — 開発環境のセットアップと PR の流れ
- [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) — このプロジェクトを形づくってきた全員
- [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

プロジェクトの出発点となったモデルとサポートを提供してくれた [DeepSeek](https://github.com/deepseek-ai)、「鯨兄弟」ファミリーに迎え入れてくれた [DataWhale](https://github.com/datawhalechina) 🐋、そしてターミナルエージェント体験で協力してくれている [OpenWarp](https://github.com/zerx-lab/warp) と [Open Design](https://github.com/nexu-io/open-design) に感謝します。

## ライセンス

[MIT](LICENSE)。独立したコミュニティプロジェクトであり、いかなるモデルプロバイダとも提携していません。

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
