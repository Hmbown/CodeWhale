#![allow(dead_code)]
//! System prompts for different modes.
//!
//! Prompts are assembled from composable layers loaded at compile time:
//!   base.md → personality overlay → mode delta → approval policy
//!
//! This keeps each concern in its own file and makes prompt tuning
//! a single-file operation.

use crate::models::SystemPrompt;
use crate::project_context::{ProjectContext, load_project_context_with_parents};
use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default)]
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
}

/// Conventional location for the structured session relay artifact (#32).
/// A previous session writes it on exit / `/compact`; the next session reads
/// it back on startup and prepends it to the system prompt so a fresh agent
/// doesn't have to re-discover open blockers from scratch.
pub const HANDOFF_RELATIVE_PATH: &str = ".deepseek/handoff.md";

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
    } else {
        "English"
    }
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
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
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

/// Render the `instructions = [...]` config array as a single
/// system-prompt block (#454). Each path is loaded in declared order;
/// missing files are skipped with a tracing warning so a stale entry
/// in `~/.deepseek/config.toml` doesn't fail the launch. Empty input
/// (or all paths missing) returns `None` so callers append nothing.
fn render_instructions_block(paths: &[PathBuf]) -> Option<String> {
    let mut sections: Vec<String> = Vec::new();
    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(raw) => {
                let trimmed = raw.trim();
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
                    "<instructions source=\"{}\">\n{}\n</instructions>",
                    path.display(),
                    body
                ));
            }
            Err(err) => {
                tracing::warn!(
                    target: "instructions",
                    ?err,
                    ?path,
                    "skipping unreadable instructions file"
                );
            }
        }
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
    let path = workspace.join(HANDOFF_RELATIVE_PATH);
    let raw = std::fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(format!(
        "## Previous Session Relay\n\nThe previous session in this workspace left a relay artifact at `{}`. Consider it the first artifact to read on this turn — open blockers, in-flight changes, and recent decisions live there. Update or rewrite it before exiting if state changes materially.\n\n{}",
        HANDOFF_RELATIVE_PATH, trimmed
    ))
}

// ── Prompt layers loaded at compile time ──────────────────────────────

/// Core: task execution, tool-use rules, output format, toolbox reference,
/// "When NOT to use" guidance, sub-agent sentinel protocol.
pub const BASE_PROMPT: &str = include_str!("prompts/base.md");

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
        "zh-Hans" | "zh-CN" | "zh" => Some(LOCALE_PREAMBLE_ZH_HANS),
        "ja" | "ja-JP" => Some(LOCALE_PREAMBLE_JA),
        "pt-BR" | "pt" => Some(LOCALE_PREAMBLE_PT_BR),
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
        "zh-Hans" | "zh-CN" | "zh" => Some(LOCALE_CLOSER_ZH_HANS),
        "ja" | "ja-JP" => Some(LOCALE_CLOSER_JA),
        "pt-BR" | "pt" => Some(LOCALE_CLOSER_PT_BR),
        _ => None,
    }
}

const LOCALE_PREAMBLE_ZH_HANS: &str = "## 语言要求\n\n\
你正在 DeepSeek TUI 中运行。无论任务上下文（代码、错误日志、文件名）\
是英文，无论系统提示的其余部分是英文，你都必须用简体中文进行 \
`reasoning_content`（内部思考）和最终回复。代码、文件路径、工具名称\
（例如 `read_file`、`exec_shell`）、环境变量、命令行参数和 URL \
保持原样 —— 只有自然语言散文要切换到简体中文。\n\n\
如果用户在会话中切换到另一种语言，从下一轮开始跟随切换。\
如果用户明确要求（例如 \"think in English\"），则覆盖此规则。";

const LOCALE_PREAMBLE_JA: &str = "## 言語要件\n\n\
DeepSeek TUI を実行しています。タスクコンテキスト（コード、エラーログ、\
ファイル名）が英語であっても、システムプロンプトの他の部分が英語で\
あっても、`reasoning_content`（内部思考）と最終的な返信は日本語で\
行ってください。コード、ファイルパス、ツール名（例：`read_file`、\
`exec_shell`）、環境変数、コマンドライン引数、URL は元のまま —— \
自然言語の文章のみ日本語に切り替えます。\n\n\
ユーザーがセッション中に別の言語に切り替えた場合は、次のターンから\
それに従ってください。ユーザーが明示的に要求した場合（例：\
\"think in English\"）はこのルールを上書きします。";

const LOCALE_PREAMBLE_PT_BR: &str = "## Requisito de Idioma\n\n\
Você está rodando dentro do DeepSeek TUI. Escreva tanto \
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
/// model knows the format to use when writing `.deepseek/handoff.md`.
pub const COMPACT_TEMPLATE: &str = include_str!("prompts/compact.md");

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
///   1. base.md        — core identity, toolbox, execution contract
///   2. personality    — voice and tone overlay
///   3. mode delta     — mode-specific permissions and workflow
///   4. approval policy — tool-approval behavior
///
/// Each layer is separated by a blank line for readability in the
/// rendered prompt (the model sees them as contiguous sections).
pub fn compose_prompt(mode: AppMode, personality: Personality) -> String {
    compose_prompt_with_approval(mode, personality, default_approval_mode_for_mode(mode))
}

pub fn compose_prompt_with_approval(
    mode: AppMode,
    personality: Personality,
    approval_mode: ApprovalMode,
) -> String {
    let parts: [&str; 4] = [
        BASE_PROMPT.trim(),
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

/// Compose for the default personality (Calm).
fn compose_mode_prompt(mode: AppMode) -> String {
    compose_prompt(mode, Personality::Calm)
}

fn compose_mode_prompt_with_approval(mode: AppMode, approval_mode: ApprovalMode) -> String {
    compose_prompt_with_approval(mode, Personality::Calm, approval_mode)
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
    instructions: Option<&[PathBuf]>,
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
        },
    )
}

pub fn system_prompt_for_mode_with_context_skills_and_session(
    mode: AppMode,
    workspace: &Path,
    _working_set_summary: Option<&str>,
    skills_dir: Option<&Path>,
    instructions: Option<&[PathBuf]>,
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
    instructions: Option<&[PathBuf]>,
    session_context: PromptSessionContext<'_>,
    approval_mode: ApprovalMode,
) -> SystemPrompt {
    let mode_prompt = compose_mode_prompt_with_approval(mode, approval_mode);

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
    let preamble = locale_reinforcement_preamble(session_context.locale_tag);

    // 1–2. Mode prompt + project context.
    // `load_project_context_with_parents` auto-generates .deepseek/instructions.md
    // when no context file exists, so the fallback should always be available.
    let mut full_prompt = if let Some(project_block) = project_context.as_system_block() {
        format!("{}\n\n{}", mode_prompt, project_block)
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

    // 2.25. Environment block — locale, platform, shell, pwd. All
    // four inputs are session-stable (workspace path is fixed for
    // the run; locale is loaded once by the caller; platform/shell
    // come from process env). Inserted above skills so it remains in
    // the workspace-static cache layer alongside the mode prompt and
    // project context.
    full_prompt = format!(
        "{full_prompt}\n\n{}",
        render_environment_block(workspace, session_context.locale_tag),
    );

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
             If you notice context is getting long (>80%), proactively suggest using `/compact` to the user.\n\n\
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
    //    when writing `.deepseek/handoff.md` on exit / `/compact`.
    full_prompt.push_str("\n\n");
    full_prompt.push_str(COMPACT_TEMPLATE);

    // ── Volatile-content boundary ─────────────────────────────────────────
    // Everything below drifts mid-session and busts the prefix cache for
    // bytes that follow. All static layers (mode, project context, env,
    // skills, context management, compact template) live above this line
    // so DeepSeek's KV prefix cache can hit on the entire system prompt
    // regardless of per-session edits to memory, goals, or instructions.

    // 6a. Configured `instructions = [...]` files (#454). Loaded
    // and concatenated in declared order. Placed below the volatile boundary
    // because these files are workspace-scoped and may differ between
    // sessions; any edit to them would otherwise bust the prefix cache for
    // all subsequent static layers.
    if let Some(paths) = instructions
        && let Some(block) = render_instructions_block(paths)
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
        full_prompt = format!("{full_prompt}\n\n{memory_block}");
    }

    // 6c. Current session goal. Also volatile: users set / change goals
    // during a session via `/goal`. Placed below the boundary for the
    // same reason as memory.
    if let Some(goal_objective) = session_context.goal_objective
        && !goal_objective.trim().is_empty()
    {
        full_prompt = format!(
            "{full_prompt}\n\n## Current Session Goal\n\n<session_goal>\n{}\n</session_goal>",
            goal_objective.trim()
        );
    }

    // 7. Previous-session relay (file-backed, rewritten by `/compact`).
    if let Some(handoff_block) = load_handoff_block(workspace) {
        full_prompt = format!("{full_prompt}\n\n{handoff_block}");
    }

    // 7. Locale-native closing reinforcement (#1118 follow-up #2). The
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
    if let Some(closer) = locale_reinforcement_closer(session_context.locale_tag) {
        full_prompt = format!("{full_prompt}\n\n{closer}");
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

// ── Legacy functions for backwards compatibility ──────────────────────

pub fn base_system_prompt() -> SystemPrompt {
    SystemPrompt::Text(BASE_PROMPT.trim().to_string())
}

pub fn normal_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Agent)
}

pub fn agent_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Agent)
}

pub fn yolo_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Yolo)
}

pub fn plan_system_prompt() -> SystemPrompt {
    system_prompt_for_mode(AppMode::Plan)
}

#[cfg(test)]
mod tests {
    // Don't assert on prose. If you wouldn't fail a code review for
    // changing the wording, don't fail a test for it.
    use super::*;
    use tempfile::tempdir;

    /// Discriminator unique to the injected relay block (not present in the
    /// agent prompt's own discussion of the convention).
    const HANDOFF_BLOCK_MARKER: &str = "left a relay artifact at `.deepseek/handoff.md`";

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
            },
            ApprovalMode::Suggest,
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };
        let preamble_marker = "## 语言要求";
        let base_marker = "You are DeepSeek TUI";
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
        // Bold "must both be in Simplified Chinese" anchor — strong
        // emphasis aimed at the failure mode V4 falls into where it
        // mirrors the user message for the final reply but defaults to
        // English for thinking.
        assert!(
            lang.contains("must both be in Simplified Chinese"),
            "expected the bold Simplified Chinese requirement"
        );
        // "overwhelmingly English" — addresses the specific trigger
        // where a Chinese question lands on a codebase whose system
        // prompt and context are English-heavy.
        assert!(
            lang.contains("overwhelmingly English"),
            "expected the context-is-English caveat"
        );
        // Explicit-user-override clause keeps the prompt useful for the
        // opposite preference (#1118 commenters who want English
        // thinking for token-cost reasons).
        for phrase in [
            "think in English",
            "\u{7528}\u{82F1}\u{6587}\u{601D}\u{8003}",
        ] {
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
        assert!(prompt.contains("You are DeepSeek TUI"));
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
        let base_pos = prompt.find("You are DeepSeek TUI").unwrap();
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
            },
        ) {
            SystemPrompt::Text(text) => text,
            SystemPrompt::Blocks(_) => panic!("expected text system prompt"),
        };

        assert!(!prompt.contains("<session_goal>"));
        assert!(!prompt.contains("## Current Session Goal"));
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
            prompt.contains("even when the `lang` field in `## Environment` is `en`"),
            "Chinese user text must override an English resolved locale for reasoning_content"
        );
        assert!(
            prompt.contains("Use the `lang` field only when"),
            "environment locale should be an ambiguity fallback, not the primary language source"
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

    #[test]
    fn workspace_orientation_guidance_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Workspace Orientation"));
        assert!(prompt.contains("canonical project root"));
        assert!(prompt.contains("AGENTS.md"));
        assert!(prompt.contains("explore` / `explorer"));
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
        assert!(prompt.contains("<deepseek:subagent.done>"));
        assert!(prompt.contains("not user input"));
        assert!(prompt.contains("Integration protocol"));
        assert!(prompt.contains("Do not tell the user they pasted sentinels"));
    }

    #[test]
    fn preamble_rhythm_section_present() {
        let prompt = compose_prompt(AppMode::Agent, Personality::Calm);
        assert!(prompt.contains("Preamble Rhythm"));
        assert!(prompt.contains("I'll start by reading the module structure"));
    }

    #[test]
    fn legacy_constants_still_available() {
        // Verify the legacy .txt constant still compiles and contains expected content
        assert!(!AGENT_PROMPT.is_empty());
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
        assert!(super::render_instructions_block(&[]).is_none());
    }

    #[test]
    fn render_instructions_block_skips_missing_files_with_warning() {
        let tmp = tempdir().expect("tempdir");
        let real = tmp.path().join("real.md");
        std::fs::write(&real, "real content here").unwrap();
        let bogus = tmp.path().join("does-not-exist.md");

        let block = super::render_instructions_block(&[bogus.clone(), real.clone()])
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

        let block = super::render_instructions_block(&[a, b]).expect("non-empty");
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

        let block = super::render_instructions_block(&[empty, real]).expect("non-empty");
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

        let block = super::render_instructions_block(&[big]).expect("non-empty");
        assert!(block.contains("[…elided]"), "truncation marker missing");
        // Block should be much smaller than the original file.
        assert!(
            block.len() < 110 * 1024,
            "block should be capped near 100 KiB"
        );
    }

    #[test]
    fn instructions_block_appears_in_system_prompt_when_configured() {
        let tmp = tempdir().expect("tempdir");
        let workspace = tmp.path();
        let extra = workspace.join("extra-instructions.md");
        std::fs::write(&extra, "EXTRA_INSTRUCTIONS_MARKER_BODY").unwrap();

        let prompt = match super::system_prompt_for_mode_with_context_and_skills(
            AppMode::Agent,
            workspace,
            None,
            None,
            Some(std::slice::from_ref(&extra)),
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
