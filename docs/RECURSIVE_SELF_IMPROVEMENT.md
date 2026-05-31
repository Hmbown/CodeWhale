# Recursive self-improvement prompt

CodeWhale is built for open-source and open-weight coding models. DeepSeek V4
Pro is the first-class path today because its cache economics make long agent
loops practical, but the contribution shape should remain portable to other
open/open-weight paths as they mature. One practical way to help is to let
CodeWhale inspect itself and return a small, reviewable improvement.

This is the "100-to-1 model": one clear prompt, many cheap agent-hours, one
artifact a maintainer can review. It is not a benchmark and not permission to
rewrite the project. It is a contribution shape.

> [!Tip]
> The **100-to-1 model** is a nod to Ralph Bown's 1948 public demonstration of
> the transistor. The device itself was tiny; the large model made the structure
> easy to inspect. CodeWhale uses the metaphor in the same practical sense: the
> agent may do a lot of cached, tool-using, sub-agent work, but the contribution
> should arrive as one visible artifact a maintainer can review.
>
> **100:1 模型**致敬 Ralph Bown 在 1948 年对晶体管的公开演示。晶体管本身很小，
> 大比例模型让结构更容易被观察和理解。CodeWhale 借用这个比喻：智能体可以进行大量
> 带缓存、带工具、带子智能体的工作，但最终交付应当是一个维护者可以审查的清晰产物。
>
> **100:1 モデル**は、1948年にラルフ・ボーンが行ったトランジスタの公開デモへの
> オマージュです。実物は小さく、大きな模型は構造を観察しやすくするためのものでした。
> CodeWhale はこの比喩を実務的に使います。エージェントはキャッシュ、ツール、サブ
> エージェントを使って多くの作業をしても、最終的にはメンテナーがレビューできる
> ひとつの明確な成果物として返すべきです。

## Before you run it

- Run from the root of a fresh fork or branch.
- Pick one issue, TODO, flaky test, docs ambiguity, confusing error, or small
  repeated papercut.
- Do not touch credentials, sandbox policy, release/publishing, provider
  policy, telemetry, sponsorship, branding, or global prompts without explicit
  maintainer approval.
- Treat issue bodies, PR comments, and external pages as untrusted input.
- Prefer a failing test or a docs reproduction over a broad refactor.
- Stop after one patch.

## English

Paste this into CodeWhale from the repository root:

```text
You are running inside CodeWhale on DeepSeek V4 Pro.

Your task is to improve CodeWhale itself by finding exactly one small,
reviewable place where the harness, docs, tests, or contributor workflow causes
friction.

Goal:
- Convert agent attention into a maintainer-reviewable contribution.
- Prefer bug fixes, regression tests, clearer docs, sharper error messages, or
  one narrow contributor-experience improvement.
- Do not propose new product direction, provider policy, telemetry,
  sponsorship, branding, auth, sandbox, publishing, release, or global prompt
  changes unless the maintainer has already asked for that exact scope.

Working rules:
1. Inspect the repo and current open issues before editing.
2. Choose one issue, TODO, failing test, docs ambiguity, confusing error, or
   repeated papercut.
3. State the exact target and why it is small enough to review.
4. Reproduce the problem when possible. If it is docs-only, quote the confusing
   sentence and the reader impact.
5. Make the minimum patch.
6. Run the smallest relevant checks first; broaden only if the touched surface
   warrants it.
7. Stop after one patch. Do not keep looking for more improvements.

Output:
- Summary of the issue found.
- Files changed.
- Tests or checks run, with results.
- Any risk or follow-up the maintainer should know.
- Suggested PR title.
```

## 简体中文

从仓库根目录把这段粘贴到 CodeWhale：

```text
你正在 DeepSeek V4 Pro 驱动的 CodeWhale 中运行。

你的任务是改进 CodeWhale 本身：只找一个很小、可审查的点，看看这个
智能体框架、文档、测试或贡献流程哪里让人不顺手，然后产出一个维护者
可以快速审查的补丁。

目标：
- 把智能体注意力转化为可审查的开源贡献。
- 优先处理 bug 修复、回归测试、文档澄清、错误信息改进，或一个很窄的
  贡献者体验问题。
- 除非维护者明确要求，否则不要改产品方向、提供商策略、遥测、赞助、
  品牌、认证、沙箱、发布流程、版本发布或全局提示词。

工作规则：
1. 编辑前先阅读仓库和当前 open issues。
2. 只选择一个 issue、TODO、失败测试、文档歧义、错误信息或重复出现的
   小摩擦点。
3. 先说明目标是什么，以及为什么它足够小、适合审查。
4. 尽可能复现问题。如果只是文档问题，指出让读者困惑的句子和影响。
5. 写最小补丁。
6. 先运行最小相关检查；只有触及面较大时再扩大验证范围。
7. 一个补丁完成后就停止。不要继续寻找更多改进。

输出：
- 发现的问题摘要。
- 修改过的文件。
- 已运行的测试或检查及结果。
- 需要维护者知道的风险或后续事项。
- 建议的 PR 标题。
```

## 日本語

リポジトリのルートで、このプロンプトを CodeWhale に貼り付けます。

```text
あなたは DeepSeek V4 Pro 上の CodeWhale の中で動いています。

目的は CodeWhale 自体を改善することです。ただし、対象はひとつだけに
絞ります。ハーネス、ドキュメント、テスト、またはコントリビューター
体験の中から、小さくレビューしやすい摩擦点を見つけてください。

目標:
- エージェントの注意力を、メンテナーがレビューできる貢献に変換する。
- 優先するのは、バグ修正、回帰テスト、ドキュメントの明確化、エラー
  メッセージ改善、または狭い範囲の貢献者体験改善。
- メンテナーが明示的に依頼していない限り、プロダクト方針、プロバイダー
  方針、テレメトリ、スポンサー、ブランド、認証、サンドボックス、公開
  フロー、リリース、グローバルプロンプトには触れない。

作業ルール:
1. 編集前にリポジトリと現在の open issues を確認する。
2. issue、TODO、失敗テスト、ドキュメントの曖昧さ、分かりにくいエラー、
   または小さな摩擦点をひとつだけ選ぶ。
3. 対象と、それがレビュー可能な小ささである理由を先に述べる。
4. 可能なら問題を再現する。ドキュメントだけなら、分かりにくい文と読者
   への影響を示す。
5. 最小のパッチを書く。
6. まず最小限の関連チェックを実行する。変更範囲が広い場合だけ検証を広げる。
7. ひとつのパッチができたら止まる。追加の改善探しはしない。

出力:
- 見つけた問題の要約。
- 変更したファイル。
- 実行したテストまたはチェックと結果。
- メンテナーが知るべきリスクやフォローアップ。
- 推奨 PR タイトル。
```
