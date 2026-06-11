# CodeWhale

> DeepSeek V4 とオープンモデルのためのローカル Agent ハーネス。自己、権威、証拠のループを扱います。

[English README](README.md) · [简体中文 README](README.zh-CN.md) · [Tiếng Việt README](README.vi.md)

![codewhale screenshot](assets/screenshot.png)

## 考え方

多くのコーディング Agent は「もっと強く」から始めます。もっと多くのツール、もっと長いコンテキスト、もっと多い自動化。CodeWhale は責任から始めます。

Agent がリポジトリを編集する前に、まず住所が必要です。このターミナル、このユーザー、このブランチ、このセッション。それが ego の層です。誇示ではなく、継続性。人格の仮面ではなく、責任が結びつく場所です。

その次に法が必要です。実際の作業ディレクトリでは、現在のユーザー意図、リポジトリの指示、Shell 出力、古い記憶、前回の引き継ぎ、安全ポリシー、未完了の変更が同じターンで衝突します。CodeWhale の Constitution は、その衝突に順序を与えます。現在のユーザー要求は古い文脈より上、ライブの証拠は推測より上、検証は自信より上、人格は声だけを決めて行動は決めません。

CodeWhale の本体は、モデルの外側にある順序づけの層です。誰が行動しているのか、どの法に従うのか、どんな証拠があるのか、次の人間や Agent がどう続けられるのかを扱います。

## できること

- ローカルファーストのターミナル TUI。
- ファイル、Shell、Git、Web、MCP、RLM、サブ Agent の型付きツール。
- 承認ゲート、サンドボックス、side-git スナップショット、`/restore` ロールバック。
- 編集後の Language Server 診断フィードバック。
- 並行サブ Agent、永続セッション、fork、relay 引き継ぎ、Runtime API。
- DeepSeek V4 を第一級として扱いながら、OpenRouter、Xiaomi MiMo、NVIDIA NIM、Arcee、SiliconFlow、Fireworks、Novita、自前の SGLang/vLLM、Ollama なども明示的な provider として扱います。

DeepSeek は第一級ですが、唯一の経路ではありません。provider、model、base URL、認証情報は別々の選択です。

## インストール

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
codewhale --model auto
```

他の方法：

```bash
# GitHub Releases にプラットフォーム別アーカイブがあります:
# https://github.com/Hmbown/CodeWhale/releases

# GitHub に安定して到達できない場合は CNB mirror を使えます:
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-tui --locked --force

# 旧 Homebrew 互換。formula はまだ deepseek-tui 名を使います。
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

`codewhale` npm wrapper も `npm install -g codewhale` で利用できます。

Docker、直接ダウンロード、中国ミラー、Windows/Scoop、Nix、チェックサム、トラブルシュートは [docs/INSTALL.md](docs/INSTALL.md) を見てください。

## 最初の起動

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

よく使う入口は `/provider`、`/model`、`/config`、`/statusline`、`/skills`、`/restore` です。入力の先頭に `!` を付けると、通常の承認とサンドボックス経路で Shell コマンドを実行できます。

## 詳細ドキュメント

README は考え方と最短経路だけを持ちます。詳細はドキュメントと [codewhale.net](https://codewhale.net/) にあります。

- [User guide](docs/GUIDE.md)
- [Install guide](docs/INSTALL.md)
- [Configuration](docs/CONFIGURATION.md)
- [Provider registry](docs/PROVIDERS.md)
- [Sub-agents](docs/SUBAGENTS.md)
- [Runtime API](docs/RUNTIME_API.md)
- [Model Lab](docs/MODEL_LAB.md)
- [Architecture](docs/ARCHITECTURE.md)
- [v0.9.0 release acceptance](docs/V0_9_0_RELEASE_ACCEPTANCE.md)

## v0.9.0 トラック

v0.9.0 はまだ統合トラックです。tag、GitHub Release、npm、Cargo、リリース成果物が実際に作成され検証されるまで、公開済みリリースとは呼びません。現在の焦点は relay / 引き継ぎ、転写の落ち着き、コマンドと provider アーキテクチャ、VS Code / GUI Runtime API、HarnessProfile、WhaleFlow、そして貢献者 credit hygiene です。

## 謝辞

このプロジェクトは、増え続けるコントリビューターのコミュニティから助けを得て出荷されています:

v0.8.48 でマージまたは取り込まれた貢献者: **[@cy2311](https://github.com/cy2311)**、**[@LING71671](https://github.com/LING71671)**、**[@axobase001](https://github.com/axobase001)**、**[@dzyuan](https://github.com/dzyuan)**、**[@mvanhorn](https://github.com/mvanhorn)**、**[@malsony](https://github.com/malsony)**、**[@gaord](https://github.com/gaord)**、**[@yuanchenglu](https://github.com/yuanchenglu)**、**[@idling11](https://github.com/idling11)**、**[@h3c-hexin](https://github.com/h3c-hexin)**、**[@AdityaVG13](https://github.com/AdityaVG13)**、**[@Sskift](https://github.com/Sskift)**、**[@cyq1017](https://github.com/cyq1017)**、**[@HUQIANTAO](https://github.com/HUQIANTAO)**、**[@New2Niu](https://github.com/New2Niu)**、**[@AiurArtanis](https://github.com/AiurArtanis)**、**[@Lee-take](https://github.com/Lee-take)**、**[@nightt5879](https://github.com/nightt5879)**、**[@AresNing](https://github.com/AresNing)**、**[@AccMoment](https://github.com/AccMoment)**、**[@reidliu41](https://github.com/reidliu41)**、**[@aboimpinto](https://github.com/aboimpinto)**、**[@zhuangbiaowei](https://github.com/zhuangbiaowei)**、**[@donglovejava](https://github.com/donglovejava)**、**[@hongqitai](https://github.com/hongqitai)**、**[@zlh124](https://github.com/zlh124)**、**[@encyc](https://github.com/encyc)**、**[@Implementist](https://github.com/Implementist)**、**[@lihuan215](https://github.com/lihuan215)**、**[@LeoAlex0](https://github.com/LeoAlex0)**、**[@jimmyzhuu](https://github.com/jimmyzhuu)**、**[@rockyzhang](https://github.com/rockyzhang)**、**[@mo-vic](https://github.com/mo-vic)**、**[@hufanexplore](https://github.com/hufanexplore)**、**[@hoclaptrinh33](https://github.com/hoclaptrinh33)**、**[@BryonGo](https://github.com/BryonGo)**、**[@gordonlu](https://github.com/gordonlu)**、**[@hongchen1993](https://github.com/hongchen1993)**。

報告、再現手順、検証で v0.8.48 を支えてくれた **[@buko](https://github.com/buko)**、**[@yyyCode](https://github.com/yyyCode)**、**[@gaslebinh-glitch](https://github.com/gaslebinh-glitch)**、**[@Dr3259](https://github.com/Dr3259)**、**[@lpeng1711694086-lang](https://github.com/lpeng1711694086-lang)**、**[@VerrPower](https://github.com/VerrPower)**、**[@yan-zay](https://github.com/yan-zay)**、**[@jretz](https://github.com/jretz)**、**[@Neo-millunnium](https://github.com/Neo-millunnium)**、**[@caeserchen](https://github.com/caeserchen)**、**[@T-Phuong-Nguyen](https://github.com/T-Phuong-Nguyen)**、**[@zhyuzhyu](https://github.com/zhyuzhyu)**、**[@0gl20shk0sbt36](https://github.com/0gl20shk0sbt36)**、**[@hatakes](https://github.com/hatakes)**、**[@goodvecn-dev](https://github.com/goodvecn-dev)**、**[@bevis-wong](https://github.com/bevis-wong)**、**[@PurplePulse](https://github.com/PurplePulse)**、**[@nbiish](https://github.com/nbiish)** にも感謝します。

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
