//! `/constitution` command surface (#3806).

use std::fmt::Write as _;
use std::path::PathBuf;

use codewhale_config::{
    ConstitutionChoice, ConstitutionSource, ConstitutionValidity, RuntimePostureSource, SetupState,
    SetupStep, UserConstitution, UserConstitutionLoad,
};

use crate::commands::traits::{CommandInfo, RegisterCommand};
use crate::localization::{Locale, MessageId};
use crate::tui::app::{App, AppAction};
use crate::tui::pager::PagerView;

use super::CommandResult;

pub(in crate::commands) const COMMAND_INFO: CommandInfo = CommandInfo {
    name: "constitution",
    aliases: &["law"],
    usage: "/constitution [status|preview|bundled|edit|review|repair|repo|explain|posture|help]",
    description_id: MessageId::CmdConstitutionDescription,
};

pub(in crate::commands) struct ConstitutionCmd;

impl RegisterCommand for ConstitutionCmd {
    fn info() -> &'static CommandInfo {
        &COMMAND_INFO
    }

    fn execute(app: &mut App, arg: Option<&str>) -> CommandResult {
        match arg.map(str::trim).filter(|arg| !arg.is_empty()) {
            None | Some("status" | "home" | "manager") => {
                open_status(app);
                CommandResult::ok()
            }
            Some("preview") => {
                open_preview(app);
                CommandResult::ok()
            }
            Some("review" | "existing") => {
                open_review(app);
                CommandResult::ok()
            }
            Some("repo" | "repo-local" | "law") => {
                open_repo_law(app);
                CommandResult::ok()
            }
            Some("explain" | "agents") => {
                open_explanation(app);
                CommandResult::ok()
            }
            Some("edit" | "guided" | "custom") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::Constitution,
                })
            }
            Some("repair" | "fix") => CommandResult::with_message_and_action(
                repair_text(app.ui_locale),
                AppAction::OpenSetupWizardAt {
                    step: SetupStep::Constitution,
                },
            ),
            Some("posture" | "runtime-posture") => {
                CommandResult::action(AppAction::OpenSetupWizardAt {
                    step: SetupStep::TrustSandbox,
                })
            }
            Some("bundled" | "default" | "use-bundled" | "use-default") => {
                CommandResult::action(AppAction::UseBundledConstitution)
            }
            Some("help") => CommandResult::message(help_text(app.ui_locale)),
            Some(other) => CommandResult::error(format!(
                "Unknown /constitution target '{other}'. Try `/constitution` for the manager."
            )),
        }
    }
}

fn open_status(app: &mut App) {
    let locale = app.ui_locale;
    let text = format_status(app, locale);
    open_pager(app, manager_title(locale), &text);
}

fn open_review(app: &mut App) {
    let locale = app.ui_locale;
    let mut text = format_status(app, locale);
    let _ = write!(text, "\n\n{}", preview_text(locale));
    open_pager(app, review_title(locale), &text);
}

fn open_preview(app: &mut App) {
    let locale = app.ui_locale;
    let text = preview_text(locale);
    open_pager(app, rendered_title(locale), &text);
}

fn open_repo_law(app: &mut App) {
    let locale = app.ui_locale;
    let context = crate::project_context::load_project_context_with_parents(&app.workspace);
    let text = match context.constitution_block {
        Some(block) => block,
        None => no_repo_law_text(locale).to_string(),
    };
    open_pager(app, repo_title(locale), &text);
}

fn open_explanation(app: &mut App) {
    let locale = app.ui_locale;
    open_pager(app, explanation_title(locale), agents_explanation(locale));
}

fn open_pager(app: &mut App, title: &str, text: &str) {
    let width = app
        .viewport
        .last_transcript_area
        .map(|area| area.width)
        .unwrap_or(80);
    app.view_stack
        .push(PagerView::from_text(title, text, width.saturating_sub(2)));
}

fn format_status(app: &App, locale: Locale) -> String {
    let state = load_setup_state();
    let load = load_user_constitution();
    let context = crate::project_context::load_project_context_with_parents(&app.workspace);
    let mut out = String::new();

    let copy = ConstitutionManagerCopy::for_locale(locale);

    let _ = writeln!(out, "{}", copy.manager_header);
    out.push('\n');
    let _ = writeln!(out, "{}", copy.active_stack_header);
    let _ = writeln!(out, "- {}", copy.bundled_active);
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.user_global_label,
        user_constitution_stack_status(state.as_ref(), &load, locale)
    );
    if let Some(path) = context.constitution_source_path.as_ref() {
        let _ = writeln!(
            out,
            "- {}: {} ({})",
            copy.repo_local_label,
            copy.present,
            path.display()
        );
    } else {
        let _ = writeln!(out, "- {}: {}", copy.repo_local_label, copy.not_present);
    }
    if let Some(path) = context.source_path.as_ref() {
        let _ = writeln!(
            out,
            "- {}: {} ({})",
            copy.agents_label,
            copy.present,
            path.display()
        );
    } else if context.instructions.is_some() {
        let _ = writeln!(out, "- {}: {}", copy.agents_label, copy.generated_fallback);
    } else {
        let _ = writeln!(out, "- {}: {}", copy.agents_label, copy.not_present);
    }
    let whale_warnings = ignored_whale_warnings(&context.warnings);
    if whale_warnings.is_empty() {
        let _ = writeln!(out, "- {}: {}", copy.legacy_whale_label, copy.not_present);
    } else {
        let _ = writeln!(
            out,
            "- {}: {} ({} {})",
            copy.legacy_whale_label,
            copy.whale_ignored,
            whale_warnings.len(),
            copy.location_count_unit(whale_warnings.len())
        );
        for warning in whale_warnings {
            let _ = writeln!(out, "  - {warning}");
        }
    }
    let handoff_path = app.workspace.join(crate::prompts::HANDOFF_RELATIVE_PATH);
    let _ = writeln!(
        out,
        "- {}: {} {}, {} {}",
        copy.memory_handoff_label,
        copy.memory_label,
        if app.use_memory {
            copy.enabled
        } else {
            copy.disabled
        },
        copy.handoff_label,
        if handoff_path.exists() {
            copy.present
        } else {
            copy.not_present
        }
    );

    out.push('\n');
    let _ = writeln!(out, "{}", copy.user_global_header);
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.choice_label,
        state.as_ref().map_or(copy.not_recorded, |s| choice_label(
            s.constitution_choice,
            locale
        ))
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.source_label,
        state.as_ref().map_or(copy.not_recorded, |s| source_label(
            s.constitution_source,
            locale
        ))
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.file_label,
        user_constitution_file_label(&load, locale)
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.validity_label,
        validity_label(load.validity_for_display(state.as_ref()), locale)
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.language_label,
        constitution_language(state.as_ref(), &load, locale)
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.last_preview_label,
        preview_record_label(state.as_ref(), locale)
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.runtime_posture_label,
        state.as_ref().map_or(copy.not_reviewed, |s| posture_label(
            s.runtime_posture_source,
            locale
        ))
    );
    let _ = writeln!(
        out,
        "- {}: {}",
        copy.checkpoint_label,
        state.as_ref().map_or(copy.not_completed.to_string(), |s| {
            s.constitution_checkpoint_completed_for
                .as_ref()
                .map_or_else(|| copy.not_completed.to_string(), |v| copy.completed_for(v))
        })
    );

    out.push('\n');
    let _ = writeln!(out, "{}", copy.preview_header);
    let _ = writeln!(out, "- {}", copy.preview_action);
    let _ = writeln!(out, "- {}", copy.repo_action);

    out.push('\n');
    let _ = writeln!(out, "{}", copy.maintenance_header);
    for action in copy.maintenance_actions {
        let _ = writeln!(out, "- {action}");
    }
    out
}

fn preview_text(locale: Locale) -> String {
    let state = load_setup_state();
    let load = load_user_constitution();
    match load {
        UserConstitutionStatus::Loaded { path, constitution } => {
            let active = user_constitution_is_active(state.as_ref());
            let mut text = String::new();
            if !active {
                text.push_str(inactive_preview_text(locale));
                text.push_str("\n\n");
            }
            text.push_str(
                &constitution
                    .render_block(Some(&path))
                    .unwrap_or_else(|| structured_empty_text(locale).to_string()),
            );
            text
        }
        UserConstitutionStatus::Missing { path } => missing_preview_text(locale, &path),
        UserConstitutionStatus::Empty { path } => empty_preview_text(locale, &path),
        UserConstitutionStatus::Invalid { path, error } => {
            invalid_preview_text(locale, &path, &error)
        }
        UserConstitutionStatus::Unreadable { path, error } => {
            unreadable_preview_text(locale, &path, &error)
        }
        UserConstitutionStatus::PathError { error } => path_error_preview_text(locale, &error),
    }
}

fn load_setup_state() -> Option<SetupState> {
    SetupState::load().ok().flatten()
}

#[derive(Debug)]
enum UserConstitutionStatus {
    Missing {
        path: PathBuf,
    },
    Empty {
        path: PathBuf,
    },
    Invalid {
        path: PathBuf,
        error: String,
    },
    Unreadable {
        path: PathBuf,
        error: String,
    },
    Loaded {
        path: PathBuf,
        constitution: Box<codewhale_config::UserConstitution>,
    },
    PathError {
        error: String,
    },
}

impl UserConstitutionStatus {
    fn validity(&self) -> ConstitutionValidity {
        match self {
            Self::Missing { .. } | Self::PathError { .. } => ConstitutionValidity::Unknown,
            Self::Empty { .. } => ConstitutionValidity::Empty,
            Self::Invalid { .. } => ConstitutionValidity::Invalid,
            Self::Unreadable { .. } => ConstitutionValidity::Unreadable,
            Self::Loaded { constitution, .. } => constitution.validity(),
        }
    }

    fn validity_for_display(&self, state: Option<&SetupState>) -> ConstitutionValidity {
        match self {
            Self::Missing { .. } | Self::PathError { .. } => {
                state.map_or(ConstitutionValidity::Unknown, |s| s.constitution_validity)
            }
            _ => self.validity(),
        }
    }
}

fn load_user_constitution() -> UserConstitutionStatus {
    let path = match UserConstitution::path() {
        Ok(path) => path,
        Err(error) => {
            return UserConstitutionStatus::PathError {
                error: error.to_string(),
            };
        }
    };

    match UserConstitution::load_from(&path) {
        UserConstitutionLoad::Missing => UserConstitutionStatus::Missing { path },
        UserConstitutionLoad::Empty => UserConstitutionStatus::Empty { path },
        UserConstitutionLoad::Invalid(error) => UserConstitutionStatus::Invalid { path, error },
        UserConstitutionLoad::Unreadable(error) => {
            UserConstitutionStatus::Unreadable { path, error }
        }
        UserConstitutionLoad::Loaded(constitution) => {
            UserConstitutionStatus::Loaded { path, constitution }
        }
    }
}

fn user_constitution_stack_status(
    state: Option<&SetupState>,
    load: &UserConstitutionStatus,
    locale: Locale,
) -> String {
    let text = match load {
        UserConstitutionStatus::Loaded { .. } if user_constitution_is_active(state) => match locale
        {
            Locale::ZhHans => "结构化用户全局准则已生效",
            _ => "active structured user-global law",
        },
        UserConstitutionStatus::Loaded { .. } => match locale {
            Locale::ZhHans => "有效但未生效（已选择内置/默认或专家覆盖）",
            _ => "valid but inactive (bundled/default or expert override selected)",
        },
        UserConstitutionStatus::Missing { .. } => match locale {
            Locale::ZhHans => "未配置；使用内置/默认准则",
            _ => "not configured; bundled/default applies",
        },
        UserConstitutionStatus::Empty { .. } => match locale {
            Locale::ZhHans => "为空；建议修复",
            _ => "empty; repair recommended",
        },
        UserConstitutionStatus::Invalid { .. } => match locale {
            Locale::ZhHans => "无效；建议修复",
            _ => "invalid; repair recommended",
        },
        UserConstitutionStatus::Unreadable { .. } => match locale {
            Locale::ZhHans => "无法读取；建议修复",
            _ => "unreadable; repair recommended",
        },
        UserConstitutionStatus::PathError { .. } => match locale {
            Locale::ZhHans => "不可用；CODEWHALE_HOME 错误",
            _ => "unavailable; CODEWHALE_HOME error",
        },
    };
    text.to_string()
}

fn user_constitution_is_active(state: Option<&SetupState>) -> bool {
    !matches!(
        state.map(|s| s.constitution_choice),
        Some(
            ConstitutionChoice::Bundled
                | ConstitutionChoice::Deferred
                | ConstitutionChoice::ExpertOverride
        )
    )
}

fn user_constitution_file_label(load: &UserConstitutionStatus, locale: Locale) -> String {
    match load {
        UserConstitutionStatus::Missing { path }
        | UserConstitutionStatus::Empty { path }
        | UserConstitutionStatus::Invalid { path, .. }
        | UserConstitutionStatus::Unreadable { path, .. }
        | UserConstitutionStatus::Loaded { path, .. } => path.display().to_string(),
        UserConstitutionStatus::PathError { .. } => match locale {
            Locale::ZhHans => "无法解析",
            _ => "unresolved",
        }
        .to_string(),
    }
}

fn constitution_language(
    state: Option<&SetupState>,
    load: &UserConstitutionStatus,
    locale: Locale,
) -> String {
    if let UserConstitutionStatus::Loaded { constitution, .. } = load
        && let Some(language) = constitution.language.as_deref()
    {
        return language.to_string();
    }
    state
        .and_then(|s| s.constitution_language.as_deref())
        .unwrap_or(match locale {
            Locale::ZhHans => "未记录",
            _ => "not recorded",
        })
        .to_string()
}

fn preview_record_label(state: Option<&SetupState>, locale: Locale) -> String {
    let Some(state) = state else {
        return match locale {
            Locale::ZhHans => "未记录",
            _ => "not recorded",
        }
        .to_string();
    };
    match state.constitution_preview_hash.as_deref() {
        Some(hash) => format!("v{} ({hash})", state.constitution_preview_version),
        None => match locale {
            Locale::ZhHans => "未记录",
            _ => "not recorded",
        }
        .to_string(),
    }
}

fn choice_label(choice: ConstitutionChoice, locale: Locale) -> &'static str {
    match (locale, choice) {
        (Locale::ZhHans, ConstitutionChoice::Unset) => "未设置",
        (Locale::ZhHans, ConstitutionChoice::Bundled) => "内置/默认",
        (Locale::ZhHans, ConstitutionChoice::GuidedCustom) => "引导式自定义",
        (Locale::ZhHans, ConstitutionChoice::ExpertOverride) => "专家覆盖",
        (Locale::ZhHans, ConstitutionChoice::Deferred) => "已暂缓；使用内置",
        (_, ConstitutionChoice::Unset) => "not set",
        (_, ConstitutionChoice::Bundled) => "bundled/default",
        (_, ConstitutionChoice::GuidedCustom) => "guided custom",
        (_, ConstitutionChoice::ExpertOverride) => "expert override",
        (_, ConstitutionChoice::Deferred) => "deferred; bundled applies",
    }
}

fn source_label(source: ConstitutionSource, locale: Locale) -> &'static str {
    match (locale, source) {
        (Locale::ZhHans, ConstitutionSource::Bundled) => "内置",
        (Locale::ZhHans, ConstitutionSource::UserGlobal) => "用户全局 constitution.json",
        (Locale::ZhHans, ConstitutionSource::ExpertOverride) => "专家提示词覆盖",
        (_, ConstitutionSource::Bundled) => "bundled",
        (_, ConstitutionSource::UserGlobal) => "user-global constitution.json",
        (_, ConstitutionSource::ExpertOverride) => "expert prompt override",
    }
}

fn validity_label(validity: ConstitutionValidity, locale: Locale) -> &'static str {
    match (locale, validity) {
        (Locale::ZhHans, ConstitutionValidity::Unknown) => "未知或非自定义",
        (Locale::ZhHans, ConstitutionValidity::Valid) => "有效",
        (Locale::ZhHans, ConstitutionValidity::Invalid) => "无效",
        (Locale::ZhHans, ConstitutionValidity::Empty) => "为空",
        (Locale::ZhHans, ConstitutionValidity::Unreadable) => "无法读取",
        (_, ConstitutionValidity::Unknown) => "unknown or not custom",
        (_, ConstitutionValidity::Valid) => "valid",
        (_, ConstitutionValidity::Invalid) => "invalid",
        (_, ConstitutionValidity::Empty) => "empty",
        (_, ConstitutionValidity::Unreadable) => "unreadable",
    }
}

fn posture_label(source: RuntimePostureSource, locale: Locale) -> &'static str {
    match (locale, source) {
        (Locale::ZhHans, RuntimePostureSource::Unset) => "未查看",
        (Locale::ZhHans, RuntimePostureSource::Inherited) => "继承自现有配置",
        (Locale::ZhHans, RuntimePostureSource::Confirmed) => "已在设置中确认",
        (_, RuntimePostureSource::Unset) => "not reviewed",
        (_, RuntimePostureSource::Inherited) => "inherited from existing config",
        (_, RuntimePostureSource::Confirmed) => "confirmed in setup",
    }
}

fn ignored_whale_warnings(warnings: &[String]) -> Vec<&str> {
    warnings
        .iter()
        .map(String::as_str)
        .filter(|warning| warning.contains("WHALE.md is ignored"))
        .collect()
}

fn help_text(locale: Locale) -> String {
    match locale {
        Locale::ZhHans => "\
用法：/constitution [status|preview|bundled|edit|review|repair|repo|explain|posture|help]

常用命令：
- /constitution：打开宪法管理器和当前层级。
- /constitution preview：显示会注入模型的确切用户全局宪法块；缺失、空、无效或不可读时显示修复说明。
- /constitution edit：打开 /setup 的引导式宪法步骤；用 1-6 调整，按 G 预览，再按 G 保存。
- /constitution repair：说明当前文件状态，然后打开同一个引导式修复步骤。
- /constitution bundled：记录使用内置/默认准则，不创建自定义文件。
- /constitution repo：查看 .codewhale/constitution.json 仓库本地准则。
- /constitution explain：解释 Constitution、用户全局宪法、仓库宪法、AGENTS.md、记忆和交接的区别。
- /constitution posture：打开运行时姿态；宪法只提供模型指导，不会更改批准、沙盒、Shell、网络、信任或 MCP 权限。",
        _ => "\
Usage: /constitution [status|preview|bundled|edit|review|repair|repo|explain|posture|help]

Common commands:
- /constitution: open the constitution manager and active stack.
- /constitution preview: show the exact user-global constitution block that would be injected; missing, empty, invalid, or unreadable files show repair guidance instead.
- /constitution edit: open the guided /setup Constitution step; tune 1-6, press G to preview, then G again to save.
- /constitution repair: explain the current file state, then open the same guided repair step.
- /constitution bundled: record bundled/default law without creating a custom file.
- /constitution repo: inspect repo-local .codewhale/constitution.json law.
- /constitution explain: compare Constitution, user-global law, repo law, AGENTS.md, memory, and handoff.
- /constitution posture: open runtime posture; constitution text is model guidance only and does not change approvals, sandbox, shell, network, trust, or MCP permissions.",
    }
    .to_string()
}

fn repair_text(locale: Locale) -> String {
    let state = load_setup_state();
    let load = load_user_constitution();
    let file = user_constitution_file_label(&load, locale);
    let choice = state.as_ref().map_or(
        match locale {
            Locale::ZhHans => "未记录",
            _ => "not recorded",
        },
        |s| choice_label(s.constitution_choice, locale),
    );
    let validity = validity_label(load.validity_for_display(state.as_ref()), locale);
    let status = user_constitution_stack_status(state.as_ref(), &load, locale);

    match locale {
        Locale::ZhHans => format!(
            "\
用户全局宪法修复

当前文件：{file}
当前状态：{status}
记录选择：{choice}
有效性：{validity}

接下来将打开 /setup 的宪法步骤。安全修复路径：
- 用 1-6 调整引导式草稿，按 G 预览，再按 G 保存新的结构化 constitution.json。
- 按 U 或运行 /constitution bundled 记录使用内置/默认准则；现有无效/空/不可读文件不会被注入。
- 用 /constitution preview 查看当前错误或渲染结果。

这只处理用户全局 constitution.json。运行时批准、沙盒、Shell、网络、信任、默认模式和 MCP 权限仍由运行时姿态/配置控制。"
        ),
        _ => format!(
            "\
User-global constitution repair

Current file: {file}
Current state: {status}
Recorded choice: {choice}
Validity: {validity}

Opening the /setup Constitution step next. Safe repair paths:
- Tune the guided draft with 1-6, press G to preview, then G again to save a fresh structured constitution.json.
- Press U or run /constitution bundled to record bundled/default law; the existing invalid/empty/unreadable file will not be injected.
- Use /constitution preview to inspect the current error or rendered result.

This only repairs the user-global constitution.json. Runtime approval, sandbox, shell, network, trust, default mode, and MCP authority still belong to runtime posture/config."
        ),
    }
}

fn manager_title(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "宪法",
        _ => "Constitution",
    }
}

fn review_title(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "宪法检查",
        _ => "Constitution Review",
    }
}

fn rendered_title(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "渲染后的用户宪法",
        _ => "Rendered User Constitution",
    }
}

fn repo_title(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "仓库本地宪法",
        _ => "Repo-Local Constitution",
    }
}

fn explanation_title(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "AGENTS.md 与宪法",
        _ => "AGENTS.md vs Constitution",
    }
}

fn no_repo_law_text(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "此工作区未找到仓库本地宪法 .codewhale/constitution.json。",
        _ => "No repo-local constitution found at .codewhale/constitution.json for this workspace.",
    }
}

fn inactive_preview_text(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "非活动预览：当前选择了内置/默认或专家覆盖。",
        _ => "Inactive preview: bundled/default or expert override is selected.",
    }
}

fn structured_empty_text(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "结构化宪法为空。",
        _ => "The structured constitution is empty.",
    }
}

fn missing_preview_text(locale: Locale, path: &std::path::Path) -> String {
    match locale {
        Locale::ZhHans => format!(
            "未在 {} 找到结构化用户全局宪法。\n\n当前使用内置准则。使用 /constitution edit 创建引导式长期偏好，或使用 /constitution bundled 明确记录内置/默认。",
            path.display()
        ),
        _ => format!(
            "No structured user-global constitution found at {}.\n\nBundled law applies. Use /constitution edit to create guided standing preferences, or /constitution bundled to record bundled/default explicitly.",
            path.display()
        ),
    }
}

fn empty_preview_text(locale: Locale, path: &std::path::Path) -> String {
    match locale {
        Locale::ZhHans => format!(
            "{} 的结构化用户全局宪法为空。使用 /constitution repair 返回引导式宪法步骤。",
            path.display()
        ),
        _ => format!(
            "The structured user-global constitution at {} is empty. Use /constitution repair to return to the guided constitution step.",
            path.display()
        ),
    }
}

fn invalid_preview_text(locale: Locale, path: &std::path::Path, error: &str) -> String {
    match locale {
        Locale::ZhHans => format!(
            "{} 的结构化用户全局宪法无效，且不会注入。\n\n{error}\n\n使用 /constitution repair 返回引导式宪法步骤。",
            path.display()
        ),
        _ => format!(
            "The structured user-global constitution at {} is invalid and is not injected.\n\n{error}\n\nUse /constitution repair to return to the guided constitution step.",
            path.display()
        ),
    }
}

fn unreadable_preview_text(locale: Locale, path: &std::path::Path, error: &str) -> String {
    match locale {
        Locale::ZhHans => format!(
            "无法读取 {} 的结构化用户全局宪法，且不会注入。\n\n{error}\n\n使用 /constitution repair 返回引导式宪法步骤。",
            path.display()
        ),
        _ => format!(
            "The structured user-global constitution at {} could not be read and is not injected.\n\n{error}\n\nUse /constitution repair to return to the guided constitution step.",
            path.display()
        ),
    }
}

fn path_error_preview_text(locale: Locale, error: &str) -> String {
    match locale {
        Locale::ZhHans => format!("无法为用户全局宪法解析 CODEWHALE_HOME：\n\n{error}"),
        _ => {
            format!("Could not resolve CODEWHALE_HOME for the user-global constitution:\n\n{error}")
        }
    }
}

fn agents_explanation(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => {
            "\
AGENTS.md 与宪法

内置 Constitution 是精简的全局判断契约：身份、事实、验证、克制和优先级顺序。

用户全局宪法是个人长期偏好。它是结构化数据，确定性渲染，并低于当前用户请求和内置 Constitution。

.codewhale/constitution.json 是仓库本地准则。它属于某个工作区，并作为独立仓库宪法块渲染。

AGENTS.md 和项目说明是项目法律/实现指导。它们可以描述构建命令、仓库规范和本地流程；按“Whose word wins”层级，它们低于当前用户请求和内置 Constitution，高于用户全局长期偏好、记忆和交接。

WHALE.md 已忽略。将普通项目说明迁移到 AGENTS.md，将 CodeWhale 专属权限策略迁移到 .codewhale/constitution.json。

运行时姿态是独立设置。宪法可以建议主动性，但不会改变批准策略、沙盒、Shell、网络、信任、MCP 权限或默认模式。使用 /constitution posture 查看这些控制。"
        }
        _ => {
            "\
AGENTS.md vs constitution

The bundled Constitution is the compact global judgment contract: identity, ground truth, verification, restraint, and precedence.

The user-global constitution is personal standing preference law. It is structured, rendered deterministically, and subordinate to the current user request and the bundled Constitution.

.codewhale/constitution.json is repo-local law. It belongs to a workspace and is rendered as a separate repo constitution block.

AGENTS.md and project instructions are project law / implementation guidance. They can describe build commands, repository norms, and local workflows. Under “Whose word wins,” they sit below the current user request and bundled Constitution, and above user-global standing preferences, memory, and handoff.

WHALE.md is ignored. Move ordinary project instructions to AGENTS.md and CodeWhale-specific authority policy to .codewhale/constitution.json.

Runtime posture is separate. A constitution can recommend autonomy, but it does not change approval policy, sandbox, shell, network, trust, MCP permissions, or default mode. Use /constitution posture to review those controls."
        }
    }
}

struct ConstitutionManagerCopy {
    manager_header: &'static str,
    active_stack_header: &'static str,
    bundled_active: &'static str,
    user_global_label: &'static str,
    repo_local_label: &'static str,
    agents_label: &'static str,
    legacy_whale_label: &'static str,
    memory_handoff_label: &'static str,
    memory_label: &'static str,
    handoff_label: &'static str,
    user_global_header: &'static str,
    choice_label: &'static str,
    source_label: &'static str,
    file_label: &'static str,
    validity_label: &'static str,
    language_label: &'static str,
    last_preview_label: &'static str,
    runtime_posture_label: &'static str,
    checkpoint_label: &'static str,
    preview_header: &'static str,
    preview_action: &'static str,
    repo_action: &'static str,
    maintenance_header: &'static str,
    maintenance_actions: &'static [&'static str],
    present: &'static str,
    not_present: &'static str,
    generated_fallback: &'static str,
    whale_ignored: &'static str,
    enabled: &'static str,
    disabled: &'static str,
    not_recorded: &'static str,
    not_reviewed: &'static str,
    not_completed: &'static str,
}

impl ConstitutionManagerCopy {
    fn for_locale(locale: Locale) -> Self {
        match locale {
            Locale::ZhHans => Self {
                manager_header: "宪法管理器",
                active_stack_header: "生效层级",
                bundled_active: "内置 Constitution：始终生效的基础准则",
                user_global_label: "用户全局宪法",
                repo_local_label: "仓库本地宪法",
                agents_label: "AGENTS/项目说明",
                legacy_whale_label: "旧版 WHALE.md",
                memory_handoff_label: "记忆/交接",
                memory_label: "记忆",
                handoff_label: "交接",
                user_global_header: "用户全局宪法",
                choice_label: "选择",
                source_label: "来源",
                file_label: "文件",
                validity_label: "有效性",
                language_label: "语言",
                last_preview_label: "上次接受的预览",
                runtime_posture_label: "运行时姿态",
                checkpoint_label: "检查点",
                preview_header: "预览",
                preview_action: "/constitution preview 会在存在时打开精确渲染的用户全局块。",
                repo_action: "/constitution repo 会在存在时显示 .codewhale/constitution.json 本地准则。",
                maintenance_header: "维护",
                maintenance_actions: &[
                    "编辑引导式宪法：/constitution edit",
                    "预览渲染后的宪法：/constitution preview",
                    "使用内置/默认：/constitution bundled",
                    "查看现有内容：/constitution review",
                    "修复无效/空/不可读文件：/constitution repair",
                    "显示仓库本地准则：/constitution repo",
                    "解释 AGENTS.md 与宪法：/constitution explain",
                    "打开运行时姿态：/constitution posture",
                ],
                present: "存在",
                not_present: "不存在",
                generated_fallback: "生成的后备内容",
                whale_ignored: "已忽略；需要迁移",
                enabled: "启用",
                disabled: "停用",
                not_recorded: "未记录",
                not_reviewed: "未查看",
                not_completed: "未完成",
            },
            _ => Self {
                manager_header: "Constitution Manager",
                active_stack_header: "Active stack",
                bundled_active: "Bundled Constitution: active base law (always on)",
                user_global_label: "User-global constitution",
                repo_local_label: "Repo-local constitution",
                agents_label: "AGENTS/project instructions",
                legacy_whale_label: "Legacy WHALE.md",
                memory_handoff_label: "Memory/handoff",
                memory_label: "memory",
                handoff_label: "handoff",
                user_global_header: "User-global constitution",
                choice_label: "Choice",
                source_label: "Source",
                file_label: "File",
                validity_label: "Validity",
                language_label: "Language",
                last_preview_label: "Last accepted preview",
                runtime_posture_label: "Runtime posture",
                checkpoint_label: "Checkpoint",
                preview_header: "Preview",
                preview_action: "/constitution preview opens the exact rendered user-global block when present.",
                repo_action: "/constitution repo shows .codewhale/constitution.json local law when present.",
                maintenance_header: "Maintenance",
                maintenance_actions: &[
                    "Edit guided constitution: /constitution edit",
                    "Preview rendered constitution: /constitution preview",
                    "Use bundled/default: /constitution bundled",
                    "Review existing: /constitution review",
                    "Repair invalid/empty/unreadable: /constitution repair",
                    "Show repo-local law: /constitution repo",
                    "Explain AGENTS.md vs constitution: /constitution explain",
                    "Open runtime posture: /constitution posture",
                ],
                present: "present",
                not_present: "not present",
                generated_fallback: "generated fallback",
                whale_ignored: "ignored; migration needed",
                enabled: "enabled",
                disabled: "disabled",
                not_recorded: "not recorded",
                not_reviewed: "not reviewed",
                not_completed: "not completed",
            },
        }
    }

    fn location_count_unit(&self, count: usize) -> &'static str {
        if self.manager_header == "宪法管理器" {
            "处"
        } else if count == 1 {
            "location"
        } else {
            "locations"
        }
    }

    fn completed_for(&self, version: &str) -> String {
        if self.manager_header == "宪法管理器" {
            format!("已完成 {version}")
        } else {
            format!("completed for {version}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::tui::app::TuiOptions;
    use crate::tui::pager::PagerView;
    use crate::tui::views::ModalKind;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn test_app() -> App {
        test_app_with_workspace(PathBuf::from("."))
    }

    fn test_app_with_workspace(workspace: PathBuf) -> App {
        let options = TuiOptions {
            model: "deepseek-v4-pro".to_string(),
            workspace,
            config_path: None,
            config_profile: None,
            allow_shell: false,
            use_alt_screen: true,
            use_mouse_capture: false,
            use_bracketed_paste: true,
            max_subagents: 1,
            skills_dir: PathBuf::from("."),
            memory_path: PathBuf::from("memory.md"),
            notes_path: PathBuf::from("notes.txt"),
            mcp_config_path: PathBuf::from("mcp.json"),
            use_memory: false,
            start_in_agent_mode: false,
            skip_onboarding: true,
            yolo: false,
            resume_session_id: None,
            initial_input: None,
        };
        App::new(options, &Config::default())
    }

    fn pop_pager_body(app: &mut App) -> String {
        let mut view = app.view_stack.pop().expect("pager view");
        let pager = view
            .as_any_mut()
            .downcast_mut::<PagerView>()
            .expect("top view should be pager");
        pager.body_text()
    }

    #[test]
    fn constitution_default_opens_manager_pager() {
        let mut app = test_app();
        app.ui_locale = Locale::En;

        let result = ConstitutionCmd::execute(&mut app, None);

        assert!(result.message.is_none());
        assert_eq!(app.view_stack.top_kind(), Some(ModalKind::Pager));
        assert!(pop_pager_body(&mut app).contains("Constitution Manager"));
    }

    #[test]
    fn constitution_manager_marks_whale_md_ignored() {
        let tmp = tempdir().expect("tempdir");
        std::fs::write(tmp.path().join("WHALE.md"), "legacy instructions").expect("write whale");
        let mut app = test_app_with_workspace(tmp.path().to_path_buf());
        app.ui_locale = Locale::En;

        let result = ConstitutionCmd::execute(&mut app, None);

        assert!(result.message.is_none());
        let body = pop_pager_body(&mut app);
        assert!(body.contains("Legacy WHALE.md: ignored"));
        assert!(body.contains("WHALE.md is ignored"));
        assert!(!body.contains("legacy instructions"));
    }

    #[test]
    fn constitution_bundled_emits_action() {
        let mut app = test_app();

        let result = ConstitutionCmd::execute(&mut app, Some("bundled"));

        assert_eq!(result.action, Some(AppAction::UseBundledConstitution));
    }

    #[test]
    fn constitution_edit_opens_setup_at_constitution() {
        let mut app = test_app();

        let result = ConstitutionCmd::execute(&mut app, Some("edit"));

        assert_eq!(
            result.action,
            Some(AppAction::OpenSetupWizardAt {
                step: SetupStep::Constitution
            })
        );
    }

    #[test]
    fn constitution_help_lists_repair_and_runtime_boundary() {
        let mut app = test_app();
        app.ui_locale = Locale::En;

        let result = ConstitutionCmd::execute(&mut app, Some("help"));

        let message = result.message.expect("help message");
        assert!(message.contains("Usage: /constitution"));
        assert!(message.contains("/constitution repair"));
        assert!(message.contains("/constitution posture"));
        assert!(message.contains("model guidance only"));
        assert!(message.contains("does not change approvals"));
    }

    #[test]
    fn constitution_repair_explains_invalid_file_and_opens_setup() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&home).expect("home");
        std::fs::write(home.join("constitution.json"), "{not valid json").expect("invalid file");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let mut app = test_app();
        app.ui_locale = Locale::En;

        let result = ConstitutionCmd::execute(&mut app, Some("repair"));

        assert_eq!(
            result.action,
            Some(AppAction::OpenSetupWizardAt {
                step: SetupStep::Constitution
            })
        );
        let message = result.message.expect("repair message");
        assert!(message.contains("User-global constitution repair"));
        assert!(message.contains("Current state: invalid; repair recommended"));
        assert!(message.contains("Validity: invalid"));
        assert!(message.contains("constitution.json"));
        assert!(message.contains("will not be injected"));
        assert!(message.contains("Runtime approval"));
    }

    #[test]
    fn constitution_preview_renders_structured_block() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&home).expect("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let constitution = UserConstitution {
            about: Some("Maintains release lanes.".to_string()),
            ..UserConstitution::default()
        };
        constitution.save().expect("save constitution");
        let mut app = test_app();

        let result = ConstitutionCmd::execute(&mut app, Some("preview"));

        assert!(result.message.is_none());
        let body = pop_pager_body(&mut app);
        assert!(body.contains("<codewhale_user_constitution"));
        assert!(body.contains("Maintains release lanes."));
    }

    #[test]
    fn constitution_manager_uses_zh_hans_copy() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&home).expect("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::ZhHans;

        let result = ConstitutionCmd::execute(&mut app, None);

        assert!(result.message.is_none());
        let body = pop_pager_body(&mut app);
        assert!(body.contains("宪法管理器"));
        assert!(body.contains("生效层级"));
        assert!(body.contains("用户全局宪法"));
        assert!(body.contains("/constitution preview 会"));
        assert!(!body.contains("Constitution Manager"));
    }

    #[test]
    fn constitution_preview_missing_uses_zh_hans_copy() {
        let _env_guard = crate::test_support::lock_test_env();
        let tmp = tempdir().expect("tempdir");
        let home = tmp.path().join("codewhale-home");
        std::fs::create_dir_all(&home).expect("home");
        let _home = crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", home.as_os_str());
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::ZhHans;

        let result = ConstitutionCmd::execute(&mut app, Some("preview"));

        assert!(result.message.is_none());
        let body = pop_pager_body(&mut app);
        assert!(body.contains("未在"));
        assert!(body.contains("当前使用内置准则"));
        assert!(!body.contains("No structured user-global constitution"));
    }

    #[test]
    fn constitution_explanation_uses_zh_hans_copy() {
        let mut app = test_app();
        app.ui_locale = crate::localization::Locale::ZhHans;

        let result = ConstitutionCmd::execute(&mut app, Some("explain"));

        assert!(result.message.is_none());
        let body = pop_pager_body(&mut app);
        assert!(body.contains("AGENTS.md 与宪法"));
        assert!(body.contains("运行时姿态是独立设置"));
        assert!(!body.contains("Runtime posture is separate"));
    }
}
