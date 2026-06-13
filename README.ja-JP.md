# CodeWhale

> コミュニティで作る、使える最良のモデルでコードを書くためのエージェントターミナル。

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [Tiếng Việt README](README.vi.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![codewhale screenshot](assets/screenshot.png)

## CodeWhale とは

CodeWhale は、あなた自身のマシンでローカルに動くオープンソースのターミナルエージェントです。自分のリポジトリで実仕事をこなします――コードを読み、コマンドを走らせ、ファイルを編集し、パッチをShipする。しかも**あなたが選んだモデル**で。シェル・ファイル編集・git・Web・MCP・サブエージェントからなる完全なツール面を、承認ゲート、巻き戻せるスナップショット、再開できるセッションと組み合わせて提供します。

もともとは DeepSeek に触発された TUI として始まりました。コミュニティがそれをより広いものへと育てました。大多数の人の手元にある最良のモデルで仕事ができるハーネス――それがどのプロバイダであれ。DeepSeek はここでも第一級の扱いですが、もはや唯一の良い選択肢ではなく、必須でもありません。

すべてはあなた自身のマシンで動きます。鍵もリポジトリも判断も、あなたの手元にあります。

## なぜ使われるのか

- **承認ゲート付きツール。** ファイル編集・シェル・git・Web・MCP・サブエージェント呼び出しはすべて、サンドボックスとあなたが制御する承認ポリシーを経由します。
- **サブエージェントと Fleet。** ヘッドレスなサブエージェントワーカーに並列の調査や実装を振り分け、複数ステップの実行を編成します。
- **スナップショットと巻き戻し。** 毎ターン side-git にスナップショットが残るので、`/restore` はリポジトリの `.git` に触れずに変更を取り消せます。
- **リアルタイム診断。** 編集後に（利用可能な環境では）言語サーバーが即座に反応し、型エラーや警告をその場で表示します。
- **永続セッション。** ターン・セッション・マシンをまたいで再開・分岐・引き継ぎ――さらにエディタや GUI 向けのランタイム API。
- **モデルは自由に。** タスクごとに最も適したプロバイダへルーティングできます。

## 対応するモデルとプロバイダ

CodeWhale は、人々が実際に使っているプロバイダの第一級ルートを同梱しています。キーを用意して、タスクに合うモデルを選んでください：

- **DeepSeek** ―― V4 Pro / Flash、および DeepSeek 互換ゲートウェイ
- **GLM / Z.ai** ―― GLM-5.1、GLM-5.2（Z.ai Coding Plan）
- **Kimi（Moonshot）** ―― Kimi K2.6 / K2.7 Code
- **MiniMax** ―― 第一級ルート
- **OpenRouter** ―― ひとつのキーで数百のモデル
- **NVIDIA NIM · Xiaomi MiMo · SiliconFlow · Fireworks · Novita · StepFun / StepFlash**
- **セルフホスト** ―― vLLM、SGLang、Ollama
- **任意の OpenAI 互換ゲートウェイ**

`/provider` と `/model` で切り替えられます。認証情報・ベース URL・能力の境界は
[docs/PROVIDERS.md](docs/PROVIDERS.md) を参照してください。

## インストール

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
```

初回起動時に、CodeWhale はプロバイダキーを尋ね、`~/.codewhale/config.toml`
に保存します。互換のため、従来の `~/.deepseek/` 設定も引き続き読み込まれます。

その他のインストール方法：

```bash
# npm ラッパー
npm install -g codewhale

# プラットフォーム別アーカイブは GitHub Releases に
# https://github.com/Hmbown/CodeWhale/releases

# CNB ミラー（GitHub への接続が不安定な場合）
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.60 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.60 codewhale-tui --locked --force

# Homebrew（formula 改名中の互換運用）
brew tap Hmbown/deepseek-tui && brew install deepseek-tui
```

Docker、直接ダウンロード、中国ミラー、Windows/Scoop、Nix、チェックサム、トラブルシューティングは
[docs/INSTALL.md](docs/INSTALL.md) を参照してください。

**従来の `deepseek-tui` パッケージからのアップグレードですか？** 設定・セッション・スキル・MCP の設定はすべて保持されます。[docs/REBRAND.md](docs/REBRAND.md) を確認したうえで、`codewhale doctor` を走らせて移行を確かめてください。

## クイックスタート

```bash
codewhale auth set --provider zai     # または：deepseek、openrouter、kimi ……
codewhale auth status
codewhale doctor
codewhale                              # TUI を起動
```

セッション内でよく使うコマンド：

- `/provider` と `/model` ―― ルートとモデルを選ぶ。
- `/config` ―― ランタイム設定を編集する。
- `/statusline` ―― 現在のルート・コスト・セッション状態。
- `/skills` ―― `~/.codewhale/skills/` から再利用可能なワークフローを読み込む。
- `/restore` ―― side-git のスナップショットから過去のターンを巻き戻す。
- `! cargo test` ―― 通常の承認・サンドボックス経路でシェルコマンドを走らせる。

## コミュニティとコントリビュート

CodeWhale はオープンに作られています――それがこのプロジェクトの要点です。目標はシンプルです。より多くの目と手によって、より多くの人々のための最良のエージェントハーネスを作る。ひとりのDeepSeek触発の個人プロジェクトが、コミュニティによって、当初の想像を超えるものへと形作られてきました。

**経験の有無を問わず、issue も pull requestも大歓迎です。** バグ報告、機能のアイデア、ドキュメントの修正、「初めてのPR」、ちょっとした疑問、どれも立派なプロジェクトへの貢献です。最終的なパッチが絞り込まれたり、遅れたり、メンテナのコミットに折り畳まれたりする場合であっても、メンテナは報告や PR を貢献として扱います――そして繰り返し貢献してくれる人は、公開記録に残り続けます。

- [Open issues](https://github.com/Hmbown/CodeWhale/issues) ―― 初回コントリビューション向けのものが揃っています。
- [CONTRIBUTING.md](CONTRIBUTING.md) ―― 開発ループを整えて PR を出す。
- [行動規範](CODE_OF_CONDUCT.md) ―― 互いに親切に。
- [コントリビューター](docs/CONTRIBUTORS.md) ―― CodeWhale を形作ってきた人々。

## ドキュメント

README はスタートラインです。詳しくは [`docs/`](docs) と [codewhale.net](https://codewhale.net/) にあります：

- [ユーザーガイド](docs/GUIDE.md) ―― CodeWhale との最初の1時間。
- [インストールガイド](docs/INSTALL.md) ―― すべてのパッケージパスとトラブルシューティング。
- [設定](docs/CONFIGURATION.md) ―― 設定ファイルとプロバイダ設定。
- [プロバイダ](docs/PROVIDERS.md) ―― モデルルート・認証情報・能力。
- [モード](docs/MODES.md) ―― Agent、Plan、YOLO。
- [サブエージェント](docs/SUBAGENTS.md) ―― 役割・ライフサイクル・リカバリ。
- [Fleet](docs/FLEET.md) ―― マルチワーカー実行とヘッドレス編成。
- [WhaleFlow 作成](docs/WHALEFLOW_AUTHORING.md) ―― 宣言的ワークフロー。
- [ランタイム API](docs/RUNTIME_API.md) ―― HTTP/SSE・ACP・エディタ/GUI 契約。
- [MCP](docs/MCP.md) ―― Model Context Protocol サーバー。
- [アーキテクチャ](docs/ARCHITECTURE.md) ―― crate 構成・ランタイムフロー・セキュリティ。
- [キーバインド](docs/KEYBINDINGS.md) ―― 完全なキーマップ。
- [サンドボックスと承認](docs/SANDBOX.md) · [アクセシビリティ](docs/ACCESSIBILITY.md)
  · [Docker](docs/DOCKER.md) · [メモリ](docs/MEMORY.md)
- [ドキュメント総目次](docs) ―― その他すべて。

## 実行アイデンティティと憲法

CodeWhale は、エージェントが実際のワークスペースで**どう振る舞うべきか**について、明確な主張を持っています。できることだけでなく。その主張は
[CodeWhale 憲法](docs/AGENT_ETHOS.md) として書き下ろされており、いくつかの考えに要約されます：

- **エージェントには居場所がある。** それは*この*ターミナル・*この*ワークスペースにおけるひとつのインスタンスであり、モデルカードでもリーダーボードのスコアでもありません。
- **証拠は叙述に勝る。** ツールの出力は推測に勝ります。失敗したコマンドは失敗として報告され、検証はタスクの一部です。
- **ユーザーの意図が至上。** あなたの現在の要求は、古いリポジトリの指示・記憶・以前の引き継ぎよりも優先されます。
- **ローカルの掟は明示的。** リポジトリは `.codewhale/constitution.json` を置いて、永続するプロジェクト権威・保護される不変条件・検証ルールを定義できます。
- **ランタイムポリシーは強制される。** モード・承認ゲート・サンドボックス・巻き戻し・ツールスキーマはコードであり、モデルが覚えておくべき助言ではありません。

このプロダクトは、モデルを取り巻く「順序のレイヤー」です。誰が行動しているか、誰の掟が適用されるか、どのような証拠があるか、そして次の人やエージェントがどう続けられるか。もしこの枠組みが役に立つなら、それは素晴らしいことです。役に立たなければ、無視してツールとしてだけ使っても構いません。

## 謝辞

CodeWhale が存在するのは、それを使い、壊し、直してくれる人たちのおかげです。

- **[DeepSeek](https://github.com/deepseek-ai)** ―― このプロジェクトを始められたモデルと支援。感謝 DeepSeek 提供模型与支持。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 ―― 支援と、「鯨兄弟」ファミリーへの歓迎に感謝します。
- **[OpenWarp](https://github.com/zerx-lab/warp)** と
  **[Open Design](https://github.com/nexu-io/open-design)** ―― より良いターミナルエージェント体験の協業に感謝します。
- **すべてのコントリビューター** ―― PR ごとの完全な記録は
  [docs/CONTRIBUTORS.md](docs/CONTRIBUTORS.md) にあります。ありがとうございます。

## コントリビュート

[CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。pull request をお待ちしています――まずは [open issues](https://github.com/Hmbown/CodeWhale/issues) からどうぞ。

## ライセンス

[MIT](LICENSE)

> *CodeWhale は独立したコミュニティプロジェクトであり、いかなるモデルプロバイダとも提携していません。*

## Star 履歴

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
