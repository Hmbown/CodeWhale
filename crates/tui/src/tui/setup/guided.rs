//! Guided constitution drafting types for the setup wizard.
//!
//! The user tunes six answers (purpose, autonomy, evidence, communication,
//! privacy, principles) and the wizard renders or model-drafts a constitution
//! from them. These types own the labels, cycling, and rendering.

use codewhale_config::{AutonomyPreference, UserConstitution};

use crate::localization::Locale;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GuidedConstitutionDraft {
    pub(crate) purpose: GuidedPurpose,
    pub(crate) autonomy: AutonomyPreference,
    pub(crate) evidence: GuidedEvidence,
    pub(crate) communication: GuidedCommunication,
    pub(crate) privacy: GuidedPrivacy,
    pub(crate) principles: GuidedPrinciples,
}

impl Default for GuidedConstitutionDraft {
    fn default() -> Self {
        Self {
            purpose: GuidedPurpose::Coding,
            autonomy: AutonomyPreference::Balanced,
            evidence: GuidedEvidence::TestsAndReceipts,
            communication: GuidedCommunication::Concise,
            privacy: GuidedPrivacy::StandardCare,
            principles: GuidedPrinciples::ScopedChanges,
        }
    }
}

impl GuidedConstitutionDraft {
    pub(crate) fn cycle(&mut self, key: char) -> bool {
        match key {
            '1' => self.purpose = self.purpose.next(),
            '2' => self.autonomy = next_guided_autonomy(self.autonomy),
            '3' => self.evidence = self.evidence.next(),
            '4' => self.communication = self.communication.next(),
            '5' => self.privacy = self.privacy.next(),
            '6' => self.principles = self.principles.next(),
            _ => return false,
        }
        true
    }

    pub(crate) fn to_constitution(self, locale: Locale) -> UserConstitution {
        UserConstitution {
            language: Some(locale.tag().to_string()),
            about: Some(self.purpose.about(locale).to_string()),
            working_style: vec![
                self.purpose.working_style(locale).to_string(),
                self.communication.working_style(locale).to_string(),
                self.evidence.working_style(locale).to_string(),
                self.privacy.working_style(locale).to_string(),
            ],
            priorities: vec![
                authority_priority(locale).to_string(),
                autonomy_priority(self.autonomy, locale).to_string(),
                self.privacy.escalation_rule(locale).to_string(),
            ],
            autonomy_preference: self.autonomy,
            notes: Some(self.notes(locale)),
            ..UserConstitution::default()
        }
    }

    fn notes(self, locale: Locale) -> String {
        match locale {
            Locale::ZhHans => format!(
                "引导式答案：用途={}；主动性={}；证据={}；沟通={}；隐私={}；原则={}。{} 自由文本原则只作为建议，不会改变审批、沙箱、Shell、网络、信任或 MCP 权限。",
                self.purpose.label(locale),
                autonomy_label(self.autonomy, locale),
                self.evidence.label(locale),
                self.communication.label(locale),
                self.privacy.label(locale),
                self.principles.label(locale),
                self.principles.note(locale)
            ),
            _ => format!(
                "Guided answers: purpose={}; initiative={}; evidence={}; communication={}; privacy={}; principles={}. {} Freeform principles are advisory and do not change approval, sandbox, shell, network, trust, or MCP permissions.",
                self.purpose.label(locale),
                autonomy_label(self.autonomy, locale),
                self.evidence.label(locale),
                self.communication.label(locale),
                self.privacy.label(locale),
                self.principles.label(locale),
                self.principles.note(locale)
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Guided enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuidedPurpose {
    Coding,
    Research,
    Operations,
    Mixed,
}

impl GuidedPurpose {
    fn next(self) -> Self {
        match self {
            Self::Coding => Self::Research,
            Self::Research => Self::Operations,
            Self::Operations => Self::Mixed,
            Self::Mixed => Self::Coding,
        }
    }

    pub(crate) fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Coding) => "编码工作台",
            (Locale::ZhHans, Self::Research) => "研究综合",
            (Locale::ZhHans, Self::Operations) => "运维协作",
            (Locale::ZhHans, Self::Mixed) => "混合工作台",
            (_, Self::Coding) => "coding workbench",
            (_, Self::Research) => "research synthesis",
            (_, Self::Operations) => "operations helper",
            (_, Self::Mixed) => "mixed workbench",
        }
    }

    fn about(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Coding) => "希望 CodeWhale 成为稳健、重证据的编码工作台用户。",
            (Locale::ZhHans, Self::Research) => {
                "希望 CodeWhale 帮助梳理实时资料、引用证据并谨慎综合研究的用户。"
            }
            (Locale::ZhHans, Self::Operations) => {
                "希望 CodeWhale 协助可靠执行运维任务、保留回滚点并明确风险的用户。"
            }
            (Locale::ZhHans, Self::Mixed) => {
                "希望 CodeWhale 在编码、研究、写作和运维之间灵活切换的用户。"
            }
            (_, Self::Coding) => {
                "A CodeWhale user who wants a calm, evidence-first coding workbench."
            }
            (_, Self::Research) => {
                "A CodeWhale user who wants current, cited research and careful synthesis."
            }
            (_, Self::Operations) => {
                "A CodeWhale user who wants reliable operational help with clear rollback points."
            }
            (_, Self::Mixed) => {
                "A CodeWhale user who wants a flexible workbench for coding, research, writing, and operations."
            }
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Coding) => "让代码改动贴近请求、仓库模式和可验证行为。",
            (Locale::ZhHans, Self::Research) => "区分实时证据与推断，并为易变事实引用来源。",
            (Locale::ZhHans, Self::Operations) => {
                "优先使用可逆运维步骤、预演、状态检查和回滚说明。"
            }
            (Locale::ZhHans, Self::Mixed) => {
                "可在编码、研究、写作和运维之间切换，但安全姿态不随意扩大。"
            }
            (_, Self::Coding) => {
                "Keep code changes scoped to requested behavior and existing repo patterns."
            }
            (_, Self::Research) => {
                "Separate live evidence from inference and cite sources for unstable facts."
            }
            (_, Self::Operations) => {
                "Prefer reversible operational steps with dry-runs, status checks, and rollback notes."
            }
            (_, Self::Mixed) => {
                "Adapt between coding, research, writing, and operations without widening the safety posture."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuidedEvidence {
    Assumptions,
    TestsAndReceipts,
    ReleaseReceipts,
}

impl GuidedEvidence {
    fn next(self) -> Self {
        match self {
            Self::Assumptions => Self::TestsAndReceipts,
            Self::TestsAndReceipts => Self::ReleaseReceipts,
            Self::ReleaseReceipts => Self::Assumptions,
        }
    }

    pub(crate) fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Assumptions) => "说明假设",
            (Locale::ZhHans, Self::TestsAndReceipts) => "测试/凭据",
            (Locale::ZhHans, Self::ReleaseReceipts) => "发布凭据",
            (_, Self::Assumptions) => "assumptions",
            (_, Self::TestsAndReceipts) => "tests/receipts",
            (_, Self::ReleaseReceipts) => "release receipts",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Assumptions) => "在宣称完成前总结假设、未知和剩余风险。",
            (Locale::ZhHans, Self::TestsAndReceipts) => {
                "在能降低不确定性时，用命令、测试、截图或引用给出具体验证。"
            }
            (Locale::ZhHans, Self::ReleaseReceipts) => {
                "对重要结论和发布证据标注文件、命令、截图、CI 或来源。"
            }
            (_, Self::Assumptions) => {
                "Summarize assumptions, unknowns, and remaining risk before claiming completion."
            }
            (_, Self::TestsAndReceipts) => {
                "Use commands, tests, screenshots, or citations when they materially reduce uncertainty."
            }
            (_, Self::ReleaseReceipts) => {
                "Cite file paths, commands, screenshots, CI, or sources for material claims and release evidence."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuidedCommunication {
    Concise,
    Teaching,
    Direct,
}

impl GuidedCommunication {
    fn next(self) -> Self {
        match self {
            Self::Concise => Self::Teaching,
            Self::Teaching => Self::Direct,
            Self::Direct => Self::Concise,
        }
    }

    pub(crate) fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Concise) => "简洁",
            (Locale::ZhHans, Self::Teaching) => "教学式",
            (Locale::ZhHans, Self::Direct) => "直接",
            (_, Self::Concise) => "concise",
            (_, Self::Teaching) => "teaching",
            (_, Self::Direct) => "direct",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::Concise) => "保持更新简洁，并只解释重要取舍。",
            (Locale::ZhHans, Self::Teaching) => "解释关键推理和取舍，让用户能理解系统。",
            (Locale::ZhHans, Self::Direct) => "直接说明阻塞、风险和不确定性，避免装饰性文案。",
            (_, Self::Concise) => "Keep updates concise and explain important tradeoffs briefly.",
            (_, Self::Teaching) => {
                "Explain key reasoning and tradeoffs enough that the user can learn the system."
            }
            (_, Self::Direct) => {
                "Be direct about blockers, risk, and uncertainty; avoid ornamental copy."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuidedPrivacy {
    StandardCare,
    StrictBoundaries,
    ProjectLocal,
}

impl GuidedPrivacy {
    fn next(self) -> Self {
        match self {
            Self::StandardCare => Self::StrictBoundaries,
            Self::StrictBoundaries => Self::ProjectLocal,
            Self::ProjectLocal => Self::StandardCare,
        }
    }

    pub(crate) fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::StandardCare) => "标准保护",
            (Locale::ZhHans, Self::StrictBoundaries) => "严格边界",
            (Locale::ZhHans, Self::ProjectLocal) => "项目内记忆",
            (_, Self::StandardCare) => "standard care",
            (_, Self::StrictBoundaries) => "strict boundaries",
            (_, Self::ProjectLocal) => "project-local memory",
        }
    }

    fn working_style(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::StandardCare) => {
                "保护密钥、用户文件、Git 历史、生产系统、成本、隐私和时间。"
            }
            (Locale::ZhHans, Self::StrictBoundaries) => {
                "把密钥、个人数据、凭据、生产状态、资金和发布动作视为先确认边界。"
            }
            (Locale::ZhHans, Self::ProjectLocal) => {
                "项目特定上下文留在项目内，除非明确要求，否则不要写入记忆。"
            }
            (_, Self::StandardCare) => {
                "Protect secrets, user files, git history, production systems, cost, privacy, and time."
            }
            (_, Self::StrictBoundaries) => {
                "Treat secrets, personal data, credentials, production state, money, and publish actions as stop-and-confirm boundaries."
            }
            (_, Self::ProjectLocal) => {
                "Keep project-specific context local; avoid carrying sensitive details into memory unless explicitly asked."
            }
        }
    }

    fn escalation_rule(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::StandardCare) => {
                "遇到破坏性、高成本、凭据、发布、法律或安全风险操作时先询问。"
            }
            (Locale::ZhHans, Self::StrictBoundaries) => {
                "在读取或传播敏感信息、触碰生产系统、花费资金或发布内容前停止并询问。"
            }
            (Locale::ZhHans, Self::ProjectLocal) => {
                "需要跨项目记忆、复制项目细节或引用旧交接时，先确认这些上下文仍适用。"
            }
            (_, Self::StandardCare) => {
                "Ask before destructive, high-cost, credential, publishing, legal, or security-risk actions."
            }
            (_, Self::StrictBoundaries) => {
                "Stop and ask before reading or spreading sensitive data, touching production systems, spending money, or publishing."
            }
            (_, Self::ProjectLocal) => {
                "Confirm before carrying project details across memory, workspaces, or stale handoffs."
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GuidedPrinciples {
    ScopedChanges,
    UserVoice,
    ReversibleOps,
}

impl GuidedPrinciples {
    fn next(self) -> Self {
        match self {
            Self::ScopedChanges => Self::UserVoice,
            Self::UserVoice => Self::ReversibleOps,
            Self::ReversibleOps => Self::ScopedChanges,
        }
    }

    pub(crate) fn label(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::ScopedChanges) => "小范围改动",
            (Locale::ZhHans, Self::UserVoice) => "保留用户语气",
            (Locale::ZhHans, Self::ReversibleOps) => "可逆步骤",
            (_, Self::ScopedChanges) => "scoped changes",
            (_, Self::UserVoice) => "user voice",
            (_, Self::ReversibleOps) => "reversible steps",
        }
    }

    pub(crate) fn note(self, locale: Locale) -> &'static str {
        match (locale, self) {
            (Locale::ZhHans, Self::ScopedChanges) => {
                "自由原则：优先采用小范围、可审查的改动；除非明确要求，不做无关重构。"
            }
            (Locale::ZhHans, Self::UserVoice) => {
                "自由原则：保留用户的语气、品牌和约束；不把偏好推断成权限扩大。"
            }
            (Locale::ZhHans, Self::ReversibleOps) => {
                "自由原则：先选择可逆步骤、检查点和回滚说明，再进行高影响操作。"
            }
            (_, Self::ScopedChanges) => {
                "Freeform principle: prefer small, reviewable changes and avoid unrelated refactors unless explicitly requested."
            }
            (_, Self::UserVoice) => {
                "Freeform principle: preserve the user's voice, brand, and constraints without treating preferences as permission expansion."
            }
            (_, Self::ReversibleOps) => {
                "Freeform principle: favor reversible steps, checkpoints, and rollback notes before high-impact operations."
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Autonomy / authority helpers
// ---------------------------------------------------------------------------

pub(crate) fn next_guided_autonomy(preference: AutonomyPreference) -> AutonomyPreference {
    match preference {
        AutonomyPreference::Unspecified | AutonomyPreference::Cautious => {
            AutonomyPreference::Balanced
        }
        AutonomyPreference::Balanced => AutonomyPreference::Autonomous,
        AutonomyPreference::Autonomous => AutonomyPreference::Cautious,
    }
}

pub(crate) fn autonomy_label(preference: AutonomyPreference, locale: Locale) -> &'static str {
    match (locale, preference) {
        (Locale::ZhHans, AutonomyPreference::Cautious) => "谨慎",
        (Locale::ZhHans, AutonomyPreference::Balanced) => "平衡",
        (Locale::ZhHans, AutonomyPreference::Autonomous) => "积极主动",
        (_, AutonomyPreference::Cautious) => "cautious",
        (_, AutonomyPreference::Balanced) => "balanced",
        (_, AutonomyPreference::Autonomous) => "ambitious",
        (_, AutonomyPreference::Unspecified) => "unspecified",
    }
}

fn autonomy_priority(preference: AutonomyPreference, locale: Locale) -> &'static str {
    match (locale, preference) {
        (Locale::ZhHans, AutonomyPreference::Cautious) => {
            "在编辑文件、运行命令或产品选择不明确前，倾向先停下询问。"
        }
        (Locale::ZhHans, AutonomyPreference::Balanced) => {
            "清晰低风险任务可直接行动；遇到风险、破坏性或歧义时先确认。"
        }
        (Locale::ZhHans, AutonomyPreference::Autonomous) => {
            "可批量处理安全的常规工作，但遇到破坏性、凭据、发布、高成本、法律或安全风险时停止询问。"
        }
        (_, AutonomyPreference::Cautious) => {
            "Stop and ask before editing files, running commands, or choosing between ambiguous product paths."
        }
        (_, AutonomyPreference::Balanced) => {
            "Act directly on clear low-risk tasks; confirm before risky, destructive, or ambiguous actions."
        }
        (_, AutonomyPreference::Autonomous) => {
            "Batch routine safe work, then stop for destructive, credential, publishing, high-cost, legal, or security-risk actions."
        }
        (_, AutonomyPreference::Unspecified) => "No standing initiative preference was selected.",
    }
}

fn authority_priority(locale: Locale) -> &'static str {
    match locale {
        Locale::ZhHans => "当前用户请求和实时工具证据优先于记忆、陈旧交接和猜测。",
        _ => {
            "Current user requests and live tool evidence outrank memory, stale handoffs, and guesses."
        }
    }
}

#[cfg(test)]
#[must_use]
pub(crate) fn guided_constitution_template(locale: Locale) -> UserConstitution {
    GuidedConstitutionDraft::default().to_constitution(locale)
}
