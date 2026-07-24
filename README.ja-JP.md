<!-- source: README.md sha256:797a23968c31 -->
# Codewhale

ターミナルで動くオープンソースのコーディングエージェント — モデルはあなたが持ち込む。

Codewhale は DeepSeek のためのネイティブ体験として始まりました。そこから、コミュニティ主導のプロジェクトへと成長しています。広がり続ける国際的なコミュニティに合い、できるだけ多くのモデルとプロバイダに対応する、ひとつのコーディングハーネスです — オープンモデルを最優先に、ホスト型でもローカルでも、どれかを特別扱いすることはありません。

プロバイダ、モデル、タスクを渡すと、コードを読み、ファイルを編集し、コマンドを実行し、自分の作業を確認して、タスクが完了するかあなたの手が必要になった時点で止まります。タスクの途中でも `/model` でモデルを切り替えられます。対話的な作業には TUI を、スクリプトと CI には `codewhale exec` を。Rust 製、MIT ライセンスで、あなたのマシン上で動きます。

私たちは常にコントリビューターと改善の方法を探しています。使っているモデルやプロバイダが見当たらないとき、あるいは何かが壊れたときは、それを知らせてもらえることが最も役に立つことのひとつです — [コントリビューション](#コントリビューション)を見てください。

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
codewhale web                            # local browser client on 127.0.0.1
```

TUI では、`/model` がプロバイダとモデルをまとめて切り替え、`/fleet` がワーカーのチームを走らせ、`/restore` がターンを取り消します。入力欄がアイドル状態のとき、`Tab` は Plan / Act / Operate を順に切り替え、`Shift+Tab` は Ask / Auto-Review / Full Access の権限スタンスを順に切り替えます。`!` は Shell コマンドを通常の承認経路で実行します。

## できること

- **どのモデルでも、どのプロバイダでも。** DeepSeek、Claude、GPT、Kimi、GLM をはじめ 30 以上のプロバイダ、そしてキー不要のあなた自身の vLLM・SGLang・Ollama が、すべてひとつのランタイムとひとつのツール群を通って動きます。コンテキスト予算と価格は実際のルートに由来し、不明な価格は $0 ではなく不明と表示されます。
- **許可するまでは読み取り専用。** Plan モードはファイルを変更せず、リスクのあるコマンドは承認でゲートされます。OS サンドボックスが実際にコマンドをラップするとき、Codewhale はそれを明示します。macOS では利用可能な Seatbelt、Linux ではオプトインの bubblewrap です。リポジトリの `constitution.json` は書き込みホールドへとコンパイルされ、Full Access でもスキップできません。
- **再開できる作業。** Fleet はすべてのステップを追記専用の台帳に記録するので、`fleet resume` で止めたところから再開できます。

## さらに詳しく

- [docs/PROVIDERS.md](docs/PROVIDERS.md) — ホスト型・ゲートウェイ・ローカル
  まで、すべてのプロバイダルート
- [docs/FLEET.md](docs/FLEET.md) — Fleet、台帳、再開
- [docs/CONFIGURATION.md](docs/CONFIGURATION.md) — `config.toml`、フック、
  constitution
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

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date)
