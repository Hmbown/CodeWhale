# CodeWhale

> あらゆるモデルのためのターミナルコーディングエージェント — オープンモデルを最優先に。

Rust 製の TUI と CLI、25 のプロバイダ。DeepSeek、OpenRouter、Hugging Face、DeepInfra、ローカルの vLLM/SGLang/Ollama を第一級のルートとして扱い、手元にあるのが Anthropic Claude や OpenAI のキーなら、それらの API もネイティブに扱えます。承認ゲート付きツール、OS サンドボックス、そして全ターンを巻き戻せる `/restore`。

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [Tiếng Việt README](README.vi.md) · [codewhale.net](https://codewhale.net/) · [Install guide](docs/INSTALL.md) · [Provider registry](docs/PROVIDERS.md) · [Changelog](CHANGELOG.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![DeepWiki project index](https://img.shields.io/badge/DeepWiki-project-blue)](https://deepwiki.com/Hmbown/CodeWhale)

![ターミナルで動作する CodeWhale](assets/screenshot.png)

## インストール

```bash
npm install -g codewhale
codewhale --version   # 0.8.63
```

npm wrapper（Node 18+）は GitHub Releases から SHA-256 検証済みのバイナリをダウンロードし、`codewhale`、`codew`、`codewhale-tui` をインストールします。ソースからビルドしたい場合は cargo（Rust 1.88+）で:

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
```

> **Linux ユーザーへ:** ビルド依存パッケージを先にインストールしてください:
> `sudo apt-get install -y build-essential pkg-config libdbus-1-dev`。
> 詳細は [INSTALL.md](docs/INSTALL.md#4-install-via-cargo-any-tier-1-rust-target) を参照。

その他の経路:

```bash
# Docker
docker pull ghcr.io/hmbown/codewhale:latest

# Nix
nix run github:Hmbown/CodeWhale

# Windows
scoop install codewhale        # または GitHub Releases の NSIS インストーラ

# GitHub に安定して到達できない場合の CNB ミラー
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.63 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.63 codewhale-tui --locked --force

# 旧 Homebrew 互換。formula の改名が完了するまで deepseek-tui 名のままです
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

Linux riscv64 を含む全プラットフォーム向けのビルド済みアーカイブは [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases) に添付されています。チェックサム、中国ミラー、Windows 固有の手順、トラブルシューティングは [docs/INSTALL.md](docs/INSTALL.md) を見てください。

## 最初の起動

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

どのプロバイダも同じ一行の形です: `--provider openrouter`、`--provider moonshot`、あるいは `vllm`、`sglang`、`ollama` を自分の localhost ランタイムに向ければキーすら要りません。Claude のキーを持っているなら、`codewhale auth set --provider anthropic` を実行するか `ANTHROPIC_API_KEY` を export するだけで、ネイティブの Messages アダプタがあとを引き受けます。

キーは `~/.codewhale/config.toml` に保存されます。互換性のため、旧 `~/.deepseek/` の設定も引き続き読み込まれます。

セッション中に便利なコマンド:

- `/provider` と `/model` — ルートとモデルをセッション中に切り替えます。
- `/restore` — side-git スナップショットから過去のターンを巻き戻します。
- `/skills` — `~/.codewhale/skills/` の再利用可能なワークフローを読み込みます。
- `/config` — ランタイム設定を編集します。`/statusline` は現在のルート、コスト、セッション状態を表示します。
- `! cargo test -p codewhale-tui` — 任意の Shell コマンドを、通常の承認・サンドボックス経路で実行します。

スクリプトや CI 向けのヘッドレス実行:

```bash
codewhale exec --allowed-tools read_file,exec_shell --max-turns 10 "fix the failing test"
```

## できること

ターミナルネイティブのエージェントハーネス — TUI + CLI、16 個の Rust クレート。安全のためのレールは、モデルが覚えておくべき助言ではなく、ランタイムの仕組みとして実装されています:

- **承認ゲート付きツールと OS サンドボックス。** ファイル、Shell、Git、Web、MCP、サブエージェントの各ツールは、明示的な承認ゲートとサンドボックスバックエンド（bwrap、Landlock、Seatbelt、seccomp）の背後で動きます。
- **信頼できるロールバック。** side-git スナップショットと `/restore` は、リポジトリの `.git` の外側に置かれます — ターンを取り消しても履歴には一切触れません。
- **Hooks v2**。`tool_call_before` フックが JSON で `allow`/`deny`/`ask` の判定を返します。deny 優先の優先順位、glob マッチャ、プロジェクトローカルな `.codewhale/hooks.toml` に対応。
- **プロバイダを認識する並行サブエージェント**。調査と実装を並列に進め、big/cheap のモデル階層はプロバイダごとに解決されます — モデル ID のハードコードはありません。
- **耐久性のあるセッション。** fork、relay 引き継ぎ、そして Plan/Agent/YOLO のモード切り替えをまたいでもバイト単位で安定する、セッション横断のディスク永続プロンプトキャッシュ。ターンはシステムのスリープも生き延びます: ストリーミング中にサスペンドしても、復帰後にリクエストが静かに再発行され、ターンは失敗しません。
- **ヘッドレスモード。** スクリプトや CI 向けに、`codewhale exec` が `--allowed-tools`、`--disallowed-tools`（deny 優先）、`--max-turns`、`--append-system-prompt` に対応。
- **どこにでも組み込める。** HTTP/SSE と ACP の Runtime API、VS Code 拡張（Phase 0）、Telegram/Feishu ブリッジ（Weixin ブリッジは実験的）。
- **日常使いの磨き込み。** MCP のクライアント*かつ*サーバー、再利用可能なスキル、7 ロケールのローカライズ、Xiaomi MiMo による音声合成（TTS）。

### あらゆるモデル、まずはオープンモデル

25 のプロバイダが、同じハーネス、同じ Constitution、同じツール群を通ります:

- **オープンモデル（ホスト型）:** `deepseek`（同格の中の筆頭）、`openrouter`、`huggingface`（Inference Providers）、`moonshot`（Kimi）、`volcengine`（Ark）、`nvidia-nim`、`together`、`fireworks`、`novita`、`siliconflow` / `siliconflow-CN`、`arcee`、`xiaomi-mimo`、`deepinfra`、`atlascloud`、`wanjie-ark`、さらに任意のゲートウェイに使える汎用の `openai` 互換ルート。
- **オープンモデル（セルフホスト型）:** `vllm`、`sglang`、`ollama` を自分の localhost エンドポイントに向けて使えます — キーは不要です。
- **クローズドプロバイダ（ネイティブ対応）:** `anthropic` は専用の `/v1/messages` アダプタ経由で、適応的 thinking、プロンプトキャッシュのブレークポイント、署名付き thinking のリプレイに対応します — OpenAI 方言のシムではありません。`openai-codex` は既存の ChatGPT/Codex CLI ログインを再利用します。

ルーティングは base URL の差し替えにとどまりません: `/reasoning` の effort は各プロバイダのワイヤ方言に翻訳され、サブエージェントの階層はプロバイダごとに解決され、システムプロンプト内のモデル情報はハードコードではなくモデルごとにテンプレート化されます。セッション中の切り替えは `/provider` と `/model` で。認証情報、base URL、能力の境界を含む完全なレジストリは [docs/PROVIDERS.md](docs/PROVIDERS.md) にあります。

サブエージェントの fanout は設定優先です。`[subagents]` に全体の既定値を置き、
`[subagents.providers.deepseek]`、`[subagents.providers.glm]`、
`[subagents.providers.openrouter]` などで API ごとの上限を調整できます。直結の
DeepSeek API は広めに、サブスクリプション型や rate-limit のあるルートは 3–5
並列に抑える、といった運用を prompt やコード変更なしで行えます。詳しくは
[docs/SUBAGENTS.md](docs/SUBAGENTS.md#concurrency-cap) を参照してください。

完全な変更履歴は [CHANGELOG.md](CHANGELOG.md) を参照してください。

## 考え方 — このバージョンに入れている mission idea

多くのコーディングエージェントは、力を足すところから始めます。もっと多くのツール、もっと長いコンテキスト、もっと強い自律性。CodeWhale は責任を割り当てるところから始めます。

（これはこのバージョンで形にしているデザインミッションです。memory や cost、remote orchestration の具体的な形はまだイテレーション中です — 下の v0.9.0 Track を参照。）

リポジトリを編集するエージェントには住所があるべきです — このターミナル、このユーザー、このブランチ、このセッション。人格ではなく、返送先の住所です。何かが壊れたとき、「モデルがやった」は答えになりません。「このインスタンスが、このセッションで、この承認のあとに」なら答えになります。

次に必要なのは法です。実際の作業セッションは衝突の積み重ねです: 現在のリクエスト、リポジトリの指示、新しい Shell 出力、古い記憶、前のエージェントの引き継ぎが、同じターンの中で競合します。**CodeWhale Constitution** は権威の順序を固定します:

1. **ユーザーの意図が主権を持つ。** 現在のリクエストは、古いリポジトリの指示、記憶、過去の引き継ぎ、人格オーバーレイより上位です。
2. **リポジトリの法は明示する。** `.codewhale/constitution.json` を追加して、プロジェクトの持続的な権威を宣言します: 保護すべき不変条件、ブランチポリシー、検証ルール。
3. **証拠は語りより上。** ツール出力は、自信たっぷりの推測に勝ちます。失敗した `cargo test` は失敗した `cargo test` として報告され、楽観へ要約されることはありません。検証はタスクの一部であり、後日談ではありません。
4. **記憶は最後。** 有用ですが、決して権威にはなりません。

重要なポリシーはプロンプトではなくコードで強制されます: 承認ゲート、サンドボックス、スナップショット、ロールバック、ツールスキーマは、モデルが口先で回避できないランタイムの仕組みです。

そして、この法はどれもモデルの中には住んでいません — だからこそモデルは交換可能なのです。ハーネスが Constitution を担い、モデルは推論を提供します。DeepSeek とオープンウェイトの世界は第一級市民であり、LAN 上で vLLM や Ollama を動かす一台のマシンも完全に対等な存在です。そして手元にあるのが Claude や OpenAI のキーなら、CodeWhale はそれらの API にもネイティブ対応します。

それがこの製品です: より大きなモデルではなく、選んだどのモデルにも掛けられる、より厳格なハーネス。モデルを交換しても、法は保たれます。

## 詳細ドキュメント

README は考え方と最初の経路だけを持ちます。詳細はドキュメントと [codewhale.net](https://codewhale.net/) にあります:

- [User guide](docs/GUIDE.md) — CodeWhale との最初の 1 時間。
- [Install guide](docs/INSTALL.md) — すべてのパッケージ経路とトラブルシューティング。
- [Configuration](docs/CONFIGURATION.md) — 設定ファイル、リポジトリ constitution、プロバイダ設定。
- [Provider registry](docs/PROVIDERS.md) — モデルルート、認証情報、base URL、能力の境界。
- [Sub-agents](docs/SUBAGENTS.md) — 役割、ライフサイクル、出力コントラクト、復旧挙動。
- [MCP](docs/MCP.md) — 外部ツールサーバーへの接続と、CodeWhale 自身を MCP サーバーとして動かす方法。
- [Runtime API](docs/RUNTIME_API.md) — HTTP/SSE、ACP、モバイル、GUI/エディタ統合のコントラクト。
- [Model Lab](docs/MODEL_LAB.md) — オープンモデルの発見と評価のロードマップ。
- [Architecture](docs/ARCHITECTURE.md) — クレート構成、ランタイムフロー、ツールシステム、拡張ポイント、セキュリティモデル。

## v0.9.0 トラック

v0.9.0 は現在の統合レーンです。そこに集まりつつある作業:

- セッションとエージェント間の relay / 引き継ぎ面の強化
- 密集したツール実行でも落ち着いて読めるトランスクリプト
- VS Code と GUI クライアント向けの Runtime API
- WhaleFlow によるブランチ/リーフのワークフローオーケストレーション

リリースごとの詳細は [CHANGELOG.md](CHANGELOG.md) にあります。

## 謝辞

- **[DeepSeek](https://github.com/deepseek-ai)** — すべてのターンを動かすモデルとサポートをありがとうございます。感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** 🐋 — サポートと、「鯨兄弟」ファミリーへ迎え入れてくださったことに感謝します。感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — codewhale 対応を優先し、より良いターミナルエージェント体験のために協力してくださっていることに感謝します。
- **[Open Design](https://github.com/nexu-io/open-design)** — デザイン主導のエージェントワークフローをめぐるサポートと協力に感謝します。

このプロジェクトは、増え続けるコントリビューターのコミュニティの助けで出荷されています。メンテナのルールはシンプルです: 報告も PR も本物のプロジェクト作業です。最終的なパッチを絞り込んだり、遅らせたり、メンテナブランチへ収穫（harvest）することになったとしても、それは変わりません。

個々のコントリビューターへの常に最新のクレジット一覧は、正準の記録である[英語版 README の Thanks セクション](README.md#thanks)を参照してください。

---

## コントリビューション

[CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。プルリクエストを歓迎します — 最初のコントリビューションには [Open Issues](https://github.com/Hmbown/CodeWhale/issues) を確認してください。

CodeWhale には良い報告と PR がたくさん届きます。メンテナの姿勢は、その扉を開いたままリリース品質を守ることです:

- Issue は人間が読めて行動に移せる状態を保ちます。インテイク自動化は、メンテナが意図的に強制を有効にしない限り、助言にとどまります。
- PR はタイトルだけでなく、コード、テスト、関連 Issue、ランタイムの挙動から評価されます。
- PR が広すぎて直接マージできない場合、メンテナが安全な部分をより狭いブランチへ収穫し、作者をクレジットして何が入ったかを説明することがあります。
- Co-author トレーラーには `.github/AUTHOR_MAP` にあるマッピング可能な GitHub noreply ID を使います。報告者や再現手順の作者には、changelog、リリースノート、クローズコメントで感謝が示されるべきです。
- 継続的なコントリビューターは `.github/APPROVED_CONTRIBUTORS` に追加でき、dry-run ゲートが邪魔をしないようになります。

サポート: [Buy me a coffee](https://www.buymeacoffee.com/hmbown)

> [!NOTE]
> *DeepSeek Inc. とは関係ありません。*

## ライセンス

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
