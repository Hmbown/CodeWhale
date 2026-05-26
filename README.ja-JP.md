# 🐳 CodeWhale

> **DeepSeek V4 のための最もエージェント的なハーネス。ルール、ツール、証拠、フィードバックループ——モデルがタスクを完了するまで働き続け、さらに改善し続けるための仕組みです。**

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/codewhale)](https://www.npmjs.com/package/codewhale)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![Sponsor](https://img.shields.io/badge/Sponsor-GitHub%20Sponsors-ea4aaa?logo=githubsponsors&logoColor=white)](https://github.com/sponsors/Hmbown)
[![DeepWiki](https://img.shields.io/badge/DeepWiki-Ask_AI-_.svg?style=flat&color=0052D9&labelColor=000000&logo=data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAACwAAAAyCAYAAAAnWDnqAAAAAXNSR0IArs4c6QAAA05JREFUaEPtmUtyEzEQhtWTQyQLHNak2AB7ZnyXZMEjXMGeK/AIi+QuHrMnbChYY7MIh8g01fJoopFb0uhhEqqcbWTp06/uv1saEDv4O3n3dV60RfP947Mm9/SQc0ICFQgzfc4CYZoTPAswgSJCCUJUnAAoRHOAUOcATwbmVLWdGoH//PB8mnKqScAhsD0kYP3j/Yt5LPQe2KvcXmGvRHcDnpxfL2zOYJ1mFwrryWTz0advv1Ut4CJgf5uhDuDj5eUcAUoahrdY/56ebRWeraTjMt/00Sh3UDtjgHtQNHwcRGOC98BJEAEymycmYcWwOprTgcB6VZ5JK5TAJ+fXGLBm3FDAmn6oPPjR4rKCAoJCal2eAiQp2x0vxTPB3ALO2CRkwmDy5WohzBDwSEFKRwPbknEggCPB/imwrycgxX2NzoMCHhPkDwqYMr9tRcP5qNrMZHkVnOjRMWwLCcr8ohBVb1OMjxLwGCvjTikrsBOiA6fNyCrm8V1rP93iVPpwaE+gO0SsWmPiXB+jikdf6SizrT5qKasx5j8ABbHpFTx+vFXp9EnYQmLx02h1QTTrl6eDqxLnGjporxl3NL3agEvXdT0WmEost648sQOYAeJS9Q7bfUVoMGnjo4AZdUMQku50McDcMWcBPvr0SzbTAFDfvJqwLzgxwATnCgnp4wDl6Aa+Ax283gghmj+vj7feE2KBBRMW3FzOpLOADl0Isb5587h/U4gGvkt5v60Z1VLG8BhYjbzRwyQZemwAd6cCR5/XFWLYZRIMpX39AR0tjaGGiGzLVyhse5C9RKC6ai42ppWPKiBagOvaYk8lO7DajerabOZP46Lby5wKjw1HCRx7p9sVMOWGzb/vA1hwiWc6jm3MvQDTogQkiqIhJV0nBQBTU+3okKCFDy9WwferkHjtxib7t3xIUQtHxnIwtx4mpg26/HfwVNVDb4oI9RHmx5WGelRVlrtiw43zboCLaxv46AZeB3IlTkwouebTr1y2NjSpHz68WNFjHvupy3q8TFn3Hos2IAk4Ju5dCo8B3wP7VPr/FGaKiG+T+v+TQqIrOqMTL1VdWV1DdmcbO8KXBz6esmYWYKPwDL5b5FA1a0hwapHiom0r/cKaoqr+27/XcrS5UwSMbQAAAABJRU5ErkJggg==)](https://deepwiki.com/Hmbown/CodeWhale)

[English README](README.md)
[简体中文 README](README.zh-CN.md)

[インストール](#インストール) · [クイックスタート](#クイックスタート) · [ドキュメント](#ドキュメント) · [コントリビューション](#コントリビューション) · [サポート](#サポート)

## インストール

`codewhale` は自己完結型の Rust バイナリとして提供されており、**実行に Node.js や Python のランタイムは必要ありません。** すでにマシンにインストールされているものを選んでください。いずれの方法でも同じバイナリが `PATH` に配置されます。

```bash
# 1. npm — すでに Node を使っているなら最も簡単。npm パッケージは
#    GitHub Releases から対応するビルド済みバイナリをダウンロードする
#    薄いインストーラーであり、codewhale 本体に Node ランタイム依存を加えるものではありません。
npm install -g codewhale

# 2. Cargo — Node 不要。
cargo install codewhale-cli --locked   # `codewhale` (エントリーポイント)
cargo install codewhale-tui     --locked   # `codewhale-tui` (TUI バイナリ)

# 3. Homebrew — macOS パッケージマネージャ。
brew tap Hmbown/deepseek-tui
brew install deepseek-tui

# 4. 直接ダウンロード — Node もツールチェーンも不要。
#    https://github.com/Hmbown/CodeWhale/releases
#    Linux x64/ARM64、macOS x64/ARM64、Windows x64 向けのビルド済みバイナリがあります。

# 5. Docker — ビルド済みリリースイメージ。
docker volume create codewhale-home
docker run --rm -it \
  -e DEEPSEEK_API_KEY="$DEEPSEEK_API_KEY" \
  -v codewhale-home:/home/codewhale/.deepseek \
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

![codewhale スクリーンショット](assets/screenshot.png)

---

## codewhale とは？

モデルは質問に答えます。エージェントはタスクを完了します。その差がハーネス——モデルを取り巻くルール、ツール、証拠、フィードバックループという動作環境です。

CodeWhale はそのハーネスであり、DeepSeek V4 Pro と Flash のために構築されました。メンテナがモデルがタスクの途中で方向を見失ったり、ユーザーの現在の要求より古い指示に従ったり、コマンドが失敗すると諦めたりすることにうんざりしたことから、個人ツールとして始まりました。そこから生まれたのが、モデルの方向性を保つシステムです：憲法的なプロンプト階層、構造化された信頼境界、並列サブエージェント、プレフィックスキャッシュ対応のコンテキスト管理、そしてモデルが自己修正するための十分なシグナルを提供する検証の鼓動。

DeepSeek V4 はこのハーネスの一部を書くのを手伝いました。これは重要です——CodeWhale がすでに V4 を使う最も効果的な方法であり、V4 が改善するにつれてハーネスも改善することを意味します。各ターンがより良いプロンプト、より良いルール、より良いハンドオフを残します。次のターンはより強い位置から始まります。

### ハーネスの仕組み

**憲法的階層。** システムプロンプトは法の抵触エンジンです。ユーザーの意図は古いメモリより上。ライブの証拠は仮定より上。検証は自信より上。各ターンは明確な権限チェーンを継承するので、モデルはどの指示に従うべきか推測する必要がありません。

**構造化された信頼。** ハードな境界を持つ3つのモード——Plan（読み取り専用）、Agent（承認ゲート付き）、YOLO（信頼済みワークスペースで自動承認）。OS レベルのサンドボックスが境界を支えます：macOS の Seatbelt、Linux の Landlock、Windows の Job Objects。危険なコマンドはモードに関係なく分類・ゲート制御されます。

**フィードバックの鼓動。** 失敗したコマンド、失敗したテスト、LSP 診断——これらは行き止まりではありません。モデルがチューニングできる信号です。非ゼロの終了コードは情報です。型エラーは修正ベクトルです。ハーネスは失敗を読み取り可能にし、ユーザーが常に舵を取り直さなくてもモデルが回復できるようにします。

**継続性。** メモリはセッションをまたいで持続します。ハンドオフはコンテキスト圧縮後も生存します。セッションは保存・再開・兄弟パスへのフォークが可能です。次の知性——人間であれ機械であれ——が、すでに学ばれたことを再発見する必要なく、前回の中断点から引き継ぎます。

**分散作業。** サブエージェントが並行実行、一度に最大 20。`agent_open` は即座に戻り、親は作業を続けます。結果は構造化された完了イベントとして到着し、境界付きハンドルで——完全なトランスクリプトで親コンテキストを埋める必要はありません。[docs/SUBAGENTS.md](docs/SUBAGENTS.md) を参照。

**適正な知性。** Fin——thinking off の安価な Flash——がモデル自動ルーティング、RLM 子呼び出し、要約、調整を処理します。Pro はアーキテクチャ、デバッグ、セキュリティレビューに使用。`--model auto` がターンごとにモデルと思考レベルを選択します。各問題に適切な量の知性を。

**長期注意の経済学。** 100 万トークンのコンテキストウィンドウとプレフィックスキャッシュテレメトリ。キャッシュヒットはミスより約 100 倍安価。`/statusline` チップがプレフィックス安定性をリアルタイムで表示し、変更がキャッシュ予算を破壊しようとしているのが見えます。

**回復付きの自由。** 毎ターンの side-git スナップショット（リポジトリの `.git` には触れません）。`/restore` と `revert_turn` でワークスペースを即座にロールバック。危険な操作は OS レベルでサンドボックス化。モデルに試させることができます。

その他の機能面：**RLM セッション**（`peek`、`search`、`chunk`、`sub_query_batch` ヘルパー付きのバッチ分析用永続 Python REPL）、**LSP 診断**（編集ごとの rust-analyzer、pyright、tsserver、gopls、clangd からのインラインエラー）、**MCP プロトコル**、**HTTP/SSE ランタイム API**、再起動を超えて存続する**永続タスクキュー**、Zed や他のエディタ向け **ACP アダプター**、**SWE-bench エクスポート**、**インストール可能なスキル**、**テーマピッカー**、**デスクトップ通知**、キャッシュヒット/ミス内訳付きの**ライブコスト追跡**。

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

ビルド済みバイナリは **Linux x64**、**Linux ARM64**（v0.8.8 以降）、**macOS x64**、**macOS ARM64**、**Windows x64** 向けに公開されています。その他のターゲット（musl、riscv64、FreeBSD など）は [ソースからのインストール](#install-from-source) または [docs/INSTALL.md](docs/INSTALL.md) を参照してください。

初回起動時に [DeepSeek API キー](https://platform.deepseek.com/api_keys) の入力を求められます。キーは `~/.deepseek/config.toml` に保存されるため、OS のクレデンシャルプロンプトなしに任意のディレクトリから利用できます。

事前に設定することもできます:

```bash
codewhale auth set --provider deepseek   # ~/.deepseek/config.toml に保存

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

[Scoop](https://scoop.sh) は Windows のパッケージマネージャです。インストール後、次を実行してください:

```bash
scoop install deepseek-tui
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
codewhale exec --auto --output-format stream-json "fix this bug"  # ツール自動承認付きの agentic exec
codewhale exec --resume <SESSION_ID> "follow up"  # 非対話セッションを継続
codewhale --model deepseek-v4-flash "summarize"   # モデルの上書き
codewhale --model auto "fix this bug"             # モデルと推論強度を自動ルーティング
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

モードとモデル自動ルーティングは別物です。`Tab` は Plan / Agent / YOLO
を切り替え、`/model auto` はモデルと thinking レベルを選びます。`/goal`
は現時点ではセッション目標と token 予算の追跡であり、将来の Goal
ワークサーフェスは `--model auto` とは別に扱います。

---

## 設定

ユーザー設定: `~/.deepseek/config.toml`。プロジェクトオーバーレイ: `<workspace>/.deepseek/config.toml`（拒否される項目: `api_key`、`base_url`、`provider`、`mcp_config_path`）。すべてのオプションは [config.example.toml](config.example.toml) にあります。

主な環境変数:

| 変数 | 用途 |
|---|---|
| `DEEPSEEK_API_KEY` | API キー |
| `DEEPSEEK_BASE_URL` | API ベース URL |
| `DEEPSEEK_HTTP_HEADERS` | 任意のモデルリクエストヘッダー |
| `DEEPSEEK_MODEL` | デフォルトモデル |
| `DEEPSEEK_STREAM_IDLE_TIMEOUT_SECS` | ストリームのアイドルタイムアウト秒数 |
| `DEEPSEEK_PROVIDER` | `codewhale`（デフォルト）、`nvidia-nim`、`openai`、`atlascloud`、`wanjie-ark`、`openrouter`、`novita`、`fireworks`、`sglang`、`vllm`、`ollama` |
| `DEEPSEEK_PROFILE` | 設定プロファイル名 |
| `DEEPSEEK_MEMORY` | `on` に設定するとユーザーメモリを有効化 |
| `DEEPSEEK_ALLOW_INSECURE_HTTP=1` | 信頼できるネットワークで非ローカル `http://` API ベース URL を許可 |
| `NVIDIA_API_KEY` / `OPENAI_API_KEY` / `ATLASCLOUD_API_KEY` / `WANJIE_ARK_API_KEY` / `OPENROUTER_API_KEY` / `NOVITA_API_KEY` / `FIREWORKS_API_KEY` / `SGLANG_API_KEY` / `VLLM_API_KEY` / `OLLAMA_API_KEY` | プロバイダー認証 |
| `OPENAI_BASE_URL` / `OPENAI_MODEL` | 汎用 OpenAI 互換エンドポイントとモデル ID |
| `ATLASCLOUD_BASE_URL` / `ATLASCLOUD_MODEL` | AtlasCloud エンドポイントとモデル上書き |
| `WANJIE_ARK_BASE_URL` / `WANJIE_ARK_MODEL` | Wanjie Ark エンドポイントとモデル上書き |
| `OPENROUTER_BASE_URL` | OpenRouter エンドポイント上書き |
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

codewhale はワークスペースのディレクトリ（`.agents/skills` → `skills` → `.opencode/skills` → `.claude/skills`）とグローバルな `~/.deepseek/skills` からスキルを発見します。各スキルは `SKILL.md` ファイルを持つディレクトリです:

```text
~/.deepseek/skills/my-skill/
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

## サポート

CodeWhale は MIT ライセンスで、利用やコントリビューションにスポンサーは必要ありません。
継続的なメンテナンスを支援する最も分かりやすい方法は
[GitHub Sponsors](https://github.com/sponsors/Hmbown) です。単発の支援は
[Buy Me a Coffee](https://www.buymeacoffee.com/hmbown) からも行えます。

スポンサーは、リリースビルド、CI/ランタイムテスト、パッケージ公開、issue 対応とレビューに使うメンテナー時間を支えます。
機能リクエスト、バグ報告、pull request にスポンサーは必要ありません。

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
- **[lloydzhou](https://github.com/lloydzhou)** と **[jeoor](https://github.com/jeoor)** — コンパクションコストの報告と npm インストーラのストリーム一時停止競合修正 (#1860)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README の明瞭化対応 (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows Scoop インストールドキュメント (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — 料金／割引情報の更新 (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **Hafeez Pizofreude** — `fetch_url` の SSRF 保護と Star History チャート
- **Unic (YuniqueUnic)** — スキーマ駆動の設定 UI（TUI + Web）
- **Jason** — SSRF セキュリティの強化
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — モデル ID の大文字小文字互換性レポート (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — `working...` 状態のバグレポート、Windows クリップボードフォールバック、MCP Streamable HTTP セッション修正、Homebrew tap 自動化 (#738, #850, #1643, #1631)
- **[reidliu41](https://github.com/reidliu41)** — 再開ヒント、ワークスペース信頼の永続化、Ollama プロバイダー対応、thinking-block ストリームの最終処理、CI キャッシュ強化、ストリーミングラップ、DeepSeek モデル補完、ヘルプ選択の改善 (#863, #870, #921, #1078, #1603, #1628, #1601, #1964)
- **[cyq1017](https://github.com/cyq1017)** — Unicode `git_status` パス、ローカル/設定スキル検出、モード切替トーストの重複防止 (#1953, #1956, #1957)
- **[xieshutao](https://github.com/xieshutao)** — プレーン Markdown スキルのフォールバック (#869)
- **[GK012](https://github.com/GK012)** — npm ラッパー `--version` フォールバック (#885)
- **[y0sif](https://github.com/y0sif)** — 直接子サブエージェント完了後の親ターンループ復帰 (#901)
- **[mac119](https://github.com/mac119)** と **[leo119](https://github.com/leo119)** — `codewhale update` コマンドのドキュメント (#838, #917)
- **[dumbjack](https://github.com/dumbjack)** — コマンド安全性の null バイト強化 (#706, #918)
- **macworkers** — フォーク確認と新しいセッション ID (#600, #919)
- **zero** と **[zerx-lab](https://github.com/zerx-lab)** — 通知条件設定と OSC 9 通知本文の拡充 (#820, #920)
- **[chnjames](https://github.com/chnjames)** — @mention 補完キャッシュ、設定リカバリ改善、Windows UTF-8 シェル出力 (#849, #927, #982, #1018)
- **[angziii](https://github.com/angziii)** — 設定安全性、非同期クリーンアップ、Docker 強化、コマンド安全性修正 (#822, #824, #827, #831, #833, #835, #837)
- **[elowen53](https://github.com/elowen53)** — UTF-8 デコードと決定論的テストカバレッジ (#825, #840)
- **[wdw8276](https://github.com/wdw8276)** — カスタムセッションタイトルの `/rename` コマンド (#836)
- **[banqii](https://github.com/banqii)** — `.cursor/skills` 検出パス対応 (#817)
- **[junskyeed](https://github.com/junskyeed)** — API リクエストの動的 `max_tokens` 計算 (#826)
- **[axobase001](https://github.com/axobase001)** — スナップショット孤児クリーンアップ、npm インストールガード、セッションテレメトリ修正、モデルスコープキャッシュクリア、シンボリックリンクスキル対応、npm ミラー迂回ガイダンス、子タスクのプロキシ保持 (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056, #1608)
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` コマンド基盤と SSE gzip/brotli 展開 (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Plan モードの読み取り専用サンドボックス安全性修正 (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — ペースト Enter 自動送信の正確な再現 (#1073)
- **[Duducoco](https://github.com/Duducoco)** と **[AlphaGogoo](https://github.com/AlphaGogoo)** — スキルスラッシュメニューと `/skills` 範囲修正 (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — macOS Terminal.app と ConHost のウィンドウリサイズアーティファクト修正 (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — OpenRouter とカスタムエンドポイントのモデル ID 保持 (#1066)
- **[Jefsky](https://github.com/Jefsky)** — DeepSeek エンドポイント修正レポート (#1079, #1084)
- **[wlon](https://github.com/wlon)** — NVIDIA NIM プロバイダー API キー優先度診断 (#1081)
- **[Horace Liu](https://github.com/liuhq)** — Nix パッケージ対応とインストールドキュメント (#1173)
- **[jieshu666](https://github.com/jieshu666)** — ターミナル再描画のちらつき軽減 (#1563)
- **[gordonlu](https://github.com/gordonlu)** — Windows Enter / CSI-u 入力修正 (#1612)
- **[mdrkrg](https://github.com/mdrkrg)** — 初回起動時の API キー欠落クラッシュ修正 (#1598)
- **[Aitensa](https://github.com/Aitensa)** — diff とページャー出力の CJK 折り返し対応 (#1622)
- **[qiyan233](https://github.com/qiyan233)** — レガシー DeepSeek CN プロバイダーエイリアス互換性 (#1645)
- **[zlh124](https://github.com/zlh124)** — WSL2/ヘッドレス起動レポートとクリップボード初期化修正 (#1772, #1773)
- **[aboimpinto](https://github.com/aboimpinto)** — Windows alt-screen ログ、Home/End コンポーザー、ランタイムログフォローアップ (#1774, #1776, #1748, #1749, #1782, #1783)
- **[LeoLin990405](https://github.com/LeoLin990405)** — プロバイダーモデル透過、推論リプレイ、thinking-only ターン、Windows 引用修正 (#1740, #1743, #1742, #1744)
- **[nightt5879](https://github.com/nightt5879)** — Ctrl+C プロンプト復元修正 (#1764)
- **[h3c-hexin](https://github.com/h3c-hexin)** — ストリーミングバッチツール呼び出し保存と CLI reasoning-effort 透過 (#1686, #1511)
- **[hxy91819](https://github.com/hxy91819)** — ツール結果整理時のプレフィックスキャッシュ保持 (#1514)
- **[JiarenWang](https://github.com/JiarenWang)** — Plan モード読み取り専用強制、承認引継ぎ最適化、Ctrl+H 削除修正、undo コンテキスト同期 (#1123, #962, #958, #1150)
- **[Liu-Vince](https://github.com/Liu-Vince)** — MCP ページネーション、マークダウンインデント保持、zh-Hans i18n 改善、環境変数ドキュメント (#1256, #1179, #1274, #1178)
- **[ChaceLyee2101](https://github.com/ChaceLyee2101)** — 推論トークンコスト集計と zh-Hans 自動 CNY 表示 (#1505, #1504)
- **[laoye2020](https://github.com/laoye2020)** — Catppuccin、Tokyo Night、Dracula、Gruvbox テーマと `/theme` ピッカー (#1534)
- **[punkcanyang](https://github.com/punkcanyang)** — Kitty (OSC 99) と Ghostty (OSC 777) デスクトップ通知対応 (#1426)
- **[Rene-Kuhm](https://github.com/Rene-Kuhm)** — スペイン語 (es-419) ラテンアメリカローカライズ (#1452)
- **[ComeFromTheMars](https://github.com/ComeFromTheMars)** — Shift+Up/Down トランスクリプトスクロールショートカット (#1432)
- **[sockerch](https://github.com/sockerch)** — 全スラッシュコマンドの拼音エイリアス (#1306)
- **[eltociear](https://github.com/eltociear)** — 日本語 README 翻訳 (#746)
- **[Ling](https://github.com/LING71671)** — `grep_files` キャンセルトークン対応と Ctrl+Z コンポーザー下書き復元 (#1839, #1911)
- **[Ben Younes](https://github.com/ousamabenyounes)** — Linux Wayland（非 wlroots）クリップボード対応 (#1938)
- **[Matt Van Horn](https://github.com/mvanhorn)** — Docker 初回起動権限修正とランタイム system prompt 回帰テスト (#1699, #1702)
- **[Kristopher Clark](https://github.com/krisclarkdev)** — compaction の user query 保持修正 (#1704)
- **[tdccccc](https://github.com/tdccccc)** — コンポーザースクロール修正と pager マウスホイール対応 (#1715, #1716)
- **[LittleBlacky](https://github.com/LittleBlacky)** — provider gated `reasoning_content` ストリーム修正 (#1680)
- **[Anaheim](https://github.com/AnaheimEX)** — `rlm_open` 空 source schema 検証レポート (#1712)
- **[THatch26](https://github.com/THatch26)** — ターミナル resize 後のページング修正 (#1724)
- **[Alvin](https://github.com/alvin1)** — Zed ACP id 互換性レポート (#1696)
- **[knqiufan](https://github.com/knqiufan)** — sub-agent ファイル書き込み委譲 (#1833)
- **[IIzzaya](https://github.com/IIzzaya)** — slash 補完の exact alias 優先アイデア (#1811)
- **[DC](https://github.com/duanchao-lab)** — ターミナル cleanup guard のアイデア (#1630)
- **[imkingjh999](https://github.com/imkingjh999)** — provider/model 切り替え修正 (#1642)
- **[Photo](https://github.com/eng2007)** — provider-aware `/model` picker catalog 作業 (#1201)
- **[chennest](https://github.com/chennest)** — diagnostics schema レポート (#1685)
- **[kunpeng-ai-lab](https://github.com/kunpeng-ai-lab)** — Windows コンポーザースクロール修正 (#1578)
- **[WuMing](https://github.com/asdfg314284230)** — Windows PowerShell ちらつき修正 (#1591)
- **[maker316](https://github.com/maker316)** — LoopGuard/checklist ループレポート (#1574)
- **[lalala](https://github.com/lalala-233)** — approval denial 回帰レポート (#1617)
- **[muyuliyan](https://github.com/muyuliyan)** — `pandoc_convert` 検証修正 (#1523)
- **[czf0718](https://github.com/czf0718)** — resize と turn-completion のちらつき修正 (#1537)
- **[MeAiRobot](https://github.com/MeAiRobot)** — toast がコンポーザー入力を覆う問題の修正 (#1485)
- **[tiger-dog](https://github.com/tiger-dog)** — approval modal 折りたたみと markdown identifier 修正 (#1455)
- **[MMMarcinho](https://github.com/MMMarcinho)** — opt-in `image_analyze` vision tool (#1467)
- **[lucaszhu-hue](https://github.com/lucaszhu-hue)** — AtlasCloud provider 統合 (#1436)
- **[sandofree](https://github.com/sandofree)** — Tavily と Bocha の `web_search` backend (#1294)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — `/change` release notes コマンド (#1416)
- **[NorethSea](https://github.com/NorethSea)** — updater companion binary refresh 修正 (#1492)
- **[Jianfengwu2024](https://github.com/Jianfengwu2024)** — Windows MSVC toolchain 環境保持 (#1487)
- **[Fire-dtx](https://github.com/Fire-dtx)** — npm postinstall recoverability 作業 (#1059)
- **[oooyuy92](https://github.com/oooyuy92)** — 長時間セッション palette 可読性レポート (#1070, #936)
- **[qinxianyuzou](https://github.com/qinxianyuzou)** — zh-Hans destructive approval 文言 (#1087, #1091)
- **[tyouter](https://github.com/tyouter)** — session title/history preview クリーンアップ (#1510)
- **[xulongzhe](https://github.com/xulongzhe)** — issue template と vision boundary follow-up (#1530, #1544)
- **[YaYII](https://github.com/YaYII)** — trusted media path 作業 (#1462)
- **[47Cid](https://github.com/47Cid)** と **[Jafar Akhondali](https://github.com/JafarAkhondali)** — 責任ある security disclosure と hardening レポート
- **[linzhiqin2003](https://github.com/linzhiqin2003)** — `--model auto` コスト節約バイアス、実行規律プロンプト、宣言的事実メモリ衛生 (#1385, #1384, #1381)
- **[lbcheng888](https://github.com/lbcheng888)** — 保存/復元間のコスト永続化とトランスクリプトスクロール修正 (#1192, #1211)
- **[pengyou200902](https://github.com/pengyou200902)** — UTF-8 安全メモリ切り捨て、切り捨てマーカー精度、キーバインドドキュメント (#968, #1122, #1095)
- **[CrepuscularIRIS](https://github.com/CrepuscularIRIS)** — Termius/SSH 向け低モーション検出と npx MCP サーバーサンドボックス修正 (#1479, #1346)
- **[sternelee](https://github.com/sternelee)** — DeepSeek プレフィックスキャッシュ安定性追跡 (#1517)
- **[Apeiron0w0](https://github.com/Apeiron0w0)** — Tabby ターミナルちらつきループの FocusGained デバウンス (#1560)
- **[greyfreedom](https://github.com/greyfreedom)** — 最新トランスクリプトへのジャンプボタン (#969)
- **[SamhandsomeLee](https://github.com/SamhandsomeLee)** — 明示的隠しファイルメンション補完 (#1270)
- **[dst1213](https://github.com/dst1213)** — クォータエラー HTTP 400 リトライ (#1203)
- **[fuleinist](https://github.com/fuleinist)** — `--yolo` フラグの CLI から TUI への転送 (#1233)
- **[heloanc](https://github.com/heloanc)** — Home/End キーコンポーザーサポート (#1246)
- **[jinpengxuan](https://github.com/jinpengxuan)** — オンボーディング中のアクティブプロバイダー認証情報保持 (#1265)
- **[lixiasky-back](https://github.com/lixiasky-back)** — 検証済み npm バイナリ採用 (#1339)
- **[J3y0r](https://github.com/J3y0r)** — ワークスペース切り替えコマンド (#1065)
- **[KhalidAlnujaidi](https://github.com/KhalidAlnujaidi)** — delegate スキルバンドル (#1144)
- **[Wenjunyun123](https://github.com/Wenjunyun123)** — ドキュメントアンカーオフセット保持 (#1282)
- **[whtis](https://github.com/whtis)** — zh-CN README ディスパッチャーパス同期 (#1235)
- **[aqilaziz](https://github.com/aqilaziz)** — memory スキルリンク修正 (#1095)
- **[wuwuzhijing](https://github.com/wuwuzhijing)** — rsproxy rustup 回避策インストールドキュメント (#1011)

---

## コントリビューション

[CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。プルリクエストを歓迎します。良い初コントリビューションは [Open Issues](https://github.com/Hmbown/CodeWhale/issues) を確認してください。

> [!Note]
> *DeepSeek Inc. とは関係ありません。*

## ライセンス

[MIT](LICENSE)

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
