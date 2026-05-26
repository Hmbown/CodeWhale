# 🐳 CodeWhale

> **このターミナルネイティブのコーディングエージェントは、DeepSeek V4 の 100 万トークンのコンテキストウィンドウとプレフィックスキャッシュ機能を中心に構築されています。`codewhale` ディスパッチャーと `codewhale-tui` ランタイムの Rust バイナリペアとして配布され、Node.js や Python のランタイムは不要です。MCP クライアント、サンドボックス、永続的なタスクキューも標準で同梱されています。**

[English README](README.md)
[简体中文 README](README.zh-CN.md)

## インストール

`codewhale` は自己完結型の Rust リリースバイナリのペアとしてインストールされます。`codewhale` はディスパッチャーで、同じ場所にある `codewhale-tui` ランタイムを起動して対話セッションを実行します。npm、Homebrew、Docker は両方を自動でインストールします。Cargo や手動インストールでは、両方を同じディレクトリ（通常は `PATH` 上のディレクトリ）に置いてください。実行に Node.js や Python のランタイムは不要です。

```bash
# 1. npm — すでに Node を使っているなら最も簡単。npm パッケージは
#    GitHub Releases から対応するビルド済みバイナリペアをダウンロードする
#    薄いインストーラーであり、codewhale 本体に Node ランタイム依存を加えるものではありません。
npm install -g codewhale

# 2. Cargo — Node 不要。2 つの crate を両方インストールします。
cargo install codewhale-cli --locked   # `codewhale` (エントリーポイント)
cargo install codewhale-tui     --locked   # `codewhale-tui` (TUI バイナリ)

# 3. Homebrew — macOS パッケージマネージャ。
#    tap/formula 名は旧名のままですが、codewhale と codewhale-tui をインストールします。
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. 直接ダウンロード — GitHub Releases のプラットフォームアーカイブ。
#    https://github.com/Hmbown/CodeWhale/releases
#    アーカイブには codewhale と codewhale-tui とインストールスクリプトが含まれます。
#    個別バイナリもスクリプト用に添付されています。手動ではペアを同じ場所に置いてください。

# 5. Docker — ビルド済みリリースイメージ。
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.codewhale \
  -v "$PWD:/workspace" \
  -w /workspace \
  ghcr.io/hmbown/codewhale:latest
```

> 中国本土では、`--registry=https://registry.npmmirror.com` を指定して npm 経由のダウンロードを高速化するか、下記の[Cargo ミラー](#中国--ミラーフレンドリーなインストール)を利用してください。

既にインストール済みの場合は、インストール方法に合わせて更新してください:

```bash
codewhale update
npm install -g codewhale@latest
brew update && brew upgrade deepseek-tui
cargo install codewhale-cli --locked --force
cargo install codewhale-tui     --locked --force
```

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![DeepWiki](https://img.shields.io/badge/DeepWiki-Ask_AI-_.svg?style=flat&color=0052D9&labelColor=000000&logo=data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACwAAAAyCAYAAAAnWDnqAAAAAXNSR0IArs4c6QAAA05JREFUaEPtmUtyEzEQhtWTQyQLHNak2AB7ZnyXZMEjXMGeK/AIi+QuHrMnbChYY7MIh8g01fJoopFb0uhhEqqcbWTp06/uv1saEDv4O3n3dV60RfP947Mm9/SQc0ICFQgzfc4CYZoTPAswgSJCCUJUnAAoRHOAUOcATwbmVLWdGoH//PB8mnKqScAhsD0kYP3j/Yt5LPQe2KvcXmGvRHcDnpxfL2zOYJ1mFwrryWTz0advv1Ut4CJgf5uhDuDj5eUcAUoahrdY/56ebRWeraTjMt/00Sh3UDtjgHtQNHwcRGOC98BJEAEymycmYcWwOprTgcB6VZ5JK5TAJ+fXGLBm3FDAmn6oPPjR4rKCAoJCal2eAiQp2x0vxTPB3ALO2CRkwmDy5WohzBDwSEFKRwPbknEggCPB/imwrycgxX2NzoMCHhPkDwqYMr9tRcP5qNrMZHkVnOjRMWwLCcr8ohBVb1OMjxLwGCvjTikrsBOiA6fNyCrm8V1rP93iVPpwaE+gO0SsWmPiXB+jikdf6SizrT5qKasx5j8ABbHpFTx+vFXp9EnYQmLx02h1QTTrl6eDqxLnGjporxl3NL3agEvXdT0WmEost648sQOYAeJS9Q7bfUVoMGnjo4AZdUMQku50McDcMWcBPvr0SzbTAFDfvJqwLzgxwATnCgnp4wDl6Aa+Ax283gghmj+vj7feE2KBBRMW3FzOpLOADl0Isb5587h/U4gGvkt5v60Z1VLG8BhYjbzRwyQZemwAd6cCR5/XFWLYZRIMpX39AR0tjaGGiGzLVyhse5C9RKC6ai42ppWPKiBagOvaYk8lO7DajerabOZP46Lby5wKjw1HCRx7p9sVMOWGzb/vA1hwiWc6jm3MvQDTogQkiqIhJV0nBQBTU+3okKCFDy9WwferkHjtxib7t3xIUQtHxnIwtx4mpg26/HfwVNVDb4oI9RHmx5WGelRVlrtiw43zboCLaxv46AZeB3IlTkwouebTr1y2NjSpHz68WNFjHvupy3q8TFn3Hos2IAk4Ju5dCo8B3wP7VPr/FGaKiG+T+v+TQqIrOqMTL1VdWV1DdmcbO8KXBz6esmYWYKPwDL5b5FA1a0hwapHiom0r/cKaoqr+27/XcrS5UwSMbQAAAABJRU5ErkJggg==)](https://deepwiki.com/Hmbown/CodeWhale)

<a href="https://www.buymeacoffee.com/hmbown" target="_blank"><img src="https://img.shields.io/badge/Buy%20me%20a%20coffee-5F7FFF?style=for-the-badge&logo=buymeacoffee&logoColor=white" alt="Buy me a coffee" /></a>

![codewhale スクリーンショット](assets/screenshot.png)

---

## codewhale とは？

モデルは質問に答えます。エージェントはタスクを完了します。その差がハーネス——モデルが迷走しないようにするルール、証拠、フィードバックのシステムです。

CodeWhale はそのハーネスであり、DeepSeek V4 を中心に構築され、3つの原則に導かれています：

| 原則 | 仕組み |
|---|---|
| **信頼から始める** | 毎ターン「A」で始まる——確実性より可能性、便利さより丁寧さ |
| **明確な管轄権** | 9階層の権威を持つ成文憲法。ユーザーの意図が古い指示より優先。検証が自信より優先。 |
| **再帰的改善** | V4 がハーネスの一部を書いた。ハーネスが改善されると V4 はより効果的になり、さらにハーネスを改善する。毎ターンがより強くなる。 |

オープンソース、ターミナルネイティブ、`codewhale` / `codewhale-tui` の Rust バイナリペアとして提供されています。

## ハーネスの仕組み

エージェントモデルは大規模な相反する情報を扱います：ユーザーの意図、プロジェクトルール、システムデフォルト、ツール出力、古いメモリが単一ターンで権威を競い合います。LLM が裁判官として機能するには管轄権が必要です——衝突したとき、どの情報源が勝つのか？

CodeWhale は**憲法**（`prompts/base.md`）でこれに答えます。これは形式化された法の階層です——第七条は憲法自体の条項から前セッションのハンドオフまで、9 つの情報源をランク付けします。ユーザーの現在のメッセージは古いプロジェクト指示より上。ライブのツール出力は仮定より上。検証は自信より上。モデルは毎ターン明確な権威チェーンを継承し、どの指示に従うべきか推測する必要がありません。

7 つの条項が階層の上にあり、モデルのアイデンティティ、義務、エージェンシーを定義します：検証義務（第5条——すべての行動は証拠を残し、信念で成功を宣言しない）、協調の遺産（第6条——次の知性のためにワークスペースを可読に保つ）、真実優先条項（第2条——下位のルールで上書きできない）。

DeepSeek V4 のプレフィックスキャッシュがこれを実用的にします。憲法は長く詳細ですが、一度キャッシュされるとコールドリードの約 100 分の 1 のコストになります。モデルはそれを再帰的に参照し——RLM セッションを通じて覗き、スキャンし、クエリし——単一の暗記パスに頼るのではなく、必要に応じて情報を再訪します。それは閉じた本のテストよりも、開いた本のテストのように機能します。

権威構造が明示的であるため、失敗は隠されません。非ゼロの終了コード、ターン間に届く rust-analyzer からの型エラー、サンドボックス拒否——これらは修正ベクトルとしてフィードバックされます。モデルは自身のドリフトを使って自己修正します。

3 つのモードが行動空間を制御します。Plan は読み取り専用。Agent は破壊的操作を承認ゲートの背後に置きます。YOLO は信頼済みワークスペースで自動承認します。macOS Seatbelt はアクティブなサンドボックス；Linux Landlock は検出されるが未適用；Windows サンドボックスは未公開。

Fin——thinking off の安価な Flash 呼び出し——がターンごとにモデル自動ルーティングを処理します。`--model auto` がデフォルトです。

毎ターン side-git スナップショットをリポジトリの `.git` 外に記録。`/restore` と `revert_turn` がワークスペースを即座にロールバックします。

サブエージェントは並行実行（最大 20）。`agent_open` は即座に戻り；結果は完了センチネルとしてインラインで到着し、サマリー付き。完全なトランスクリプトは `agent_eval` を通じて境界付きハンドルに保持されます。[docs/SUBAGENTS.md](docs/SUBAGENTS.md) を参照。

その他の機能面：編集ごとの LSP 診断（rust-analyzer、pyright、typescript-language-server、gopls、clangd）、バッチ分析用 RLM セッション、MCP プロトコル、HTTP/SSE ランタイム API、永続タスクキュー、Zed 向け ACP アダプター、SWE-bench エクスポート、キャッシュヒット/ミス内訳付きライブコスト追跡。

---

## ハーネス

`codewhale`（ディスパッチャー CLI）→ `codewhale-tui`（コンパニオンバイナリ）→ ratatui インターフェース ↔ 非同期エンジン ↔ OpenAI 互換のストリーミングクライアント。ツール呼び出しは型付きレジストリ（シェル、ファイル操作、Git、Web、サブエージェント、MCP、RLM）を経由してルーティングされ、結果はトランスクリプトへとストリーム返送されます。エンジンはセッション状態、ターン管理、永続タスクキューを管理し、LSP サブシステムは編集後の診断を次の推論ステップ前にモデルのコンテキストへ供給します。

詳しくは [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) を参照してください。

---

## クイックスタート

```bash
npm install -g codewhale
codewhale --version
codewhale --model auto
```

ビルド済みバイナリペアとプラットフォームアーカイブは **Linux x64**、**Linux ARM64**（v0.8.8 以降）、**macOS x64**、**macOS ARM64**、**Windows x64** 向けに公開されています。その他のターゲット（musl、riscv64、FreeBSD など）は [ソースからのインストール](#install-from-source) または [docs/INSTALL.md](docs/INSTALL.md) を参照してください。

初回起動時に [DeepSeek API キー](https://platform.deepseek.com/api_keys) の入力を求められます。キーは `~/.codewhale/config.toml`（旧 `~/.deepseek/config.toml` も互換性維持）に保存されるため、OS のクレデンシャルプロンプトなしに任意のディレクトリから利用できます。

事前に設定することもできます:

```bash
codewhale auth set --provider deepseek   # ~/.codewhale/config.toml に保存

export DEEPSEEK_API_KEY="YOUR_KEY"      # 環境変数による代替方法。非対話シェルでは ~/.zshenv を使用
codewhale

codewhale doctor                         # セットアップを検証
```

> 保存済みキーをローテーション／削除するには: `codewhale auth clear --provider deepseek`。

### Linux ARM64（Raspberry Pi、Asahi、Graviton、HarmonyOS PC）

`npm i -g codewhale` は v0.8.8 以降、glibc ベースの ARM64 Linux で動作します。[Releases ページ](https://github.com/Hmbown/CodeWhale/releases) からビルド済みバイナリをダウンロードし、`PATH` 上に並べて配置することもできます。

### 中国 / ミラーフレンドリーなインストール

中国本土から GitHub または npm のダウンロードが遅い場合は、Cargo レジストリのミラーを利用してください:

```toml
# ~/.cargo/config.toml
[source.crates-io]
replace-with = "tuna"

[source.tuna]
registry = "sparse+https://mirrors.tuna.tsinghua.edu.cn/crates.io-index/"
```

その後、両方のバイナリをインストールしてください（ディスパッチャーは実行時に TUI へ委譲します）:

```bash
cargo install codewhale-cli --locked   # `codewhale` を提供
cargo install codewhale-tui     --locked   # `codewhale-tui` を提供
codewhale --version
```

ビルド済みバイナリは [GitHub Releases](https://github.com/Hmbown/CodeWhale/releases) からもダウンロードできます。ミラーされたリリースアセットには `DEEPSEEK_TUI_RELEASE_BASE_URL` を使ってください。

### Windows（Scoop）

[Scoop](https://scoop.sh) は Windows のパッケージマネージャです。`codewhale`
パッケージは Scoop main bucket にありますが、manifest は GitHub/npm/Cargo
リリースより遅れることがあります。先に更新し、インストール後に
`codewhale --version` で確認してください:

```bash
scoop update
scoop install codewhale
codewhale --version
```


<details id="install-from-source">
<summary>ソースからのインストール</summary>

任意の Tier-1 Rust ターゲット — musl、riscv64、FreeBSD、古い ARM64 ディストロを含む — で動作します。

```bash
# Linux のビルド依存関係 (Debian/Ubuntu/RHEL):
#   sudo apt-get install -y build-essential pkg-config libdbus-1-dev
#   sudo dnf install -y gcc make pkgconf-pkg-config dbus-devel

git clone https://github.com/Hmbown/CodeWhale.git
cd CodeWhale

cargo install --path crates/cli --locked   # Rust 1.88+ が必要。`codewhale` を提供
cargo install --path crates/tui --locked   # `codewhale-tui` を提供
```

両方のバイナリが必要です。クロスコンパイルとプラットフォーム固有の注意事項: [docs/INSTALL.md](docs/INSTALL.md)。

</details>

### その他の API プロバイダー

```bash
# NVIDIA NIM
codewhale auth set --provider nvidia-nim --api-key "YOUR_NVIDIA_API_KEY"
codewhale --provider nvidia-nim

# AtlasCloud
codewhale auth set --provider atlascloud --api-key "YOUR_ATLASCLOUD_API_KEY"
codewhale --provider atlascloud

# Wanjie Ark
codewhale auth set --provider wanjie-ark --api-key "YOUR_WANJIE_API_KEY"
codewhale --provider wanjie-ark --model deepseek-reasoner

# OpenRouter
codewhale auth set --provider openrouter --api-key "YOUR_OPENROUTER_API_KEY"
codewhale --provider openrouter --model deepseek/deepseek-v4-pro

# Xiaomi MiMo
codewhale auth set --provider xiaomi-mimo --api-key "YOUR_XIAOMI_MIMO_API_KEY"
codewhale --provider xiaomi-mimo --model mimo-v2.5-pro

# Novita
codewhale auth set --provider novita --api-key "YOUR_NOVITA_API_KEY"
codewhale --provider novita --model deepseek/deepseek-v4-pro

# Fireworks
codewhale auth set --provider fireworks --api-key "YOUR_FIREWORKS_API_KEY"
codewhale --provider fireworks --model deepseek-v4-pro

# 汎用 OpenAI 互換エンドポイント
codewhale auth set --provider openai --api-key "YOUR_OPENAI_COMPATIBLE_API_KEY"
OPENAI_BASE_URL="https://openai-compatible.example/v4" codewhale --provider openai --model glm-5

# セルフホスト SGLang
SGLANG_BASE_URL="http://localhost:30000/v1" codewhale --provider sglang --model deepseek-v4-flash

# セルフホスト vLLM
VLLM_BASE_URL="http://localhost:8000/v1" codewhale --provider vllm --model deepseek-v4-flash

# セルフホスト Ollama
ollama pull codewhale-coder:1.3b
codewhale --provider ollama --model codewhale-coder:1.3b
```

TUI 内では `/provider` でプロバイダーピッカー、`/model` でローカルのモデル/思考モードピッカーを開けます。`/provider openrouter` や `/model <id>` で直接切り替え、`/models` で対応プロバイダーのライブモデル一覧を明示的に取得できます。

---

## リリースノート

バージョンごとの変更点は [CHANGELOG.md](CHANGELOG.md) にまとめています。この README は、現在のインストール方法、主要ワークフロー、プロバイダー設定、ランタイムインターフェース、拡張ポイントに絞っています。

---

## 使い方

```bash
codewhale                                         # インタラクティブ TUI
codewhale "explain this function"                 # ワンショットプロンプト
codewhale exec --auto --output-format stream-json "fix this bug"  # NDJSON バックエンドストリーム
codewhale exec --resume <SESSION_ID> "follow up"  # 非対話セッションを継続
codewhale --model deepseek-v4-flash "summarize"   # モデルの上書き
codewhale --model auto "fix this bug"             # モデルと推論強度を自動選択
codewhale --yolo                                  # ツールを自動承認
codewhale auth set --provider deepseek            # API キーの保存
codewhale doctor                                  # セットアップと接続性のチェック
codewhale doctor --json                           # 機械可読の診断
codewhale setup --status                          # 読み取り専用のセットアップ状態
codewhale setup --tools --plugins                 # ツール／プラグインディレクトリの雛形作成
codewhale models                                  # ライブ API モデル一覧
codewhale sessions                                # 保存済みセッション一覧
codewhale resume --last                           # 最新セッションを再開
codewhale resume <SESSION_ID>                     # UUID 指定で特定セッションを再開
codewhale fork <SESSION_ID>                       # 保存済みセッションを兄弟パスに fork
codewhale serve --http                            # HTTP/SSE API サーバー
codewhale serve --acp                             # Zed/カスタムエージェント向け ACP stdio アダプター
codewhale run pr <N>                              # PR を取得しレビュープロンプトに先行投入
codewhale mcp list                                # 設定された MCP サーバー一覧
codewhale mcp validate                            # MCP の設定／接続性を検証
codewhale mcp-server                              # ディスパッチャー MCP stdio サーバーを実行
codewhale update                                  # バイナリ更新の確認と適用
```

### キーボードショートカット

| キー | 動作 |
|---|---|
| `Tab` | `/` または `@` のエントリ補完。実行中はドラフトをフォローアップとしてキューに追加。それ以外はモード切替 |
| `Shift+Tab` | 推論努力の切替: off → high → max |
| `F1` | 検索可能なヘルプオーバーレイ |
| `Esc` | 戻る／閉じる |
| `Ctrl+K` | コマンドパレット |
| `Ctrl+R` | 以前のセッションを再開 |
| `Alt+R` | プロンプト履歴を検索し、消去したドラフトを復元 |
| `Ctrl+S` | 現在のドラフトを退避（`/stash list`、`/stash pop` で復元） |
| `@path` | コンポーザーにファイル／ディレクトリのコンテキストを添付 |
| `↑`（コンポーザー先頭で） | 添付ファイル行を選択して削除 |
| `Alt+↑` | キュー済みの最後のメッセージを編集 |

ショートカット完全版: [docs/KEYBINDINGS.md](docs/KEYBINDINGS.md)。

---

## モード

| モード | 動作 |
| --- | --- |
| **Plan** 🔍 | 読み取り専用の調査 — 変更を加える前に、モデルが探索して計画を提案（`update_plan` + `checklist_write`） |
| **Agent** 🤖 | デフォルトのインタラクティブモード — 承認ゲート付きのマルチステップなツール利用。モデルは `checklist_write` で作業を概説 |
| **YOLO** ⚡ | 信頼できるワークスペースですべてのツールを自動承認。可視性のための計画とチェックリストは引き続き維持 |

---

## 設定

ユーザー設定: `~/.codewhale/config.toml`（旧 `~/.deepseek/config.toml` も互換性維持）。プロジェクトオーバーレイ: `<workspace>/.codewhale/config.toml`（旧 `<workspace>/.deepseek/config.toml`）（拒否される項目: `api_key`、`base_url`、`provider`、`mcp_config_path`）。すべてのオプションは [config.example.toml](config.example.toml) にあります。

主な環境変数:

| 変数 | 用途 |
|---|---|
| `DEEPSEEK_API_KEY` | API キー |
| `DEEPSEEK_BASE_URL` | API ベース URL |
| `DEEPSEEK_HTTP_HEADERS` | 任意のモデルリクエストヘッダー |
| `DEEPSEEK_MODEL` | デフォルトモデル |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | ストリームのアイドルタイムアウト秒数 |
| `DEEPSEEK_PROVIDER` | `codewhale`（デフォルト）、`nvidia-nim`、`openai`、`atlascloud`、`wanjie-ark`、`openrouter`、`xiaomi-mimo`、`novita`、`fireworks`、`sglang`、`vllm`、`ollama` |
| `DEEPSEEK_PROFILE` | 設定プロファイル名 |
| `DEEPSEEK_MEMORY` | `on` に設定するとユーザーメモリを有効化 |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | 信頼できるネットワークで非ローカル `http://` API ベース URL を許可 |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `OPENROUTER_API_KEY` / `XIAOMI_MIMO_API_KEY` / `MIMO_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | プロバイダー認証 |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | 汎用 OpenAI 互換エンドポイントとモデル ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud エンドポイントとモデル上書き |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Wanjie Ark エンドポイントとモデル上書き |
| `OPENROUTER_BASE_URL` | OpenRouter エンドポイント上書き |
| `XIAOMI_MIMO_BASE_URL` / `MIMO_BASE_URL` / `XIAOMI_MIMO_MODEL` / `MIMO_MODEL` | Xiaomi MiMo エンドポイントとモデル上書き |
| `NOVITA_BASE_URL` | Novita エンドポイント上書き |
| `FIREWORKS_BASE_URL` | Fireworks エンドポイント上書き |
| `SGLANG_BASE_URL` | セルフホスト SGLang のエンドポイント |
| `SGLANG_MODEL` | セルフホスト SGLang のモデル ID |
| `VLLM_BASE_URL` | セルフホスト vLLM のエンドポイント |
| `VLLM_MODEL` | セルフホスト vLLM のモデル ID |
| `OLLAMA_BASE_URL` | セルフホスト Ollama のエンドポイント |
| `OLLAMA_MODEL` | セルフホスト Ollama のモデルタグ |
| `NO_ANIMATIONS=1` | 起動時にアクセシビリティモードを強制 |
| `SSL_CERT_FILE` | 企業プロキシ向けのカスタム CA バンドル |

UI のロケールはモデルの言語とは別です。`settings.toml` で `locale` を設定するか、`/config locale zh-Hans` を使うか、`LC_ALL`/`LANG` に依存させてください。詳しくは [docs/CONFIGURATION.md](docs/CONFIGURATION.md) と [docs/MCP.md](docs/MCP.md) を参照してください。

---

## モデルと料金

| モデル | コンテキスト | 入力（キャッシュヒット） | 入力（キャッシュミス） | 出力 |
|---|---|---|---|---|
| `deepseek-v4-pro` | 1M | $0.003625 / 1M | $0.435 / 1M | $0.87 / 1M |
| `deepseek-v4-flash` | 1M | $0.0028 / 1M | $0.14 / 1M | $0.28 / 1M |

レガシーエイリアス `deepseek-chat` / `deepseek-reasoner` は `deepseek-v4-flash` にマップされます。NVIDIA NIM のバリアントはあなたの NVIDIA アカウント条件に従います。

> [!Note]
> 上記の V4 Pro レートは恒久的な料金になりました。DeepSeek は、2026 年 5 月 31 日 15:59 UTC に 75% 期間限定割引が終了するタイミングで、元の料金を 4 分の 1 に正式に調整しました。TUI のコスト見積もりはすでにこれらの値を使用しているため、コード上の変更は不要です。今後の価格変更については、公式の [DeepSeek 価格ページ](https://api-docs.deepseek.com/zh-cn/quick_start/pricing) を参照してください。

---

## 自分のスキルを公開する

codewhale はワークスペースのディレクトリ（`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills`）とグローバルな `~/.codewhale/skills`（旧 `~/.deepseek/skills` も互換性維持）からスキルを発見します。各スキルは `SKILL.md` ファイルを持つディレクトリです:

```text
~/.codewhale/skills/my-skill/
└── SKILL.md
```

必要なフロントマター:

```markdown
---
name: my-skill
description: DeepSeek にカスタムワークフローを実行させたいときに利用する。
---

# My Skill
ここにエージェント向けの指示を記述します。
```

コマンド: `/skills`（一覧）、`/skill <name>`（有効化）、`/skill new`（雛形）、`/skill install github:<owner>/<repo>`（コミュニティ）、`/skill update` / `uninstall` / `trust`。GitHub からのコミュニティインストールにバックエンドサービスは不要です。インストール済みのスキルはモデルに見えるセッションコンテキストに表示され、タスクが説明文にマッチした場合はエージェントが `load_skill` ツールを通じて関連スキルを自動選択できます。

---

## ドキュメント

| ドキュメント | トピック |
|---|---|
| [ARCHITECTURE.md](docs/ARCHITECTURE.md) | コードベース内部 |
| [CONFIGURATION.md](docs/CONFIGURATION.md) | 設定の完全リファレンス |
| [MODES.md](docs/MODES.md) | Plan / Agent / YOLO モード |
| [MCP.md](docs/MCP.md) | Model Context Protocol 統合 |
| [RUNTIME_API.md](docs/RUNTIME_API.md) | HTTP/SSE API サーバー |
| [INSTALL.md](docs/INSTALL.md) | プラットフォーム別インストールガイド |
| [DOCKER.md](docs/DOCKER.md) | GHCR イメージ、ボリューム、Docker 利用方法 |
| [CNB_MIRROR.md](docs/CNB_MIRROR.md) | CNB ミラーと中国向けインストールメモ |
| [TENCENT_CLOUD_REMOTE_FIRST.md](docs/TENCENT_CLOUD_REMOTE_FIRST.md) | Tencent/CNB/Lighthouse/Feishu のリモート優先パス |
| [TENCENT_LIGHTHOUSE_HK.md](docs/TENCENT_LIGHTHOUSE_HK.md) | Tencent Lighthouse 香港インスタンス設定 |
| [MEMORY.md](docs/MEMORY.md) | ユーザーメモリ機能ガイド |
| [SUBAGENTS.md](docs/SUBAGENTS.md) | サブエージェントの役割分類とライフサイクル |
| [KEYBINDINGS.md](docs/KEYBINDINGS.md) | ショートカット完全カタログ |
| [RELEASE_RUNBOOK.md](docs/RELEASE_RUNBOOK.md) | リリースプロセス |
| [OPERATIONS_RUNBOOK.md](docs/OPERATIONS_RUNBOOK.md) | 運用とリカバリ |

完全な変更履歴: [CHANGELOG.md](CHANGELOG.md)。

---

## 謝辞

このプロジェクトは、増え続けるコントリビューターのコミュニティから助けを得て出荷されています:

- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — 機能、修正、VS Code 拡張のスキャフォールドにまたがる 28 件の PR (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — 表、太字／斜体、水平線の Markdown レンダリング (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Windows と中国向けインストールドキュメント (#578)
- **[20bytes](https://github.com/20bytes)** — ユーザーメモリのドキュメントとヘルプの磨き込み (#569)
- **[staryxchen](https://github.com/staryxchen)** — glibc 互換性のプリフライト (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — glibc 互換性の改善 (#565)
- **[shentoumengxin](https://github.com/shentoumengxin)** — シェル `cwd` の境界バリデーション (#524)
- **[toi500](https://github.com/toi500)** — Windows 貼り付け修正の報告
- **[xsstomy](https://github.com/xsstomy)** — ターミナル起動時の再描画報告
- **[melody0709](https://github.com/melody0709)** — スラッシュ接頭辞の Enter アクティベーション報告
- **[lloydzhou](https://github.com/lloydzhou)** と **[jeoor](https://github.com/jeoor)** — コンパクションコストの報告
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README の明瞭化対応 (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows Scoop インストールドキュメント (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — 料金／割引情報の更新 (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **Hafeez Pizofreude** — `fetch_url` の SSRF 保護と Star History チャート
- **Unic (YuniqueUnic)** — スキーマ駆動の設定 UI（TUI + Web）
- **Jason** — SSRF セキュリティの強化

---

## コントリビューション

[CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。プルリクエストを歓迎します。良い初コントリビューションは [Open Issues](https://github.com/Hmbown/CodeWhale/issues) を確認してください。

> [!Note]
> *DeepSeek Inc. とは関係ありません。*

## ライセンス

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
