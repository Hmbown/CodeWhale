<!-- source: README.md sha256:e421b94e9f7e -->
# Codewhale

Codewhale 是一款面向终端的开源编程智能体，使用 Rust 构建，并与用户一起在公开协作中不断改进。

![Codewhale 在终端中运行](assets/screenshot.webp)

[English](README.md) · [日本語](README.ja-JP.md) · [Tiếng Việt](README.vi.md) · [Bahasa Indonesia](README.id.md) · [한국어](README.ko-KR.md) · [Español](README.es-419.md) · [Português](README.pt-BR.md) · [Русский](README.ru.md) · [Українська](README.uk.md) · [Français](README.fr.md) · [Deutsch](README.de.md) · [繁體中文](README.zh-TW.md) · [हिन्दी](README.hi.md) · [Türkçe](README.tr.md) · [Italiano](README.it.md) · [Polski](README.pl.md) · [العربية](README.ar.md) · [Català](README.ca.md)

[![CI](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml/badge.svg)](https://github.com/Hmbown/CodeWhale/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/codewhale-cli?label=crates.io)](https://crates.io/crates/codewhale-cli)
[![npm](https://img.shields.io/npm/v/codewhale?label=npm)](https://www.npmjs.com/package/codewhale)
[![Discord](https://img.shields.io/badge/Discord-join-5865F2?logo=discord&logoColor=white)](https://discord.gg/37gfS3ksug)

## 安装

```bash
npm install -g codewhale
codewhale
```

首次运行会帮助你连接提供商，也可以选择保持离线。Codewhale 还支持 Cargo、Docker、Nix、Scoop、预构建压缩包、Android/Termux 和 CNB 镜像。请参阅[安装指南](docs/INSTALL.md)。

每种 shell 只需一条命令即可启用 Tab 补全——`codewhale completion bash|zsh|fish|powershell|elvish`。请参阅 [shell 补全](docs/INSTALL.md#8-shell-completions)。

## 使用

像与队友交流一样向 Codewhale 描述任务：

```text
Fix the failing tests and explain what changed.
```

也可以不打开 TUI，直接运行任务：

```bash
codewhale exec "fix the failing tests and explain what changed"
```

Codewhale 可以读取你的代码仓库、编辑文件、运行命令、检查结果，并持续推进目标。由你决定授予它多少访问权限。

## 为什么选择 Codewhale

- **使用你想要的模型。** 连接托管提供商，或通过 Ollama、vLLM、SGLang 使用本地模型。使用 `/model` 切换提供商和模型。
- **掌控始终在你手中。** Plan 模式为只读。Ask、Auto-Review 和 Full Access 会清晰展示审批行为。`/undo` 可撤销上一轮操作，`/restore` 可将工作区恢复到较早的快照。
- **让长时间任务井然有序。** 保存会话、设置持久的 `/goal`、在工作流运行前进行审查，并协调多个智能体，同时不让其内部指令混入你的对话记录。
- **扩展你已有的智能体。** 连接 MCP 服务器和技能、配置钩子，并将智能体角色作为可读文件保存在项目或个人设置中。

在 TUI 中运行 `/help` 可查看命令和键盘快捷键。

## 安全

Codewhale 在你的机器上运行，并仅拥有你授予的访问权限。审批模式和仓库规则会限制智能体的行为；在支持的平台上，可选的操作系统沙箱可提供更强的执行边界。未知的模型价格会保持显示为未知，而不会被误报为免费。

阅读[授权顺序](docs/AUTHORIZATION_ORDER.md)了解确切的策略层级，阅读[配置](docs/CONFIGURATION.md)了解本地设置。

## 文档

- [提供商和本地模型](docs/PROVIDERS.md)
- [智能体团队](docs/FLEET.md)
- [MCP](docs/MCP.md)、[钩子](docs/HOOKS.md)和[配置](docs/CONFIGURATION.md)
- [本地 Web 客户端](docs/WEB.md)
- [全部文档](docs)

## 加入社区

当人们使用 Codewhale、反馈不顺手之处并帮助修复问题时，它就会变得更好。如果缺少某个提供商、工作流体验不佳，或终端界面妨碍了你，请[提交 issue](https://github.com/Hmbown/CodeWhale/issues)。如果你知道如何改进，请[提交 pull request](CONTRIBUTING.md)。我们欢迎首次贡献，贡献者也会保留已合入工作的署名。

加入 [Discord](https://discord.gg/37gfS3ksug)，或在微信添加 Hunter（`hunterbown`）并申请加入 Whale Brothers 群。

## 项目历史

Codewhale 起初名为 `deepseek-tui`，至今仍保留与其配置和会话的兼容性。如今它已不偏向任何提供商，由社区独立维护，也不隶属于任何模型提供商。

感谢每一位贡献者，以及帮助项目成长的开源社区。请参阅[贡献者记录](docs/CONTRIBUTORS.md)。

## 许可证

[MIT](LICENSE)。从其他开源项目改编的部分记录在[第三方声明](docs/THIRD_PARTY_NOTICES.md)中。
