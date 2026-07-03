//! Constitution file state, provenance, ratification rendering, and
//! persistence helpers for the setup wizard.
//!
//! These types and functions are shared between the guided constitution step
//! and the verification report.

use codewhale_config::{
    ConstitutionChoice, ConstitutionSource, ConstitutionValidity, SetupState, UserConstitution,
    UserConstitutionLoad,
};

use crate::localization::{Locale, MessageId, tr};

/// State of the user's `constitution.json` on disk, resolved at wizard-open
/// time so the card avoids a re-read on every render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupConstitutionFileState {
    NotChecked,
    Missing,
    Loaded,
    Empty,
    Invalid,
    Unreadable,
    PathError,
}

impl SetupConstitutionFileState {
    pub(crate) fn load() -> Self {
        match UserConstitution::path() {
            Ok(path) => Self::from_load(&UserConstitution::load_from(&path)),
            Err(_) => Self::PathError,
        }
    }

    fn from_load(load: &UserConstitutionLoad) -> Self {
        match load {
            UserConstitutionLoad::Missing => Self::Missing,
            UserConstitutionLoad::Empty => Self::Empty,
            UserConstitutionLoad::Invalid(_) => Self::Invalid,
            UserConstitutionLoad::Unreadable(_) => Self::Unreadable,
            UserConstitutionLoad::Loaded(_) => Self::Loaded,
        }
    }

    pub(crate) fn label(self, choice: ConstitutionChoice, locale: Locale) -> &'static str {
        match locale {
            Locale::ZhHans => self.zh_hans_label(choice),
            _ => self.english_label(choice),
        }
    }

    fn english_label(self, choice: ConstitutionChoice) -> &'static str {
        match self {
            Self::NotChecked => "not checked yet",
            Self::Missing => "no constitution.json found; bundled/default applies",
            Self::Loaded if choice == ConstitutionChoice::GuidedCustom => {
                "valid constitution.json present and selected"
            }
            Self::Loaded if choice.is_explicit() => {
                "valid constitution.json present but inactive under the recorded choice"
            }
            Self::Loaded => "valid constitution.json present; preview or save guided to select it",
            Self::Empty => "constitution.json is empty; use G to regenerate or U for bundled",
            Self::Invalid => "constitution.json is invalid; use repair/regenerate or bundled",
            Self::Unreadable => "constitution.json is unreadable; use repair/regenerate or bundled",
            Self::PathError => "CODEWHALE_HOME could not be resolved for constitution.json",
        }
    }

    fn zh_hans_label(self, choice: ConstitutionChoice) -> &'static str {
        match self {
            Self::NotChecked => "尚未检查",
            Self::Missing => "未找到 constitution.json；使用内置/默认准则",
            Self::Loaded if choice == ConstitutionChoice::GuidedCustom => {
                "有效 constitution.json 已存在并已选择"
            }
            Self::Loaded if choice.is_explicit() => {
                "有效 constitution.json 已存在，但当前记录选择使其不生效"
            }
            Self::Loaded => "有效 constitution.json 已存在；预览或保存引导式宪法即可选择",
            Self::Empty => "constitution.json 为空；按 G 重新生成或按 U 使用内置",
            Self::Invalid => "constitution.json 无效；请修复/重新生成，或使用内置",
            Self::Unreadable => "constitution.json 无法读取；请修复/重新生成，或使用内置",
            Self::PathError => "无法解析 CODEWHALE_HOME 中的 constitution.json",
        }
    }
}

/// Who authored the draft being previewed for ratification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DraftProvenance {
    /// Rendered deterministically from the guided answers.
    Guided,
    /// Drafted by the named model, then sanitized and bounded by CodeWhale.
    Model(String),
    /// The user's existing `constitution.json`, shown unchanged for the
    /// keep-existing checkpoint completion (#3794).
    Existing,
}

pub(crate) fn ratification_preview_title(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "用户宪法 — 批准前草案",
        _ => "User Constitution — Draft for Ratification",
    }
}

/// The ratification artifact shown in the pager: provenance, what a
/// constitution is, the exact block that will be injected (byte-identical to
/// prompt assembly's rendering), its authority boundaries, and how to ratify
/// or amend. Only the scaffold differs between guided and model drafts — the
/// law itself always comes from the same renderer.
pub(crate) fn constitution_ratification_text(
    locale: Locale,
    constitution: &UserConstitution,
    provenance: &DraftProvenance,
) -> String {
    const RULE: &str = "──────────────────────────────────────────────────────";
    let rendered = constitution
        .render_block(None)
        .unwrap_or_else(|| match locale {
            Locale::ZhHans => "结构化宪法为空。".to_string(),
            _ => "The structured constitution is empty.".to_string(),
        });
    let layer_order = tr(locale, MessageId::SetupCheckpointLayerOrder);

    match locale {
        Locale::ZhHans => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "由 {label} 根据你的引导式答案起草，并已由 CodeWhale 完成结构校验与边界限制。"
                ),
                DraftProvenance::Guided => "由你的引导式答案确定性生成。".to_string(),
                DraftProvenance::Existing => {
                    "你现有的宪法，读取自 constitution.json——原样展示，未做任何修改。".to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "这已是你现行的准则。关闭此预览后按 K 保留并完成检查点——文件不会被修改。\
                     之后可随时用 /constitution 或 /setup 修订。"
                }
                _ => {
                    "未经你确认，任何内容都不会成为准则。关闭此预览后按 G 批准并保存；\
                     之后可随时用 /constitution 或 /setup 修订。"
                }
            };
            format!(
                "CODEWHALE · 用户宪法\n{RULE}\n\n{drafted_by}\n\n\
                 这是 CodeWhale 与你协作的长期准则。像优秀的宪法一样：足够简短因而可用，由持久原则而非详尽规则构成，并且可以随你修订。\
                 它界定权力与边界，而非裁决每个具体决定；它让协作跨会话延续——但它不是记忆，它承载的是原则，而非历史。\n\n\
                 {rendered}\n\n\
                 权限层级\n{layer_order}\n你的直接指令始终高于本文件。\n\n\
                 它不能做什么\n\
                 它只提供行为指导，不能授予或更改审批策略、沙箱、Shell、网络、信任、MCP 权限、默认模式、发布或支出权限——这些始终由你在运行时掌控。\n\n\
                 批准\n{ratify_how}"
            )
        }
        _ => {
            let drafted_by = match provenance {
                DraftProvenance::Model(label) => format!(
                    "Drafted by {label} from your guided answers, then schema-checked and bounded by CodeWhale."
                ),
                DraftProvenance::Guided => {
                    "Rendered deterministically from your guided answers.".to_string()
                }
                DraftProvenance::Existing => {
                    "Your existing constitution, loaded from constitution.json — shown unchanged."
                        .to_string()
                }
            };
            let ratify_how = match provenance {
                DraftProvenance::Existing => {
                    "This is already your standing law. Close this preview, then press K to \
                     keep it and complete the checkpoint — the file is not modified. Amend \
                     anytime with /constitution or /setup."
                }
                _ => {
                    "Nothing becomes law until you confirm. Close this preview, then press G to \
                     ratify and save. Amend anytime with /constitution or /setup."
                }
            };
            format!(
                "CODEWHALE · USER CONSTITUTION\n{RULE}\n\n{drafted_by}\n\n\
                 This is the standing law for how CodeWhale works with you. Like the best \
                 constitutions, it is short enough to use, made of durable principles rather \
                 than exhaustive rules, and amendable as you change. It frames powers and \
                 limits rather than deciding every case, and it gives your collaboration \
                 continuity across sessions — but it is not memory: it carries principles, \
                 not history.\n\n\
                 {rendered}\n\n\
                 HIERARCHY OF AUTHORITY\n{layer_order}\nYour direct requests always outrank this document.\n\n\
                 WHAT THIS CANNOT DO\n\
                 It guides behavior. It cannot grant or change approval policy, sandbox, shell, \
                 network, trust, MCP permissions, default mode, publishing, or spending \
                 authority — those stay under your hand at runtime.\n\n\
                 RATIFICATION\n{ratify_how}"
            )
        }
    }
}

/// Card line inviting the user to let their configured model draft the law.
pub(crate) fn model_draft_invitation_line(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::ZhHans => {
            format!("A {model_label} 起草，你批准。未经确认不会保存。")
        }
        _ => format!("A {model_label} can draft it. You ratify it. Nothing saves without you."),
    }
}

/// Card line offering to keep an existing valid constitution unchanged.
pub(crate) fn keep_existing_invitation_line(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "K 保留现有宪法——先查看，再保留，文件不变。",
        _ => "K Keep your existing constitution — review it, keep it, file unchanged.",
    }
}

/// Card line shown while a model draft awaits ratification.
pub(crate) fn model_draft_ready_line(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::ZhHans => {
            format!("{model_label} 的草案待批准——按 G 查看并批准；按 1-6 会丢弃草案。")
        }
        _ => format!(
            "Draft by {model_label} awaits ratification — G to review and ratify; 1-6 discards it."
        ),
    }
}

/// Host-facing status line after a successful model draft.
pub(crate) fn model_draft_ready_message(locale: Locale, model_label: &str) -> String {
    match locale {
        Locale::ZhHans => format!("{model_label} 已起草你的宪法。请查看预览，然后按 G 批准。"),
        _ => format!(
            "{model_label} drafted your constitution. Review the preview, then press G to ratify."
        ),
    }
}

/// Host-facing status line when model drafting fails or is unavailable. The
/// guided deterministic draft always remains the standing fallback.
pub(crate) fn model_draft_failed_message(
    locale: Locale,
    model_label: &str,
    reason: &str,
) -> String {
    match locale {
        Locale::ZhHans => {
            format!("{model_label} 未能完成起草（{reason}）。引导式草案仍然有效——按 G 预览并批准。")
        }
        _ => format!(
            "{model_label} could not draft your constitution ({reason}). Your guided draft still \
             stands — press G to preview and ratify."
        ),
    }
}

pub(crate) fn constitution_choice_label(choice: ConstitutionChoice) -> &'static str {
    match choice {
        ConstitutionChoice::Unset => "unset",
        ConstitutionChoice::Bundled => "bundled/default",
        ConstitutionChoice::GuidedCustom => "guided custom",
        ConstitutionChoice::ExpertOverride => "expert override",
        ConstitutionChoice::Deferred => "deferred",
    }
}

pub(crate) fn constitution_source_label(source: ConstitutionSource) -> &'static str {
    match source {
        ConstitutionSource::Bundled => "bundled",
        ConstitutionSource::UserGlobal => "user-global constitution.json",
        ConstitutionSource::ExpertOverride => "expert full Markdown override",
    }
}

pub(crate) fn constitution_validity_label(validity: ConstitutionValidity) -> &'static str {
    match validity {
        ConstitutionValidity::Unknown => "unknown",
        ConstitutionValidity::Valid => "valid",
        ConstitutionValidity::Invalid => "invalid",
        ConstitutionValidity::Empty => "empty",
        ConstitutionValidity::Unreadable => "unreadable",
    }
}

pub fn persist_user_constitution_choice(
    constitution: &UserConstitution,
    state: &SetupState,
) -> anyhow::Result<()> {
    let constitution_path = UserConstitution::path()?;
    let setup_state_path = SetupState::path()?;
    let mut transaction = codewhale_config::persistence::SetupTransaction::new();
    transaction.stage_json(constitution_path, &constitution.bounded())?;
    transaction.stage_json(setup_state_path, state)?;
    transaction.commit()
}
