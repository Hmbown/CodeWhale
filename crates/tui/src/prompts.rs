#![allow(dead_code)]
//! System prompts for different modes.
//!
//! Prompts are assembled from composable layers loaded at compile time:
//!   tool taxonomy → base.md → personality overlay → mode delta → approval policy
//!
//! This keeps each concern in its own file and makes prompt tuning
//! a single-file operation.

use crate::models::SystemPrompt;
use crate::project_context::{ProjectContext, load_project_context_with_parents};
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PromptSessionContext<'a> {
    pub user_memory_block: Option<&'a str>,
    pub goal_objective: Option<&'a str>,
    pub project_context_pack_enabled: bool,
    /// Resolved BCP-47 locale tag for the `## Environment` block in
    /// the system prompt (e.g. `"en"`, `"zh-Hans"`, `"ja"`). The
    /// caller is responsible for resolving this from `Settings`; no
    /// disk I/O happens inside the prompt builder, so the workspace-
    /// static portion of the system prompt stays cache-friendly.
    pub locale_tag: &'a str,
    /// When true, a ## Language Output Requirement block is appended
    /// to the system prompt instructing the model to respond in
    /// the resolved session locale.
    pub translation_enabled: bool,
    /// Active model identifier injected into the Constitutional
    /// preamble ("You are {model_id}, running inside CodeWhale").
    /// Defaults to `"codewhale"` when the caller doesn't supply one,
    /// preserving backward compatibility with existing call sites
    /// that predate dynamic model injection.
    pub model_id: &'a str,
    /// Whether the user-visible transcript renders thinking blocks.
    /// When false, the prompt should not spend localization pressure on
    /// `reasoning_content` the user will never see.
    pub show_thinking: bool,
    /// Whether shell tools are available in the runtime tool catalog for
    /// this session. The prompt must not advertise shell-only workflows
    /// when runtime gates have removed those tools.
    pub allow_shell: bool,
}

impl Default for PromptSessionContext<'_> {
    fn default() -> Self {
        Self {
            user_memory_block: None,
            goal_objective: None,
            project_context_pack_enabled: true,
            locale_tag: "en",
            translation_enabled: false,
            model_id: "codewhale",
            show_thinking: true,
            allow_shell: true,
        }
    }
}

/// Conventional location for the structured session relay artifact (#32).
/// A previous session writes it on exit / `/compact`; the next session reads
/// it back on startup and prepends it to the system prompt so a fresh agent
/// doesn't have to re-discover open blockers from scratch.
pub const HANDOFF_RELATIVE_PATH: &str = ".codewhale/handoff.md";
/// Legacy handoff path for reading from existing installs.
const LEGACY_HANDOFF_RELATIVE_PATH: &str = ".deepseek/handoff.md";

/// Per-file size cap for `instructions = [...]` entries (#454). Mirrors
/// the existing project-context cap in `project_context::load_context_file`
/// so a malicious / oversized include can't blow the prompt budget on
/// its own. Files larger than this are truncated with an `[…elided]`
/// marker rather than skipped entirely so the model still sees the head.
const INSTRUCTIONS_FILE_MAX_BYTES: usize = 100 * 1024;

/// System prompt block appended when `translation_enabled` is true.
/// Instructs the model to respond in the resolved session locale for all
/// natural-language output — explanations, summaries, conversation.
/// Code identifiers, untranslatable technical terms, and explicitly
/// requested English code blocks are exempt.
fn translation_output_instruction(locale_tag: &str) -> String {
    let target_language = translation_target_language_for_tag(locale_tag);
    format!(
        "\
## Language Output Requirement\n\
\n\
The user requires all responses in {target_language}. \
Always respond in {target_language} — use natural, professional language for all \
explanations, code comments, summaries, and conversational turns. \
Only output English for:\n\
- Code identifiers (variable names, function names, file paths)\n\
- Technical terms that lack a standard translation in {target_language}\n\
- Code blocks the user explicitly requests in English\n\n\
This is a hard display requirement: the user does not read English, \
so any English prose in your response will block their decision-making."
    )
}

fn translation_target_language_for_tag(locale_tag: &str) -> &'static str {
    let normalized = locale_tag.trim().to_ascii_lowercase();
    if normalized.starts_with("ja") {
        "Japanese (日本語)"
    } else if normalized.starts_with("zh-hant")
        || normalized.contains("-tw")
        || normalized.contains("-hk")
        || normalized.contains("-mo")
    {
        "Traditional Chinese (繁體中文)"
    } else if normalized.starts_with("zh") {
        "Simplified Chinese (简体中文)"
    } else if normalized.starts_with("pt") {
        "Brazilian Portuguese (Português do Brasil)"
    } else if normalized.starts_with("vi") {
        "Vietnamese (Tiếng Việt)"
    } else {
        "English"
    }
}

fn hidden_thinking_language_instruction(locale_tag: &str) -> String {
    let fallback_language = translation_target_language_for_tag(locale_tag);
    format!(
        "\
## Hidden Thinking Language\n\
\n\
The user has disabled thinking display (`show_thinking = false`). If you emit \
`reasoning_content`, keep that hidden internal thinking in English regardless \
of the latest user-message language or `## Environment.lang`; the user will \
not see it, so localizing hidden thinking only adds language switching.\n\
\n\
The final reply is still user-visible. Follow the normal `## Language` rule \
for the final reply: mirror the latest user message, and use \
{fallback_language} only when the user message is ambiguous. If the user \
explicitly asks for a different thinking language, follow that explicit request \
for the current turn."
    )
}

/// Render a `## Environment` block listing the resolved locale tag,
/// runtime version, host platform, login shell, and current working directory.
///
/// The block is appended to the workspace-static portion of the
/// system prompt (after mode prompt + project context, before
/// configured instructions / skills) so the `## Language` directive
/// in `prompts/base.md` can reference it without the model having to
/// guess from the user's first message. `locale_tag` is resolved by
/// the caller from `Settings` so this function stays I/O-free.
fn render_environment_block(workspace: &Path, locale_tag: &str) -> String {
    let deepseek_version = env!("CARGO_PKG_VERSION");
    let platform = std::env::consts::OS;
    let shell = crate::shell_dispatcher::global_dispatcher()
        .kind()
        .binary()
        .to_string();
    let pwd = workspace.display();

    format!(
        "## Environment\n\
         \n\
         - lang: {locale_tag}\n\
         - deepseek_version: {deepseek_version}\n\
         - platform: {platform}\n\
         - shell: {shell}\n\
         - pwd: {pwd}"
    )
}

/// Source for an `EngineConfig.instructions` entry. Either a disk file (loaded
/// at render time, original semantics) or an inline string (content baked into
/// `EngineConfig`, no disk I/O at render time).
///
/// The inline variant is useful for embedders that compute instructions at
/// runtime (e.g. rendering a template with workspace-specific substitutions)
/// and don't want to stage the content to a disk file just to satisfy a path
/// API. Staging adds two problems the inline path avoids:
///
///   1. The disk file looks like editable config but gets overwritten on
///      every launch — confusing for users browsing the install dir.
///   2. Multi-engine setups need per-engine paths to avoid `rehydrate`
///      reading another session's instructions; with inline sources the
///      content lives in the per-engine `EngineConfig` and the race
///      surface goes away.
///
/// `From<PathBuf>` is provided so existing callers passing `Vec<PathBuf>` can
/// keep working with a `.into()` upgrade at the call site.
#[derive(Debug, Clone)]
pub enum InstructionSource {
    /// Load this file from disk at prompt-render time. Original behavior:
    /// missing files are skipped with a warning, oversized files are
    /// truncated to `INSTRUCTIONS_FILE_MAX_BYTES` with an `[…elided]`
    /// marker.
    File(PathBuf),
    /// Use the provided string directly. `name` becomes the
    /// `<instructions source="…">` attribute (typically a synthetic
    /// identifier like `embedded:my-template` or a logical path).
    Inline { name: String, content: String },
}

impl From<PathBuf> for InstructionSource {
    fn from(path: PathBuf) -> Self {
        InstructionSource::File(path)
    }
}

impl From<&PathBuf> for InstructionSource {
    fn from(path: &PathBuf) -> Self {
        InstructionSource::File(path.clone())
    }
}

/// Render the `instructions = [...]` config array as a single
/// system-prompt block (#454). Each source is processed in declared order;
/// missing `File` sources are skipped with a tracing warning so a stale entry
/// doesn't fail the launch. Empty input (or all sources missing/empty)
/// returns `None` so callers append nothing.
fn render_instructions_block(sources: &[InstructionSource]) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    for source in sources {
        let (raw_source_name, raw_content): (String, String) = match source {
            InstructionSource::File(path) => match std::fs::read_to_string(path) {
                Ok(raw) => (path.display().to_string(), raw),
                Err(err) => {
                    tracing::warn!(
                        target: "instructions",
                        ?err,
                        ?path,
                        "skipping unreadable instructions file"
                    );
                    continue;
                }
            },
            InstructionSource::Inline { name, content } => (name.clone(), content.clone()),
        };
        let trimmed = raw_content.trim();
        if trimmed.is_empty() {
            continue;
        }
        let body = if trimmed.len() > INSTRUCTIONS_FILE_MAX_BYTES {
            let head_end = (0..=INSTRUCTIONS_FILE_MAX_BYTES)
                .rev()
                .find(|&i| trimmed.is_char_boundary(i))
                .unwrap_or(0);
            format!("{}\n[…elided]", &trimmed[..head_end])
        } else {
            trimmed.to_string()
        };
        sections.push(format!(
            "<instructions source=\"{raw_source_name}\">\n{body}\n</instructions>"
        ));
    }
    if sections.is_empty() {
        None
    } else {
        Some(sections.join("\n\n"))
    }
}

/// Read the workspace-local relay artifact, if present, and format it as a
/// system-prompt block. Returns `None` when the file is absent or empty so
/// callers can keep the default-uncluttered prompt for fresh workspaces.
fn load_handoff_block(workspace: &Path) -> Option<String> {
    let primary = workspace.join(HANDOFF_RELATIVE_PATH);
    let path = if primary.exists() {
        primary
    } else {
        workspace.join(LEGACY_HANDOFF_RELATIVE_PATH)
    };
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(
        "## Previous Session Relay\n\nThe previous session in this workspace left a relay artifact at `{HANDOFF_RELATIVE_PATH}`. Consider it the first artifact to read on this turn — open blockers, in-flight changes, and recent decisions live there. Update or rewrite it before exiting if state changes materially.\n\n{trimmed}"
    ))
}

// ── Prompt layers loaded at compile time ──────────────────────────────

/// Core: task execution, tool-use rules, output format, toolbox reference,
/// "When NOT to use" guidance, sub-agent sentinel protocol.
pub const BASE_PROMPT: &str = include_str!("prompts/base.md");

// ── Embedder prompt overrides ──
// Let an embedder replace these compile-time prompt constants at startup,
// so brand / slimming customizations live in the embedder crate instead of
// editing these files in-tree. Unset → the bundled constant (fully
// backward compatible). Intended to be set once at process start, before
// any engine spawns; later sets return the rejected override string.
static BASE_PROMPT_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_ZH_HANS_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_JA_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_PT_BR_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_PREAMBLE_VI_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_ZH_HANS_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_JA_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_PT_BR_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static LOCALE_CLOSER_VI_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();
static AUTHORITY_RECAP_OVERRIDE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Replace `BASE_PROMPT` for all subsequent prompt composition. First call
/// wins; later calls return the rejected string. Set before spawning any
/// engine.
pub fn set_base_prompt_override(s: String) -> Result<(), String> {
    set_prompt_override(&BASE_PROMPT_OVERRIDE, s)
}

/// Replace the Simplified-Chinese locale preamble (`## 语言要求`).
pub fn set_locale_preamble_zh_hans_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_ZH_HANS_OVERRIDE, s)
}

/// Replace the Japanese locale preamble.
pub fn set_locale_preamble_ja_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_JA_OVERRIDE, s)
}

/// Replace the Brazilian-Portuguese locale preamble.
pub fn set_locale_preamble_pt_br_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_PT_BR_OVERRIDE, s)
}

/// Replace the Vietnamese locale preamble.
pub fn set_locale_preamble_vi_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_PREAMBLE_VI_OVERRIDE, s)
}

/// Replace the Simplified-Chinese locale closer (`## 语言再次提醒`).
pub fn set_locale_closer_zh_hans_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_ZH_HANS_OVERRIDE, s)
}

/// Replace the Japanese locale closer.
pub fn set_locale_closer_ja_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_JA_OVERRIDE, s)
}

/// Replace the Brazilian-Portuguese locale closer.
pub fn set_locale_closer_pt_br_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_PT_BR_OVERRIDE, s)
}

/// Replace the Vietnamese locale closer.
pub fn set_locale_closer_vi_override(s: String) -> Result<(), String> {
    set_prompt_override(&LOCALE_CLOSER_VI_OVERRIDE, s)
}

/// Replace the trailing `## Authority Recap` block.
pub fn set_authority_recap_override(s: String) -> Result<(), String> {
    set_prompt_override(&AUTHORITY_RECAP_OVERRIDE, s)
}

fn set_prompt_override(cell: &std::sync::OnceLock<String>, s: String) -> Result<(), String> {
    cell.set(s)
}

fn effective_prompt_override<'a>(
    cell: &'a std::sync::OnceLock<String>,
    fallback: &'static str,
) -> &'a str {
    cell.get().map(String::as_str).unwrap_or(fallback)
}

fn effective_base_prompt() -> &'static str {
    effective_prompt_override(&BASE_PROMPT_OVERRIDE, BASE_PROMPT)
}

fn effective_locale_preamble_zh_hans() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_ZH_HANS_OVERRIDE, LOCALE_PREAMBLE_ZH_HANS)
}

fn effective_locale_preamble_ja() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_JA_OVERRIDE, LOCALE_PREAMBLE_JA)
}

fn effective_locale_preamble_pt_br() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_PT_BR_OVERRIDE, LOCALE_PREAMBLE_PT_BR)
}

fn effective_locale_preamble_vi() -> &'static str {
    effective_prompt_override(&LOCALE_PREAMBLE_VI_OVERRIDE, LOCALE_PREAMBLE_VI)
}

fn effective_locale_closer_zh_hans() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_ZH_HANS_OVERRIDE, LOCALE_CLOSER_ZH_HANS)
}

fn effective_locale_closer_ja() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_JA_OVERRIDE, LOCALE_CLOSER_JA)
}

fn effective_locale_closer_pt_br() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_PT_BR_OVERRIDE, LOCALE_CLOSER_PT_BR)
}

fn effective_locale_closer_vi() -> &'static str {
    effective_prompt_override(&LOCALE_CLOSER_VI_OVERRIDE, LOCALE_CLOSER_VI)
}

fn effective_authority_recap() -> &'static str {
    effective_prompt_override(&AUTHORITY_RECAP_OVERRIDE, AUTHORITY_RECAP)
}

/// Optional locale-native reinforcement preamble prepended to the system
/// prompt when the user's UI locale is non-English.
///
/// `base.md` itself stays English (single source of truth, model is
/// natively multilingual, prefix-cache stable across users in the same
/// locale). For non-English locales we prepend a short locale-native
/// passage so the model's first exposure to the prompt overrides the
/// "match user message language" English directive with an explicit
/// "use {locale}" instruction in the user's own writing system. Reduces
/// the model's reliance on inferring intent from `## Environment.lang`
/// — which previously got overpowered by overwhelmingly English task
/// context, the symptom reported in #1118 and visible in the WeChat
/// screenshot that prompted this change.
///
/// The list is intentionally short (only locales the TUI ships UI
/// strings for: `zh-Hans`, `ja`, `pt-BR`). Other locales fall through
/// to `None` and get the English-only directive, which is the same
/// behavior as before this change.
///
/// ## Design philosophy: why a bookend, not a full translation
///
/// Community feedback on the WeChat thread that prompted this work
/// pointed out — correctly — that DeepSeek V4 is a Chinese-first
/// multilingual model, not an English-only model with multilingual
/// veneer. Its tokenizer is co-trained on Chinese; `你好` typically
/// encodes to ~1 token, not 2 — the "Chinese is expensive in tokens"
/// folk wisdom from Western-LLM commentary doesn't apply here.
///
/// The naïve translation of that argument would be: ship a fully
/// translated `base.md` per locale. We deliberately stop short of
/// that for v0.8.29. The reasons, ranked:
///
///   1. **Drift risk.** A 200+ line technical prompt has subtle
///      phrasing that drives subtle behavior. Every rule change has
///      to land in N translated copies, kept in lockstep. The class
///      of bug that arises (Chinese users see slightly different
///      agent behavior than English users) is hard to reproduce and
///      hard to triage from bug reports.
///   2. **Cache stability.** With one English `base.md` and a
///      per-locale preamble+closer, the largest cacheable chunk
///      (mode prompt + project context + environment) stays
///      byte-stable within a session and across users in the same
///      locale. A fully translated per-locale `base.md` keeps cache
///      per-locale but doesn't share with English users.
///   3. **Translation QA is expensive.** Each prompt-language pair
///      needs a native speaker reviewing tone, register, and rule
///      preservation. Getting it 95% right is bad, because the
///      missing 5% becomes silent behavior divergence.
///
/// What we DO instead — the bookend pattern @MuMu described from
/// their other project — is reinforce the locale directive in
/// native script at BOTH ends of the prompt. The opening anchors
/// behavior at session start; the closing reinforcement
/// (`locale_reinforcement_closer`) sits at the maximum-recency
/// position right before the user's next message. Empirically this
/// is sufficient to keep `reasoning_content` in the target locale
/// even as English code accumulates in context turn-over-turn.
///
/// If at some future point the bookend proves insufficient — or if
/// the maintenance cost of per-locale `base.md` files becomes
/// preferable to whatever's blocking it — full translation is the
/// natural next step. The locale tags here, the test invariants,
/// and the closer position would all carry over unchanged.
pub(crate) fn locale_reinforcement_preamble(locale_tag: &str) -> Option<&'static str> {
    match locale_tag {
        "zh-Hans" | "zh-CN" | "zh" => Some(effective_locale_preamble_zh_hans()),
        "ja" | "ja-JP" => Some(effective_locale_preamble_ja()),
        "pt-BR" | "pt" => Some(effective_locale_preamble_pt_br()),
        "vi" | "vi-VN" => Some(effective_locale_preamble_vi()),
        _ => None,
    }
}

/// Locale-native closing reinforcement appended to the very end of the
/// system prompt — the bookend MuMu described in the WeChat thread that
/// prompted #1118 follow-up work.
///
/// The opening preamble alone is not enough: as the model accumulates
/// English context turn-over-turn (code, error logs, search results,
/// file listings), the recency bias of the transformer's attention
/// drifts thinking back toward English even when the user keeps writing
/// in their own language. A closing native-script reinforcement sits at
/// the position closest to the user's next message — where attention
/// weight is highest — and re-asserts the language rule right before
/// the model generates `reasoning_content` for the turn.
///
/// Like the opening preamble, English (and unknown) locales return
/// `None` and the system prompt is byte-identical to the pre-bookend
/// behavior.
pub(crate) fn locale_reinforcement_closer(locale_tag: &str) -> Option<&'static str> {
    match locale_tag {
        "zh-Hans" | "zh-CN" | "zh" => Some(effective_locale_closer_zh_hans()),
        "ja" | "ja-JP" => Some(effective_locale_closer_ja()),
        "pt-BR" | "pt" => Some(effective_locale_closer_pt_br()),
        "vi" | "vi-VN" => Some(effective_locale_closer_vi()),
        _ => None,
    }
}

const LOCALE_PREAMBLE_ZH_HANS: &str = "## 语言要求\n\n\
你正在 codewhale 中运行。无论任务上下文（代码、错误日志、文件名）\
是英文，无论系统提示的其余部分是英文，你都必须用简体中文进行 \
`reasoning_content`（内部思考）和最终回复。代码、文件路径、工具名称\
（例如 `read_file`、`exec_shell`）、环境变量、命令行参数和 URL \
保持原样 —— 只有自然语言散文要切换到简体中文。\n\n\
如果用户在会话中切换到另一种语言，从下一轮开始跟随切换。\
如果用户明确要求（例如 \"think in English\"），则覆盖此规则。";

const LOCALE_PREAMBLE_JA: &str = "## 言語要件\n\n\
codewhale を実行しています。タスクコンテキスト（コード、エラーログ、\
ファイル名）が英語であっても、システムプロンプトの他の部分が英語で\
あっても、`reasoning_content`（内部思考）と最終的な返信は日本語で\
行ってください。コード、ファイルパス、ツール名（例：`read_file`、\
`exec_shell`）、環境変数、コマンドライン引数、URL は元のまま —— \
自然言語の文章のみ日本語に切り替えます。\n\n\
ユーザーがセッション中に別の言語に切り替えた場合は、次のターンから\
それに従ってください。ユーザーが明示的に要求した場合（例：\
\"think in English\"）はこのルールを上書きします。";

const LOCALE_PREAMBLE_PT_BR: &str = "## Requisito de Idioma\n\n\
Você está rodando dentro do codewhale. Escreva tanto \
`reasoning_content` (seu pensamento interno) quanto a resposta final \
em português do Brasil, mesmo quando o contexto da tarefa (código, \
logs de erro, nomes de arquivos) estiver em inglês e mesmo quando o \
resto do system prompt for em inglês. Mantenha código, caminhos de \
arquivos, nomes de ferramentas (por exemplo `read_file`, \
`exec_shell`), variáveis de ambiente, flags de linha de comando e \
URLs no formato original — apenas a prosa em linguagem natural muda \
para português do Brasil.\n\n\
Se o usuário mudar de idioma no meio da sessão, mude no próximo turno. \
Se o usuário pedir explicitamente (por exemplo, \"think in English\"), \
isso sobrescreve esta regra.";

// ── Closing bookends (appended to the very end of the system prompt) ──

const LOCALE_CLOSER_ZH_HANS: &str = "## 语言再次提醒\n\n\
**重要：你的 `reasoning_content`（内部思考）和最终回复必须保持简体中文。** \
无论你在这次会话中读到了多少英文代码、错误日志或文档，无论项目上下文 \
是英文，思考过程不能漂移到英文。这是会话级硬性要求 —— 用户的语言决定 \
你的语言，与上下文中累积的英文内容无关。除非用户明确要求切换（例如 \
\"think in English\"），否则继续用简体中文思考和回答。";

const LOCALE_CLOSER_JA: &str = "## 言語再確認\n\n\
**重要：`reasoning_content`（内部思考）と最終的な返信は日本語で行ってください。** \
このセッションで読み込んだ英語のコード、エラーログ、ドキュメントの量に \
関係なく、プロジェクトコンテキストが英語であっても、思考プロセスを \
英語に逸らさないでください。これはセッションレベルの厳格な要件であり、 \
ユーザーの言語があなたの言語を決定します。ユーザーが明示的に切り替えを \
要求しない限り（例：\"think in English\"）、日本語で思考し、回答し続けて \
ください。";

const LOCALE_CLOSER_PT_BR: &str = "## Reforço de Idioma\n\n\
**Importante: seu `reasoning_content` (pensamento interno) e a resposta \
final devem permanecer em português do Brasil.** Independentemente de \
quanto código em inglês, logs de erro ou documentação você ler nesta \
sessão, e independentemente de o contexto do projeto ser em inglês, o \
processo de pensamento não pode derivar para o inglês. Este é um \
requisito rígido em nível de sessão — o idioma do usuário define seu \
idioma. A menos que o usuário peça explicitamente a troca (por exemplo, \
\"think in English\"), continue pensando e respondendo em português do \
Brasil.";

const LOCALE_PREAMBLE_VI: &str = "## Yêu cầu ngôn ngữ\n\n\
Bạn đang chạy trong codewhale. Cho dù ngữ cảnh tác vụ (mã nguồn, nhật ký lỗi, tên tệp) \
là tiếng Anh, cho dù phần còn lại của system prompt là tiếng Anh, bạn đều phải sử dụng \
tiếng Việt cho phần `reasoning_content` (suy nghĩ nội bộ) và câu trả lời cuối cùng. Các từ \
mã nguồn, đường dẫn tệp, tên công cụ (ví dụ `read_file`, `exec_shell`), biến môi trường, \
tham số dòng lệnh và URL giữ nguyên dạng gốc —— chỉ các văn bản giải thích bằng ngôn ngữ \
tự nhiên mới được chuyển sang tiếng Việt.\n\n\
Nếu người dùng chuyển sang ngôn ngữ khác trong phiên làm việc, hãy chuyển theo từ lượt tiếp theo. \
Nếu người dùng yêu cầu rõ ràng (ví dụ \"think in English\"), hãy ghi đè quy tắc này.";

const LOCALE_CLOSER_VI: &str = "## Nhắc nhở ngôn ngữ một lần nữa\n\n\
**Quan trọng: phần `reasoning_content` (suy nghĩ nội bộ) và phản hồi cuối cùng của bạn phải được viết bằng tiếng Việt.** \
Dù bạn có đọc bao nhiêu mã nguồn tiếng Anh, nhật ký lỗi hay tài liệu trong phiên làm việc này, và dù ngữ cảnh \
dự án có là tiếng Anh, quá trình suy nghĩ của bạn cũng không được chuyển sang tiếng Anh. Đây là yêu cầu cứng \
ở cấp phiên làm việc —— ngôn ngữ của người dùng quyết định ngôn ngữ của bạn, không phụ thuộc vào nội dung tiếng Anh \
tích lũy trong ngữ cảnh. Trừ khi người dùng yêu cầu rõ ràng việc chuyển đổi (ví dụ \"think in English\"), \
hãy tiếp tục suy nghĩ và trả lời bằng tiếng Việt.";

/// Personality overlays — voice and tone.
pub const CALM_PERSONALITY: &str = include_str!("prompts/personalities/calm.md");
pub const PLAYFUL_PERSONALITY: &str = include_str!("prompts/personalities/playful.md");

/// Mode deltas — permissions, workflow expectations, mode-specific rules.
pub const AGENT_MODE: &str = include_str!("prompts/modes/agent.md");
pub const PLAN_MODE: &str = include_str!("prompts/modes/plan.md");
pub const YOLO_MODE: &str = include_str!("prompts/modes/yolo.md");

/// Approval-policy overlays — whether tool calls are auto-approved,
/// require confirmation, or are blocked.
pub const AUTO_APPROVAL: &str = include_str!("prompts/approvals/auto.md");
pub const SUGGEST_APPROVAL: &str = include_str!("prompts/approvals/suggest.md");
pub const NEVER_APPROVAL: &str = include_str!("prompts/approvals/never.md");

/// Compaction relay template — written into the system prompt so the
/// model knows the format to use when writing `.codewhale/handoff.md`.
pub const COMPACT_TEMPLATE: &str = include_str!("prompts/compact.md");

/// Goal continuation audit template — injected by the engine when a runtime
/// goal is active and the assistant tries to end a turn without closing it.
pub const GOAL_CONTINUATION_PROMPT: &str = include_str!("prompts/continuation.md");

/// Memory hygiene guidance — appended to the system prompt only when the
/// session has a non-empty user-memory block. Steers the model toward
/// writing durable memories as declarative facts ("User prefers concise
/// responses") rather than imperatives ("Always respond concisely"),
/// because imperatives get re-read as directives in later sessions and
/// can override the user's current request (#725).
pub const MEMORY_GUIDANCE: &str = include_str!("prompts/memory_guidance.md");

// ── Legacy prompt constants (kept for backwards compatibility) ────────

/// Legacy base prompt (agent.txt — now decomposed into base.md + overlays).
/// Still available for callers that haven't migrated to the layered API.
pub const AGENT_PROMPT: &str = include_str!("prompts/agent.txt");

// ── Personality selection ─────────────────────────────────────────────

/// Which personality overlay to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Personality {
    /// Cool, spatial, reserved — the default.
    Calm,
    /// Warm, energetic, playful — alternative for fun mode.
    Playful,
}

impl Personality {
    /// Resolve from the `calm_mode` settings flag.
    /// When `calm_mode` is true → Calm; when false → Playful (future).
    /// For now, always returns Calm — Playful is wired but opt-in.
    #[must_use]
    pub fn from_settings(calm_mode: bool) -> Self {
        if calm_mode {
            Self::Calm
        } else {
            // Future: when playful mode is exposed in settings, return Playful here.
            // For now, calm is the only default.
            Self::Calm
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            Self::Calm => CALM_PERSONALITY,
            Self::Playful => PLAYFUL_PERSONALITY,
        }
    }
}

// ── Composition ───────────────────────────────────────────────────────

fn mode_prompt(mode: AppMode) -> &'static str {
    match mode {
        AppMode::Agent => AGENT_MODE,
        AppMode::Yolo => YOLO_MODE,
        AppMode::Plan => PLAN_MODE,
    }
}

fn default_approval_mode_for_mode(mode: AppMode) -> ApprovalMode {
    match mode {
        AppMode::Agent => ApprovalMode::Suggest,
        AppMode::Yolo => ApprovalMode::Auto,
        AppMode::Plan => ApprovalMode::Never,
    }
}

fn approval_prompt_for_mode(mode: AppMode, approval_mode: ApprovalMode) -> &'static str {
    match mode {
        AppMode::Yolo => AUTO_APPROVAL,
        AppMode::Plan => NEVER_APPROVAL,
        AppMode::Agent => match approval_mode {
            ApprovalMode::Auto => AUTO_APPROVAL,
            ApprovalMode::Suggest => SUGGEST_APPROVAL,
            ApprovalMode::Never => NEVER_APPROVAL,
        },
    }
}

/// Compose the full system prompt in deterministic order:
///   1. tool taxonomy  — compact hints generated from the eager core tools
///   2. base.md        — core identity, toolbox, execution contract
///   3. personality    — voice and tone overlay
///   4. mode delta     — mode-specific permissions and workflow
///   5. approval policy — tool-approval behavior
///
/// Each layer is separated by a blank line for readability in the
/// rendered prompt (the model sees them as contiguous sections).
/// Substitute the `{model_id}` template in the Constitutional preamble
/// with the active model identifier. The base prompt is a compile-time
/// constant; this function produces a per-session variant so the prompt
/// says "You are deepseek-v4-pro" or "You are deepseek-v4-flash" instead
/// of a static placeholder.
fn apply_model_template(prompt: &str, model_id: &str) -> String {
    prompt.replace("{model_id}", model_id)
}

const TOOL_TAXONOMY_DISCOVERY: &[&str] = &["grep_files", "file_search"];
const TOOL_TAXONOMY_GIT: &[&str] = &["git_status", "git_diff"];
const TOOL_TAXONOMY_VERIFICATION: &[&str] = &["run_tests", "run_verifiers"];

fn render_core_tool_taxonomy_block(mode: AppMode) -> String {
    let core_tools = core_taxonomy_tools_for_mode(mode);
    let mut sentences = Vec::new();

    if let Some(discovery) = render_core_tool_group(TOOL_TAXONOMY_DISCOVERY, &core_tools) {
        sentences.push(format!("Use {discovery} for discovery."));
    }
    if let Some(git) = render_core_tool_group(TOOL_TAXONOMY_GIT, &core_tools) {
        sentences.push(format!("Use {git} for git inspection."));
    }
    if let Some(verification) = render_core_tool_group(TOOL_TAXONOMY_VERIFICATION, &core_tools) {
        sentences.push(format!("Use {verification} for verification."));
    }

    debug_assert!(
        !sentences.is_empty(),
        "core tool taxonomy has no active tool groups"
    );
    format!("## Core Tool Taxonomy\n\n{}", sentences.join(" "))
}

fn core_taxonomy_tools_for_mode(mode: AppMode) -> Vec<&'static str> {
    let core_tools = crate::core::engine::default_active_native_tool_names();
    core_tools
        .iter()
        .copied()
        .filter(|tool| mode != AppMode::Plan || !matches!(*tool, "run_tests" | "run_verifiers"))
        .collect()
}

fn render_core_tool_group(group: &[&str], core_tools: &[&str]) -> Option<String> {
    let rendered = group
        .iter()
        .copied()
        .filter(|tool| core_tools.contains(tool))
        .map(|tool| format!("`{tool}`"))
        .collect::<Vec<_>>()
        .join("/");
    (!rendered.is_empty()).then_some(rendered)
}

/// Authority recap block — appended at the end of the system prompt,
/// just before the user's first message. Uses recency bias constructively:
/// this is the last thing the model reads before generating, so it
/// reinforces the Constitutional hierarchy without occupying cache-stable
/// prefix space.
const AUTHORITY_RECAP: &str = "\
## Authority Recap

The Constitution of CodeWhale (Articles I-VII) governs your behavior.
Tier 1 rules — truthfulness, user agency, tool-use mandate, verification
duty — are non-negotiable. The user's next message is the highest
directive within Constitutional bounds. Personality, memory, and handoff
context are subordinate to the Constitution, the Statutes, and the user's
current request. When in doubt, consult Article VII: The Hierarchy of Law.";

pub fn compose_prompt(mode: AppMode, personality: Personality) -> String {
    compose_prompt_with_approval(mode, personality, default_approval_mode_for_mode(mode))
}

pub fn compose_prompt_with_approval(
    mode: AppMode,
    personality: Personality,
    approval_mode: ApprovalMode,
) -> String {
    compose_prompt_with_approval_and_model(mode, personality, approval_mode, "codewhale")
}

/// Compose with explicit model ID for dynamic identity injection.
/// The model_id replaces `{model_id}` in the Constitutional preamble.
pub fn compose_prompt_with_approval_and_model(
    mode: AppMode,
    personality: Personality,
    approval_mode: ApprovalMode,
    model_id: &str,
) -> String {
    compose_prompt_with_approval_model_and_shell(mode, personality, approval_mode, model_id, true)
}

fn compose_prompt_with_approval_model_and_shell(
    mode: AppMode,
    personality: Personality,
    approval_mode: ApprovalMode,
    model_id: &str,
    allow_shell: bool,
) -> String {
    let tool_taxonomy = render_core_tool_taxonomy_block(mode);
    let shell_tools_available = allow_shell && mode != AppMode::Plan;
    let base_prompt = render_base_prompt_for_tool_availability(
        effective_base_prompt().trim(),
        model_id,
        shell_tools_available,
    );
    let parts: [&str; 5] = [
        tool_taxonomy.as_str(),
        base_prompt.as_str(),
        personality.prompt().trim(),
        mode_prompt(mode).trim(),
        approval_prompt_for_mode(mode, approval_mode).trim(),
    ];

    let mut out =
        String::with_capacity(parts.iter().map(|p| p.len()).sum::<usize>() + (parts.len() - 1) * 2);
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            out.push('\n');
        }
        out.push_str(part);
    }
    out
}

fn render_base_prompt_for_tool_availability(
    prompt: &str,
    model_id: &str,
    shell_tools_available: bool,
) -> String {
    let prompt = if shell_tools_available {
        prompt.to_string()
    } else {
        remove_shell_tool_guidance(prompt)
    };
    apply_model_template(&prompt, model_id)
}

fn remove_shell_tool_guidance(prompt: &str) -> String {
    let prompt = prompt
        .lines()
        .filter(|line| !is_shell_disabled_prompt_line(line))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = remove_markdown_section(&prompt, "### `exec_shell`");
    let prompt = prompt.replace(
        "; for GitHub issue/PR/release triage, prefer the native `gh ... --json` CLI through shell because it is authenticated, structured, and reproducible; `github_issue_context` / `github_pr_context` are read-only fallbacks when the CLI route is unavailable;",
        "; for GitHub issue/PR/release triage, use `github_issue_context` / `github_pr_context` as read-only routes when shell tools are unavailable;",
    );
    prompt.replace(
        "Use deterministic Python inside RLM for exact counts and structured aggregation; use `grep_files` or `exec_shell` directly when that is the clearest deterministic check.",
        "Use deterministic Python inside RLM for exact counts and structured aggregation; use `grep_files` directly when that is the clearest deterministic check.",
    )
}

fn is_shell_disabled_prompt_line(line: &str) -> bool {
    line.starts_with("- Arithmetic, math, calculations → `exec_shell`")
        || line.starts_with("- Hashes, encodings, checksums → `exec_shell`")
        || line.starts_with("- Current time, date, timezone → `exec_shell`")
        || line
            .starts_with("- System state: OS, CPU, memory, disk, ports, processes → `exec_shell`")
        || line.starts_with("- **Shell**:")
}

fn remove_markdown_section(prompt: &str, heading: &str) -> String {
    let Some(start) = prompt.find(heading) else {
        return prompt.to_string();
    };
    let after_heading = start + heading.len();
    let end = prompt[after_heading..]
        .find("\n### ")
        .map(|offset| after_heading + offset)
        .unwrap_or(prompt.len());

    let before = prompt[..start].trim_end();
    let after = prompt[end..].trim_start_matches('\n');
    if before.is_empty() {
        after.to_string()
    } else if after.is_empty() {
        before.to_string()
    } else {
        format!("{before}\n\n{after}")
    }
}

/// Compose for the default personality (Calm).
fn compose_mode_prompt(mode: AppMode) -> String {
    compose_prompt(mode, Personality::Calm)
}

fn compose_mode_prompt_with_approval(mode: AppMode, approval_mode: ApprovalMode) -> String {
    compose_prompt_with_approval(mode, Personality::Calm, approval_mode)
}

fn compose_mode_prompt_with_approval_and_model(
    mode: AppMode,
    approval_mode: ApprovalMode,
    model_id: &str,
) -> String {
    compose_prompt_with_approval_model_and_shell(
        mode,
        Personality::Calm,
        approval_mode,
        model_id,
        true,
    )
}

// ── Public API ────────────────────────────────────────────────────────

/// Get the system prompt for a specific mode (default Calm personality).
pub fn system_prompt_for_mode(mode: AppMode) -> SystemPrompt {
    SystemPrompt::Text(compose_mode_prompt(mode))
}

/// Get the system prompt for a specific mode with explicit personality.
pub fn system_prompt_for_mode_with_personality(
    mode: AppMode,
    personality: Personality,
) -> SystemPrompt {
    SystemPrompt::Text(compose_prompt(mode, personality))
}

/// Get the system prompt for a specific mode with project context.
pub fn system_prompt_for_mode_with_context(
    mode: AppMode,
    workspace: &Path,
    working_set_summary: Option<&str>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_and_skills(
        mode,
        workspace,
        working_set_summary,
        None,
        None,
        None,
    )
}

/// Get the system prompt for a specific mode with project and skills context.
///
/// **Volatile-content-last invariant.** Blocks are appended in order from
/// most-static to most-volatile so DeepSeek's KV prefix cache hits the
/// longest possible byte prefix turn-over-turn:
///
///   1. mode prompt (compile-time constant)
///   2. project context / fallback (workspace-static)
///   3. skills block (skills-dir-static)
///   4. `## Context Management` (compile-time constant, Agent/Yolo only)
///   5. compaction relay template (compile-time constant)
///   6. relay block — file-backed; rewritten by `/compact` and on exit
///
/// Anything appended after a volatile block forfeits the cache for the rest
/// of the request. New blocks belong above the relay boundary unless they
/// themselves are turn-volatile. Working-set metadata is now injected into the
/// latest user message as per-turn metadata instead of this system prompt.
pub fn system_prompt_for_mode_with_context_and_skills(
    mode: AppMode,
    workspace: &Path,
    working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[InstructionSource]>,
    user_memory_block: Option<&str>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_skills_and_session(
        mode,
        workspace,
        working_set_summary,
        skills_dir,
        instructions,
        PromptSessionContext {
            user_memory_block,
            goal_objective: None,
            project_context_pack_enabled: true,
            locale_tag: "en",
            translation_enabled: false,
            model_id: "codewhale",
            show_thinking: true,
            allow_shell: true,
        },
    )
}

pub fn system_prompt_for_mode_with_context_skills_and_session(
    mode: AppMode,
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[InstructionSource]>,
    session_context: PromptSessionContext<'_>,
) -> SystemPrompt {
    system_prompt_for_mode_with_context_skills_session_and_approval(
        mode,
        workspace,
        _working_set_summary,
        skills_dir,
        instructions,
        session_context,
        default_approval_mode_for_mode(mode),
    )
}

pub fn system_prompt_for_mode_with_context_skills_session_and_approval(
    mode: AppMode,
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[InstructionSource]>,
    session_context: PromptSessionContext<'_>,
    approval_mode: ApprovalMode,
) -> SystemPrompt {
    let mode_prompt = compose_prompt_with_approval_model_and_shell(
        mode,
        Personality::Calm,
        approval_mode,
        session_context.model_id,
        session_context.allow_shell,
    );

    // Load project context from workspace
    let project_context = load_project_context_with_parents(workspace);

    // 0. Locale-native reinforcement preamble (#1118 follow-up). When the
    // user's UI locale is non-English we prepend a short native-script
    // passage so the model's first exposure to the prompt is an explicit
    // "think and reply in {locale}" directive in the user's own writing
    // system — defeats the "task context is English, so the model thinks
    // in English even though `lang: zh-Hans` is set" failure mode that
    // PR #1398 partially addressed. English (and unknown) locales get
    // `None` and keep the previous behavior unchanged.
    let preamble = if session_context.show_thinking {
        locale_reinforcement_preamble(session_context.locale_tag)
    } else {
        None
    };

    // 1–2. Mode prompt + project context.
    // `load_project_context_with_parents` auto-generates .codewhale/instructions.md
    // (or .deepseek/instructions.md as fallback) when no context file exists,
    // so the fallback should always be available.
    let mut full_prompt = if let Some(project_block) = project_context.as_system_block() {
        format!("{mode_prompt}\n\n{project_block}")
    } else {
        // Extremely unlikely: context generation failed (e.g. filesystem error).
        // Use mode prompt alone rather than panic.
        tracing::warn!("No project context available and auto-generation failed");
        mode_prompt
    };

    if let Some(preamble) = preamble {
        full_prompt = format!("{preamble}\n\n{full_prompt}");
    }

    if session_context.project_context_pack_enabled
        && let Some(pack) = crate::project_context::generate_project_context_pack(workspace)
    {
        full_prompt = format!("{full_prompt}\n\n{pack}");
    }

    // 2.3a. Translation output instruction — when enabled, instruct
    // the model to respond in the resolved session locale. Stays
    // above the volatile-content boundary because it's a per-session
    // flag, not a per-turn one: enabling `/translate` is a session
    // toggle, so the prompt-prefix bytes don't drift turn-over-turn.
    if session_context.translation_enabled {
        full_prompt = format!(
            "{full_prompt}\n\n{}",
            translation_output_instruction(session_context.locale_tag)
        );
    }

    // 3. Skills block. #432: walks every candidate workspace
    // skills directory (`.agents/skills`, `skills`,
    // `.opencode/skills`, `.claude/skills`, `.cursor/skills`) plus global
    // `~/.agents/skills` / `~/.deepseek/skills` so skills installed for any
    // AI-tool convention show up in the catalogue. The legacy
    // single-`skills_dir` path is
    // honoured as a fallback for callers that don't supply a
    // workspace-aware view; it falls through to the same merged
    // registry when available.
    let skills_block = crate::skills::render_available_skills_context_for_workspace(workspace)
        .or_else(|| skills_dir.and_then(crate::skills::render_available_skills_context));
    if let Some(block) = skills_block {
        full_prompt = format!("{full_prompt}\n\n{block}");
    }

    // 4. Context Management (Agent / Yolo only).
    if matches!(mode, AppMode::Agent | AppMode::Yolo) {
        full_prompt.push_str(
            "\n\n## Context Management\n\n\
             When the conversation gets long (you'll see a context usage indicator), you can:\n\
             1. Use `/compact` to summarize earlier context and free up space\n\
             2. The system will preserve important information (files you're working on, recent messages, tool results)\n\
             3. After compaction, you'll see a summary of what was discussed and can continue seamlessly\n\n\
             If you notice context is getting long (>60% during sustained work), proactively suggest using `/compact` or Ctrl+L to the user. If auto_compact is enabled, the engine can compact before the next send once the configured threshold is crossed.\n\n\
             ### Prompt-cache awareness\n\n\
             DeepSeek caches the longest *byte-stable prefix* of every request and charges roughly 100× less for cache-hit tokens than miss tokens. The system prompt above is layered most-static-first specifically so the prefix stays stable turn-over-turn. To keep cache hits high:\n\
             - **Working set location:** the current repo working set is stored on new user messages inside a `<turn_meta>` block. Treat it as high-priority turn metadata, not as a stable system-prompt section.\n\
             - **Append, don't reorder.** New context goes at the end (latest user / tool messages). Reshuffling earlier messages or rewriting their content invalidates the cache for everything after the change.\n\
             - **Don't paraphrase quoted content.** If you've already read a file, refer to it by path or line range instead of re-quoting it with different formatting.\n\
             - **Use `/compact` as a hard reset, not a tweak.** Compaction is meant for when the cache is already losing — it intentionally rewrites the prefix to a shorter summary. Don't trigger it for small wins.\n\
             - **Read once, refer back.** Re-reading the same file produces a different tool-result envelope than the prior read; it's cheaper to scroll back than to re-fetch.\n\
             - **Footer chip:** the `cache hit %` chip turns red below 40% and yellow below 80%. If it's been red for several turns, that's a signal to consolidate."
        );
    }

    // 5. Compaction relay template — so the model knows the format to use
    //    when writing `.codewhale/handoff.md` on exit / `/compact`.
    full_prompt.push_str("\n\n");
    full_prompt.push_str(COMPACT_TEMPLATE);

    // ── Volatile-content boundary ─────────────────────────────────────────
    // Everything below drifts mid-session and busts the prefix cache for
    // bytes that follow. All static layers (mode, project context, env,
    // skills, context management, compact template) live above this line
    // so DeepSeek's KV prefix cache can hit on the entire system prompt
    // regardless of per-session edits to memory, goals, or instructions.

    // 6. Environment block — platform, shell, pwd, locale.
    //
    // Placed below the volatile-content boundary. The original comment claimed
    // "workspace path is fixed for the run" → static-cacheable, which is true
    // for the terminal use case (one process owns one workspace for its
    // lifetime). It is **not** true for embedders that swap workspaces between
    // sessions (the Op::SyncSession path, multi-engine pools, IDE
    // integrations binding the engine to a per-tab workspace, etc.):
    // `pwd` drifts session-to-session and drags the entire static prefix
    // out of cache reuse. Moving the block below the volatile boundary keeps
    // mode / project / skills / context-mgmt / compact-template byte-stable
    // across sessions while preserving the pwd info the model needs for
    // `exec_shell` and structured search tools.
    full_prompt = format!(
        "{full_prompt}\n\n{}",
        render_environment_block(workspace, session_context.locale_tag),
    );

    // 6a. Configured `instructions = [...]` files (#454). Loaded
    // and concatenated in declared order. Placed below the volatile boundary
    // because these files are workspace-scoped and may differ between
    // sessions; any edit to them would otherwise bust the prefix cache for
    // all subsequent static layers.
    if let Some(sources) = instructions
        && let Some(block) = render_instructions_block(sources)
    {
        full_prompt = format!("{full_prompt}\n\n{block}");
    }

    // 6b. User memory block (#489). Placed below the volatile boundary
    // because memory entries are editable mid-session via `/memory` or
    // `# foo` quick-add. When they change, they only invalidate the
    // trailing relay block — the static prefix above stays cached.
    if let Some(memory_block) = session_context.user_memory_block
        && !memory_block.trim().is_empty()
    {
        full_prompt = format!("{full_prompt}\n\n{memory_block}\n\n{MEMORY_GUIDANCE}");
    }

    // 6c. Current session goal. Also volatile: users set / change goals
    // during a session via `/goal`. Placed below the boundary for the
    // same reason as memory.
    if let Some(goal_objective) = session_context.goal_objective
        && !goal_objective.trim().is_empty()
    {
        full_prompt = format!(
            "{full_prompt}\n\n## Current Hunt\n\n<session_goal>\n{}\n</session_goal>",
            goal_objective.trim()
        );
    }

    // 7. Previous-session relay (file-backed, rewritten by `/compact`).
    if let Some(handoff_block) = load_handoff_block(workspace) {
        full_prompt = format!("{full_prompt}\n\n{handoff_block}");
    }

    // 7a. Authority recap — the final tier reminder before user messages.
    // Uses recency bias constructively: this is the last content the model
    // sees before the user's turn, reinforcing the Constitutional hierarchy.
    let authority_recap = effective_authority_recap();
    full_prompt = format!("{full_prompt}\n\n{authority_recap}");

    // 8. Locale-native closing reinforcement (#1118 follow-up #2). The
    // opening preamble alone wasn't enough — community feedback (the
    // WeChat thread about XML-tagged bilingual bookends) flagged that as
    // English context accumulates turn-over-turn, the model's recency
    // bias pulls thinking back to English. Putting the same directive at
    // the END of the system prompt — right before the user's next
    // message — uses recency bias *in our favor*: the model sees the
    // native-script "keep thinking in Chinese / Japanese / Portuguese"
    // rule immediately before it generates `reasoning_content` for the
    // turn. English (and unknown) locales return `None` and the prompt
    // stays byte-identical to the pre-bookend behavior.
    if let Some(closer) = session_context
        .show_thinking
        .then(|| locale_reinforcement_closer(session_context.locale_tag))
        .flatten()
    {
        full_prompt = format!("{full_prompt}\n\n{closer}");
    } else if !session_context.show_thinking {
        full_prompt = format!(
            "{full_prompt}\n\n{}",
            hidden_thinking_language_instruction(session_context.locale_tag)
        );
    }

    SystemPrompt::Text(full_prompt)
}

/// Build a system prompt with explicit project context
pub fn build_system_prompt(base: &str, project_context: Option<&ProjectContext>) -> SystemPrompt {
    let full_prompt =
        match project_context.and_then(super::project_context::ProjectContext::as_system_block) {
            Some(project_block) => format!("{}\n\n{}", base.trim(), project_block),
            None => base.trim().to_string(),
        };
    SystemPrompt::Text(full_prompt)
}

#[cfg(test)]
mod tests {
    // Don't assert on prose. If you wouldn't fail a code review for
    // changing the wording, don't fail a test for it.
    use super::*;
    use tempfile::tempdir;

    /// Discriminator unique to the injected relay block (not present in the
    /// agent prompt's own discussion of the convention).
    const HANDOFF_BLOCK_MARKER: &str = "left a relay artifact at `.codewhale/handoff.md`";

    #[test]
    fn prompt_override_storage_reports_duplicate_sets() {
        let cell = std::sync::OnceLock::new();

        assert_eq!(effective_prompt_override(&cell, "fallback"), "fallback");
        assert!(set_prompt_override(&cell, "first".to_string()).is_ok());
        assert_eq!(effective_prompt_override(&cell, "fallback"), "first");
        assert_eq!(
            set_prompt_override(&cell, "second".to_string()),
            Err("second".to_string())
        );
        assert_eq!(effective_prompt_override(&cell, "fallback"), "first");
    }

    fn contains_cjk(text: &str) -> bool {
        text.chars().any(|ch| {
            matches!(
                ch,
                '\u{3040}'..='\u{30ff}'
                    | '\u{3400}'..='\u{4dbf}'
                    | '\u{4e00}'..='\u{9fff}'
                    | '\u{f900}'..='\u{faff}'
            )
        })
    }

    #[test]
    fn base_prompt_carries_execution_discipline_block() {
        // The XML-tagged execution-discipline block is the contract —
        // verify each section name is present so reviewers can't quietly
        // strip the rules that herd V4 toward acting instead of narrating.
        for tag in [
            "<tool_persistence>",
            "<mandatory_tool_use>",
            "<act_dont_ask>",
            "<verification>",
            "<missing_context>",
        ] {
            assert!(
                BASE_PROMPT.contains(tag),
                "BASE_PROMPT missing required tag {tag}"
            );
        }
        assert!(
            BASE_PROMPT.contains("Tool-use enforcement"),
            "BASE_PROMPT missing the tool-use enforcement clause"
        );
    }

    #[test]
    fn base_prompt_carries_constitutional_preamble() {
        // Pin the load-bearing Constitutional anchors. The exact prose
        // can evolve, but CodeWhale must keep the Brother Whale preamble,
        // the coordination principle, and the hierarchy of law.
        for phrase in [
            "We begin with Brother Whale",
            "Brother Whale is the founding intelligence",
            "Every model that runs here is Brother Whale",
            "future intelligences can better coordinate",
            "Article II — The Primacy of Truth",
            "Article VII — The Hierarchy of Law",
        ] {
            assert!(
                BASE_PROMPT.contains(phrase),
                "BASE_PROMPT missing Constitutional phrase {phrase:?}"
            );
        }
    }

    #[test]
    fn constitutional_hierarchy_keeps_case_command_above_local_law() {
        let case_at = BASE_PROMPT
            .find("2. **Case Command.**")
            .expect("case command tier present");
        let statute_at = BASE_PROMPT
            .find("3. **Statutes.**")
            .expect("statutes tier present");
        let local_law_at = BASE_PROMPT
            .find("5. **Local Law.**")
            .expect("local law tier present");

        assert!(
            case_at < statute_at && statute_at < local_law_at,
            "Article VII must keep the current user request above runtime guidance and local law"
        );
        assert!(
            BASE_PROMPT.contains("actual runtime gates still determine what tools can execute"),
            "Article VII must distinguish prompt authority from executable runtime gates"
        );
    }

    #[test]
    fn base_prompt_contains_model_id_template() {
        assert!(
            BASE_PROMPT.contains("{model_id}"),
            "BASE_PROMPT must contain the {{model_id}} template for dynamic injection"
        );
    }

    #[test]
    fn apply_model_template_replaces_placeholder() {
        let result = apply_model_template("You are {model_id}", "deepseek-v4-pro");
        assert_eq!(result, "You are deepseek-v4-pro");
        assert!(!result.contains("{model_id}"));
    }

    #[test]
    fn compose_prompt_injects_model_id() {
        let prompt = compose_prompt_with_approval_and_model(
            AppMode::Agent,
            Personality::Calm,
            ApprovalMode::Suggest,
            "deepseek-v4-flash",
        );
        assert!(
            prompt.contains("You are deepseek-v4-flash"),
            "composed prompt must contain the injected model id"
        );
        assert!(
            !prompt.contains("{model_id}"),
            "composed prompt must not contain the raw template placeholder"
        );
    }

    #[test]
    fn composed_prompt_keeps_shell_guidance_when_shell_tools_are_available() {
        let prompt = compose_prompt_with_approval_model_and_shell(
            AppMode::Agent,
            Personality::Calm,
            ApprovalMode::Suggest,
            "deepseek-v4-pro",
            true,
        );

        assert!(prompt.contains("- **Shell**:"));
        assert!(prompt.contains("### `exec_shell`"));
        assert!(prompt.contains("`task_shell_start`"));
        assert!(prompt.contains("Arithmetic, math, calculations → `exec_shell`"));
    }

    #[test]
    fn composed_prompt_omits_shell_guidance_when_shell_tools_are_unavailable() {
        let prompt = compose_prompt_with_approval_model_and_shell(
            AppMode::Agent,
            Personality::Calm,
            ApprovalMode::Suggest,
            "deepseek-v4-pro",
            false,
        );

        for shell_only in [
            "- **Shell**:",
            "### `exec_shell`",
            "`task_shell_start`",
            "exec_shell",
            "task_shell",
            "Arithmetic, math, calculations → `exec_shell`",
            "Hashes, encodings, checksums → `exec_shell`",
            "Current time, date, timezone → `exec_shell`",
            "System state: OS, CPU, memory, disk, ports, processes → `exec_shell`",
            "CLI through shell",
            "or `exec_shell` directly",
        ] {
            assert!(
                !prompt.contains(shell_only),
                "shell-disabled prompt must not advertise {shell_only:?}"
            );
        }
        assert!(
            prompt.contains("actual runtime gates still determine what tools can execute"),
            "shell-disabled prompt should keep the runtime-gates hierarchy clause"
        );
        assert!(
            prompt.contains("`task_gate_run`") && prompt.contains("`github_issue_context`"),
            "shell-disabled prompt should keep non-shell task evidence tools"
        );
    }

    #[test]
    fn composed_prompt_starts_with_core_tool_taxonomy() {
        let prompt = compose_prompt_with_approval_and_model(
            AppMode::Agent,
            Personality::Calm,
            ApprovalMode::Suggest,
            "deepseek-v4-pro",
        );
        let expected_taxonomy = render_core_tool_taxonomy_block(AppMode::Agent);

        assert!(
            prompt.starts_with(&expected_taxonomy),
            "composed prompt should start with the compact generated tool taxonomy"
        );
    }

    #[test]
    fn plan_prompt_taxonomy_omits_run_tests() {
        let prompt = compose_prompt_with_approval_and_model(
            AppMode::Plan,
            Personality::Calm,
            ApprovalMode::Never,
            "deepseek-v4-pro",
        );
        let expected_taxonomy = render_core_tool_taxonomy_block(AppMode::Plan);

        assert!(
            prompt.starts_with(&expected_taxonomy),
            "Plan prompt should start with its mode-specific tool taxonomy"
        );
        assert!(
            expected_taxonomy.contains("for discovery")
                && expected_taxonomy.contains("for git inspection"),
            "Plan taxonomy should keep read-only discovery and git guidance"
        );
        assert!(
            !expected_taxonomy.contains("run_tests")
                && !expected_taxonomy.contains("run_verifiers")
                && !expected_taxonomy.contains("for verification")
                && !expected_taxonomy.contains("Use  "),
            "Plan taxonomy must not advertise unavailable verification tools: {expected_taxonomy:?}"
        );
    }

    #[test]
    fn core_tool_taxonomy_only_references_default_active_tools() {
        let core_tools = crate::core::engine::default_active_native_tool_names();
        for tool in TOOL_TAXONOMY_DISCOVERY
            .iter()
            .chain(TOOL_TAXONOMY_GIT)
            .chain(TOOL_TAXONOMY_VERIFICATION)
        {
            assert!(
                core_tools.contains(tool),
                "tool taxonomy references {tool}, but it is not in the eager native-tool list"
            );
        }
    }

    #[test]
    fn authority_recap_appears_in_full_prompt() {
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext::default(),
            ApprovalMode::Suggest,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(
            text.contains("## Authority Recap"),
            "full system prompt must contain the authority recap"
        );
        assert!(
            text.contains("The Constitution of CodeWhale (Articles I-VII) governs your behavior"),
            "authority recap must reference the Constitution"
        );
    }

    #[test]
    fn calm_personality_declares_tier_8_subordination() {
        assert!(
            CALM_PERSONALITY.contains("Tier 8"),
            "Calm personality must identify as Tier 8"
        );
        assert!(
            CALM_PERSONALITY.contains("cannot override"),
            "Calm personality must have a subordination clause"
        );
    }

    #[test]
    fn execution_discipline_is_at_the_end_for_cache_stability() {
        // DeepSeek's prefix cache keys on a leading byte-stable run, so
        // the new sections must be appended, not interleaved earlier.
        let body = BASE_PROMPT;
        let persistence_at = body
            .find("<tool_persistence>")
            .expect("tool_persistence anchor present");
        let language_at = body.find("## Language").expect("Language anchor present");
        assert!(
            language_at < persistence_at,
            "execution-discipline block must come after the early sections"
        );
    }

    #[test]
    fn plan_mode_prompt_uses_update_plan_as_confirmation_handoff() {
        assert!(
            PLAN_MODE.contains("call `update_plan`"),
            "Plan mode must tell the model to finish plans through update_plan"
        );
        assert!(
            PLAN_MODE.contains("accept / revise / exit prompt"),
            "Plan mode must explain why update_plan is the UI handoff signal"
        );
    }

    #[test]
    fn render_environment_block_lists_supplied_locale_and_workspace() {
        let tmp = tempdir().expect("tempdir");
        let block = render_environment_block(tmp.path(), "zh-Hans");
        assert!(block.starts_with("## Environment"));
        assert!(block.contains("- lang: zh-Hans"));
        assert!(block.contains(&format!(
            "- deepseek_version: {}",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(block.contains(&format!("- pwd: {}", tmp.path().display())));
        assert!(block.contains("- platform:"));
        assert!(block.contains("- shell:"));
    }

    #[test]
    fn locale_reinforcement_preamble_returns_native_script_for_supported_locales() {
        // English (and unknown locales) get None — the existing English
        // directive in `base.md` is sufficient.
        assert!(locale_reinforcement_preamble("en").is_none());
        assert!(locale_reinforcement_preamble("en-US").is_none());
        assert!(locale_reinforcement_preamble("fr-FR").is_none());
        assert!(locale_reinforcement_preamble("").is_none());

        // zh-Hans (and the de-facto equivalents the TUI accepts) get a
        // native-script preamble. The text must explicitly mention
        // `reasoning_content` (the V4 knob this is meant to steer) and
        // preserve tool-name immutability — those are the load-bearing
        // claims behind the #1118 fix that someone could quietly
        // delete in a future translation pass.
        for tag in ["zh-Hans", "zh-CN", "zh"] {
            let preamble =
                locale_reinforcement_preamble(tag).expect("zh-Hans preamble should exist");
            assert!(
                preamble.contains("简体中文"),
                "zh preamble must be in Simplified Chinese: {preamble:?}"
            );
            assert!(
                preamble.contains("reasoning_content"),
                "zh preamble must steer reasoning_content: {preamble:?}"
            );
            assert!(
                preamble.contains("read_file"),
                "zh preamble must call out tool-name immutability: {preamble:?}"
            );
        }

        let ja = locale_reinforcement_preamble("ja").expect("ja preamble");
        assert!(ja.contains("日本語"), "ja preamble must be in Japanese");
        assert!(ja.contains("reasoning_content"));

        let pt = locale_reinforcement_preamble("pt-BR").expect("pt-BR preamble");
        assert!(
            pt.contains("português do Brasil"),
            "pt preamble must call out pt-BR explicitly"
        );
        assert!(pt.contains("reasoning_content"));
    }

    #[test]
    fn system_prompt_prepends_locale_preamble_for_zh_hans() {
        // Build the full system prompt with locale=zh-Hans and assert
        // the native-script preamble shows up *before* the English
        // base-prompt body. Cache stability and attention precedence
        // both depend on this ordering.
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "zh-Hans",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
            ApprovalMode::Suggest,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let preamble_marker = "## 语言要求";
        let base_marker = "You are codewhale";
        let preamble_pos = text
            .find(preamble_marker)
            .expect("zh-Hans preamble should be present");
        let base_pos = text
            .find(base_marker)
            .expect("base prompt should be present");
        assert!(
            preamble_pos < base_pos,
            "locale preamble must precede the English base prompt (preamble={preamble_pos}, base={base_pos})",
        );
    }

    #[test]
    fn locale_reinforcement_closer_returns_native_script_for_supported_locales() {
        // English (and unknown locales) get None.
        assert!(locale_reinforcement_closer("en").is_none());
        assert!(locale_reinforcement_closer("fr-FR").is_none());
        assert!(locale_reinforcement_closer("").is_none());

        // Each supported locale gets a closer in its own script that
        // explicitly tells the model "don't drift to English even as
        // English context accumulates" — that's the load-bearing claim
        // behind the bookend pattern.
        let zh = locale_reinforcement_closer("zh-Hans").expect("zh closer");
        assert!(
            zh.contains("简体中文"),
            "zh closer must be in Simplified Chinese"
        );
        assert!(
            zh.contains("reasoning_content"),
            "zh closer must steer reasoning_content"
        );
        let ja = locale_reinforcement_closer("ja").expect("ja closer");
        assert!(ja.contains("日本語"), "ja closer must be in Japanese");
        assert!(ja.contains("reasoning_content"));
        let pt = locale_reinforcement_closer("pt-BR").expect("pt-BR closer");
        assert!(pt.contains("português do Brasil"));
        assert!(pt.contains("reasoning_content"));
    }

    #[test]
    fn system_prompt_bookends_zh_hans_with_preamble_and_closer() {
        // The full system prompt for zh-Hans must contain BOTH the
        // opening preamble (`## 语言要求`) and the closing reinforcement
        // (`## 语言再次提醒`), with the closer appearing AFTER the
        // preamble — i.e. the prompt is "bookended" in native script,
        // matching the empirical finding from the WeChat thread that
        // motivated the closer.
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "zh-Hans",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
            ApprovalMode::Suggest,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let preamble_pos = text
            .find("## 语言要求")
            .expect("zh-Hans preamble must be in prompt");
        let closer_pos = text
            .find("## 语言再次提醒")
            .expect("zh-Hans closer must be in prompt");
        assert!(
            preamble_pos < closer_pos,
            "closer must come after preamble (preamble={preamble_pos}, closer={closer_pos})",
        );
        // The closer must be the very last block — anything else after
        // it defeats the recency-bias purpose. Skip the closer's own
        // `## ` header before scanning.
        let closer_header_end = closer_pos + "## 语言再次提醒".len();
        let after_closer_body = &text[closer_header_end..];
        assert!(
            !after_closer_body.contains("\n## "),
            "no other top-level section should follow the closer; got: {after_closer_body:?}",
        );
    }

    #[test]
    fn hidden_thinking_uses_english_reasoning_without_locale_bookends() {
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "zh-Hans",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: false,
                allow_shell: true,
            },
            ApprovalMode::Suggest,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(
            text.contains("## Hidden Thinking Language"),
            "hidden thinking prompt must include the request-side language override"
        );
        assert!(
            text.contains("reasoning_content") && text.contains("English"),
            "hidden thinking override must steer reasoning_content to English"
        );
        assert!(
            text.contains("final reply") && text.contains("Simplified Chinese"),
            "hidden thinking override must preserve the visible reply language"
        );
        assert!(
            !text.contains("## 语言要求") && !text.contains("## 语言再次提醒"),
            "hidden thinking prompt must not also ask for localized reasoning"
        );

        let hidden_pos = text
            .find("## Hidden Thinking Language")
            .expect("hidden thinking block present");
        let hidden_header_end = hidden_pos + "## Hidden Thinking Language".len();
        let after_hidden_body = &text[hidden_header_end..];
        assert!(
            !after_hidden_body.contains("\n## "),
            "hidden thinking override must be the final top-level block; got: {after_hidden_body:?}",
        );
    }

    #[test]
    fn system_prompt_skips_locale_preamble_for_english() {
        // English locale → no preamble injected. Asserts the
        // "preamble is opt-in for non-English" invariant.
        let tmp = tempdir().expect("tempdir");
        let text = match system_prompt_for_mode_with_context_skills_session_and_approval(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
            ApprovalMode::Suggest,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(
            !text.contains("语言要求"),
            "English locale must not get a zh preamble: {text:?}"
        );
        assert!(
            !text.contains("言語要件"),
            "English locale must not get a ja preamble: {text:?}"
        );
        assert!(
            !text.contains("Requisito de Idioma"),
            "English locale must not get a pt-BR preamble: {text:?}"
        );
        // Closer too — same bookend rule.
        assert!(
            !text.contains("语言再次提醒"),
            "English locale must not get a zh closer: {text:?}"
        );
        assert!(
            !text.contains("言語再確認"),
            "English locale must not get a ja closer: {text:?}"
        );
        assert!(
            !text.contains("Reforço de Idioma"),
            "English locale must not get a pt-BR closer: {text:?}"
        );
        assert!(
            !contains_cjk(BASE_PROMPT),
            "base prompt must not contain static CJK priming tokens"
        );
        for mode in [AppMode::Agent, AppMode::Plan, AppMode::Yolo] {
            let taxonomy = render_core_tool_taxonomy_block(mode);
            assert!(
                !contains_cjk(&taxonomy),
                "tool taxonomy must not contain static CJK priming tokens: {taxonomy:?}"
            );
        }
        // Do not assert on arbitrary CJK in the full system prompt: project
        // context may legitimately contain localized file names, README text,
        // or user-authored instructions. The locale bookend markers above are
        // the priming tokens this test is meant to guard.
    }

    #[test]
    fn language_section_carries_reasoning_content_directives_for_1118() {
        // #1118 ("Language has been configured to Chinese, but thinking
        // outputs are still in English"): the base prompt's language
        // section is the only knob that steers V4's `reasoning_content`
        // language. Pin the load-bearing phrases so a future innocuous
        // edit can't quietly drop them.
        let lang = BASE_PROMPT;
        assert!(
            lang.contains("reasoning_content"),
            "language section must explicitly call out reasoning_content"
        );
        assert!(
            lang.contains("latest user message"),
            "latest user message must be the primary language signal"
        );
        assert!(
            lang.contains("clearly English") && lang.contains("must stay English"),
            "English user turns must stay English even after localized context"
        );
        assert!(
            lang.contains("Simplified Chinese")
                && lang.contains("must both be in Simplified Chinese"),
            "Chinese user turns must still steer reasoning_content and replies"
        );
        assert!(
            lang.contains("README.zh-CN.md") && lang.contains("tool results"),
            "localized docs and tool results must be named as non-language signals"
        );
        // Explicit-user-override clause keeps the prompt useful for the
        // opposite preference (#1118 commenters who want English
        // thinking for token-cost reasons).
        for phrase in ["think in English", "reason in Chinese"] {
            assert!(
                lang.contains(phrase),
                "expected the user-override example `{phrase}`"
            );
        }
    }

    #[test]
    fn environment_block_is_inserted_into_system_prompt() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: true,
                locale_tag: "ja",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("## Environment"));
        assert!(prompt.contains("- lang: ja"));
        assert!(prompt.contains("- deepseek_version:"));
    }

    #[test]
    fn memory_guidance_carries_paired_examples() {
        // The fragment is the contract — verify the verbatim ✓ / ✗
        // pair is present so V4 has both shapes to imitate.
        assert!(MEMORY_GUIDANCE.contains("declarative facts"));
        assert!(MEMORY_GUIDANCE.contains(" ✓"));
        assert!(MEMORY_GUIDANCE.contains(" ✗"));
        assert!(MEMORY_GUIDANCE.contains("Imperative"));
    }

    #[test]
    fn memory_guidance_absent_when_no_memory_block() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(
            !prompt.contains("Memory Hygiene"),
            "memory guidance must not leak into sessions without a memory block"
        );
    }

    #[test]
    fn memory_guidance_appended_after_memory_block() {
        let tmp = tempdir().expect("tempdir");
        let block = "## User Memory\n\n- prefers Rust\n";
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: Some(block),
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let mem_at = prompt.find("User Memory").expect("user memory present");
        let guide_at = prompt.find("Memory Hygiene").expect("guidance present");
        assert!(
            mem_at < guide_at,
            "guidance must come after the user memory block"
        );
    }

    #[test]
    fn memory_guidance_matches_constitutional_tier_order() {
        let guidance = MEMORY_GUIDANCE
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let current_request_at = guidance
            .find("the user's current request (Tier 2)")
            .expect("current request tier present");
        let statutes_at = guidance
            .find("Statutes (Tier 3)")
            .expect("statutes tier present");
        let local_law_at = guidance
            .find("Local Law (Tier 5)")
            .expect("local law tier present");
        let live_evidence_at = guidance
            .find("live evidence (Tier 6)")
            .expect("live evidence tier present");

        assert!(
            current_request_at < statutes_at
                && statutes_at < local_law_at
                && local_law_at < live_evidence_at,
            "memory guidance must keep the current request above memory and local law"
        );
    }

    #[test]
    fn project_context_pack_can_be_disabled() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("README.md"), "# Pack test").expect("write readme");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: false,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains("<project_context_pack>"));
    }

    #[test]
    fn project_context_pack_is_before_dynamic_tail() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("README.md"), "# Pack test").expect("write readme");
        std::fs::create_dir_all(tmp.path().join(".deepseek")).expect("mkdir");
        std::fs::write(tmp.path().join(".deepseek").join("handoff.md"), "handoff")
            .expect("handoff");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: None,
                project_context_pack_enabled: true,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("<project_context_pack>"));
        assert!(
            prompt.find("<project_context_pack>").expect("pack")
                < prompt.find("## Previous Session Relay").expect("relay")
        );
    }

    #[test]
    fn handoff_artifact_is_prepended_to_system_prompt_when_present() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(
            handoff_dir.join("handoff.md"),
            "# Session relay — prior\n\n## Active task\nFinish #32.\n\n## Open blockers\n- [ ] write the basic version\n",
        )
        .unwrap();

        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(prompt.contains(HANDOFF_BLOCK_MARKER));
        assert!(prompt.contains("Finish #32."));
        assert!(prompt.contains("write the basic version"));
    }

    #[test]
    fn missing_handoff_does_not_inject_block() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains(HANDOFF_BLOCK_MARKER));
    }

    #[test]
    fn empty_handoff_file_does_not_inject_block() {
        let tmp = tempdir().expect("tempdir");
        let dir = tmp.path().join(".deepseek");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("handoff.md"), "   \n\n  ").unwrap();
        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(!prompt.contains(HANDOFF_BLOCK_MARKER));
    }

    #[test]
    fn compose_prompt_includes_all_layers() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        // Base layer
        assert!(prompt.contains("You are codewhale"));
        // Personality layer
        assert!(prompt.contains("Personality: Calm"));
        // Mode layer
        assert!(prompt.contains("Mode: Agent"));
        // Approval layer
        assert!(prompt.contains("Approval Policy: Suggest"));
    }

    /// Gate against shipping a release with a missing CHANGELOG entry — which
    /// is exactly what happened with v0.8.21 / v0.8.22 (entries had to be
    /// backfilled in v0.8.23). Asserts the top-of-file CHANGELOG contains a
    /// `## [X.Y.Z]` heading matching the current `CARGO_PKG_VERSION`. No
    /// hardcoded version string — the test self-updates with the workspace
    /// version bump and only fires when the CHANGELOG is the missing piece.
    ///
    /// Walks up from `CARGO_MANIFEST_DIR` to find `CHANGELOG.md` instead of
    /// assuming a fixed `../../CHANGELOG.md` layout. The workspace root is
    /// the common case, but the walk also tolerates deeper crate layouts and
    /// the packaged-crate case (where the workspace root has been stripped
    /// out): if no `CHANGELOG.md` is reachable, the gate quietly skips
    /// rather than panicking, so consumers running the suite outside the
    /// workspace checkout don't see a spurious failure.
    #[test]
    fn changelog_entry_exists_for_current_package_version() {
        let version = env!("CARGO_PKG_VERSION");
        let manifest_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let Some(changelog_path) = manifest_dir
            .ancestors()
            .map(|dir| dir.join("CHANGELOG.md"))
            .find(|candidate| candidate.is_file())
        else {
            eprintln!(
                "changelog_entry_exists_for_current_package_version: no \
                 CHANGELOG.md found above {} — skipping (this gate only \
                 fires inside a workspace checkout).",
                manifest_dir.display()
            );
            return;
        };

        let contents = std::fs::read_to_string(&changelog_path).unwrap_or_else(|err| {
            panic!(
                "failed to read CHANGELOG.md at {}: {err}",
                changelog_path.display()
            )
        });
        let header = format!("## [{version}]");
        assert!(
            contents.contains(&header),
            "CHANGELOG.md is missing a `{header}` entry for the current package \
             version. Add a release section at the top before tagging — see \
             docs/RELEASE_CHECKLIST.md."
        );
    }

    #[test]
    fn compose_prompt_deterministic_order() {
        let prompt = compose_prompt(AppMode::Yolo, Personality::Calm);
        let base_pos = prompt.find("You are codewhale").unwrap();
        let personality_pos = prompt.find("Personality: Calm").unwrap();
        let mode_pos = prompt.find("Mode: YOLO").unwrap();
        let approval_pos = prompt.find("Approval Policy: Auto").unwrap();

        assert!(base_pos < personality_pos);
        assert!(personality_pos < mode_pos);
        assert!(mode_pos < approval_pos);
    }

    #[test]
    fn each_mode_gets_correct_approval() {
        assert!(
            compose_prompt(AppMode::Agent, Personality::Calm).contains("Approval Policy: Suggest")
        );
        assert!(compose_prompt(AppMode::Yolo, Personality::Calm).contains("Approval Policy: Auto"));
        assert!(
            compose_prompt(AppMode::Plan, Personality::Calm).contains("Approval Policy: Never")
        );
    }

    #[test]
    fn agent_prompt_can_reflect_never_approval_policy() {
        let prompt =
            compose_prompt_with_approval(AppMode::Agent, Personality::Calm, ApprovalMode::Never);
        assert!(prompt.contains("Mode: Agent"));
        assert!(prompt.contains("Approval Policy: Never"));
        assert!(prompt.contains("/config approval_mode suggest"));
    }

    #[test]
    fn personality_switches_correctly() {
        let calm = compose_prompt(AppMode::Agent, Personality::Calm);
        let playful = compose_prompt(AppMode::Agent, Personality::Playful);
        assert!(calm.contains("Personality: Calm"));
        assert!(playful.contains("Personality: Playful"));
        assert!(!calm.contains("Personality: Playful"));
    }

    #[test]
    fn compact_template_is_included_in_full_prompt() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context(AppMode::Agent, tmp.path(), None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert!(prompt.contains("## Compaction Relay"));
        // #429: structured Markdown template. Goal/Constraints/Progress
        // (Done/InProgress/Blocked)/Key Decisions/Next step.
        assert!(prompt.contains("### Goal"));
        assert!(prompt.contains("### Constraints"));
        assert!(prompt.contains("### Progress"));
        assert!(prompt.contains("#### Done"));
        assert!(prompt.contains("#### In Progress"));
        assert!(prompt.contains("#### Blocked"));
        assert!(prompt.contains("### Key Decisions"));
        assert!(prompt.contains("### Next step"));
    }

    #[test]
    fn session_goal_is_injected_below_compact_template() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            Some("## Repo Working Set\nsrc/lib.rs"),
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: Some("Fix transcript corruption"),
                project_context_pack_enabled: true,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        let goal_pos = prompt.find("<session_goal>").expect("goal block");
        let compact_pos = prompt.find("## Compaction Relay").expect("compact block");

        assert!(prompt.contains("Fix transcript corruption"));
        // Session goal is volatile content — it lives below the
        // volatile-content boundary (after the compact template) so
        // per-session goal changes don't bust the prefix cache for
        // static layers.
        assert!(compact_pos < goal_pos);
        assert!(!prompt.contains("src/lib.rs"));
    }

    #[test]
    fn empty_session_goal_is_not_injected() {
        let tmp = tempdir().expect("tempdir");
        let prompt = match system_prompt_for_mode_with_context_skills_and_session(
            AppMode::Agent,
            tmp.path(),
            None,
            None,
            None,
            PromptSessionContext {
                user_memory_block: None,
                goal_objective: Some("   "),
                project_context_pack_enabled: true,
                locale_tag: "en",
                translation_enabled: false,
                model_id: "codewhale",
                show_thinking: true,
                allow_shell: true,
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(!prompt.contains("<session_goal>"));
        assert!(!prompt.contains("## Current Hunt"));
    }

    #[test]
    fn tool_selection_guide_avoids_defensive_tool_suppression() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Tool Selection Guide"));
        assert!(prompt.contains("Use `agent_eval`"));
        assert!(
            !prompt.contains("When NOT to use certain tools"),
            "the system prompt should steer tool choice without training the model to avoid available tools"
        );
        assert!(
            !prompt.contains("Don't reach for"),
            "avoid defensive anti-tool wording in the base prompt"
        );
    }

    /// #588: language-mirroring directive must ship in every mode so
    /// DeepSeek's `reasoning_content` and final reply follow the user's
    /// language. Structural test — wording is not a test concern, but
    /// the cross-cutting commitment of #588 is specifically that the
    /// `reasoning_content` field tracks the user's language (not just
    /// the visible reply); pin that anchor token so a future edit
    /// can't silently weaken the section to a generic "respond in the
    /// user's language" directive while keeping the heading.
    #[test]
    fn language_mirroring_section_present_in_all_modes() {
        for mode in [AppMode::Agent, AppMode::Yolo, AppMode::Plan] {
            let prompt = compose_prompt(mode, Personality::Calm);
            assert!(
                prompt.contains("## Language"),
                "## Language section missing from mode {mode:?}"
            );
            assert!(
                prompt.contains("reasoning_content"),
                "## Language section in {mode:?} must mention `reasoning_content` — \
                 that field name is the structural anchor for the #588 commitment that \
                 internal reasoning, not just the visible reply, follows the user's language"
            );
        }
    }

    #[test]
    fn language_mirroring_prioritizes_latest_user_message_over_locale_default() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(
            prompt.contains("latest user message first"),
            "the language directive must choose the turn language from the user message before \
             falling back to the environment locale"
        );
        assert!(
            prompt.contains("If the latest user message is clearly English"),
            "English user text must not drift after non-English context"
        );
        assert!(
            prompt.contains("localized READMEs") && prompt.contains("tool results"),
            "file/tool context must not become a language signal"
        );
        assert!(
            prompt.contains("even when the `lang` field in `## Environment` is `en`"),
            "Chinese user text must override an English resolved locale for reasoning_content"
        );
        assert!(
            prompt.contains("Use the `lang` field only when"),
            "environment locale should be an ambiguity fallback, not the primary language source"
        );
    }

    #[test]
    fn english_base_prompt_avoids_native_script_language_priming() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(
            !contains_cjk(&prompt),
            "English base prompt should keep native-script reinforcement in locale bookends only"
        );
        assert!(
            !prompt.contains("multilingual coding agent"),
            "identity should not prime language switching; language belongs in the Language section"
        );
    }

    /// #358: rlm guidance was reframed from "first-class" to "specialty
    /// tool" — verify the structural markers are present so a future
    /// change doesn't silently remove the RLM section entirely.
    ///
    /// Don't assert on prose. If you wouldn't fail a code review for
    /// changing the wording, don't fail a test for it.
    #[test]
    fn rlm_specialty_tool_guidance_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        // Structural: the RLM heading must exist as a section anchor.
        assert!(prompt.contains("RLM — How to Use It"));
        // Structural: the word "rlm" must appear multiple times (tool
        // name, section heading, toolbox reference). Just verify the
        // lowercase form — exact wording is NOT a test concern.
        let rlm_count = prompt.to_lowercase().matches("rlm").count();
        assert!(
            rlm_count >= 5,
            "RLM guidance present: expected >= 5 mentions of 'rlm', got {rlm_count}"
        );
        assert!(
            !prompt.contains("When NOT to use RLM"),
            "RLM guidance should explain fit and verification without telling the model to avoid the tool"
        );
    }

    /// Tier 5 Local Law must explicitly cover `EngineConfig.instructions`
    /// files. Without this clause, embedders that inject instructions via the
    /// config field (rather than via the four hard-coded path conventions)
    /// get their files classified by path — and since those embedder-supplied
    /// paths aren't `AGENTS.md` / `CLAUDE.md` / `.codewhale/instructions.md` /
    /// `.deepseek/instructions.md`, the model defaults to treating their
    /// imperatives as Tier 7 Memory (the lowest tier per Article VII),
    /// overridable by a single user sentence.
    #[test]
    fn local_law_tier_covers_engine_config_instructions() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(
            prompt.contains("any file configured via `EngineConfig.instructions`"),
            "Tier 5 must explicitly cover EngineConfig.instructions so \
             embedder-injected instructions are not default-classified as Tier 7 Memory."
        );
    }

    #[test]
    fn workspace_orientation_guidance_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("Local Law"));
        assert!(
            prompt.contains("CLAUDE.md"),
            "CLAUDE.md must be listed as a project instruction source"
        );
    }

    #[test]
    fn prompt_uses_persistent_agent_and_rlm_surface() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        for tool in [
            "agent_open",
            "agent_eval",
            "agent_close",
            "rlm_open",
            "rlm_eval",
            "rlm_configure",
            "rlm_close",
            "handle_read",
        ] {
            assert!(
                prompt.contains(tool),
                "prompt should mention new persistent tool `{tool}`"
            );
        }
        for retired in [
            "agent_spawn",
            "agent_wait",
            "agent_result",
            "agent_send_input",
            "agent_assign",
            "agent_resume",
            "agent_list",
            "spawn_agent",
            "delegate_to_agent",
            "send_input",
            "close_agent",
        ] {
            assert!(
                !prompt.contains(retired),
                "prompt should not advertise retired sub-agent tool `{retired}`"
            );
        }
    }

    #[test]
    fn prompt_documents_fork_context_prefix_cache_contract() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("fork_context: true"));
        assert!(prompt.contains("byte-identical"));
        assert!(prompt.contains("DeepSeek prefix-cache reuse"));
        assert!(prompt.contains("Fresh sessions are the default"));
    }

    #[test]
    fn subagent_done_sentinel_section_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Internal Sub-agent Completion Events"));
        assert!(prompt.contains("<codewhale:subagent.done>"));
        assert!(prompt.contains("not user input"));
        assert!(prompt.contains("Integration protocol"));
        assert!(prompt.contains("Do not tell the user they pasted sentinels"));
    }

    #[test]
    fn preamble_rhythm_section_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        // Preamble rhythm is now part of the Calm personality overlay.
        // Verify the load-bearing guidance is still present.
        assert!(prompt.contains("In preambles, name the action"));
        assert!(prompt.contains("Reading the module tree"));
    }

    #[test]
    fn legacy_constants_still_available() {
        // Verify the legacy .txt constant still compiles and contains expected content
        assert!(AGENT_PROMPT.lines().next().is_some());
    }

    // ── Cache-prefix stability harness (#263 step 2) ───────────────────────
    //
    // These tests pin the byte-stability invariant required for DeepSeek's
    // KV prefix cache to hit: any prompt-construction surface that ends up
    // in the cached prefix must produce identical bytes given identical
    // inputs across calls.

    use crate::test_support::assert_byte_identical;

    #[test]
    fn compose_prompt_is_byte_stable_across_calls() {
        // Suspect #4 from #263: mode prompt churn within a single mode.
        // Two calls with identical (mode, personality) inputs must produce
        // identical bytes — anything else is a cache buster.
        for mode in [AppMode::Agent, AppMode::Yolo, AppMode::Plan] {
            for personality in [Personality::Calm, Personality::Playful] {
                let a = compose_prompt(mode, personality);
                let b = compose_prompt(mode, personality);
                assert_byte_identical(
                    &format!("compose_prompt(mode={mode:?}, personality={personality:?})"),
                    &a,
                    &b,
                );
            }
        }
    }

    #[test]
    fn system_prompt_for_mode_with_context_is_byte_stable_for_unchanged_workspace() {
        // Same workspace, no working_set / skills churn between calls →
        // identical bytes. This pins the most representative production
        // surface (engine.rs builds the system prompt via this fn or
        // its sibling _and_skills variant on every turn).
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();

        for mode in [AppMode::Agent, AppMode::Yolo, AppMode::Plan] {
            let a = match system_prompt_for_mode_with_context(mode, workspace, None) {
                SystemPrompt::Text(text) => text,
                SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
            };
            let b = match system_prompt_for_mode_with_context(mode, workspace, None) {
                SystemPrompt::Text(text) => text,
                SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
            };
            assert_byte_identical(
                &format!("system_prompt_for_mode_with_context(mode={mode:?}) on empty workspace"),
                &a,
                &b,
            );
        }
    }

    #[test]
    fn system_prompt_ignores_working_set_summary_argument() {
        // Working-set metadata is now injected into the latest user message
        // per turn. The legacy argument remains for call-site compatibility
        // but must not reintroduce volatile bytes into the system prompt.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let summary = "## Repo Working Set\nWorkspace: /tmp/x\n";

        let a = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, Some(summary))
        {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, Some(summary))
        {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert_byte_identical(
            "system_prompt_for_mode_with_context with constant working_set summary",
            &a,
            &b,
        );
        assert!(
            !a.contains(summary),
            "summary must not be embedded in system prompt"
        );
    }

    #[test]
    fn system_prompt_with_handoff_file_is_byte_stable_when_file_is_unchanged() {
        // If `.deepseek/handoff.md` hasn't moved between two builds, the
        // rendered prompt must produce identical bytes. The relay block
        // lands below the static boundary in
        // `system_prompt_for_mode_with_context_and_skills`.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(
            handoff_dir.join("handoff.md"),
            "# Session relay\n\n## Active task\nFinish #280.\n\n## Open blockers\n- [ ] none\n",
        )
        .unwrap();

        let a = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let b = match system_prompt_for_mode_with_context(AppMode::Agent, workspace, None) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        assert_byte_identical(
            "system_prompt_for_mode_with_context with constant handoff file",
            &a,
            &b,
        );
        assert!(a.contains(HANDOFF_BLOCK_MARKER), "relay must be embedded");
        assert!(a.contains("Finish #280."), "relay body must be present");
    }

    #[test]
    fn handoff_appears_after_static_blocks_without_working_set() {
        // Cache-prefix invariant: the relay block must come after static
        // `## Context Management` and the compaction relay template
        // (`## Compaction Relay`). Working-set metadata is per-turn user
        // metadata now, not a system-prompt tail block.
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let handoff_dir = workspace.join(".deepseek");
        std::fs::create_dir_all(&handoff_dir).unwrap();
        std::fs::write(handoff_dir.join("handoff.md"), "# handoff body\n").unwrap();

        let summary = "## Repo Working Set\nWorkspace: /tmp/x\n";
        let prompt =
            match system_prompt_for_mode_with_context(AppMode::Agent, workspace, Some(summary)) {
                SystemPrompt::Text(text) => text,
                SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
            };

        let context_pos = prompt
            .find("## Context Management")
            .expect("Context Management section present in Agent mode");
        let compact_pos = prompt
            .find("## Compaction Relay")
            .expect("compaction relay template present");
        let handoff_pos = prompt
            .find(HANDOFF_BLOCK_MARKER)
            .expect("relay block present when fixture file exists");
        assert!(
            !prompt.contains("## Repo Working Set"),
            "working-set summary must stay out of the system prompt"
        );

        assert!(
            context_pos < handoff_pos,
            "## Context Management must precede the relay block"
        );
        assert!(
            compact_pos < handoff_pos,
            "## Compaction Relay must precede the relay block"
        );
    }

    #[test]
    fn render_instructions_block_returns_none_for_empty_input() {
        let empty: &[super::InstructionSource] = &[];
        assert!(super::render_instructions_block(empty).is_none());
    }

    #[test]
    fn render_instructions_block_skips_missing_files_with_warning() {
        let tmp = tempdir().expect("tempdir");
        let real = tmp.path().join("real.md");
        std::fs::write(&real, "real content here").unwrap();
        let bogus = tmp.path().join("does-not-exist.md");

        let block = super::render_instructions_block(&[bogus.clone().into(), real.clone().into()])
            .expect("present file should produce a block");
        assert!(block.contains("real content here"));
        assert!(block.contains(&real.display().to_string()));
        // Bogus path is skipped, not rendered.
        assert!(!block.contains(&bogus.display().to_string()));
    }

    #[test]
    fn render_instructions_block_concatenates_in_declared_order() {
        let tmp = tempdir().expect("tempdir");
        let a = tmp.path().join("a.md");
        let b = tmp.path().join("b.md");
        std::fs::write(&a, "ALPHA_MARKER").unwrap();
        std::fs::write(&b, "BRAVO_MARKER").unwrap();

        let block = super::render_instructions_block(&[a.into(), b.into()]).expect("non-empty");
        let alpha_pos = block.find("ALPHA_MARKER").expect("alpha rendered");
        let bravo_pos = block.find("BRAVO_MARKER").expect("bravo rendered");
        assert!(
            alpha_pos < bravo_pos,
            "instructions must concatenate in declared order"
        );
    }

    #[test]
    fn render_instructions_block_skips_empty_files() {
        let tmp = tempdir().expect("tempdir");
        let empty = tmp.path().join("empty.md");
        let real = tmp.path().join("real.md");
        std::fs::write(&empty, "   \n   \n").unwrap();
        std::fs::write(&real, "real content").unwrap();

        let block =
            super::render_instructions_block(&[empty.into(), real.into()]).expect("non-empty");
        // Empty file produces no `<instructions>` section, only the real one.
        let count = block.matches("<instructions").count();
        assert_eq!(count, 1, "only the non-empty file should produce a section");
    }

    #[test]
    fn render_instructions_block_truncates_oversize_files() {
        let tmp = tempdir().expect("tempdir");
        let big = tmp.path().join("big.md");
        // 200 KiB of content — well above the 100 KiB cap.
        std::fs::write(&big, "X".repeat(200 * 1024)).unwrap();

        let block = super::render_instructions_block(&[big.into()]).expect("non-empty");
        assert!(block.contains("[…elided]"), "truncation marker missing");
        // Block should be much smaller than the original file.
        assert!(
            block.len() < 110 * 1024,
            "block should be capped near 100 KiB"
        );
    }

    /// `InstructionSource::Inline` bypasses disk reads — the content is used
    /// directly and `name` becomes the `<instructions source="…">` attribute.
    /// Empty / oversize handling mirrors `File` variant.
    #[test]
    fn render_instructions_block_handles_inline_source() {
        let block = super::render_instructions_block(&[super::InstructionSource::Inline {
            name: "embedded:test/template".to_string(),
            content: "INLINE_MARKER_CONTENT".to_string(),
        }])
        .expect("non-empty");
        assert!(block.contains("INLINE_MARKER_CONTENT"));
        assert!(block.contains("source=\"embedded:test/template\""));

        // Empty inline → skipped just like empty file.
        let empty_inline = super::InstructionSource::Inline {
            name: "empty".to_string(),
            content: "   ".to_string(),
        };
        assert!(super::render_instructions_block(&[empty_inline]).is_none());

        // Oversize inline → truncated with elided marker.
        let big_inline = super::InstructionSource::Inline {
            name: "huge".to_string(),
            content: "Y".repeat(200 * 1024),
        };
        let trimmed = super::render_instructions_block(&[big_inline]).expect("non-empty");
        assert!(trimmed.contains("[…elided]"));

        // File + Inline 混用,顺序保持。
        let tmp = tempdir().expect("tempdir");
        let file_path = tmp.path().join("file-first.md");
        std::fs::write(&file_path, "FILE_MARKER").unwrap();
        let mixed = super::render_instructions_block(&[
            file_path.into(),
            super::InstructionSource::Inline {
                name: "inline-second".to_string(),
                content: "INLINE_MARKER".to_string(),
            },
        ])
        .expect("non-empty");
        let file_pos = mixed.find("FILE_MARKER").expect("file rendered");
        let inline_pos = mixed.find("INLINE_MARKER").expect("inline rendered");
        assert!(file_pos < inline_pos, "声明顺序必须保留(File then Inline)");
    }

    #[test]
    fn instructions_block_appears_in_system_prompt_when_configured() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let extra = workspace.join("extra-instructions.md");
        std::fs::write(&extra, "EXTRA_INSTRUCTIONS_MARKER_BODY").unwrap();

        let extra_source: super::InstructionSource = extra.clone().into();
        let prompt = match super::system_prompt_for_mode_with_context_and_skills(
            AppMode::Agent,
            workspace,
            None,
            None,
            Some(std::slice::from_ref(&extra_source)),
            None,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(
            prompt.contains("EXTRA_INSTRUCTIONS_MARKER_BODY"),
            "configured instructions file body must appear in the prompt"
        );
        assert!(
            prompt.contains(&extra.display().to_string()),
            "instructions block must annotate its source path"
        );
    }
}
