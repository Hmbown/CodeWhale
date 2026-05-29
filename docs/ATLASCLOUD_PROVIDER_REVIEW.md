# Atlas Cloud Provider Review

## 改了什么

- 补齐 `crates/agent/src/lib.rs` 中 Atlas Cloud 的静态 `ModelRegistry` 条目。
- 让 CLI 在 `--provider atlascloud` 场景下能正确回落到
  `deepseek-ai/deepseek-v4-flash`，并支持 `deepseek-v4-pro` /
  `deepseek-v4-flash` 别名解析。
- 为 Atlas Cloud 增加静态模型解析测试，避免后续 provider 回归。
- 在根目录 `.env.example` 中补充 Atlas Cloud 的本地配置模板。
- 在 `README.md` 增加 Atlas Cloud 的 Logo、接入介绍和带 UTM 的官方链接。
- 在 `docs/PROVIDERS.md` 补充 Atlas Cloud 的静态模型注册说明。

## 设计取舍

- 保持现有 `atlascloud` provider ID、环境变量命名和 OpenAI-compatible 接入方式不变。
- 静态默认模型与现有配置默认值保持一致，统一使用
  `deepseek-ai/deepseek-v4-flash`，避免 CLI 解析结果和配置文档不一致。
- README 改动控制在现有 “Other API Providers” 区块内，不新增复杂结构。

## 本地测试

- Rust 单测：
  - 运行 `/Users/zby/.cargo/bin/cargo test -p codewhale-agent`
  - 结果：`22 passed; 0 failed`
- 项目内最小链路验证：
  - 运行 `/Users/zby/.cargo/bin/cargo run -p codewhale-tui -- exec 'Reply with OK only'`
  - Provider 环境：`atlascloud`
  - 结果：成功返回 `OK`
- Atlas Cloud 实网验证（临时脚本与命令，仅本地使用，不提交到仓库）：
  - `GET /v1/models` 成功返回模型列表，实测可见 `deepseek-ai/deepseek-v4-flash`、`openai/gpt-4o-mini`、`anthropic/claude-opus-4.7` 等模型
  - `POST /v1/chat/completions` with `deepseek-ai/deepseek-v4-flash` 成功返回 `OK`
  - `POST /v1/chat/completions` with `openai/gpt-4o-mini` 成功返回 `OK`
  - `POST /v1/chat/completions` with `anthropic/claude-sonnet-4.5-20250929` 成功返回 `OK`
  - `stream=true` 实测可收到流式 chunk，并最终输出 `OK`

## 本地密钥

- Atlas Cloud API Key 按你的要求仅保存在本地环境文件中，不会提交到 Git。
