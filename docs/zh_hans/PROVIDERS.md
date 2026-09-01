# 提供商注册表

> 本文翻译自英文版 [PROVIDERS.md](../PROVIDERS.md)，与英文修订 `a0f5aa504`（2026-08-19）同步。

本注册表描述已接入当前 Codewhale 代码库的提供商行为。它刻意保持保守：随附条目仅限于代码已知的提供商 ID、配置键、认证路径、base URL、模型解析和能力元数据。

DeepSeek 仍是默认提供商，但 `ProviderKind::ALL` 中的每个条目都是一等公民、可选的提供商路由。`ALL` 是目录/选择器表面——每个厂商一个身份。双线协议方言种类（`*Anthropic`，例如 `deepseek-anthropic`）和 Model Studio 套餐变体保留在枚举中用于 serde 和 `provider_for_kind`，但刻意**不**作为目录行：套餐是主提供商配置（`crates/config/src/provider_kind.rs:221-226`）上的 `mode`/`base_url`，方言则是 `wire = openai|anthropic`。托管路由、通用 OpenAI 兼容端点、OpenAI Codex/ChatGPT 路由、原生 Anthropic 以及本地运行时，都在所选提供商/模型/base URL 上运行同一个终端 harness。

初级设置模板（`crates/config/src/provider_templates.rs`）覆盖 OpenCode Zen、OpenCode Go、SenseNova 和 Agnes。Zen/Go 复用下方的一等路由。SenseNova 在 `https://token.sensenova.cn/v1` 上填入一个具名 OpenAI 兼容表，默认模型为 `deepseek-v4-flash`。Agnes 在本仓库中没有已发布的 URL，因此被编目为未发布，并且不会虚构主机。`/provider` 的 `P` 打开列表；`S` 仍然填入 SenseNova；`T` 探测 `/models` 并只记录可达性（2xx 并不代表模型可用）。

需要保持同步的来源：

- `crates/config/src/lib.rs` —— 共享的提供商 ID、默认值、环境变量优先级。
- `crates/tui/src/config.rs` —— TUI 提供商 ID、提供商能力元数据以及提供商特定的环境变量处理。
- `crates/agent/src/lib.rs` —— `codewhale model list` 和 `codewhale model resolve` 使用的静态 `ModelRegistry`。
- `config.example.toml` 和 `docs/CONFIGURATION.md` —— 面向用户的配置示例和环境变量参考。
- `scripts/check-provider-registry.py` —— 对规范提供商 ID、实时 TUI 提供商 ID、TOML 表名、静态注册表行和文档化默认值的漂移检查。

## 提供商选择

规范的提供商 ID 是 `ProviderKind::ALL`（`crates/config/src/provider_kind.rs`）的 42 个条目，按该顺序排列：

`deepseek`, `nvidia-nim`, `openai`, `atlascloud`, `wanjie-ark`, `volcengine`,
`openrouter`, `orcarouter`, `xiaomi-mimo`, `novita`, `fireworks`, `siliconflow`, `arcee`,
`siliconflow-CN`, `moonshot`, `sglang`, `vllm`, `ollama`, `ollama-cloud`, `huggingface`,
`together`, `qianfan`, `openai-codex`, `anthropic`, `openmodel`, `zai`,
`stepfun`, `minimax`, `deepinfra`, `sakana`, `longcat`, `opencode-go`,
`opencode-zen`, `meta`, `xai`, `mistral`, `telecomjs`, `modelstudio-token-plan`,
`google`, `antigravity`, `edenai`, 和 `custom`。

`deepseek-anthropic` *不在*此列表中——它是 `deepseek` 的线协议方言，通过 `wire = "anthropic"` 访问，而不是一个可单独选择的路由。

可以使用以下任一界面选择提供商：

- CLI：`codewhale --provider <id>`
- TUI：`/provider <id>` 或提供商选择器
- 环境变量：`CODEWHALE_PROVIDER=<id>`；`DEEPSEEK_PROVIDER=<id>` 是旧别名
- 配置：`provider = "<id>"`

`deepseek-cn`、`deepseek_china`、`deepseekcn` 和 `deepseek-china` 被接受为 `deepseek` 的旧别名。它们不会选择不同的官方主机；DeepSeek 在全球使用同一个官方 API 主机。

`deepseek_anthropic`、`deepseek-claude` 和 `deepseek_claude` 选择 `deepseek-anthropic`，这是 DeepSeek 的可选路由，在 `https://api.deepseek.com/anthropic` 上使用 Anthropic Messages API。它保留正常的 DeepSeek API 密钥路径，但使用 `x-api-key` 加上 `anthropic-version: 2023-06-01`，而不是 Bearer 认证。如果密钥已经存在于官方 DeepSeek Harness（`dsh`）的 `$DSH_HOME/.credentials.yaml` 中，请使用 `codewhale auth external-consent --provider deepseek --mode read-only` 授予只读访问权限。Codewhale 从不写入该文件，只读取 `DEEPSEEK_API_KEY`。

`huggingface`、`hugging-face`、`hugging_face` 和 `hf` 都选择 Hugging Face Inference Providers 路由。这是用于聊天/推理的 OpenAI 兼容路由器路径，不是 Hub 浏览、模型卡检查、上传或工件导出。

`telecomjs`、`telecom-js`、`telecom_js`、`telecomjs-cn` 和 `tokenhub` 都选择 TelecomJS TokenHub 路由。其经过认证的 `/models` 目录按密钥隔离，并与所有其他提供商的实时快照保持隔离。

新的共享配置写入 `~/.codewhale/config.toml`。出于兼容性，仍然会读取现有的 `~/.deepseek/config.toml` 文件。

### 线协议兼容性

提供商选择是显式的。诸如 `deepseek-ai/...`、`deepseek/...`、`qwen/...` 或 `arcee-ai/...` 之类的模型字符串前缀，是所选提供商下的、由提供商拥有的线协议 ID 或目录命名空间提示。它不是提供商切换，绝不能被视为该路由是 DeepSeek、OpenRouter 或任何其他提供商的证据。

使用 `provider = "<id>"`、`CODEWHALE_PROVIDER=<id>` 或 `codewhale --provider <id>` 设置路由。使用 `CODEWHALE_MODEL`、提供商特定的模型环境变量、顶层 `default_text_model` 或 `[providers.<table>].model` 设置请求模型。使用 `CODEWHALE_BASE_URL`、提供商特定的 base URL 环境变量或 `[providers.<table>].base_url` 设置端点。使用 `codewhale auth set --provider <id>`、`[providers.<table>].api_key` 或列出的提供商环境变量设置认证。

| 提供商 ID | TOML 表 | 线协议 | 认证环境变量 |
| --- | --- | --- | --- |
| `deepseek` | `[providers.deepseek]` | OpenAI Chat Completions | `DEEPSEEK_API_KEY` |
| `deepseek-anthropic` | `[providers.deepseek_anthropic]` | Anthropic Messages | `DEEPSEEK_API_KEY` |
| `nvidia-nim` | `[providers.nvidia_nim]` | OpenAI Chat Completions | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, `DEEPSEEK_API_KEY` |
| `openai` | `[providers.openai]` | OpenAI Chat Completions | `OPENAI_API_KEY` |
| `atlascloud` | `[providers.atlascloud]` | OpenAI Chat Completions | `ATLASCLOUD_API_KEY` |
| `wanjie-ark` | `[providers.wanjie_ark]` | OpenAI Chat Completions | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` |
| `volcengine` | `[providers.volcengine]` | OpenAI Chat Completions | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` |
| `openrouter` | `[providers.openrouter]` | OpenAI Chat Completions | `OPENROUTER_API_KEY` |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | OpenAI Chat Completions | `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`, `MIMO_TOKEN_PLAN_API_KEY`, `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` |
| `novita` | `[providers.novita]` | OpenAI Chat Completions | `NOVITA_API_KEY` |
| `fireworks` | `[providers.fireworks]` | OpenAI Chat Completions | `FIREWORKS_API_KEY` |
| `siliconflow` | `[providers.siliconflow]` | OpenAI Chat Completions | `SILICONFLOW_API_KEY` |
| `arcee` | `[providers.arcee]` | OpenAI Chat Completions | `ARCEE_API_KEY` |
| `siliconflow-CN` | `[providers.siliconflow_cn]` | OpenAI Chat Completions | `SILICONFLOW_API_KEY` |
| `moonshot` | `[providers.moonshot]` | OpenAI Chat Completions | `MOONSHOT_API_KEY`, `KIMI_API_KEY` |
| `sglang` | `[providers.sglang]` | OpenAI Chat Completions | `SGLANG_API_KEY` |
| `vllm` | `[providers.vllm]` | OpenAI Chat Completions | `VLLM_API_KEY` |
| `ollama` | `[providers.ollama]` | 本地 OpenAI 兼容 Chat Completions | `OLLAMA_API_KEY`（可选；仅用于需要认证的本地路由） |
| `ollama-cloud` | `[providers.ollama_cloud]` | 托管 OpenAI 兼容 Chat Completions | `OLLAMA_CLOUD_API_KEY`, `OLLAMA_API_KEY` |
| `huggingface` | `[providers.huggingface]` | OpenAI Chat Completions | `HUGGINGFACE_API_KEY`, `HF_TOKEN` |
| `together` | `[providers.together]` | OpenAI Chat Completions | `TOGETHER_API_KEY` |
| `qianfan` | `[providers.qianfan]` | OpenAI Chat Completions | `QIANFAN_API_KEY`, `BAIDU_QIANFAN_API_KEY` |
| `openai-codex` | `[providers.openai_codex]` | OpenAI Responses | `OPENAI_CODEX_ACCESS_TOKEN`, `CODEX_ACCESS_TOKEN` |
| `anthropic` | `[providers.anthropic]` | Anthropic Messages | `ANTHROPIC_API_KEY` |
| `openmodel` | `[providers.openmodel]` | Anthropic Messages | `OPENMODEL_API_KEY` |
| `zai` | `[providers.zai]` | OpenAI Chat Completions | `ZAI_API_KEY`, `Z_AI_API_KEY` |
| `stepfun` | `[providers.stepfun]` | OpenAI Chat Completions | `STEPFUN_API_KEY`, `STEP_API_KEY` |
| `minimax` | `[providers.minimax]` | OpenAI Chat Completions | `MINIMAX_API_KEY` |
| `deepinfra` | `[providers.deepinfra]` | OpenAI Chat Completions | `DEEPINFRA_API_KEY`, `DEEPINFRA_TOKEN` |
| `sakana` | `[providers.sakana]` | OpenAI Chat Completions | `FUGU_API_KEY`, `SAKANA_API_KEY` |
| `longcat` | `[providers.longcat]` | OpenAI Chat Completions | `LONGCAT_API_KEY` |
| `opencode-go` | `[providers.opencode_go]` | OpenAI Chat Completions | `OPENCODE_GO_API_KEY` |
| `opencode-zen` | `[providers.opencode_zen]` | 模型感知：OpenAI Responses、Anthropic Messages 或 OpenAI Chat Completions | `OPENCODE_ZEN_API_KEY`, `OPENCODE_API_KEY` |
| `meta` | `[providers.meta]` | OpenAI Chat Completions | `META_MODEL_API_KEY`, `MODEL_API_KEY` |
| `telecomjs` | `[providers.telecomjs]` | OpenAI Chat Completions | `TELECOMJS_API_KEY` |
| `xai` | `[providers.xai]` | OpenAI Chat Completions | `XAI_API_KEY` |
| `mistral` | `[providers.mistral]` | OpenAI Chat Completions | `MISTRAL_API_KEY` |
| `google` | `[providers.google]` | OpenAI Chat Completions（官方 Gemini OpenAI 兼容路由；在工具调用时捕获并回放思维签名） | `GOOGLE_API_KEY`, `GEMINI_API_KEY` |
| `antigravity` | `[providers.antigravity]` | 无——请求默认失败关闭；仅凭据导入 | `ANTIGRAVITY_API_KEY`（密钥平面）；`AGY_ADC_AUTH`（进程环境变量） |
| `edenai` | `[providers.edenai]` | OpenAI Chat Completions | `EDENAI_API_KEY` |
| `modelstudio-token-plan` | `[providers.modelstudio_token_plan]` | OpenAI Chat Completions | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` |
| `modelstudio-token-plan-anthropic` | `[providers.modelstudio_token_plan_anthropic]` | Anthropic Messages | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` |
| `modelstudio-coding-plan` | `[providers.modelstudio_coding_plan]` | OpenAI Chat Completions | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` |
| `modelstudio-coding-plan-anthropic` | `[providers.modelstudio_coding_plan_anthropic]` | Anthropic Messages | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` |

每条路由的默认 base URL 和模型列在下方随附的提供商表中。上面的线协议值源自 `crates/config/src/provider.rs`：`ChatCompletions` 是默认值，`openai-codex` 覆盖为 `Responses`；`deepseek-anthropic`、`anthropic` 和 `openmodel` 覆盖为 `AnthropicMessages`；而 `opencode-zen` 根据所选模型的精选供应解析协议。

## 认证与环境变量规则

对于托管提供商，`codewhale auth set --provider <id>` 会为该提供商保存一个 API 密钥。API 密钥环境变量是已保存配置和密钥环凭据之后的回退输入；显式的进程级 `--api-key` 在该次启动中仍然优先。

对于 base URL 和模型选择，优先使用：

- 活动提供商对应的 `CODEWHALE_BASE_URL` / `CODEWHALE_MODEL`。
- 下方列出的提供商特定 base URL/模型环境变量。
- `DEEPSEEK_BASE_URL`、`DEEPSEEK_MODEL` 和 `DEEPSEEK_DEFAULT_TEXT_MODEL` 作为旧别名。

非本地的 `http://` base URL 会被拒绝，除非设置了 `DEEPSEEK_ALLOW_INSECURE_HTTP=1`。环回 HTTP URL 允许用于自托管运行时。

## 自定义 DeepSeek 兼容端点

大多数自定义的 DeepSeek 兼容部署都可以使用现有的提供商 ID。不要创建 `[providers.deepseek_custom]`；提供商表名是固定的。相反，选择最接近的随附路由并覆盖其端点/模型：

- DeepSeek 兼容托管 API：保持 `provider = "deepseek"` 并设置 `[providers.deepseek].base_url` 加上 `[providers.deepseek].model`，或者使用 `DEEPSEEK_BASE_URL` 和 `DEEPSEEK_MODEL` 启动。
- 通用 OpenAI 兼容网关：使用 `provider = "openai"`，设置 `[providers.openai].base_url` 加上 `[providers.openai].model`，或者使用 `OPENAI_BASE_URL` 和 `OPENAI_MODEL` 启动。
- 多个具名 OpenAI 兼容网关，或者你想从 AgentProfile 固定的本地路由，可以使用自定义表，例如 `[providers.lm-studio] kind = "openai-compatible"`，并用 `provider = "lm-studio"` 或配置文件 `provider = "lm-studio"` 选择它。
- 本地 OpenAI 兼容运行时：使用 `provider = "vllm"`、`"sglang"` 或 `"ollama"`，并配上匹配的提供商特定 base URL/模型值。

DeepSeek 兼容主机的用户配置示例：

```toml
provider = "deepseek"

[providers.deepseek]
api_key = "YOUR_API_KEY"
base_url = "https://your-provider.example/v1"
model = "deepseek-ai/DeepSeek-V4-Pro"
```

通用网关的用户配置示例：

```toml
provider = "openai"

[providers.openai]
api_key = "YOUR_GATEWAY_API_KEY"
base_url = "https://gateway.example/v1"
model = "your-deepseek-compatible-model"
```

阿里云百炼（Bailian / DashScope）自 v0.9.4 起是一等提供商，具有两个套餐配置：Token 套餐（个人/团队）和 Coding 套餐。两个套餐都暴露 OpenAI 兼容的 Chat Completions 端点和 Anthropic 兼容的 Messages 端点。

**Token 套餐**（个人和团队共享同一个 AP-Southeast 端点）：

```toml
provider = "modelstudio-token-plan"

[providers.modelstudio_token_plan]
api_key = "YOUR_MODELSTUDIO_API_KEY"
# base_url 默认为 https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1
model = "qwen3.8-max"   # 或 qwen3.8-max-preview | qwen3.7-plus | qwen3.7-max |
                        #    qwen3.6-flash | deepseek-v4-pro | deepseek-v4-flash-0731 |
                        #    glm-5.2
```

**Coding 套餐**（独立的国际端点）：

```toml
provider = "modelstudio-coding-plan"

[providers.modelstudio_coding_plan]
api_key = "YOUR_MODELSTUDIO_API_KEY"
# base_url 默认为 https://coding-intl.dashscope.aliyuncs.com/v1
model = "qwen3.8-max"
```

**Anthropic 兼容方言** —— 两个套餐还暴露原生 Anthropic Messages 路径。使用 `-anthropic` 提供商后缀选择它：

```toml
provider = "modelstudio-token-plan-anthropic"

[providers.modelstudio_token_plan_anthropic]
api_key = "YOUR_MODELSTUDIO_API_KEY"
# base_url 默认为 https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic
model = "qwen3.8-max"
```

从[百炼控制台](https://bailian.console.aliyun.com/)创建或复制一个 Model Studio API 密钥。该 API 密钥在上述四个提供商 ID 之间共享；只有 base URL 和线协议不同。

**思考/推理。** 推理会在 TUI 的 Thinking 视图上以两种方言呈现，依据 Model Studio 的[深度思考文档](https://www.alibabacloud.com/help/en/model-studio/deep-thinking)。

在 OpenAI 兼容路由上，顶层控制是**路由和模型特定**的，Codewhale 失败关闭：只有当配置的 `base_url` 是官方阿里 Chat Completions 主机（`*.maas.aliyuncs.com/compatible-mode/v1`，包括工作区级主机，或 `coding-intl.dashscope.aliyuncs.com/v1`）时，它们才会被发送。同一提供商 ID 上的自定义 `base_url` 会剥离 `thinking`、`enable_thinking`、`preserve_thinking` 和 `reasoning_effort`，因此任意的 OpenAI 兼容网关永远不会收到阿里的方言。在已验证的主机上：

- **混合模型**（`qwen3.7-*`、`qwen3.6-*`、`deepseek-v4*`、`glm-*`、`kimi-k2.6*`）获得 `enable_thinking`：`off` 时为 `false`，否则为 `true`。
- **仅思考模型** —— `qwen3.8-max`（编目为 `thinking: always_on`）、`qwen3.8-max-preview`（提供 effort/预算选项，无开关）和 `kimi-k2.7-code` —— 完全没有启用/禁用开关。发送一个最多只会被忽略。
- 对文档记录为接受它的模型（`qwen3.7-max`/`-plus`、`qwen3.6-max-preview`/`-plus`/`-flash`、`kimi-k2.6*`、`kimi-k2.7-code`）发送 `preserve_thinking`，这样下一轮会保留助手的轨迹。
- `reasoning_effort` 只对具有文档化档位阶梯的两个系列发送 —— `deepseek-v4*` 和 `glm-5`/`5.1`/`5.2` —— 映射到 `high` 或 `max`。

推理以 `delta.reasoning_content` 流式返回。在后续回合中，只会对上述 `preserve_thinking` 模型和仅思考模型重放给提供商；`deepseek-v3.1`、`deepseek-v3.2` 和 `glm-*` 历史保持剥离，直到实时确认 DashScope 接受输入消息中的 `reasoning_content`。（`deepseek-v4*` 无论如何都会重放——DeepSeek 思考模式契约要求每个提供商都这样做。）

在 Anthropic 兼容路由上，思考使用[Anthropic 兼容 Messages API](https://www.alibabacloud.com/help/en/model-studio/anthropic-api-messages)中记录的 `{"type":"enabled","budget_tokens":N}` / `{"type":"disabled"}` 形态，`budget_tokens` 由 effort 级别推导而来。

由 Model Studio 提供的 DeepSeek（`deepseek-v4-pro`、`deepseek-v4-flash-0731`）和 GLM（`glm-5.2`）模型是提供商范围的，不会与第一方 DeepSeek 或智谱/Z.ai 路由冲突。Model Studio 没有发布 `glm-5.3` 条目，因此 Codewhale 不会在此路由上提供它。
按量付费的 workspace-id 模板化尚未进入内置提供商；在该后续跟进添加它之前，请为那个套餐使用自定义提供商条目。

证书损坏或被拦截的私有网关应使用带受信 CA 包的 `SSL_CERT_FILE`。旧版 `insecure_skip_tls_verify = true` 键仍然会被解析，以便 `codewhale doctor` 可以报告过时配置，但提供商客户端会拒绝它，而不是跳过 TLS 证书验证。

将 `provider`、`api_key` 和 `base_url` 保存在用户配置或进程环境中。项目本地配置覆盖层刻意不能设置这些键，因此仓库无法静默地把提示或凭据重定向到另一个端点。

## 本地模型（DS4、Ollama、vLLM、SGLang）

自托管的 OpenAI 兼容运行时是一等路由，默认无密钥——只有当你的服务器需要密钥时才设置 API 密钥。启动你的运行时，然后用 `--provider` / `/provider` 或配置表把 Codewhale 指向它。

| 运行时 | 默认 base URL | 默认模型 | Base URL 覆盖 |
| --- | --- | --- | --- |
| `ollama` | `http://localhost:11434/v1` | 来自 `GET /v1/models` 的实况标签（刷新前：`unknown`） | `OLLAMA_BASE_URL` |
| `vllm` | `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `VLLM_BASE_URL` |
| `sglang` | `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro` | `SGLANG_BASE_URL` |

### DS4（DwarfStar）

[DS4](https://github.com/antirez/ds4/tree/84cc882352757baf628a1776badf7cc54d584e28) 通过 OpenAI 兼容 API 在本地提供 DeepSeek V4 Flash 和 Pro。启动 DS4，然后打开 Codewhale 预填好的、无密钥设置表单：

```bash
./ds4-server --ctx 100000 --kv-disk-dir /tmp/ds4-kv --kv-disk-space-mb 8192
codewhale
# 在 Codewhale 中：/setup provider ds4
```

检查预填的路由并按 Enter 保存。该预设为 100,000 token 的上下文做预算，以匹配该启动命令，并默认使用 Flash 兼容别名。用 `codewhale doctor --probe-local` 显式检查本地路由。

DS4 在服务器启动时加载实际的 GGUF。它的 `deepseek-v4-flash` 和 `deepseek-v4-pro` API id 是兼容别名；更改 `/model` 不会替换驻留的模型。要运行 Pro，请下载受支持的 Pro 权重并按 DS4 的描述启动 `ds4-server -m <pro.gguf> ...`。每当服务器的 `--ctx` 值变化时，更新 `context_window`。

等效配置是：

```toml
provider = "ds4"

[providers.ds4]
kind = "openai-compatible"
base_url = "http://127.0.0.1:8000/v1"
model = "deepseek-v4-flash"
auth_mode = "none"
context_window = 100000
```

Codewhale 为 DS4 复用它现有的 OpenAI 兼容传输和 DeepSeek 推理/工具调用整形。它不会虚构 API 密钥、把 API 别名与已加载的 GGUF 混淆，或静默切换到托管的 DeepSeek 路由。固定的 DS4 [agent-client 契约](https://github.com/antirez/ds4/blob/84cc882352757baf628a1776badf7cc54d584e28/README.md#agent-client-usage)记录了 `/v1` 上的 Chat Completions、DeepSeek 思考重放、流式用量、`max_tokens` 以及无严格工具模式；Codewhale 遵循这些确切的路由事实，而不是从通用网关继承不受支持的能力。面向模型的行为的主要来源是 DeepSeek 官方的[思考模式](https://api-docs.deepseek.com/guides/thinking_mode)、[工具调用](https://api-docs.deepseek.com/guides/tool_calls)和[Chat Completion](https://api-docs.deepseek.com/api/create-chat-completion)契约。固定的 [DeepSeek Harness 适配器](https://github.com/deepseek-ai/deepseek-harness/blob/47f943859bef60e4160492346772ded9b24f765a/packages/llm/llm-deepseek/README.md)只是次要的实现交叉检查；它不是 API 契约。

### Ollama

```bash
ollama serve          # 如果尚未运行
ollama pull <model>   # 例如 deepseek-v4-flash，或你喜欢的任何标签
codewhale --provider ollama --model <model>
```

提供商提示的模型名会原样发送，因此 `--model qwen3:8b` 对 Ollama 已拉取的任何标签都有效。

### Ollama Cloud

Ollama Cloud 是单独的托管提供商。它使用经过认证的 OpenAI 兼容 `/v1/chat/completions` 路由，默认为 `gpt-oss:120b`：

```toml
provider = "ollama-cloud"

[providers.ollama_cloud]
base_url = "https://ollama.com/v1"
model = "gpt-oss:120b"
```

在 [Ollama 账户设置](https://ollama.com/settings/keys)中创建密钥，然后运行 `codewhale auth set --provider ollama-cloud`。对于环境认证，`OLLAMA_CLOUD_API_KEY` 优先于 Ollama 官方的 `OLLAMA_API_KEY`。`OLLAMA_CLOUD_BASE_URL` 和 `OLLAMA_CLOUD_MODEL` 覆盖 Cloud 默认值；任意的提供商自有模型 ID 原样通过。本地 `ollama` 仍然是单独的、默认无密钥的提供商。

兼容性是只读且存在于内存中的：已发布的配置如果选择了 `provider = "ollama"` 且精确规范化的 `[providers.ollama] base_url = "https://ollama.com/v1"` 元组，则运行时按 `ollama-cloud` 处理。只有那个精确元组才能回退到旧版 `ollama` 密钥槽。Codewhale 不会重写配置、复制或删除密钥、迁移相邻路径，也不会让显式的 `ollama-cloud` 路由消耗旧版密钥槽。

### vLLM

```bash
vllm serve <model> --port 8000
# 或：python -m vllm.entrypoints.openai.api_server --model <model> --port 8000
codewhale --provider vllm --model <model>
```

vLLM 的 OpenAI 兼容服务器默认监听 8000 端口，与 Codewhale 的 `VLLM_BASE_URL` 匹配。

### SGLang

```bash
python -m sglang.launch_server --model-path <model> --port 30000
codewhale --provider sglang --model <model>
```

SGLang 的默认端口 30000 与 Codewhale 的 `SGLANG_BASE_URL` 匹配。

### 在配置中固定本地路由

```toml
provider = "ollama"       # 或 "vllm" / "sglang"

[providers.ollama]
model = "qwen3:8b"        # 默认是 deepseek-v4-flash
# base_url 默认为 http://localhost:11434/v1
```

打印工具调用 JSON 但没有线协议标记的本地模型：参见[当本地模型输出工具 JSON 时](#当本地模型输出工具-json-时)。

## 凭据链接

提供商设置界面使用与 onboarding、`/provider`、`/links`、设置回执和 doctor 输出相同的类型化凭据元数据。缺失 URL 是有意为之：本地、仅 OAuth 和用户定义的路由会显示其支持的配置路径，而不是猜测厂商页面。

| 提供商 ID | 凭据或控制台链接 |
| --- | --- |
| `deepseek`, `deepseek-anthropic` | [DeepSeek API 密钥](https://platform.deepseek.com/api_keys) |
| `nvidia-nim` | [NVIDIA NIM API 密钥](https://build.nvidia.com/settings/api-keys) |
| `openai` | [OpenAI API 密钥](https://platform.openai.com/api-keys) |
| `atlascloud` | [Atlas Cloud API 密钥](https://atlascloud.ai/docs/en/api-keys) |
| `wanjie-ark` | [Wanjie MaaS APIKEY 文档](https://docs.wanjiedata.com/maas/maas-openapi-v1.html) |
| `volcengine` | [Volcengine Ark API 密钥](https://console.volcengine.com/ark/apiKey) |
| `openrouter` | [OpenRouter 密钥](https://openrouter.ai/settings/keys) |
| `xiaomi-mimo` | [Xiaomi MiMo Token 套餐](https://platform.xiaomimimo.com/token-plan) |
| `novita` | [Novita 密钥管理](https://novita.ai/en/settings/key-management) |
| `fireworks` | [Fireworks API 密钥](https://fireworks.ai/api-keys) |
| `siliconflow` | [SiliconFlow 全球 API 密钥](https://cloud.siliconflow.com/account/ak) |
| `siliconflow-CN` | [SiliconFlow 中国 API 密钥](https://cloud.siliconflow.cn/account/ak) |
| `arcee` | [Arcee API 密钥指南](https://docs.arcee.ai/other/create-your-first-api-key) |
| `moonshot` | [Kimi API 平台密钥](https://platform.kimi.ai/console/api-keys) 或 [Kimi Code 会员控制台](https://www.kimi.com/code/console) |
| `zai` | [Z.ai 模型 API](https://z.ai/model-api) |
| `stepfun` | [StepFun 开放平台](https://platform.stepfun.ai/) |
| `minimax`, `minimax-anthropic` | [MiniMax 接口密钥](https://platform.minimax.io/user-center/basic-information/interface-key) |
| `huggingface` | [Hugging Face token](https://huggingface.co/settings/tokens) |
| `deepinfra` | [DeepInfra API 密钥](https://deepinfra.com/dash/api_keys) |
| `together` | [Together API 密钥](https://api.together.ai/settings/api-keys) |
| `qianfan` | [百度云访问密钥](https://console.bce.baidu.com/iam/#/iam/accesslist) |
| `anthropic` | [Anthropic API 密钥](https://console.anthropic.com/settings/keys) |
| `openmodel` | [OpenModel 控制台](https://console.openmodel.ai/)（[认证指南](https://docs.openmodel.ai/en/docs/getting-started/authentication)） |
| `openai-codex` | 运行 `codex login`，然后显式授予 Codewhale 对该精确凭据文件的只读访问权限；不会存储 Codewhale API 密钥。 |
| `sglang`, `vllm` | 本地 OpenAI 兼容端点默认无密钥；仅当服务器需要时才配置密钥。 |
| `ollama` | 本地 Ollama 默认无密钥；仅当本地服务器需要时才配置密钥。 |
| `ollama-cloud` | 创建[Ollama API 密钥](https://ollama.com/settings/keys)，用 `codewhale auth set --provider ollama-cloud` 保存，或按该优先级顺序设置 `OLLAMA_CLOUD_API_KEY` / `OLLAMA_API_KEY`。 |
| `sakana` | [Sakana AI API 密钥](https://console.sakana.ai/api-keys)（[入门](https://console.sakana.ai/get-started)） |
| `longcat` | [美团 LongCat 平台](https://longcat.chat/platform) |
| `opencode-go` | [OpenCode Go](https://opencode.ai/docs/go/) |
| `opencode-zen` | [OpenCode Zen](https://opencode.ai/docs/zen/) |
| `meta` | [Meta Model API](https://developer.meta.com/ai/) |
| `telecomjs` | [TelecomJS TokenHub](https://aigw.telecomjs.com/) |
| `xai` | [xAI 控制台](https://console.x.ai/) 获取 API 密钥、Codewhale 自有的设备登录，或明确同意的只读 Grok CLI 凭据。 |
| `mistral` | [Mistral 控制台 (la Plateforme)](https://console.mistral.ai/api-keys) |
| `google` | [Google AI Studio](https://aistudio.google.com/apikey) —— Codewhale 使用官方 Gemini OpenAI 兼容端点，绝不读取 Google OAuth 文件。 |
| `antigravity` | 用官方 `agy` CLI（1.1.13）登录。在 `codewhale auth external-consent` 之后，Codewhale 可以只读地读取该登录的精确定点 `state.vscdb` 中的 OAuth token；它从不写入或刷新该文件。`ANTIGRAVITY_API_KEY` 或进程的 `AGY_ADC_AUTH` 优先于该文件。云代码线协议未实现：请求失败关闭并给出可操作的消息——Gemini 模型请使用 `google`。 |
| `edenai` | [Eden AI API 密钥](https://app.edenai.run/settings/api-keys) |
| `modelstudio-token-plan`, `modelstudio-token-plan-anthropic`, `modelstudio-coding-plan`, `modelstudio-coding-plan-anthropic` | [阿里云百炼 Model Studio（百炼控制台）](https://bailian.console.aliyun.com/) —— 创建或复制 Model Studio API 密钥。 |
| `custom` | 设置该具名提供商的 `base_url` 和 `api_key_env` 或 `api_key`；不存在规范的厂商凭据页面。 |

对于 Kimi，官方[快速入门](https://platform.kimi.ai/docs/overview)引导用户登录、打开 **API 密钥**、创建并复制密钥，并保守秘密。Codewhale 直接链接到该控制台并接受复制的密钥。它从不探测或冒充 `kimi_cli`/`kimi_code_cli`；一等 Kimi OAuth 仍然受阻于厂商注册的 Codewhale 身份。

### 外部 CLI 凭据授权

归另一 CLI 所有的凭据文件默认禁用。没有显式授权，提供商发现、设置、路由、`auth status` 和 doctor 不会 stat、读取、刷新、联系身份提供商或重写 Codex、Grok、Kimi 或未来的外部凭据文件。

Codewhale 目前为 Codex CLI 和 Grok CLI 支持精确路径、提供商范围的**只读**授权：

```bash
codex login
codewhale auth external-consent --provider openai-codex --mode read-only

grok login
codewhale auth external-consent --provider xai --mode read-only

codewhale auth status --provider openai-codex
codewhale auth external-revoke --provider openai-codex
```

当外部 CLI 使用自定义位置时，传入 `--path /absolute/path/to/auth.json`。授权会持久化提供商、外部所有者、精确绝对路径和授权模式版本。之后的环境变量更改不会把该权威重定向到另一个文件。只读授权从不刷新、联系身份/发现服务或重写外部文件；对显式所选提供商的正常请求可以使用其 token。过期的 token 会带着登录指引失败。Doctor 在不打开凭据文件的情况下报告结构性的授权/配置状态，并且始终不执行变更。

`managed` 保留给未来的提供商特定保留适配器。v0.9.1 在文件或网络 I/O 之前拒绝它，因为还没有经过审查的适配器能够安全地保留每一个未知的外部模式字段。Codewhale 启动的 xAI 设备登录改为原子地激活一个 Codewhale 自有的、名为 `$CODEWHALE_HOME/credentials/xai-auth-<generation>.json` 的 generation，只把该已验证的基本名存入配置，并撤销任何 Grok 文件授权。被取代的 generation 只在新配置指针提交之后清理。
Kimi 仍然仅支持 API 密钥；对 Kimi 的外部授权被拒绝。

官方 DeepSeek Harness（`dsh`）是第三个只读凭据所有者：`codewhale auth external-consent --provider deepseek --mode read-only` 授予对 `$DSH_HOME/.credentials.yaml`（或 `~/.dsh/.credentials.yaml`）中 `DEEPSEEK_API_KEY` 的精确路径读取访问，Codewhale 从不写入、刷新或将其加载到进程环境中。这与 DSH *harness* 集成（`codewhale integrations dsh …`，参见 [INTEGRATIONS_DSH.md](../INTEGRATIONS_DSH.md)）是分开的，后者在两个方向上都不接触凭据：它把 Codewhale 的路由身份固定进一个 `--patch` 覆盖层，并让 DSH 解析自己的密钥。

## 随附的提供商

| 提供商 ID | TOML 表 | 认证环境变量 | Base URL 环境变量和默认值 | 默认或静态模型 | 备注 |
| --- | --- | --- | --- | --- | --- |
| `deepseek` | `[providers.deepseek]` | `DEEPSEEK_API_KEY` | `CODEWHALE_BASE_URL` / `DEEPSEEK_BASE_URL`；默认 `https://api.deepseek.com/beta` | `deepseek-v4-pro`, `deepseek-v4-flash`；兼容别名 `deepseek-chat`, `deepseek-reasoner` | 一等默认。线上 Pro 后端标记为 `DeepSeek-V4-Pro-0813`；可调用的 API ID 仍是 `deepseek-v4-pro`。Beta URL 启用严格工具模式、chat 前缀补全和 FIM 补全。显式设置 `https://api.deepseek.com` 或 `/v1` 可退出 beta 专属功能。推理强度映射到文档化的线协议阶梯 `low`/`high`/`max` 加上 `thinking` 开关：`off` 发送 `thinking: {"type":"disabled"}`，`low` 发送 `reasoning_effort: "low"`，`medium` 向上取整为 `"high"`（线协议没有 medium），`high`/`max` 直通。 |
| `deepseek-anthropic` | `[providers.deepseek_anthropic]` | `DEEPSEEK_API_KEY` | `DEEPSEEK_ANTHROPIC_BASE_URL`；默认 `https://api.deepseek.com/anthropic` | `deepseek-v4-pro`, `deepseek-v4-flash`；兼容别名 `deepseek-chat`, `deepseek-reasoner` | 面向 Anthropic Messages 线协议的 DeepSeek 显式选择加入路由。使用 `/v1/messages`、`x-api-key` 和 `anthropic-version: 2023-06-01`。默认 Chat Completions 路径请保持 `provider = "deepseek"`。 |
| `nvidia-nim` | `[providers.nvidia_nim]` | `NVIDIA_API_KEY`, `NVIDIA_NIM_API_KEY`, 回退 `DEEPSEEK_API_KEY` | `NVIDIA_NIM_BASE_URL`, `NIM_BASE_URL`, `NVIDIA_BASE_URL`；默认 `https://integrate.api.nvidia.com/v1` | `deepseek-ai/deepseek-v4-pro`, `deepseek-ai/deepseek-v4-flash` | 通过 NVIDIA NIM 托管的 DeepSeek V4。TUI 配置路径接受 `NVIDIA_NIM_MODEL`。 |
| `openai` | `[providers.openai]` | `OPENAI_API_KEY` | `OPENAI_BASE_URL`；默认 `https://api.openai.com/v1` | 注册表条目：`deepseek-v4-pro`, `deepseek-v4-flash`, `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`；默认配置模型 `deepseek-v4-pro` | 面向网关和自定义端点的通用 OpenAI 兼容路由，包括配置该端点时的阿里云百炼 / Model Studio DashScope。[GPT-5.6 系列](https://developers.openai.com/api/docs/models/gpt-5.6-sol)使用 OpenAI 文档化的 1.05M 上下文、128K 最大输出和推理级别。对显式的第三方 OpenAI 兼容路由使用此路由，而不是发明新的提供商 ID。接受 `OPENAI_MODEL`。 |
| `atlascloud` | `[providers.atlascloud]` | `ATLASCLOUD_API_KEY` | `ATLASCLOUD_BASE_URL`；默认 `https://api.atlascloud.ai/v1` | 默认 `deepseek-ai/deepseek-v4-flash`；选择 AtlasCloud 时显式的 `vendor/model-id` 值直通 | OpenAI 兼容托管路由。TUI 配置路径接受 `ATLASCLOUD_MODEL`，静态 `ModelRegistry` 保留 DeepSeek V4 回退行，提供商提示的 CLI 模型 ID 按原样发送给 AtlasCloud。当前提供商自有的模型列表与定价请查看 Atlas Cloud 自己的目录或 Coding Plan 页面。 |
| `wanjie-ark` | `[providers.wanjie_ark]` | `WANJIE_ARK_API_KEY`, `WANJIE_API_KEY`, `WANJIE_MAAS_API_KEY` | `WANJIE_ARK_BASE_URL`, `WANJIE_BASE_URL`, `WANJIE_MAAS_BASE_URL`；默认 `https://maas-openapi.wanjiedata.com/api/v1` | `deepseek-reasoner` | OpenAI 兼容托管路由。接受 `WANJIE_ARK_MODEL`, `WANJIE_MODEL` 和 `WANJIE_MAAS_MODEL`。 |
| `volcengine` | `[providers.volcengine]` | `VOLCENGINE_API_KEY`, `VOLCENGINE_ARK_API_KEY`, `ARK_API_KEY` | `VOLCENGINE_BASE_URL`, `VOLCENGINE_ARK_BASE_URL`, `ARK_BASE_URL`；默认 `https://ark.cn-beijing.volces.com/api/coding/v3` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | 火山引擎/Volcengine Ark OpenAI 兼容编码端点。接受 `VOLCENGINE_MODEL` 和 `VOLCENGINE_ARK_MODEL`。 |
| `openrouter` | `[providers.openrouter]` | `OPENROUTER_API_KEY` | `OPENROUTER_BASE_URL`；默认 `https://openrouter.ai/api/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`；近期大模型 ID 包括 `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3`, `xiaomi/mimo-v2.5-pro`, `qwen/qwen3.6-flash`, `qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`, `qwen/qwen3.6-plus`, `google/gemma-4-31b-it`, `z-ai/glm-5.1`, `z-ai/glm-5.2`, `moonshotai/kimi-k2.7-code`, `moonshotai/kimi-k2.6` | 附加的开放模型路由层。它不是 DeepSeek 的替代品；它让用户在选择 OpenRouter 时把受支持的模型 ID 路由过去。 |
| `orcarouter` | `[providers.orcarouter]` | `ORCAROUTER_API_KEY` | `ORCAROUTER_BASE_URL`；默认 `https://api.orcarouter.ai/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`；路由别名 `orcarouter/auto`；近期大模型 ID 镜像 OpenRouter 的命名空间目录 | [OrcaRouter](https://www.orcarouter.ai) OpenAI 兼容聚合网关。与 OpenRouter 共享命名空间的 `vendor/model` 线协议模型格式和 DeepSeek 模型集，因此 OpenRouter 的 base-URL 和模型规范化规则适用。接受 `ORCAROUTER_MODEL`。提供商别名：`orcarouter`, `orca_router`, `orca`。 |
| `xiaomi-mimo` | `[providers.xiaomi_mimo]` | `XIAOMI_MIMO_TOKEN_PLAN_API_KEY`, `MIMO_TOKEN_PLAN_API_KEY`, `XIAOMI_MIMO_API_KEY`, `XIAOMI_API_KEY`, `MIMO_API_KEY` | `XIAOMI_MIMO_BASE_URL`, `MIMO_BASE_URL`, `XIAOMI_MIMO_MODE`, `MIMO_MODE`；默认 `https://token-plan-sgp.xiaomimimo.com/v1` | 聊天：`mimo-v2.5-pro`, `mimo-v2.5-pro-ultraspeed`, `mimo-v2.5`；语音/TTS：`mimo-v2.5-tts`, `mimo-v2.5-tts-voicedesign`, `mimo-v2.5-tts-voiceclone`, `mimo-v2-tts` | 小米 MiMo OpenAI 兼容聊天补全路由。Token Plan 密钥（`tp-...`）默认使用 `api-key` 认证和 token-plan 端点；按量付费模式使用标准 API 密钥（`sk-...`）和 `https://api.xiaomimimo.com/v1`。它发送 `max_completion_tokens` 并用 MiMo 的 `thinking` 字段进行推理控制。Token Plan 成本/用量基于额度/配额；在小米暴露可靠的余额 API 之前，Codewhale 将其显示为未知。`codewhale speech` / `tts` 使用 TTS 模型。 |
| `novita` | `[providers.novita]` | `NOVITA_API_KEY` | `NOVITA_BASE_URL`；默认 `https://api.novita.ai/openai/v1` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | 面向 DeepSeek 模型 ID 的 OpenAI 兼容托管路由。模型覆盖使用配置或 `CODEWHALE_MODEL` / `DEEPSEEK_MODEL`。 |
| `fireworks` | `[providers.fireworks]` | `FIREWORKS_API_KEY` | `FIREWORKS_BASE_URL`；默认 `https://api.fireworks.ai/inference/v1` | `accounts/fireworks/models/deepseek-v4-pro` | OpenAI 兼容托管路由。模型覆盖使用配置或 `CODEWHALE_MODEL` / `DEEPSEEK_MODEL`。 |
| `siliconflow` | `[providers.siliconflow]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`；默认 `https://api.siliconflow.com/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | OpenAI 兼容托管路由。官方文档使用 `.com` 端点。接受 `SILICONFLOW_MODEL`。推理别名 `deepseek-reasoner` 和 `deepseek-r1` 映射到 Pro；`deepseek-chat` 和 `deepseek-v3` 映射到 Flash。 |
| `siliconflow-CN` | `[providers.siliconflow_cn]` | `SILICONFLOW_API_KEY` | `SILICONFLOW_BASE_URL`；默认 `https://api.siliconflow.cn/v1` | 使用 SiliconFlow 模型集 | 中国区 SiliconFlow 路由。未设置时回退到 `[providers.siliconflow]` 的 api_key / base_url / model。用 `provider = "siliconflow-CN"` 或 `CODEWHALE_PROVIDER=siliconflow-CN` 选择它。 |
| `arcee` | `[providers.arcee]` | `ARCEE_API_KEY` | `ARCEE_BASE_URL`；默认 `https://api.arcee.ai/api/v1` | `trinity-large-thinking`, `trinity-large-preview` | Arcee AI 直连 OpenAI 兼容路由，按 256K 上下文 BF16 服务跟踪。接受 `ARCEE_MODEL`。OpenRouter 的 `arcee-ai/trinity-large-thinking` 仍是 OpenRouter 命名空间的模型 ID；直连 Arcee 使用裸 `trinity-large-thinking` ID。 |
| `moonshot` | `[providers.moonshot]` | `MOONSHOT_API_KEY`, `KIMI_API_KEY` | `MOONSHOT_BASE_URL`, `KIMI_BASE_URL`；默认 `https://api.moonshot.ai/v1` | 直连 Moonshot：`kimi-k3`, `kimi-k2.7-code`, `kimi-k2.7-code-highspeed`, `kimi-k2.6`；Kimi Code 会员：`k3`, `kimi-for-coding`, `kimi-for-coding-highspeed`（在 `https://api.kimi.com/coding/v1`） | Moonshot/Kimi 路由。`kimi` 和 `kimi-k2` 别名选择 `kimi-k2.7-code`；接受 `MOONSHOT_MODEL`, `KIMI_MODEL_NAME` 和 `KIMI_MODEL`。Kimi 思考通过 `reasoning_content` 流式返回；Codewhale 把它保留在 Thinking 单元格中，并为思考/工具调用连续性重放它。直连 K3 请使用精确的 `base_url = "https://api.moonshot.ai/v1"` 和 `model = "kimi-k3"`；它始终思考，接收顶层 `reasoning_effort = "low" | "high" | "max"`（`off` 规范化为 `low`），只使用 `max_completion_tokens`，并按 [K3 快速入门](https://platform.kimi.ai/docs/guide/kimi-k3-quickstart)省略 `temperature`/`top_p`。Kimi Code K3 请使用来自 [Kimi Code 控制台](https://www.kimi.com/code/console)的密钥、精确的 `base_url = "https://api.kimi.com/coding/v1"` 和裸 `model = "k3"`；`off` 变成启用的 `low`，而正常分派的 `auto` 选择并发送具体的 Codewhale 层级。只有省略推理设置时才由提供商默认值控制。该会员路由默认安全地使用 262,144 上下文 token；[Kimi Code 模型层级表](https://www.kimi.com/code/docs/en/kimi-code/models.html)授予 Allegretto 及以上套餐最高 1M，这些套餐可将其表示为 `context_window = 1048576`。`k3[1m]` 仅限 Claude Code，Codewhale 拒绝它。`kimi-for-coding` 仍是有效的 K2.7 会员路由，`kimi-for-coding-highspeed` 是它自己的高速名册条目（262,144 上下文）；会员 id 在直连平台端点上被拒绝，`kimi-k3` 在会员端点上仍被拒绝。计费由路由解析到的端点决定，对照两个精确的产品端点各判断一次：直连 Moonshot（`https://api.moonshot.ai/v1` 或默认值）按用量计费并显示美元估算，精确的 Kimi Code 会员端点按 Kimi Code 额度计费且从不显示美元估算，其他任何情况——网关主机、邻近的 Kimi 托管路径——报告 `cost: unknown` 而不是借用任一产品。导入的没有 `base_url` 的 Kimi Code token 仍解析到会员端点，因此按 Kimi Code 额度计费，绝不累积美元。完成的回合（父或子代理）从构建其客户端时不可变的端点回执计费，绝不从之后重新读取的配置计费：`MOONSHOT_BASE_URL`/`KIMI_BASE_URL` 只合并到*活动*提供商的表中，回合内的提供商切换可以把常驻配置移离实际运行的路由。遗留的 `auth_mode = "kimi_oauth"` 在不探测 Kimi CLI 文件的情况下失败于 API 密钥指引。Codewhale 不冒充 `kimi_cli` 或 `kimi_code_cli`。**中国区密钥：** 贡献者现场证据（@vFONGv，PR #5229，Windows 10 上验证）报告中国区 Moonshot 密钥必须搭配 `base_url = "https://api.moonshot.cn/v1"`；留在默认国际主机（`https://api.moonshot.ai/v1`）上会认证失败。我们自己没有中国区密钥来验证这一点，因此记录为用户报告而非测试过的路由。另请注意，仅编辑 `base_url` 不会生效，直到为该提供商重新运行 `codewhale auth set`。 |
| `antigravity` | `[providers.antigravity]` | `ANTIGRAVITY_API_KEY` | `ANTIGRAVITY_BASE_URL`；默认 `https://cloudcode-pa.googleapis.com/v1internal` | 无公布——在云代码线协议存在之前请求失败关闭 | Antigravity（`agy` 1.1.13）凭据平面：经同意门控的只读导入官方 CLI 的 `state.vscdb` OAuth token（`antigravityUnifiedStateSync.oauthToken`），固定到精确的按 OS 应用 profile 路径。存储通过安全的不跟随（no-follow）边界以只读方式打开，并做 inode 复查；Codewhale 从不写入、刷新或重新认证。优先级：`ANTIGRAVITY_API_KEY` > 进程 `AGY_ADC_AUTH` > 经同意的文件。不是任何其他 harness 的嵌入。本环境未做真实调用。 |
| `google` | `[providers.google]` | `GOOGLE_API_KEY`, `GEMINI_API_KEY` | `GOOGLE_BASE_URL`, `GEMINI_BASE_URL`；默认 `https://generativelanguage.googleapis.com/v1beta/openai/` | `gemini-3.1-pro-preview`（默认）；`/model` 还列出 `gemini-3-pro-preview`, `gemini-3.7-flash`, `gemini-3.6-flash`, `gemini-3.5-flash`, `gemini-3.5-flash-lite`, `gemini-2.5-pro`, `gemini-2.5-flash` | Google Gemini 作为官方 OpenAI 兼容 Chat Completions 路由上的自有后端。思考模型在工具调用时捕获 `extra_content.google.thought_signature` 并在助手工具调用消息中重放它；重放一个未捕获签名的工具调用会失败关闭并给出可操作的错误，而不是让工具循环中断。`gemini-2.5-flash-lite` 默认关闭思考，并以警告降级。推理强度映射到文档化的 `google.thinking_config.thinking_level`（`low`/`high`）。该方言绑定精确的官方 base URL：指向其他网关的 `google` 行获得普通 OpenAI 语义且无签名要求。Codewhale 从不读取 Google OAuth 文件；只使用 AI Studio API 密钥。本环境未对真实端点做实时测试。 |
| `zai` | `[providers.zai]` | `ZAI_API_KEY`, `Z_AI_API_KEY` | `ZAI_BASE_URL`, `Z_AI_BASE_URL`；默认 `https://api.z.ai/api/coding/paas/v4`；通用 API `https://api.z.ai/api/paas/v4` | `GLM-5.3` 默认；`/model` 还列出 `GLM-5.2`, `GLM-5.1` 和 `GLM-5-Turbo` | Z.AI GLM Coding Plan 路由。`GLM-5.3` 是默认值且是一等选择器行（`model = "GLM-5.3"` 或 `ZAI_MODEL=GLM-5.3`）；显式选择 `GLM-5.2` 保留它自己的 id。在 Z.ai 发布不同的 5.3 元数据之前，限制和推理选项继承自 `GLM-5.2`；它不带价格。未为 5.3 配置的账户上，真实调用仍可能以权利代码 1311 返回 429。 |
| `stepfun` | `[providers.stepfun]` | `STEPFUN_API_KEY`, `STEP_API_KEY` | `STEPFUN_BASE_URL`, `STEP_BASE_URL`；默认 `https://api.stepfun.ai/v1`；Coding Plan 端点 `https://api.stepfun.ai/step_plan/v1` | `step-3.7-flash` | StepFun / StepFlash 直连 OpenAI 兼容路由。`/provider` 设置询问密钥属于哪种计费路由——按量付费还是 Step Plan——对照所选端点验证密钥，并把答案只写入 `[providers.stepfun].base_url`。既非两条已识别路由的 base URL 会被保留，问题被跳过。你也可以手动把 `[providers.stepfun].base_url` 或 `STEP_BASE_URL` 设为 Coding Plan URL。离线核算把已识别路由标记为 `stepfun-payg` 或 `stepfun-plan`，不持久化原始端点，只有标准 PAYG 路由接收 token 定价。接受 `STEPFUN_MODEL` 和 `STEP_MODEL`。 |
| `minimax` | `[providers.minimax]` | `MINIMAX_API_KEY` | `MINIMAX_BASE_URL`；默认 `https://api.minimax.io/v1`；中国 `https://api.minimaxi.com/v1` | `MiniMax-M3`, `MiniMax-M2.7`, `MiniMax-M2.7-highspeed`, `MiniMax-M2.5`, `MiniMax-M2.5-highspeed`, `MiniMax-M2.1`, `MiniMax-M2.1-highspeed`, `MiniMax-M2` | MiniMax 直连 OpenAI 兼容路由。Codewhale 发送 `reasoning_split = true`，这样 MiniMax 思考与答案文本分开到达。两种 MiniMax 方言都在相同端点和相同密钥上售卖按量付费和 Token Plan，因此计费从凭据*产品*分类，绝不由端点或默认值决定。`[providers.minimax]`/`[providers.minimax_anthropic]` 中的 `mode = "token-plan"`，或形如 `sk-cp…` 的 Token Plan 密钥，按 MiniMax Token Plan 额度计费且无美元估算；显式的按量付费模式（`pay-as-you-go`/`payg`/`metered`）胜过密钥形状。密钥的产品前缀只有在密钥位于配置中、由 `api_key_env` 绑定或作为 `MINIMAX_API_KEY` 导出到官方端点时才可见——通过 `codewhale auth set` 保存的密钥（secret store / OS keyring）刻意不被读取用于计费分类。没有显式模式且没有可见的产品标记时，路由报告 `cost: unknown` 而不是假定按量付费，因此 Token Plan 账户永远不会被收取虚构的美元。自定义/网关端点也以 `cost: unknown` 失败关闭。官方 M3 输入模态是文本、图像和视频；M2.7 仅文本。 |
| `minimax-anthropic` | `[providers.minimax_anthropic]` | `MINIMAX_API_KEY` | `MINIMAX_ANTHROPIC_BASE_URL`；默认 `https://api.minimax.io/anthropic`；中国 `https://api.minimaxi.com/anthropic` | `MiniMax-M3`, `MiniMax-M2.7` | MiniMax 直连 Anthropic 兼容 Messages 路由。保留 `/anthropic` 后缀，因为 Codewhale 会追加 `/v1/messages`；该路由使用 `x-api-key`。M3 支持自适应或禁用思考。M2.7 始终启用思考。 |
| `sglang` | `[providers.sglang]` | 可选 `SGLANG_API_KEY` | `SGLANG_BASE_URL`；默认 `http://localhost:30000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 自托管 OpenAI 兼容路由。本地部署通常省略认证。接受 `SGLANG_MODEL`。 |
| `vllm` | `[providers.vllm]` | 可选 `VLLM_API_KEY` | `VLLM_BASE_URL`；默认 `http://localhost:8000/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 自托管 vLLM OpenAI 兼容路由。本地部署通常省略认证。接受 `VLLM_MODEL`。 |
| `ollama` | `[providers.ollama]` | 本地可选 `OLLAMA_API_KEY` | `OLLAMA_BASE_URL`；默认 `http://localhost:11434/v1` | 本地目录的实况标签；刷新前占位 `unknown`；提供商提示的自定义标签直通 | 本地 Ollama 默认无密钥。接受 `OLLAMA_MODEL`。标题栏不得绘制本机守护进程未列出的托管 id。 |
| `ollama-cloud` | `[providers.ollama_cloud]` | `OLLAMA_CLOUD_API_KEY`, 然后是 `OLLAMA_API_KEY` | `OLLAMA_CLOUD_BASE_URL`；默认 `https://ollama.com/v1` | `gpt-oss:120b`；任意的提供商自有 ID 直通 | 托管 OpenAI 兼容 `/v1/chat/completions` 路由。在 `ollama-cloud` 下保存凭据；精确的已发布 `ollama` + Cloud URL 元组与其遗留表和秘密槽位具有有界的只读内存兼容性。接受 `OLLAMA_CLOUD_MODEL`。 |
| `huggingface` | `[providers.huggingface]` | `HUGGINGFACE_API_KEY`, `HF_TOKEN` | `HUGGINGFACE_BASE_URL`, `HF_BASE_URL`；默认 `https://router.huggingface.co/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | Hugging Face Inference Providers OpenAI 兼容路由器路由。接受的别名：`huggingface`, `hugging-face`, `hugging_face`, `hf`。Org 前缀的模型 ID 直通。接受 `HUGGINGFACE_MODEL` 和 `HF_MODEL`。Hub 浏览/导出是未来的独立功能。 |
| `deepinfra` | `[providers.deepinfra]` | `DEEPINFRA_API_KEY`, `DEEPINFRA_TOKEN` | `DEEPINFRA_BASE_URL`；默认 `https://api.deepinfra.com/v1/openai` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | DeepInfra OpenAI 兼容路由。OpenAI SDK 的直接替代品。 |
| `together` | `[providers.together]` | `TOGETHER_API_KEY` | `TOGETHER_BASE_URL`；默认 `https://api.together.xyz/v1` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash`, `thinkingmachines/inkling` | Together AI OpenAI 兼容路由。接受 `TOGETHER_MODEL`。模型别名 `deepseek-v4-pro` 和 `deepseek-v4-flash` 规范化为 Together 的 org 前缀 ID；`inkling` 和 `together-inkling` 规范化为 Together 发布的小写 Inkling 线协议 ID。Inkling 使用 Thinking Machines 的[官方模型仓库](https://huggingface.co/thinkingmachines/Inkling)中精确的 `none`/`minimal`/`low`/`medium`/`high`/`max` 推理词汇。Together 的[发布帖](https://www.together.ai/blog/together-ai-brings-thinking-machines-labs-new-model-inkling-on-day-0)目前称 Inkling 以 1M 上下文上线，而其[模型详情页](https://www.together.ai/models/inkling)称即将推出 256K 上下文且不公布价格。在 Together 活跃的 `/models` 端点和 Models.dev 目录解决该冲突之前，Inkling 不会种入 Codewhale 的离线选择器，也不会推断路由特定的上下文或成本。 |
| `qianfan` | `[providers.qianfan]` | `QIANFAN_API_KEY`, `BAIDU_QIANFAN_API_KEY` | `QIANFAN_BASE_URL`, `BAIDU_QIANFAN_BASE_URL`；默认 `https://api.baiduqianfan.ai/v1` | `ernie-4.0-turbo-8k`；提供商范围的定制 Qianfan 服务/模型 ID 直通 | 百度千帆 OpenAI 兼容路由。请求使用 Bearer 认证和 Chat Completions 负载。接受 `QIANFAN_MODEL` 和 `BAIDU_QIANFAN_MODEL`；别名 `baidu-qianfan`, `baidu_qianfan` 和 `baidu` 解析到该提供商。千帆文档中工具/函数调用按模型范围限定，因此 Codewhale 保留所选线协议模型，把实时能力证明留给后续的路由/能力工作。 |
| `openai-codex` | `[providers.openai_codex]` | 通过 `OPENAI_CODEX_ACCESS_TOKEN`/`CODEX_ACCESS_TOKEN` 的进程 token，或 `codex login` 之后的精确路径只读授权 | `OPENAI_CODEX_BASE_URL`/`CODEX_BASE_URL`；默认 `https://chatgpt.com/backend-api` | `gpt-5.5` | **实验性。** 与 `/codex/responses` 上的 OpenAI Responses API 对话。Codex CLI 文件默认禁用；`codewhale auth external-consent --provider openai-codex --mode read-only` 授予对一个精确文件的访问。Codewhale 从不刷新或重写那个外部文件，过期的 token 失败关闭。接受 `OPENAI_CODEX_MODEL`/`CODEX_MODEL` 和 `OPENAI_CODEX_ACCOUNT_ID`/`CODEX_ACCOUNT_ID`。即使公共 API 模型表为原生 `gpt-5.5` 列出更大的窗口，Codewhale 也按 400K Codex 家族有效上下文窗口为该路由做预算。 |
| `anthropic` | `[providers.anthropic]` | `ANTHROPIC_API_KEY` | `ANTHROPIC_BASE_URL`；默认 `https://api.anthropic.com` | `claude-opus-4-8`, `claude-sonnet-4-6`（默认）, `claude-haiku-4-5` | 原生 Anthropic Messages API 路由（`/v1/messages`、`x-api-key` + `anthropic-version: 2023-06-01`）——不是 OpenAI 兼容。通过 `cache_control` 断点提示缓存、自适应思考 + `output_config.effort`、按原样重放带签名的思考块、按 #2961 规范化的缓存遥测。接受 `ANTHROPIC_MODEL`。 |
| `openmodel` | `[providers.openmodel]` | `OPENMODEL_API_KEY` | `OPENMODEL_BASE_URL`；默认 `https://api.openmodel.ai` | `deepseek-v4-flash`；提供商范围的定制模型 ID 直通 | OpenModel Anthropic 兼容 Messages 路由。使用 `/v1/messages`、Bearer 认证和 `anthropic-version: 2023-06-01`；OpenModel 按模型 id 选择 DeepSeek、DashScope、Xiaomi、Claude 和其他路由。接受 `OPENMODEL_MODEL`。 |
| `sakana` | `[providers.sakana]` | `FUGU_API_KEY`, `SAKANA_API_KEY` | `SAKANA_BASE_URL`；默认 `https://api.sakana.ai/v1` | `fugu`（默认）, `fugu-ultra-20260615` | Sakana AI Fugu OpenAI 兼容路由。标准 Chat Completions 线协议；支持流式。`fugu-ultra-20260615` 是重型/推理变体。环境变量别名：`FUGU_API_KEY`（主）, `SAKANA_API_KEY`；提供商别名：`sakana-ai`, `sakana_ai`, `fugu`。 |
| `longcat` | `[providers.longcat]` | `LONGCAT_API_KEY` | `LONGCAT_BASE_URL`；默认 `https://api.longcat.chat/openai/v1` | `LongCat-2.0`（默认） | 美团 LongCat 精选模型网关。OpenAI 兼容 Chat Completions 线协议。在 https://longcat.chat/platform 注册获取 API 密钥。提供商别名：`long-cat`, `meituan-longcat`, `meituan`。 |
| `opencode-go` | `[providers.opencode_go]` | `OPENCODE_GO_API_KEY` | `OPENCODE_GO_BASE_URL`；默认 `https://opencode.ai/zen/go/v1` | `deepseek-v4-pro`（默认）, `grok-4.5`, `glm-5.2`, `glm-5.1`, `kimi-k3`, `kimi-k2.7-code`, `kimi-k2.6`, `deepseek-v4-flash`, `mimo-v2.5`, `mimo-v2.5-pro` | [OpenCode Go](https://opencode.ai/docs/go/) 订阅路由，使用 OpenAI 兼容 Chat Completions。接受 `OPENCODE_GO_MODEL`。Codewhale 使用裸线协议 ID；常见的 `opencode-go/<model-id>` 输入别名规范化为裸 ID。只在 Anthropic `/messages` 端点文档化的 Go 模型，在 Codewhale 支持按模型选择线协议之前被该路由刻意不公布。计费界面显示 Go 额度而不是 token 价格估算。 |
| `opencode-zen` | `[providers.opencode_zen]` | `OPENCODE_ZEN_API_KEY`, 回退 `OPENCODE_API_KEY` | `OPENCODE_ZEN_BASE_URL`；默认 `https://opencode.ai/zen/v1` | `gpt-5.5`（默认）；当前文档化的 GPT、Claude、Qwen、DeepSeek、MiniMax、GLM、Kimi、Grok 和免费模型 ID | [OpenCode Zen](https://opencode.ai/docs/zen/) 模型感知网关。接受 `OPENCODE_ZEN_MODEL`，官方 `opencode/<model-id>` 选择器规范化为裸线协议 ID。GPT 行使用 `/responses`；Claude 和 Qwen 行使用 `/messages`；DeepSeek、MiniMax、GLM、Kimi、Grok 和列出的免费行使用 `/chat/completions`。Responses 和 Chat Completions 用 Bearer `Authorization` 认证，而 Anthropic Messages 用 `x-api-key`；这些路由都不用 ChatGPT/Codex OAuth 指引或头。Gemini 目前失败关闭，因为其模型特定的 Google 线协议未实现。未知模型在精选目录中存在其协议之前也失败关闭。 |
| `meta` | `[providers.meta]` | `META_MODEL_API_KEY`, `MODEL_API_KEY` | `META_MODEL_API_BASE_URL`, `MODEL_API_BASE_URL`；默认 `https://api.meta.ai/v1` | `muse-spark-1.2`（默认） | [Meta Model API](https://developer.meta.com/ai/resources/blog/build-with-muse-spark/) 公开预览路由，使用 OpenAI 兼容 Chat Completions。Muse Spark 1.2 保留其线协议 ID、工具支持、1M 上下文、32K 输出元数据和 `none` 到 `xhigh` 的推理强度。接受 `META_MODEL_API_MODEL` 和 `MODEL_API_MODEL`。提供商别名：`meta-ai`, `meta_model_api`, `muse`, `muse-spark`。 |
| `telecomjs` | `[providers.telecomjs]` | `TELECOMJS_API_KEY` | `TELECOMJS_BASE_URL`；默认 `https://aigw.telecomjs.com/v1` | 保守回退 `deepseek-v4-pro`；配置密钥后认证的 `/models` 行 | TelecomJS TokenHub OpenAI 兼容 Chat Completions 路由。实时目录按提供商和密钥指纹隔离，过期的行在瞬时刷新失败时存活，不支持的推理请求字段被省略。接受 `TELECOMJS_MODEL`。提供商别名：`telecom-js`, `telecom_js`, `telecomjs-cn`, `tokenhub`。 |
| `mistral` | `[providers.mistral]` | `MISTRAL_API_KEY` | `MISTRAL_BASE_URL`；默认 `https://api.mistral.ai/v1` | `mistral-code-latest`（默认；`codestral-latest` 接受为别名）, `mistral-medium-latest`（别名：`mistral-medium-3-5`）, `mistral-small-latest`（别名：`mistral-small-2603`）, `mistral-large-latest` | Mistral AI（la Plateforme）OpenAI 兼容 Chat 路由。在文档化的第一方 HTTPS `/v1` 主机上，Medium 和 Small 发送可调的 `reasoning_effort`（仅 `none` 或 `high`），解析 Mistral 的多态 thinking/text 块，并以相同线协议形状重放存储的思考。已废弃的原生 Magistral ID 仍是显式配置兼容路由：它们始终思考，从不接收可调 effort 字段。Code 和 Large 不推理。除非是文档化的第一方主机之一，自定义 `MISTRAL_BASE_URL` 保持通用 Chat 语义。接受 `MISTRAL_MODEL`。提供商别名：`mistral-ai`, `mistralai`, `la-plateforme`。 |
| `edenai` | `[providers.edenai]` | `EDENAI_API_KEY` | `EDENAI_BASE_URL`；默认 `https://api.edenai.run/v3`；欧盟 `https://api.eu.edenai.run/v3` | `deepseek/deepseek-v4-pro`（默认）；`provider/model` id 的实时 `/models` 目录 | Eden AI OpenAI 兼容聚合网关。目录行保持提供商范围；通用推理控制被省略，因为受支持的字段取决于所选上游家族。接受 `EDENAI_MODEL`。默认的 `deepseek/deepseek-v4-pro` 只列在全球目录上；在欧盟端点上把 `EDENAI_MODEL`（或 `model`）设为欧盟 `/models` 列表中的一行，例如 `qwen/deepseek-v4-pro`。提供商别名：`eden-ai`, `eden_ai`。 |
| `xai` | `[providers.xai]` | `XAI_API_KEY`、Codewhale 自有的设备 OAuth，或显式的只读 Grok CLI 授权 | `XAI_BASE_URL`；默认 `https://api.x.ai/v1` | `grok-4.6`（默认）, `grok-4.5`, `grok-4.3`, `grok-build`, `grok-composer-2.5-fast`, `grok-4.20-0309-reasoning`, `grok-4.20-0309-non-reasoning` | xAI/Grok OpenAI 兼容 Chat Completions 路由。Grok 4.6 有 500K 上下文窗口、文本/图像输入、函数调用、结构化输出、服务端 web 搜索和 `low`/`medium`/`high`/`xhigh` 推理（默认 `high`）。提示达到 200K token 时其标准费率翻倍；同样的 2 倍长上下文规则适用于 `grok-4.5`（500K 上下文，$2.00 / $0.30 缓存 / $6.00）和 `grok-4.3`（1M 上下文，$1.25 / $0.20 缓存 / $2.50），按其[模型页](https://docs.x.ai/docs/models/grok-4.5)。没有文档化的 `latest`/`fast` 别名，也没有公布的数字输出上限。**API 密钥**（默认）：来自 console.x.ai 的 Bearer token，通过 `XAI_API_KEY` / keyring / `api_key`。**OAuth**：`codewhale auth xai-device` 使用 SSH 友好的设备登录和 Codewhale 自有的存储，它可能自行刷新。现有的 Grok CLI 凭据需要 `codewhale auth external-consent --provider xai --mode read-only`；被授予的外部文件从不刷新或重写。OAuth 在某些 SuperGrok 层级上可能返回 HTTP 403——把 API 密钥作为可靠回退。接受 `XAI_MODEL`。提供商别名：`x-ai`, `x_ai`, `grok`。 |
| `modelstudio-token-plan` | `[providers.modelstudio_token_plan]` | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` | `MODELSTUDIO_TOKEN_PLAN_BASE_URL`；默认 `https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1` | `qwen3.8-max`（默认）, `qwen3.8-max-preview`, `qwen3.7-plus`, `qwen3.7-max`, `qwen3.6-flash`, `deepseek-v4-pro`, `deepseek-v4-flash-0731`, `glm-5.2` | 阿里云 Model Studio Token Plan OpenAI 兼容 Chat Completions 路由。Token Plan 个人版和团队版共享该端点。列出的所有模型都是可推理的文本/编码模型。DeepSeek 和 GLM 条目是提供商范围的，不与第一方路由冲突。接受 `MODELSTUDIO_TOKEN_PLAN_MODEL`。提供商别名：`modelstudio-token-plan`, `alibaba-token-plan`, `dashscope-token-plan`。 |
| `modelstudio-token-plan-anthropic` | `[providers.modelstudio_token_plan_anthropic]` | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` | 默认 `https://token-plan.ap-southeast-1.maas.aliyuncs.com/apps/anthropic` | 与 `modelstudio-token-plan` 相同的模型目录 | Token Plan Anthropic 兼容 Messages 路由（`/apps/anthropic`）。与 OpenAI 方言相同的 API 密钥。提供商别名：`modelstudio-token-plan-anthropic`, `alibaba-token-plan-anthropic`。 |
| `modelstudio-coding-plan` | `[providers.modelstudio_coding_plan]` | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` | `MODELSTUDIO_CODING_PLAN_BASE_URL`；默认 `https://coding-intl.dashscope.aliyuncs.com/v1` | `qwen3.8-max`（默认）；与 Token Plan 相同的目录 | 阿里云 Model Studio Coding Plan OpenAI 兼容 Chat Completions 路由。接受 `MODELSTUDIO_CODING_PLAN_MODEL`。提供商别名：`modelstudio-coding-plan`, `alibaba-coding-plan`, `dashscope-coding-plan`。 |
| `modelstudio-coding-plan-anthropic` | `[providers.modelstudio_coding_plan_anthropic]` | `MODELSTUDIO_API_KEY`, `DASHSCOPE_API_KEY` | 默认 `https://coding-intl.dashscope.aliyuncs.com/apps/anthropic` | 与 `modelstudio-coding-plan` 相同的模型目录 | Coding Plan Anthropic 兼容 Messages 路由（`/apps/anthropic`）。提供商别名：`modelstudio-coding-plan-anthropic`, `alibaba-coding-plan-anthropic`。 |

### OpenCode Zen 协议目录

Zen Responses 和 Chat Completions 请求使用 Bearer `Authorization` 认证；Zen Anthropic Messages 请求使用 `x-api-key`。这些路由都不会添加 ChatGPT/Codex OAuth 头。

捆绑的 Zen 传输快照遵循[官方端点表](https://opencode.ai/docs/zen/)并且刻意保持显式：

- Responses：`gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna`, `gpt-5.5`,
  `gpt-5.5-pro`, `gpt-5.4`, `gpt-5.4-pro`, `gpt-5.4-mini`, `gpt-5.4-nano`,
  `gpt-5.3-codex`, `gpt-5.3-codex-spark`, `gpt-5.2`, `gpt-5.2-codex`,
  `gpt-5.1`, `gpt-5.1-codex`, `gpt-5.1-codex-max`,
  `gpt-5.1-codex-mini`, `gpt-5`, `gpt-5-codex`, `gpt-5-nano`。
- Anthropic Messages：`claude-fable-5`, `claude-opus-4-8`,
  `claude-opus-4-7`, `claude-opus-4-6`, `claude-opus-4-5`,
  `claude-sonnet-5`, `claude-sonnet-4-6`, `claude-sonnet-4-5`,
  `claude-haiku-4-5`, `qwen3.7-max`, `qwen3.7-plus`, `qwen3.6-plus`,
  `qwen3.5-plus`。
- Chat Completions：`deepseek-v4-pro`, `deepseek-v4-flash`, `minimax-m3`,
  `minimax-m2.7`, `minimax-m2.5`, `glm-5.2`, `glm-5.1`, `glm-5`,
  `kimi-k2.5`, `kimi-k2.6`, `kimi-k2.7-code`, `grok-4.5`,
  `grok-build-0.1`, `big-pickle`, `mimo-v2.5-free`,
  `north-mini-code-free`, `nemotron-3-ultra-free`,
  `deepseek-v4-flash-free`。

Gemini 条目被排除，因为官方表给它们分配了 Google 的模型特定协议。目录缺失永远不会回退到另一种 Zen 线协议形态，包括配置了自定义 Zen base URL 时。

### Hugging Face Provider 与 MCP 与 Hub

Codewhale 的 `huggingface` 提供商 ID 只是通过 Hugging Face Inference Providers 的 OpenAI 兼容聊天推理路由。它用 `/provider huggingface`、`CODEWHALE_PROVIDER=huggingface` 或 `provider = "huggingface"` 选择。

Hugging Face MCP 是单独的外部工具路由。通过 `docs/MCP.md` 中描述的 MCP 配置来配置它，最好使用 <https://huggingface.co/settings/mcp> 生成的设置片段。在 TUI 中，`/hf mcp status` 检查 Hugging Face MCP 服务器是否出现在解析后的 MCP 配置中，`/hf mcp setup` 打印设置工作流和一个仅占位符的形态，`/hf concepts` 解释 provider/MCP/Hub 的区别。

Hub 发布或仓库管理仍然是通过 Hub 原生工具（如 `huggingface_hub` 或 git）的显式用户操作。`/hf` 助手不会上传到 Hugging Face，也不会执行直接的 Hugging Face Hub HTTP 搜索。

### Xiaomi MiMo 说明

`xiaomi-mimo` 默认为 `mimo-v2.5-pro`，用于长上下文推理和编码工作。聊天选择器还暴露 `mimo-v2.5-pro-ultraspeed` 和最新的 Omni 模型 `mimo-v2.5`。Xiaomi MiMo TTS 可通过 `codewhale --provider xiaomi-mimo speech "text" --model tts`（或 `tts` 别名）使用。在 Act 和 Operate 中，提供商特定的 `speech` / `tts` 工具在配置了 Xiaomi MiMo 路由时可通过延迟发现使用。

`/provider xiaomi-mimo ultraspeed` 和 `/provider xiaomi-mimo pro-ultraspeed` 都选择 `mimo-v2.5-pro-ultraspeed`。`tts`、`voice-design` 和 `voice-clone` 等语音别名与正常的聊天默认值分开。

Token 套餐密钥默认指向新加坡端点 `https://token-plan-sgp.xiaomimimo.com/v1`。如果你的 MiMo 账户为中国区配置，请在 `[providers.xiaomi_mimo]` 中显式设置 `base_url = "https://token-plan-cn.xiaomimimo.com/v1"` 或设置 `mode = "token-plan-cn"`。欧洲 Token 套餐账户可以设置 `base_url = "https://token-plan-ams.xiaomimimo.com/v1"` 或使用 `mode = "token-plan-ams"`；`mode = "pay-as-you-go"` 选择标准 API 端点和标准 MiMo 密钥族。Xiaomi Token 套餐文档和控制台暴露积分/配额语义，但 Codewhale 目前没有可轮询的文档化余额端点，因此成本显示保持未知，而不是复用另一提供商的 token 价格估算。

Voice-design 和 voice-clone 简写映射到 `mimo-v2.5-tts-voicedesign` 和 `mimo-v2.5-tts-voiceclone`。小米当前的[图像理解指南](https://platform.xiaomimimo.com/docs/en-US/usage-guide/multimodal-understanding/image-understanding)包含用于图像输入的 `mimo-v2.5`。Codewhale 通过单独的 `[vision_model]` / `image_analyze` 路径暴露图像分析；使用 MiMo 做视觉时，把该模型设置为 `mimo-v2.5`。

### OpenRouter 兼容的 Base URL

OpenRouter 兼容网关通常应该留在 `openrouter` 提供商上，使用提供商范围的 `base_url` 覆盖，而不是走通用 `openai` 路由。这样可以让 OpenRouter 风格的推理、流式、缓存使用和命名空间化的线协议模型解析附着在所选路由上：

```toml
provider = "openrouter"

[providers.openrouter]
api_key = "sk-..."
base_url = "https://openrouter-compatible.example/v1"
model = "deepseek/deepseek-v4-pro"
```

Codewhale 在 OpenRouter 提供商范围内保留 `deepseek/` 线协议模型前缀；它不会从该模型字符串推断切换到直接 DeepSeek 提供商。当上游网关发送 `prompt_cache_hit_tokens`、`prompt_cache_miss_tokens` 和 `prompt_tokens_details.cached_tokens` 等缓存字段时，会解析它们。如果某个密钥/账户类型省略了这些字段，Codewhale 将该响应视为这些字段不存在，而不是不同的提供商路由。

OrcaRouter（`https://api.orcarouter.ai/v1`）是专用的具名路由（[OrcaRouter](https://www.orcarouter.ai)），它说同样的 OpenAI Chat Completions 线协议，并提供同样的命名空间化 `vendor/model` 目录。它不需要 OpenRouter 兼容的 `base_url` 覆盖：选择 `provider = "orcarouter"`，它的命名空间化线协议模型（例如 `deepseek/deepseek-v4-pro` 或它自己的 `orcarouter/auto` 路由器）会原样通过，就像它们在 OpenRouter 提供商范围上一样。

### 近期 OpenRouter 大模型

OpenRouter completions 和静态注册表行包括自 2026 年 4 月起通过 OpenRouter 模型元数据验证的大模型：`arcee-ai/trinity-large-thinking`、`qwen/qwen3.6-flash`、`qwen/qwen3.6-35b-a3b`、`qwen/qwen3.6-max-preview`、`qwen/qwen3.6-27b`、`qwen/qwen3.6-plus`、`minimax/minimax-m3`、`xiaomi/mimo-v2.5-pro`、`xiaomi/mimo-v2.5`、`moonshotai/kimi-k2.7-code`、`moonshotai/kimi-k2.6`、`z-ai/glm-5.1`、`z-ai/glm-5.2`、`z-ai/glm-5-turbo`、`tencent/hy3-preview`、`google/gemma-4-31b-it`、`google/gemma-4-26b-a4b-it` 和 `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`。`minimax/minimax-m3` 是从 OpenRouter 2026 年 5 月 31 日的列表中新增的，作为 1M 上下文的多模态模型，用于编码、工具使用和长周期代理工作。`GLM-5.3` 现在是直接 Z.AI Coding 套餐的默认模型；`GLM-5.2` / `z-ai/glm-5.2` 仍然可用（显式选择保留自己的 id），`GLM-5.1` / `z-ai/glm-5.1` 作为更小的模型仍然可用，`GLM-5-Turbo` / `z-ai/glm-5-turbo` 作为同族更快的兄弟模型，供 faster/explore 子代理使用。`GLM-5.3` / `z-ai/glm-5.3` 是 Z.ai 和 OpenRouter 路由上的一等选择器 id（`/provider zai` 之后的 `/model`，或 `model = "GLM-5.3"`）。限制和推理选项继承自 `GLM-5.2`，直到 Z.ai 发布不同的 5.3 元数据，而且它们不带价格。在未为 5.3 配置的账户上，实时调用仍可能以 entitlement 代码 1311 返回 429。

## 静态模型注册表

`codewhale model list` 和 `codewhale model resolve` 使用 `crates/agent/src/lib.rs` 中的静态注册表。这与实时 `/models` 发现不同。当端点支持模型列表时，使用 `/models` 或 `codewhale models` 从活动的 API 端点获取模型 ID。

| 提供商 | 静态注册表条目 | 工具调用 | 注册表推理标志 |
| --- | --- | --- | --- |
| `deepseek` | `deepseek-v4-pro`, `deepseek-v4-flash` | 是 | 是 |
| `nvidia-nim` | `deepseek-ai/deepseek-v4-pro`, `deepseek-ai/deepseek-v4-flash` | 是 | 是 |
| `openai` | `deepseek-v4-pro`, `deepseek-v4-flash`, `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | 是 | 是 |
| `atlascloud` | `deepseek-ai/deepseek-v4-flash`, `deepseek-ai/deepseek-v4-pro` | 是 | 是 |
| `wanjie-ark` | `deepseek-reasoner` | 是 | 是 |
| `volcengine` | `DeepSeek-V4-Pro`, `DeepSeek-V4-Flash` | 是 | 是 |
| `openrouter` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`, `arcee-ai/trinity-large-thinking`, `minimax/minimax-m3`, `minimax/minimax-m2.7`, `xiaomi/mimo-v2.5-pro`, `xiaomi/mimo-v2.5`, `qwen/qwen3.6-flash`, `qwen/qwen3.6-35b-a3b`, `qwen/qwen3.6-max-preview`, `qwen/qwen3.6-27b`, `qwen/qwen3.6-plus`, `qwen/qwen3.7-max`, `moonshotai/kimi-k2.7-code`, `moonshotai/kimi-k2.6`, `z-ai/glm-5.1`, `z-ai/glm-5.2`, `z-ai/glm-5.3`, `z-ai/glm-5-turbo`, `tencent/hy3-preview`, `google/gemma-4-31b-it`, `google/gemma-4-26b-a4b-it`, `nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free`, `nvidia/nemotron-3-ultra-550b-a55b` | 是 | 是 |
| `orcarouter` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash`, `orcarouter/auto` | 是 | 是 |
| `xiaomi-mimo` | `mimo-v2.5-pro`, `mimo-v2.5-pro-ultraspeed`, `mimo-v2.5`；语音/TTS ID 通过 `codewhale speech` / `tts` 选择 | 是 | 聊天模型是；语音/TTS 模型否 |
| `novita` | `deepseek/deepseek-v4-pro`, `deepseek/deepseek-v4-flash` | 是 | 是 |
| `fireworks` | `accounts/fireworks/models/deepseek-v4-pro` | 是 | 是 |
| `siliconflow` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 是 | 是 |
| `arcee` | `trinity-large-thinking`, `trinity-large-preview`；提供商提示的自定义模型 ID 原样通过 | 是 | `trinity-large-thinking` 是；`trinity-large-preview` 否 |
| `moonshot` | `kimi-k2.7-code`, `kimi-k2.6` | 是 | 是 |
| `zai` | `GLM-5.3`, `GLM-5.2`, `GLM-5.1`, `GLM-5-Turbo`；提供商提示的自定义模型 ID 原样通过 | 是 | 是 |
| `stepfun` | `step-3.7-flash` | 是 | 否 |
| `minimax` | `MiniMax-M3`, `MiniMax-M2.7`, `MiniMax-M2.7-highspeed`, `MiniMax-M2.5`, `MiniMax-M2.5-highspeed`, `MiniMax-M2.1`, `MiniMax-M2.1-highspeed`, `MiniMax-M2` | 是 | 是 |
| `minimax-anthropic` | `MiniMax-M3`, `MiniMax-M2.7` | 是 | 是 |
| `sglang` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 是 | 是 |
| `vllm` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 是 | 是 |
| `ollama` | 实况本地标签；提供商提示为 `ollama` 时自定义标签原样通过 | 是 | 否 |
| `ollama-cloud` | `gpt-oss:120b`；任意的提供商自有模型 ID 原样通过 | 是 | 是 |
| `huggingface` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 是 | 否 |
| `deepinfra` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash` | 是 | 是 |
| `together` | `deepseek-ai/DeepSeek-V4-Pro`, `deepseek-ai/DeepSeek-V4-Flash`, `thinkingmachines/inkling` | 是 | 是 |
| `openai-codex` | `gpt-5.5` | 是 | 是 |
| `anthropic` | `claude-opus-5`, `claude-opus-4-8`, `claude-sonnet-5`, `claude-sonnet-4-6`, `claude-fable-5`, `claude-haiku-4-5` | 是 | 除 `claude-haiku-4-5` 外都是 |
| `openmodel` | `deepseek-v4-flash`；提供商范围的自定义模型 ID 原样通过 | 是 | 取决于模型 |
| `sakana` | `fugu`, `fugu-ultra-20260615` | 是 | `fugu-ultra-20260615` 是 |
| `longcat` | `LongCat-2.0` | 是 | 是 |
| `opencode-go` | `deepseek-v4-pro`, `grok-4.5`, `glm-5.2`, `glm-5.1`, `kimi-k3`, `kimi-k2.7-code`, `kimi-k2.6`, `deepseek-v4-flash`, `mimo-v2.5`, `mimo-v2.5-pro` | 是 | 是 |
| `meta` | `muse-spark-1.2` | 是 | 是 |
| `xai` | `grok-4.6`, `grok-4.5`, `grok-4.3`, `grok-build`, `grok-composer-2.5-fast`, `grok-4.20-0309-reasoning`, `grok-4.20-0309-non-reasoning` | 是 | `grok-4.6`, `grok-4.5`, `grok-4.3`, `grok-build` 和 `grok-4.20-0309-reasoning` 是 |
| `google` | `gemini-3.1-pro-preview`, `gemini-3-pro-preview`, `gemini-3.7-flash`, `gemini-3.6-flash`, `gemini-3.5-flash`, `gemini-3.5-flash-lite`, `gemini-2.5-pro`, `gemini-2.5-flash` | 是 | 除 `gemini-3.5-flash-lite` 外都是 |
| `mistral` | `mistral-code-latest`, `mistral-medium-latest`, `mistral-small-latest`, `mistral-large-latest` | 是 | Medium 和 Small 是（在精确的第一方路由上 `reasoning_effort` 为 `none` 或 `high`）；已弃用的原生 Magistral 仍然是始终启用的显式兼容 ID；Code 和 Large 否 |
| `modelstudio-token-plan`, `modelstudio-coding-plan` | `qwen3.8-max`, `qwen3.8-max-preview`, `qwen3.7-plus`, `qwen3.7-max`, `qwen3.6-flash`, `deepseek-v4-pro`, `deepseek-v4-flash-0731`, `glm-5.2` | 是 | 是 |

AtlasCloud 与配置层保持相同的默认模型，并为 Pro 和 Flash 行添加提供商范围的别名。其他 AtlasCloud 模型 ID 仍应通过 `ATLASCLOUD_MODEL`、配置或可用的实时模型列表来选择。

## 能力元数据

`codewhale-tui doctor --json` 暴露 `capability` 对象。它是静态元数据，不是实时 API 探测。当前字段是：

`resolved_provider`, `resolved_model`, `context_window`, `max_output`,
`thinking_supported`, `cache_telemetry_supported` 和 `request_payload_mode`。

当配置无法加载或验证时，`doctor --json` 以非零退出，并打印一个有界、密钥已打码的 JSON 错误包，`status = "error"` 且 `error.kind = "config_validation"`，而不是发出误导性的路由或能力元数据。

大多数随附提供商使用 Chat Completions 请求负载模式。原生 Messages 路由（包括 `minimax-anthropic`）使用 `/v1/messages`，而 `openai-codex` 使用 Responses。

对于实际窗口与静态表不同的 OpenAI 兼容网关或自托管运行时，设置 `[providers.<name>] context_window = N`。配置的值会成为提示、上下文压力检查、压缩和输出预算的生效路由上下文窗口。

`max_output` 是可选的并且如实:当路由没有发布我们能担保的输出上限时,它是 `null`(并在线协议上的 capability 结构中被省略)--Kimi Code 会员 `kimi-for-coding` 家族是典型示例,因为会员目录拥有它们的限制。未知的输出上限永远不会用占位符回填,而且它不会对一轮请求的 `max_tokens` 应用**任何**兼容性钳制;只有具体的路由/供应上限才会收窄请求。目录中根本没有该模型的行是另一个事实--缺失不是许可,所以未编目的 id 会保持保守的上限。下表中只要不存在文档化的上限，"Max output 元数据"列就显示 `unknown`。

确切的 Kimi Code 会员名册包含 `k3`、`k3-256k`、`kimi-for-coding` 和 `kimi-for-coding-highspeed`。两个 K3 id 共享相同的推理和固定采样契约；`k3-256k` 保持在 262,144 token，而裸 `k3` 可以使用有资格的 1M 覆盖。

| 提供商/模型类别 | 上下文窗口 | Max output 元数据 | 思考支持 | 缓存遥测 | FIM 端点 |
| --- | --- | --- | --- | --- | --- |
| DeepSeek V4（`deepseek-v4-pro`, `deepseek-v4-flash`） | 1,000,000 | 384,000 | 是 | 是 | 仅 DeepSeek beta |
| DeepSeek 兼容别名（`deepseek-chat`, `deepseek-reasoner`） | 1,000,000 | 384,000 | 是 | 是 | 仅 DeepSeek beta |
| NVIDIA NIM V4 注册表模型 | 1,000,000 | 384,000 | 是 | 是 | 代码中未记录 |
| Volcengine Ark V4 模型 ID | 1,000,000 | 384,000 | 是 | 是 | 代码中未记录 |
| OpenRouter、Novita、Fireworks、SiliconFlow、SGLang 和 vLLM V4 模型 ID | 1,000,000 | 384,000 | 是 | 否 | 代码中未记录 |
| Xiaomi MiMo `mimo-v2.5-pro`, `mimo-v2.5-pro-ultraspeed`, `mimo-v2.5` | 1,000,000 | 131,072 | 是 | 否 | 代码中未记录 |
| OpenRouter Qwen 3.6 Flash / Plus | 1,000,000 | 65,536 | 是 | 否 | 代码中未记录 |
| OpenRouter Qwen 3.6 35B / 27B | 262,144 | 262,140 | 是 | 否 | 代码中未记录 |
| OpenRouter Qwen 3.6 Max Preview | 262,144 | 65,536 | 是 | 否 | 代码中未记录 |
| OpenAI API `gpt-5.5` | 1,050,000 | 128,000 | 是 | 否 | 代码中未记录 |
| OpenAI API `gpt-5.6`, `gpt-5.6-sol`, `gpt-5.6-terra`, `gpt-5.6-luna` | 1,050,000 | 128,000 | 是 | 否 | 代码中未记录 |
| Anthropic API `claude-opus-5`, `claude-opus-4-8`, `claude-sonnet-5`, `claude-sonnet-4-6`, `claude-fable-5` | 1,000,000 | 128,000 | 是 | 是 | 代码中未记录 |
| Google Gemini API `gemini-3.7-flash`, `gemini-3.6-flash`, `gemini-3.5-flash`, `gemini-3.5-flash-lite`, `gemini-3.1-pro-preview`, `gemini-2.5-pro`, `gemini-2.5-flash` | 1,048,576 | 65,536 | 取决于模型 | 否 | 代码中未记录 |
| Meta Model API `muse-spark-1.2` | 1,000,000 | 32,000 | 是 | 否 | 代码中未记录 |
| OpenAI Codex / ChatGPT 路由（`openai-codex`） | 生效 400,000 | 128,000 | 是 | 否 | 路由在 `/codex/responses` 使用 Responses 负载 |
| OpenModel 默认/自定义模型 ID | 回退 200,000，除非模型元数据或配置覆盖 | 回退 64,000 | 取决于模型 | 否 | 路由在 `/v1/messages` 使用 Messages 负载 |
| Wanjie Ark `reasoner` / `r1` 模型 ID | 128,000 | 未知（无文档化上限） | 是 | 否 | 代码中未记录 |
| 直接 Arcee API `trinity-large-thinking` | 262,144 | 262,144 | 是 | 否 | 代码中未记录 |
| 直接 Arcee API `trinity-large-preview` | 262,144 | 未知（无文档化上限） | doctor 能力元数据中否 | 否 | 代码中未记录 |
| 直接 Moonshot `kimi-k3` | 1,048,576 | 文档化上限 1,048,576；提供商默认 131,072 | 是 | 否 | 精确路由使用 `max_completion_tokens` 并省略固定采样字段（[K3 快速入门](https://platform.kimi.ai/docs/guide/kimi-k3-quickstart) |
| Kimi Code 会员 `k3` | 安全基线 262,144；显式 entitle-plan 覆盖时为 1,048,576 | 保守默认上限 131,072；会员上限未发布 | 是 | 否 | 精确 `https://api.kimi.com/coding/v1` 路由 |
| Kimi Code 会员 `k3-256k` | 262,144 固定 | 保守默认上限 131,072；会员上限未发布 | 是 | 否 | 精确 `https://api.kimi.com/coding/v1` 路由 |
| 直接 Moonshot/Kimi K2.7/K2.6（`kimi-k2.7-code`, `kimi-k2.7-code-highspeed`, `kimi-k2.6`） | 262,144 | 32,768 | 是 | 否 | 提供商报告的捆绑目录 |
| Kimi Code 会员 `kimi-for-coding`, `kimi-for-coding-highspeed` | 262,144 | 未知——会员目录拥有这些限制，不声称客户端侧上限 | 是 | 否 | 精确 `https://api.kimi.com/coding/v1` 路由 |
| 直接 Z.AI `GLM-5.3`（默认） | 1,000,000 | 131,072 | 是 | 否 | GLM Coding 套餐上实时可用；限制继承自 `GLM-5.2`，直到 Z.ai 发布不同的 5.3 数字；无 USD 价格 |
| 直接 Z.AI `GLM-5.2` | 1,000,000 | 131,072 | 是 | 否 | 代码中未记录 |
| 直接 Z.AI `GLM-5.1` | 202,752 | 131,072 | 是 | 否 | 代码中未记录 |
| 直接 Z.AI `GLM-5-Turbo` | 202,752 | 131,072 | 是 | 否 | faster/explore 子代理兄弟模型 |
| 直接 MiniMax `MiniMax-M3` | 1,000,000 | 524,288 | 是 | 否 | 代码中未记录 |
| 直接 MiniMax M2.x 模型 | 204,800 | 未知，直到 MiniMax output 元数据被提升 | 是 | 否 | 代码中未记录 |
| MiniMax Messages 路由（`MiniMax-M3`, `MiniMax-M2.7`） | 上面的模型特定值 | 上面的模型特定值 | 是 | 否 | 路由使用 `/anthropic/v1/messages` |
| 通用 `openai` 和 AtlasCloud | 128,000 | 未知（无文档化上限） | doctor 能力元数据中否 | 否 | 代码中未记录 |
| Ollama | 8,192 | 未知（无文档化上限） | 否 | 否 | 代码中未记录 |
| Hugging Face Inference Providers V4 模型 ID | 131,072 | 未知（无文档化上限） | 是 | 否 | 代码中未记录 |
| 其他已识别的 DeepSeek 模型 ID | 128,000，除非模型名带显式 `Nk` 提示 | 未知（无文档化上限） | 除非 V4/reasoner 逻辑匹配，否则否 | 仅 DeepSeek/NIM | 仅 DeepSeek beta |

MiniMax M3 使用输入长度和服务层级。Codewhale 省略 `service_tier`，因此请求使用标准层级，成本估算从总输入用量中选择正确的标准费率。优先费率被列出以保持官方层级结构可见。价格为每百万 token 的美元。

| 模型 / 服务层级 | 输入长度 | 输入 | 输出 | 缓存读取 | 缓存写入 |
| --- | --- | ---: | ---: | ---: | ---: |
| `MiniMax-M3` 标准 | 最多 512,000 个输入 token | $0.30 | $1.20 | $0.06 | 未发布 |
| `MiniMax-M3` 标准 | 超过 512,000 个输入 token | $0.60 | $2.40 | $0.12 | 未发布 |
| `MiniMax-M3` 优先 | 最多 512,000 个输入 token | $0.45 | $1.80 | $0.09 | 未发布 |
| `MiniMax-M3` 优先 | 超过 512,000 个输入 token | $0.90 | $3.60 | $0.18 | 未发布 |
| `MiniMax-M2.7` 标准 | 所有受支持的输入 | $0.30 | $1.20 | $0.06 | $0.375 |

这些值来自 [MiniMax 按量付费定价指南](https://platform.minimax.io/docs/guides/pricing-paygo)。M3 思考是自适应的或禁用；OpenAI 兼容 API 默认为自适应，Anthropic 兼容 API 默认为禁用。M2.7 思考无法禁用。当用户选择推理模式时，Codewhale 发送显式控制。

工具调用支持由静态 `ModelRegistry` 和端点接受 OpenAI 兼容 `tools` 负载的能力分别跟踪。自定义 OpenAI 兼容或本地端点仍然可以拒绝工具调用，即使 Codewhale 能够发送该模式。

### Hugging Face Inference Providers 说明

随附的 Hugging Face 路由指向 `https://router.huggingface.co/v1` 的 OpenAI 兼容 Inference Providers 路由器。先用 `HUGGINGFACE_API_KEY` 配置认证，或回退到 `HF_TOKEN`。先用 `HUGGINGFACE_BASE_URL` 配置端点，或回退到 `HF_BASE_URL`；先用 `HUGGINGFACE_MODEL` 配置模型，或回退到 `HF_MODEL`。

此路由不意味着 Hub 浏览、模型卡元数据、数据集访问、Jobs、上传或导出。那些仍然是显式的 Model Lab 工作项，以便提供商认证和工件移动保持分离。

### 当本地模型输出工具 JSON 时

只有提供商返回 Chat Completions `tool_calls` 或流式的 `delta.tool_calls` 时，Codewhale 才会执行工具。如果本地模型在助手消息中打印诸如 `{"name":"File","arguments":{"action":"search_content",...}}` 之类的文本，那是普通的模型输出，不是可执行的工具请求。

对于 OpenAI 兼容或本地运行时，请检查：

- 端点接受 `/v1/chat/completions` 请求中的 `tools` 数组。
- 所选模型或聊天模板已为 function/tool 调用配置。
- 服务器在响应中返回 `tool_calls`，而不是普通 JSON 文本。
- 兼容层不会在转发请求前剥离 tools。
- 如有疑问，在调试 Codewhale 的工具注册表之前，先针对已知的、支持工具调用的模型测试一个小的 `File` `read` 或 `search_content` 动作。

更改 `provider`、`base_url` 或 `model` 可以选择支持 OpenAI 兼容负载形态的路由，但 Codewhale 无法在模型已将其作为散文发出之后，把任意的 JSON 文本转换成受信的工具调用。

DeepSeek 将于 2026-07-24 15:59 UTC 退役 `deepseek-chat` 和 `deepseek-reasoner`。在请求到达 DeepSeek 第一方 OpenAI 或 Anthropic 端点之前，Codewhale 会把任一名称迁移到 `deepseek-v4-flash`。如果未配置推理层级，`deepseek-chat` 也会迁移到 `off`，`deepseek-reasoner` 迁移到 `high`，保留它们原先的非思考/思考意图；显式的 `reasoning_effort` 仍然权威。该映射刻意不是全局的：Wanjie Ark、聚合器、自托管运行时和自定义端点继续拥有它们自己的模型 id。

## 推理强度

`/reasoning <effort>`（以及 `reasoning_effort` 配置键）会在请求发送前由客户端翻译成每个提供商的线协议方言。`off` 在路由支持的地方禁用思考。两个精确的 K3 路由都把 `off` 映射到它们支持的最低层级 `low`，而且模型绝不会为了满足 `off` 而切换——但它们这样做的原因不同：

- **Kimi Code 会员 K3**(精确 `https://api.kimi.com/coding/v1`，`model = "k3"` 或 `model = "k3-256k"`)——会员名册声明 K3 始终思考,所以不改变模型是什么就无法满足 `off`。钳制保留固定的 K3 身份。
- **直接 Moonshot K3**（精确 `https://api.moonshot.ai/v1`，`model = "kimi-k3"`）——这个钳制是*防御性的*，不是文档化契约。直接平台不为 K3 发布 `off` 状态，Codewhale 也不会断言它无法针对给定密钥的 entitlement 验证的固定思考保证，因此请求的 `off` 被规范化为最低层级，同时实时 entitlement 保持未知。

正常分派的 `auto` 使用 Codewhale 的自动推理选择器并发送具体的层级；只有省略推理设置才会让提供商默认值控制。标记为 "omitted" 的提供商在该层级根本不会收到推理字段。

| 提供商 | `off` | `low`/`medium`/`high` | `max`/`xhigh` |
| --- | --- | --- | --- |
| `deepseek`, `deepseek-cn`, `siliconflow`, `siliconflow-CN`, `sglang`, `volcengine`, `atlascloud` | `thinking: {type: disabled}` | `reasoning_effort: "high"` + `thinking: {type: enabled}` | `reasoning_effort: "max"` + `thinking: {type: enabled}` |
| `openrouter`, `novita`, 其他 `together` 模型 | `thinking: {type: disabled}` | `reasoning_effort` 直通 + `thinking: {type: enabled}` | `reasoning_effort: "xhigh"` + `thinking: {type: enabled}` |
| `together` + `thinkingmachines/inkling` | `reasoning_effort: "none"` | 精确的 `minimal`/`low`/`medium`/`high` `reasoning_effort` | `reasoning_effort: "max"` |
| 精确 `https://api.moonshot.ai/v1` 上的直接 Moonshot `kimi-k3` | 顶层 `reasoning_effort: "low"`（生效的规范化） | 顶层 `reasoning_effort: "low"` / `"high"`（`medium` 变成 `high`） | 顶层 `reasoning_effort: "max"` |
| 精确 `https://api.kimi.com/coding/v1` 上的 Kimi Code 会员 `k3`, `k3-256k` | `thinking: {type: enabled, effort: "low"}`（生效的规范化） | `thinking: {type: enabled, effort: "low" | "high"}` | `thinking: {type: enabled, effort: "max"}` |
| 其他 `moonshot` 路由 | `thinking: {type: disabled}` | `thinking: {type: enabled}` | `thinking: {type: enabled}` |
| `ollama` | `think: false` | `think: true` | `think: true` |
| `ollama-cloud` | `reasoning_effort: "none"` | 精确的 `low`/`medium`/`high` `reasoning_effort` | `reasoning_effort: "max"` |
| `xiaomi-mimo` | `thinking: {type: disabled}` | `thinking: {type: enabled}` | `thinking: {type: enabled}` |
| 第一方 `minimax` `MiniMax-M3` | `reasoning_split: true` + `thinking: {type: disabled}` | `reasoning_split: true` + `thinking: {type: adaptive}`；生效的层级粒度不可用 | `reasoning_split: true` + `thinking: {type: adaptive}`；生效的层级粒度不可用 |
| 第一方 Z.ai `GLM-5.2` | `thinking: {type: disabled}`；无 `reasoning_effort` | 启用思考；只有生效的 `high` 才添加 `reasoning_effort: "high"` | 启用思考 + `reasoning_effort: "max"` |
| 第一方 Z.ai `GLM-5.3` | `thinking: {type: disabled}`；无 `reasoning_effort` | 启用思考；只有生效的 `high` 才添加 `reasoning_effort: "high"` | 启用思考 + `reasoning_effort: "max"` |
| 第一方 Z.ai `GLM-5-Turbo` | `thinking: {type: disabled}` | 启用思考；effort 粒度不可用 | 启用思考；effort 粒度不可用 |
| 配置为 `zai` 的兼容网关 | omitted；生效值不可用 | omitted；生效值不可用 | omitted；生效值不可用 |
| `nvidia-nim` | `chat_template_kwargs.thinking: false` | `chat_template_kwargs`：`thinking: true` + `reasoning_effort: "high"` | `chat_template_kwargs`：`thinking: true` + `reasoning_effort: "max"` |
| `vllm` | `chat_template_kwargs.enable_thinking: false` | `chat_template_kwargs.enable_thinking: true` + `reasoning_effort` low/medium/high | `chat_template_kwargs.enable_thinking: true` + `reasoning_effort: "high"`（vLLM 没有 max 层级） |
| `arcee`, `huggingface` | omitted | `reasoning_effort` 直通 | `reasoning_effort: "high"` |
| `fireworks` | omitted | `reasoning_effort: "high"` | `reasoning_effort: "max"` |
| `openai`, `wanjie-ark`, `telecomjs` | omitted | omitted | omitted |
| `openmodel` | Anthropic Messages 适配器处理思考/输出配置 | Anthropic Messages 适配器处理思考/输出配置 | Anthropic Messages 适配器处理思考/输出配置 |
| `openai-codex` | Responses API `reasoning` 字段（由 Responses 桥处理） | Responses API `reasoning` 字段 | Responses API `reasoning` 字段 |

AtlasCloud 提供 DeepSeek 模型，因此它说 DeepSeek 推理方言，包括 `max` 层级（#3024）。

在精确的 MiniMax OpenAI 兼容 Chat 端点上，`MiniMax-M3` 使用 `max_completion_tokens`。其他 MiniMax 模型和兼容网关保留 `max_tokens`；MiniMax Anthropic 端点使用单独的 Messages 适配器。

## 漂移检查

在更改提供商 ID、提供商 TOML 表、静态模型注册表行或提供商默认字符串之前运行它：

```bash
python3 scripts/check-provider-registry.py
```

以下情况检查会失败：

- `docs/PROVIDERS.md` 遗漏了一个规范的 `ProviderKind::as_str()` ID。
- `crates/tui/src/config.rs` 的 `ApiProvider::as_str()` 偏离 `ProviderKind::as_str()`，除了显式的 `deepseek-cn` 旧别名。
- 随附提供商表遗漏或添加了一个 `[providers.*]` TOML 表。
- 静态模型注册表表偏离 `crates/agent/src/lib.rs` 使用的提供商。
- `crates/tui/src/config.rs` 中的提供商默认模型或 base URL 常量在这里不再被提及。

## 已规划、尚未随附

这些项目属于 v0.8.48+ 提供商抽象里程碑或相关的提供商文档工作，但它们不是本 checkout 中原生随附的行为：

- `codewhale-agent` 中一个统一的 `Provider` trait，拥有环境变量优先级、密钥解析、base URL 规范化、认证头构造和提供商元数据。这些职责目前仍然分散在 `crates/config`、`crates/secrets` 和 `crates/tui/src/client.rs` 中。
- 选择器中的 Hugging Face 模型护照元数据，包括许可证、基础模型、上下文长度、聊天模板、工具调用支持、推理支持以及 gated/private 状态。
