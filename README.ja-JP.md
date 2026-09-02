<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale は Rust で構築された、ターミナル向けのオープンソース・コーディングエージェントです。利用者とともに、公開の場で改善を続けています。

![ターミナルで動作する Codewhale](assets/screenshot.webp)

[English](README.md) · [简体中文](README.zh-CN.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## インストール

```bash
npm install -g codewhale
codewhale
```

初回起動時にプロバイダーへの接続を案内しますが、オフラインのまま使うこともできます。Codewhale は Cargo、Docker、Nix、Scoop、ビルド済みアーカイブ、Android/Termux、CNB ミラーにも対応しています。詳しくは[インストールガイド](docs/INSTALL.md)をご覧ください。

各シェルの Tab 補完はコマンド一つで設定できます — `codewhale completion bash|zsh|fish|powershell|elvish`。詳しくは[シェル補完](docs/INSTALL.md#8-shell-completions)をご覧ください。

## 使い方

チームメイトに話しかけるのと同じように、Codewhale に依頼します：

```text
Fix the failing tests and explain what changed.
```

TUI を開かずにタスクを実行することもできます：

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale はリポジトリを読み、ファイルを編集し、コマンドを実行して結果を確認しながら、目標に向かって作業を続けます。どこまでアクセスを許可するかは、あなたが決められます。

## Codewhale を選ぶ理由

- **使いたいモデルを選べます。** ホスト型プロバイダーに接続するほか、Ollama、vLLM、SGLang 経由でローカルモデルも利用できます。`/model` でプロバイダーとモデルを切り替えられます。
- **主導権を保てます。** Plan は読み取り専用です。Ask、Auto-Review、Full Access により、承認の挙動が明確になります。`/undo` は直前のターンを取り消し、`/restore` はワークスペースを以前のスナップショットへ戻します。
- **長い作業も整理できます。** セッションを保存し、永続的な `/goal` を設定し、ワークフローを実行前に確認できます。さらに、エージェントの内部指示を会話履歴に混ぜることなく、複数のエージェントを連携させられます。
- **今あるエージェントを拡張できます。** MCP サーバーやスキルを接続し、フックを設定し、エージェントの役割をプロジェクトまたは個人設定内の読みやすいファイルとして管理できます。

コマンドとキーボードショートカットは、TUI で `/help` を実行して確認できます。

## 安全性

Codewhale は、あなたが許可した範囲のアクセス権で、あなたのマシン上で動作します。承認モードとリポジトリのルールがエージェントの操作を制限し、対応環境では任意の OS サンドボックスがさらに強固な実行境界を加えます。不明なモデル料金は、無料と表示せず不明のまま扱います。

正確なポリシーの適用順序は[認可の順序](docs/AUTHORIZATION_ORDER.md)、ローカル設定は[設定ガイド](docs/CONFIGURATION.md)をご覧ください。

## ドキュメント

- [プロバイダーとローカルモデル](docs/PROVIDERS.md)
- [エージェントチーム](docs/FLEET.md)
- [MCP](docs/MCP.md)、[フック](docs/HOOKS.md)、[設定](docs/CONFIGURATION.md)
- [ローカル Web クライアント](docs/WEB.md)
- [すべてのドキュメント](docs)

## コミュニティに参加

Codewhale は、実際に使い、違和感を報告し、修正を手伝ってくださる皆さんとともに成長します。必要なプロバイダーがない、ワークフローが使いづらい、ターミナル UI が作業を妨げるといった場合は、[issue を作成](https://github.com/Hmbown/CodeWhale/issues)してください。改善方法をご存じなら、[pull request を作成](CONTRIBUTING.md)してください。初めてのコントリビューションも歓迎し、採用された成果にはコントリビューターのクレジットを残します。

[Discord](https://discord.gg/37gfS3ksug) に参加するか、WeChat で Hunter（`hunterbown`）を追加して Whale Brothers グループへの参加を依頼してください。

## プロジェクトの沿革

Codewhale は `deepseek-tui` として始まり、その設定とセッションとの互換性を現在も維持しています。今ではプロバイダーに依存せず、独立して保守されており、いかなるモデルプロバイダーとも提携していません。

すべてのコントリビューターと、プロジェクトの成長を支えたオープンソースコミュニティに感謝します。[コントリビューターの記録](docs/CONTRIBUTORS.md)もご覧ください。

## ライセンス

[MIT](LICENSE)。他のオープンソースプロジェクトを基にした部分は[サードパーティー通知](docs/THIRD_PARTY_NOTICES.md)に記載しています。
