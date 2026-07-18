 # Issue #4407 分析文档：工件类型技能的运行时就绪状态报告
 
 **Issue:** [v0.9.1: Report artifact-skill readiness and define a managed dependency runtime](https://github.com/Hmbown/CodeWhale/issues/4407)
 **作者:** Hunter Bown (`@Hmbown`)
 **创建时间:** 2026-07-16
 **状态:** `OPEN`
 **标签:** `bug`, `enhancement`, `workflow-runtime`, `tui`
 
 ---
 
 ## 出了什么问题
 
 CodeWhale 内置了四个工件类型的工作流配方（recipe）——演示文稿、电子表格、PDF 和文档。用户可以在 `/skills` 界面中看到它们列为 **Available / Built-in**（可用/内置），但实际上它们是**依赖条件性的**——要真正跑起来，宿主机需要额外安装 Python、Node.js、Poppler、LibreOffice 等外部工具。
 
 **问题在于：用户没有任何办法判断自己的机器是否具备运行这些配方的条件。**
 
 界面宣称它们是"可用"的，但如果缺少依赖，用户点击后只会得到失败体验，没有前置提示，也没有诊断手段。
 
 ---
 
 ## 细化：实际哪里有问题、哪里没有问题
 
 v0.9.0 审计中曾将这四个技能全部标记为"损坏"，但这个判断过于粗糙。实际情况分三类：
 
 | 技能 | 实际状况 |
 |------|----------|
 | **文档**（结构写作、CSV 分析） | **可用**：不需要额外依赖，当前宿主就能跑 |
 | **PDF 读取** | **可用**：CodeWhale 内置了 Rust PDF 提取器，不需要 Poppler/Python |
 | **XLSX/PPTX 创作**、**高级 PDF/文档渲染** | **需要安装依赖**：可能需要 Python、Node.js、Poppler、LibreOffice |
 
 所以真正的缺陷不是"技能坏了"，而是：
 
 1. **运行时就绪状态缺失**——没有探测机制，没有结果展示，用户面对的是一个黑箱。
 2. **内置/可用标签误导**——"Built-in" 对用户暗示即开即用，但实际上可能是需要手动配环境的。
 3. **诊断工具不覆盖**——`doctor` 命令（人工模式）可以探测部分宿主机工具，但 `doctor --json` 没有把探测结果映射到具体技能上。
 
 ---
 
 ## 为什么不能简单地把 Codex 的技能复制过来
 
 Codex 也有一组同名的文档/电子表格/演示文稿技能，但它们跑在 Codex 自己的**托管运行时（managed runtime）**之上——那个运行时不是 CodeWhale 的一部分。直接把技能文件复制过来不能解决问题。
 
 另外，**在自己的维护机装上 Python 包能让自己跑起来，但解决不了产品层面的缺口**：CodeWhale 有 npm、Cargo、二进制包、Docker 四种分发渠道，其他用户的机器上没有这些依赖，装到维护机只是把问题藏起来了。
 
 ---
 
 ## 问题的影响面
 
 1. **用户体验断层**：用户发现一个新技能 → 尝试使用 → 失败 → 没有提示原因。这直接降低了对产品的信任。
 2. **诊断空白**：`doctor` 没有映射到技能级别，调试无从下手。
 3. **Docker 用户完全暴露**：Docker 镜像只打了 Rust 二进制，没有 Python/Node/Poppler/LibreOffice，四个技能全部不可用但不报错。
 4. **审计误判**：v0.9.0 审计中因为缺少就绪检测，无法区分"真的坏了"和"缺运行时"，导致错误地将所有四个技能标记为损坏。
 
 ---
 
 ## 解决方案的范围
 
 Issue 提出了以下修复范围：
 
 ### 核心功能
 
 1. **为每个内置技能添加确定性就绪探测**，状态分为三类：
    - `ready`（就绪——依赖满足，可以运行）
    - `partial`（部分就绪——部分功能可用）
    - `needs_setup`（需要设置——缺少关键依赖）
 
 2. **在 `/skills` 和 `doctor --json` 中暴露探测结果**，不要把依赖不全的配方说成即开即用。
 
 3. **明确"内置技能"的定义**：它们是依赖条件性的工作流指南，不是自包含的可执行模块。
 
 4. **确定长期运行时方案**：
    - 方案 A：CodeWhale 自己提供校验过的、版本化的、跨平台的工件运行时和加载器
    - 方案 B：明确依赖外部工具，并提供引导安装指南
 
 5. **将 PDF 读取的就绪状态与 PDF 创建/编辑/渲染的就绪状态区分开**——前者已经有 Rust 提取器支撑，后者需要额外依赖。
 
 ### 验收标准
 
 - 干净的 Docker 测试环境能如实报告四个内置工件技能的状态
 - PDF 读取在 Rust 提取器存在时报告 `ready`，即使没有 Poppler/Python
 - XLSX/PPTX 创作在缺少工具时报告 `needs_setup`
 - `doctor --json` 输出每个技能的就绪状态和缺失能力
 - `/skills` 界面不把配方发现和运行时就绪混为一谈
 - 测试使用 mock 命令/模块探测，不全局安装包
 - 如果内置 SKILL.md 内容变更，`BUNDLED_SKILL_VERSION` 同步升级，迁移行为有测试覆盖
 
 ### 明确不做的事
 
 - 不依赖维护者机器上的 Codex 托管运行时
 - 不静默执行 `pip install` 或修改用户的系统 Python
 - 不删除 PDF 读取（它已经是自包含的，没有依赖问题）
 
 ---
 
 ## 涉及的代码文件
 
 - `crates/tui/src/skills/system.rs` —— 内置技能注册入口
 - `crates/tui/assets/skills/{presentations,spreadsheets,pdf,documents}/SKILL.md` —— 四个工件技能配方
 - `crates/tui/src/tools/file.rs` —— PDF 提取工具（Rust 实现，自包含）
 - `crates/tui/src/config.rs` —— 可能涉及 `doctor --json` 的结构扩展
 
 ---
 
 ## 总结：一句话
 
 **CodeWhale 把四个靠外部依赖才能跑的技能标成了"内置/可用"，但没有告诉用户你的机器能不能跑。这是信息鸿沟，不是代码坏了。** 修复方向是给每个技能加一个就绪探测，把结果展示给用户，同时想清楚长期怎么管理这些外部运行时依赖。
