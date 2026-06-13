# CodeWhale

> 面向 DeepSeek V4 和开放模型的本地 Agent 运行框架：自我、权威、证据闭环。

[English README](README.md) · [日本語 README](README.ja-JP.md) · [Tiếng Việt README](README.vi.md)

![codewhale screenshot](assets/screenshot.png)

## 核心想法

多数编程 Agent 从“更强”开始：更多工具、更长上下文、更多自动化。CodeWhale 从责任开始。

Agent 改仓库前，先要有一个地址：这个终端、这个用户、这个分支、这个会话。这就是 ego 层。不是炫耀，而是连续性；不是人格面具，而是责任落点。

然后它需要法律。真实工作区是一组冲突来源：用户当前意图、仓库规则、Shell 输出、旧记忆、上一次交接、安全策略和未完成改动都会挤在同一轮里。CodeWhale 用 Constitution 给这些来源排出顺序：当前用户请求高于旧上下文；实时证据高于假设；验证高于自信；个性只影响语气，不决定行为。

CodeWhale 的产品本质是模型外面的排序层：谁在行动、听谁的 law、有什么证据，以及下一个人或 Agent 如何继续。

## 已经具备的能力

- 本地优先的终端 TUI；
- 文件、Shell、Git、Web、MCP、RLM、子 Agent 等带 schema 的工具；
- 审批门、沙箱、side-git 快照和 `/restore` 回滚；
- 编辑后的语言服务器诊断反馈；
- 并发子 Agent、持久会话、fork、relay 交接和运行时 API；
- DeepSeek V4 一等支持，同时保留 OpenRouter、Xiaomi MiMo、NVIDIA NIM、Arcee、SiliconFlow、Fireworks、Novita、自托管 SGLang/vLLM、Ollama 等显式 provider 路由。

DeepSeek 是一等路径，不是唯一边界。provider、model、base URL 和凭据是分开的选择。

## 安装

```bash
cargo install codewhale-cli --locked
cargo install codewhale-tui --locked
codewhale --version
codewhale --model auto
```

其他路径：

```bash
# GitHub Releases 提供平台归档包：
# https://github.com/Hmbown/CodeWhale/releases

# 如果 GitHub 访问不稳定，可以使用 CNB 镜像：
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-cli --locked --force
cargo install --git https://cnb.cool/codewhale.net/codewhale --tag v0.8.58 codewhale-tui --locked --force

# 旧 Homebrew 兼容路径，formula 仍使用 deepseek-tui 名称
brew tap Hmbown/deepseek-tui
brew install deepseek-tui
```

`codewhale` npm wrapper 也可通过 `npm install -g codewhale` 安装。

Docker、直接下载、中国大陆镜像、Windows/Scoop、Nix、校验和和故障排查见 [docs/INSTALL.md](docs/INSTALL.md)。

## 第一次运行

```bash
codewhale auth set --provider deepseek
codewhale auth status
codewhale doctor
codewhale
```

常用入口：`/provider`、`/model`、`/config`、`/statusline`、`/skills`、`/restore`。在输入框前加 `!` 可以通过正常审批和沙箱路径运行 Shell 命令。

## 更多文档

README 只保留概念和最快路径。细节放在文档和 [codewhale.net](https://codewhale.net/)：

- [用户指南](docs/GUIDE.md)
- [安装指南](docs/INSTALL.md)
- [配置和仓库 constitution](docs/CONFIGURATION.md)
- [Provider 注册表](docs/PROVIDERS.md)
- [子 Agent](docs/SUBAGENTS.md)
- [Runtime API](docs/RUNTIME_API.md)
- [Model Lab](docs/MODEL_LAB.md)
- [架构](docs/ARCHITECTURE.md)
- [v0.9.0 发布验收](docs/V0_9_0_RELEASE_ACCEPTANCE.md)

## v0.9.0 轨道

v0.9.0 仍是集成轨道，只有在 tag、GitHub Release、npm、Cargo 和发布产物都真实切出并验证后才算发布。当前重点包括 relay/交接、转录降噪、命令和 provider 架构、VS Code/GUI runtime API、HarnessProfile、WhaleFlow，以及贡献者 credit hygiene。

## 致谢

- **[DeepSeek](https://github.com/deepseek-ai)** — 感谢 DeepSeek 提供模型与支持，让每一次交互成为可能。
- **[DataWhale](https://github.com/datawhalechina)** — 感谢 DataWhale 的支持，并欢迎我们加入“鲸兄弟”大家庭。
- **[OpenWarp](https://github.com/zerx-lab/warp)** — 感谢 OpenWarp 优先支持 codewhale，并一起打磨更好的终端智能体体验。
- **[Open Design](https://github.com/nexu-io/open-design)** — 感谢 Open Design 对面向设计的智能体工作流提供支持与协作。

本项目由不断壮大的贡献者社区共同打造：

v0.8.48 合并或吸收的贡献者包括：**[@cy2311](https://github.com/cy2311)**、**[@LING71671](https://github.com/LING71671)**、**[@axobase001](https://github.com/axobase001)**、**[@dzyuan](https://github.com/dzyuan)**、**[@mvanhorn](https://github.com/mvanhorn)**、**[@malsony](https://github.com/malsony)**、**[@gaord](https://github.com/gaord)**、**[@yuanchenglu](https://github.com/yuanchenglu)**、**[@idling11](https://github.com/idling11)**、**[@h3c-hexin](https://github.com/h3c-hexin)**、**[@AdityaVG13](https://github.com/AdityaVG13)**、**[@Sskift](https://github.com/Sskift)**、**[@cyq1017](https://github.com/cyq1017)**、**[@HUQIANTAO](https://github.com/HUQIANTAO)**、**[@New2Niu](https://github.com/New2Niu)**、**[@AiurArtanis](https://github.com/AiurArtanis)**、**[@Lee-take](https://github.com/Lee-take)**、**[@nightt5879](https://github.com/nightt5879)**、**[@AresNing](https://github.com/AresNing)**、**[@AccMoment](https://github.com/AccMoment)**、**[@reidliu41](https://github.com/reidliu41)**、**[@aboimpinto](https://github.com/aboimpinto)**、**[@zhuangbiaowei](https://github.com/zhuangbiaowei)**、**[@donglovejava](https://github.com/donglovejava)**、**[@hongqitai](https://github.com/hongqitai)**、**[@zlh124](https://github.com/zlh124)**、**[@encyc](https://github.com/encyc)**、**[@Implementist](https://github.com/Implementist)**、**[@lihuan215](https://github.com/lihuan215)**、**[@LeoAlex0](https://github.com/LeoAlex0)**、**[@jimmyzhuu](https://github.com/jimmyzhuu)**、**[@rockyzhang](https://github.com/rockyzhang)**、**[@mo-vic](https://github.com/mo-vic)**、**[@hufanexplore](https://github.com/hufanexplore)**、**[@hoclaptrinh33](https://github.com/hoclaptrinh33)** 、**[@BryonGo](https://github.com/BryonGo)**、**[@gordonlu](https://github.com/gordonlu)** 和 **[@hongchen1993](https://github.com/hongchen1993)**。

同样感谢提供报告、复现和验证的 **[@buko](https://github.com/buko)**、**[@yyyCode](https://github.com/yyyCode)**、**[@gaslebinh-glitch](https://github.com/gaslebinh-glitch)**、**[@Dr3259](https://github.com/Dr3259)**、**[@lpeng1711694086-lang](https://github.com/lpeng1711694086-lang)**、**[@VerrPower](https://github.com/VerrPower)**、**[@yan-zay](https://github.com/yan-zay)**、**[@jretz](https://github.com/jretz)**、**[@Neo-millunnium](https://github.com/Neo-millunnium)**、**[@caeserchen](https://github.com/caeserchen)**、**[@T-Phuong-Nguyen](https://github.com/T-Phuong-Nguyen)**、**[@zhyuzhyu](https://github.com/zhyuzhyu)**、**[@0gl20shk0sbt36](https://github.com/0gl20shk0sbt36)**、**[@hatakes](https://github.com/hatakes)**、**[@goodvecn-dev](https://github.com/goodvecn-dev)**、**[@bevis-wong](https://github.com/bevis-wong)**、**[@PurplePulse](https://github.com/PurplePulse)** 和 **[@nbiish](https://github.com/nbiish)**。

- **[merchloubna70-dot](https://github.com/merchloubna70-dot)** — 28 个 PR，涵盖功能、修复和 VS Code 扩展基础架构 (#645–#681)
- **[WyxBUPT-22](https://github.com/WyxBUPT-22)** — Markdown 表格、粗体/斜体和水平线渲染 (#579)
- **[loongmiaow-pixel](https://github.com/loongmiaow-pixel)** — Windows + 中国安装文档 (#578)
- **[20bytes](https://github.com/20bytes)** — 用户记忆文档和帮助优化 (#569)
- **[staryxchen](https://github.com/staryxchen)** — glibc 兼容性预检 (#556)
- **[Vishnu1837](https://github.com/Vishnu1837)** — glibc 兼容性改进 (#565)
- **[shentoumengxin](https://github.com/shentoumengxin)** — Shell `cwd` 边界验证 (#524)
- **[toi500](https://github.com/toi500)** — Windows 粘贴修复报告
- **[xsstomy](https://github.com/xsstomy)** — 终端启动重绘报告
- **[melody0709](https://github.com/melody0709)** — 斜杠前缀回车激活报告
- **[lloydzhou](https://github.com/lloydzhou)** 和 **[jeoor](https://github.com/jeoor)** — 压缩成本报告；lloydzhou 还贡献了确定性的环境上下文注入 (#813, #922) 和 KV 前缀缓存稳定化 (#1080)
- **[Agent-Skill-007](https://github.com/Agent-Skill-007)** — README 清晰化改进 (#685)
- **[woyxiang](https://github.com/woyxiang)** — Windows 安装文档 (#696)
- **[wangfeng](mailto:wangfengcsu@qq.com)** — 价格/折扣信息更新 (#692)
- **[zichen0116](https://github.com/zichen0116)** — CODE_OF_CONDUCT.md (#686)
- **[dfwqdyl-ui](https://github.com/dfwqdyl-ui)** — 模型 ID 大小写兼容性报告 (#729)
- **[Oliver-ZPLiu](https://github.com/Oliver-ZPLiu)** — `working...` 卡死状态 Bug 报告和 Windows 剪贴板兜底修复 (#738, #850)
- **[reidliu41](https://github.com/reidliu41)** — 退出后的恢复提示、工作区信任持久化、Ollama provider 支持，以及思考块流式终结修复 (#863, #870, #921, #1078)
- **[xieshutao](https://github.com/xieshutao)** — 纯 Markdown skill 兜底解析 (#869)
- **[GK012](https://github.com/GK012)** — npm wrapper 的 `--version` 兜底 (#885)
- **[y0sif](https://github.com/y0sif)** — 直接子智能体完成后唤醒父级 turn loop (#901)
- **[mac119](https://github.com/mac119)** 和 **[leo119](https://github.com/leo119)** — `codewhale update` 命令文档 (#838, #917)
- **[dumbjack](https://github.com/dumbjack)** / **浩淼的mac** — shell 命令空字节安全加固 (#706, #918)
- **macworkers** — fork 完成后显示新 session id (#600, #919)
- **zero** 和 **[zerx-lab](https://github.com/zerx-lab)** — 通知条件配置和更完整的 OSC 9 通知正文 (#820, #920)
- **[chnjames](https://github.com/chnjames)** — @mention 补全缓存、配置恢复优化，以及 Windows UTF-8 shell 输出修复 (#849, #927, #982, #1018)
- **[angziii](https://github.com/angziii)** — 配置安全、异步清理、Docker 加固和命令安全修复 (#822, #824, #827, #831, #833, #835, #837)
- **[elowen53](https://github.com/elowen53)** — UTF-8 解码和确定性测试覆盖 (#825, #840)
- **[wdw8276](https://github.com/wdw8276)** — 用于自定义 session 标题的 `/rename` 命令 (#836)
- **[banqii](https://github.com/banqii)** — `.cursor/skills` 发现路径支持 (#817)
- **[junskyeed](https://github.com/junskyeed)** — API 请求动态 `max_tokens` 计算 (#826)
- **Hafeez Pizofreude** — `fetch_url` 的 SSRF 保护和 Star History 图表
- **Unic (YuniqueUnic)** — 基于 schema 的配置 UI（TUI + web）
- **Jason** — SSRF 安全加固
- **[axobase001](https://github.com/axobase001)** — 快照孤儿文件清理、npm 安装守卫、会话遥测修复、模型作用域缓存清理、符号链接技能支持，以及 npm 镜像逃生路径指引 (#975, #1032, #1047, #1049, #1052, #1019, #1051, #1056)
- **[MengZ-super](https://github.com/MengZ-super)** — `/theme` 命令基础和 SSE gzip/brotli 解压支持 (#1057, #1061)
- **[DI-HUO-MING-YI](https://github.com/DI-HUO-MING-YI)** — Plan 模式只读沙箱安全修复 (#1077)
- **[bevis-wong](https://github.com/bevis-wong)** — 粘贴-回车自动提交问题的精确复现 (#1073)
- **[Duducoco](https://github.com/Duducoco)** 和 **[AlphaGogoo](https://github.com/AlphaGogoo)** — 技能斜杠菜单和 `/skills` 覆盖范围修复 (#1068, #1083)
- **[ArronAI007](https://github.com/ArronAI007)** — macOS Terminal.app 和 ConHost 窗口大小调整残留修复 (#993)
- **[THINKER-ONLY](https://github.com/THINKER-ONLY)** — OpenRouter 和自定义端点模型 ID 保留 (#1066)
- **[Jefsky](https://github.com/Jefsky)** — `deepseek-cn` 官方端点默认值 (#1079, #1084)
- **[wlon](https://github.com/wlon)** — NVIDIA NIM provider API key 优先级诊断 (#1081)
- **[donglovejava](https://github.com/donglovejava)** — paste @file 整合、CJK panic 修复、用户反馈、RLM 路由、edit_file 重试 (#2154–#2168)
- **[encyc](https://github.com/encyc)** — session token 分解显示和 `/status` (#2152)
- **[saieswar237](https://github.com/saieswar237)** — 审查流程文档 (#2178)
- **[sximelon](https://github.com/sximelon)** — paste Enter 抑制、键盘处理提取 (#2174, #2042)
- **[nanookclaw](https://github.com/nanookclaw)** — search provider 显示在 doctor (#2135)
- **[Sskift](https://github.com/Sskift)** — CLI 默认环境变量覆盖防止 (#2119)
- **[xin1104](https://github.com/xin1104)** — Homebrew codewhale 二进制安装 (#2105)
- **[mrluanma](https://github.com/mrluanma)** — Metaso 搜索提供商 (#2059)
- **[Lellansin](https://github.com/Lellansin)** — 主目录下跳过配置合并 (#2055)
- **[zhuangbiaowei](https://github.com/zhuangbiaowei)** — 更新发布渠道 (#2145)

---

## 贡献

欢迎提交 pull request——请先查看 [CONTRIBUTING.md](CONTRIBUTING.md) 并留意[开放 issue](https://github.com/Hmbown/CodeWhale/issues) 中的好入门任务。

*本项目与 DeepSeek Inc. 无隶属关系。*

## 许可证

[MIT](LICENSE)

## Star 历史

[![Star History Chart](https://api.star-history.com/chart?repos=Hmbown/CodeWhale&type=date&legend=top-left)](https://www.star-history.com/?repos=Hmbown%2FCodeWhale&type=date&logscale=&legend=top-left)
