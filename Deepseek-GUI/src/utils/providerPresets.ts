// 模型配置：服务商预设（与 deepseek-tui ProviderKind + 常用 OpenAI 兼容网关对齐）

/** 单个服务商/网关预设 */
export interface ProviderPreset {
  /** 下拉唯一键 */
  key: string;
  /** 写入 config.toml 的 provider 字段 */
  providerId: string;
  /** 界面展示名 */
  label: string;
  /** 默认 API Base URL */
  baseUrl: string;
  /** 切换到此预设时的默认模型 */
  defaultModel: string;
  /** 模型名称 datalist 候选 */
  models: string[];
  /** API Key 申请提示 */
  keyHint?: string;
  /** Base URL 说明 */
  urlHint?: string;
}

/** 全部服务商预设（顺序即下拉顺序） */
export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    key: "deepseek",
    providerId: "deepseek",
    label: "DeepSeek 云端",
    baseUrl: "https://api.deepseek.com",
    defaultModel: "deepseek-v4-pro",
    models: ["deepseek-v4-pro", "deepseek-v4-flash", "deepseek-chat", "deepseek-reasoner", "auto"],
    keyHint: "platform.deepseek.com/api_keys",
  },
  {
    key: "deepseek-beta",
    providerId: "deepseek",
    label: "DeepSeek 云端（Beta 端点）",
    baseUrl: "https://api.deepseek.com/beta",
    defaultModel: "deepseek-v4-pro",
    models: ["deepseek-v4-pro", "deepseek-v4-flash", "deepseek-chat", "deepseek-reasoner"],
    keyHint: "platform.deepseek.com/api_keys",
    urlHint: "严格工具模式、FIM 等 Beta 功能需 /beta",
  },
  {
    key: "openai",
    providerId: "openai",
    label: "OpenAI",
    baseUrl: "https://api.openai.com/v1",
    defaultModel: "gpt-4.1",
    models: ["gpt-4.1", "gpt-4o", "gpt-4o-mini", "o3-mini", "o1", "o1-mini"],
    keyHint: "platform.openai.com/api-keys",
  },
  {
    key: "nvidia-nim",
    providerId: "nvidia-nim",
    label: "NVIDIA NIM",
    baseUrl: "https://integrate.api.nvidia.com/v1",
    defaultModel: "deepseek-ai/deepseek-v4-pro",
    models: [
      "deepseek-ai/deepseek-v4-pro",
      "deepseek-ai/deepseek-v4-flash",
      "meta/llama-3.3-70b-instruct",
    ],
    keyHint: "build.nvidia.com",
  },
  {
    key: "openrouter",
    providerId: "openrouter",
    label: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    defaultModel: "deepseek/deepseek-v4-pro",
    models: [
      "deepseek/deepseek-v4-pro",
      "deepseek/deepseek-v4-flash",
      "anthropic/claude-sonnet-4",
      "anthropic/claude-3.5-sonnet",
      "google/gemini-2.5-pro-preview",
      "google/gemini-2.0-flash-001",
      "openai/gpt-4o",
      "meta-llama/llama-3.3-70b-instruct",
    ],
    keyHint: "openrouter.ai/keys",
  },
  {
    key: "novita",
    providerId: "novita",
    label: "Novita AI",
    baseUrl: "https://api.novita.ai/v1",
    defaultModel: "deepseek/deepseek-v4-pro",
    models: ["deepseek/deepseek-v4-pro", "deepseek/deepseek-v4-flash"],
    keyHint: "novita.ai",
  },
  {
    key: "fireworks",
    providerId: "fireworks",
    label: "Fireworks AI",
    baseUrl: "https://api.fireworks.ai/inference/v1",
    defaultModel: "accounts/fireworks/models/deepseek-v4-pro",
    models: [
      "accounts/fireworks/models/deepseek-v4-pro",
      "accounts/fireworks/models/deepseek-v4-flash",
    ],
    keyHint: "fireworks.ai/account/api-keys",
  },
  {
    key: "atlascloud",
    providerId: "atlascloud",
    label: "AtlasCloud",
    baseUrl: "https://api.atlascloud.ai/v1",
    defaultModel: "deepseek-ai/deepseek-v4-flash",
    models: ["deepseek-ai/deepseek-v4-flash", "deepseek-ai/deepseek-v4-pro"],
    keyHint: "atlascloud.ai",
  },
  {
    key: "groq",
    providerId: "openai",
    label: "Groq（OpenAI 兼容）",
    baseUrl: "https://api.groq.com/openai/v1",
    defaultModel: "llama-3.3-70b-versatile",
    models: ["llama-3.3-70b-versatile", "llama-3.1-8b-instant", "mixtral-8x7b-32768"],
    keyHint: "console.groq.com/keys",
  },
  {
    key: "together",
    providerId: "openai",
    label: "Together AI（OpenAI 兼容）",
    baseUrl: "https://api.together.xyz/v1",
    defaultModel: "deepseek-ai/DeepSeek-V3",
    models: [
      "deepseek-ai/DeepSeek-V3",
      "meta-llama/Llama-3.3-70B-Instruct-Turbo",
      "Qwen/Qwen2.5-72B-Instruct-Turbo",
    ],
    keyHint: "api.together.xyz/settings/api-keys",
  },
  {
    key: "moonshot",
    providerId: "openai",
    label: "Moonshot 月之暗面（OpenAI 兼容）",
    baseUrl: "https://api.moonshot.cn/v1",
    defaultModel: "moonshot-v1-8k",
    models: ["moonshot-v1-8k", "moonshot-v1-32k", "moonshot-v1-128k"],
    keyHint: "platform.moonshot.cn/console/api-keys",
  },
  {
    key: "zhipu",
    providerId: "openai",
    label: "智谱 AI（OpenAI 兼容）",
    baseUrl: "https://open.bigmodel.cn/api/paas/v4",
    defaultModel: "glm-4-plus",
    models: ["glm-4-plus", "glm-4-flash", "glm-4-air"],
    keyHint: "open.bigmodel.cn/usercenter/apikeys",
  },
  {
    key: "siliconflow",
    providerId: "openai",
    label: "硅基流动 SiliconFlow（OpenAI 兼容）",
    baseUrl: "https://api.siliconflow.cn/v1",
    defaultModel: "deepseek-ai/DeepSeek-V3",
    models: [
      "deepseek-ai/DeepSeek-V3",
      "deepseek-ai/DeepSeek-R1",
      "Qwen/Qwen2.5-72B-Instruct",
      "Pro/deepseek-ai/DeepSeek-V3",
    ],
    keyHint: "cloud.siliconflow.cn/account/ak",
  },
  {
    key: "ollama",
    providerId: "ollama",
    label: "Ollama 本地",
    baseUrl: "http://localhost:11434/v1",
    defaultModel: "deepseek-coder:1.3b",
    models: ["deepseek-coder:1.3b", "llama3.2", "qwen2.5", "mistral", "codellama"],
    urlHint: "本地需先 ollama pull 对应模型",
  },
  {
    key: "vllm",
    providerId: "vllm",
    label: "vLLM 本地",
    baseUrl: "http://localhost:8000/v1",
    defaultModel: "deepseek-ai/DeepSeek-V4-Pro",
    models: ["deepseek-ai/DeepSeek-V4-Pro", "deepseek-ai/DeepSeek-V4-Flash"],
    urlHint: "默认 http://localhost:8000/v1",
  },
  {
    key: "sglang",
    providerId: "sglang",
    label: "SGLang 本地",
    baseUrl: "http://localhost:30000/v1",
    defaultModel: "deepseek-ai/DeepSeek-V4-Pro",
    models: ["deepseek-ai/DeepSeek-V4-Pro", "deepseek-ai/DeepSeek-V4-Flash"],
    urlHint: "默认 http://localhost:30000/v1",
  },
  {
    key: "custom",
    providerId: "",
    label: "自定义 / 留空",
    baseUrl: "",
    defaultModel: "",
    models: ["deepseek-v4-pro", "deepseek-v4-flash", "gpt-4o", "auto"],
  },
];

/** 根据当前表单匹配预设键；无匹配时返回 custom */
export function presetKeyForForm(provider: string, baseUrl: string): string {
  const normalized = baseUrl.trim().replace(/\/+$/, "");
  const hit = PROVIDER_PRESETS.find((p) => {
    if (p.key === "custom") return false;
    const presetUrl = p.baseUrl.trim().replace(/\/+$/, "");
    return p.providerId === provider && presetUrl === normalized;
  });
  if (hit) return hit.key;
  if (!provider.trim() && !normalized) return "custom";
  return "custom";
}

/** 按 key 查找预设 */
export function findPreset(key: string): ProviderPreset | undefined {
  return PROVIDER_PRESETS.find((p) => p.key === key);
}
