//! Unified read-only inventory for Codewhale extensions.
//!
//! This is deliberately a projection over the existing owners of Hooks,
//! Plugins, Marketplace catalogs, Skills, and MCP. It has no registry, trust
//! database, installer, or network fetch of its own. Future actions emitted by
//! this view must delegate to the existing command/mutation controllers.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt::Write as _;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::{
    CommandPaletteAction, ModalKind, ModalView, ViewAction, ViewEvent, render_modal_footer,
    render_underwater_surface, truncate_view_text,
};
use crate::localization::{Locale, MessageId, tr};
use crate::palette;
use crate::tui::app::App;

fn localize(locale: Locale, id: MessageId, replacements: &[(&str, &str)]) -> String {
    let mut value = tr(locale, id).into_owned();
    for (name, replacement) in replacements {
        value = value.replace(&format!("{{{name}}}"), replacement);
    }
    value
}

/// All extension surfaces in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionsTab {
    Hooks,
    Plugins,
    Marketplace,
    Skills,
    Mcp,
}

impl ExtensionsTab {
    pub const ALL: [Self; 5] = [
        Self::Hooks,
        Self::Plugins,
        Self::Marketplace,
        Self::Skills,
        Self::Mcp,
    ];

    #[must_use]
    fn label(self, locale: Locale) -> String {
        match self {
            Self::Hooks => tr(locale, MessageId::ExtensionsTabHooks),
            Self::Plugins => tr(locale, MessageId::ExtensionsTabPlugins),
            Self::Marketplace => tr(locale, MessageId::ExtensionsTabMarketplace),
            Self::Skills => tr(locale, MessageId::HelpSkills),
            Self::Mcp => tr(locale, MessageId::ConfigSectionMcp),
        }
        .into_owned()
    }

    const fn index(self) -> usize {
        match self {
            Self::Hooks => 0,
            Self::Plugins => 1,
            Self::Marketplace => 2,
            Self::Skills => 3,
            Self::Mcp => 4,
        }
    }

    const fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    const fn previous(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// A real capability contributed by one plugin product.
///
/// Recommendations use the same component vocabulary as installed plugin
/// bundles. An MCP, Skill, browser driver, or sandbox helper is therefore a
/// component of a product, not a parallel kind of install pretending to be a
/// complete plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginProductComponentKind {
    Mcp,
    Skills,
    BrowserDriver,
    SandboxRuntime,
    NativeRuntime,
}

impl PluginProductComponentKind {
    fn label(self, locale: Locale) -> String {
        match self {
            Self::Mcp => tr(locale, MessageId::ConfigSectionMcp),
            Self::Skills => tr(locale, MessageId::HelpSkills),
            Self::BrowserDriver => tr(locale, MessageId::ExtensionsComponentBrowserDriver),
            Self::SandboxRuntime => tr(locale, MessageId::ExtensionsComponentSandboxRuntime),
            Self::NativeRuntime => tr(locale, MessageId::ExtensionsComponentNativeRuntime),
        }
        .into_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginProductComponent {
    pub kind: PluginProductComponentKind,
    pub name: String,
}

/// Marketplace-facing recommendation model.
///
/// `source_reference` is display provenance only. It is intentionally not an
/// install command or executable plan; explicit installation still enters the
/// reviewed plugin installer and trust flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginProduct {
    pub id: String,
    pub name: String,
    pub description: String,
    pub publisher: String,
    pub source_reference: String,
    pub components: Vec<PluginProductComponent>,
    pub maturity: String,
}

impl PluginProduct {
    fn into_row(self, locale: Locale) -> ExtensionItem {
        let mut components = String::new();
        for (index, component) in self.components.iter().enumerate() {
            if index > 0 {
                components.push_str(", ");
            }
            let _ = write!(
                components,
                "{} ({})",
                component.name,
                component.kind.label(locale)
            );
        }
        ExtensionItem {
            id: self.id,
            label: self.name,
            description: self.description,
            state: self.maturity,
            detail: localize(
                locale,
                MessageId::ExtensionsProductDetail,
                &[
                    ("publisher", &self.publisher),
                    ("components", &components),
                    ("source", &self.source_reference),
                ],
            ),
            action: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionItem {
    pub id: String,
    pub label: String,
    pub description: String,
    pub state: String,
    pub detail: String,
    pub action: Option<ExtensionAction>,
}

/// A row affordance. Executable actions route back through the existing slash
/// command controller; status-only actions explain why Enter will not mutate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionAction {
    Command { label: String, command: String },
    Status { label: String },
}

impl ExtensionAction {
    fn label(&self) -> &str {
        match self {
            Self::Command { label, .. } | Self::Status { label } => label,
        }
    }

    fn command(&self) -> Option<&str> {
        match self {
            Self::Command { command, .. } => Some(command),
            Self::Status { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionGroup {
    pub id: String,
    pub label: String,
    pub items: Vec<ExtensionItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionsTabModel {
    pub groups: Vec<ExtensionGroup>,
    pub problem: Option<String>,
}

/// Read model captured when the modal opens. No source is contacted over the
/// network and no extension process is started while building it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtensionsSnapshot {
    tabs: [ExtensionsTabModel; 5],
}

impl ExtensionsSnapshot {
    #[must_use]
    pub fn from_app(app: &App) -> Self {
        let mut snapshot = Self::default();
        snapshot.tabs[ExtensionsTab::Hooks.index()] = hooks_model(app, app.ui_locale);
        snapshot.tabs[ExtensionsTab::Plugins.index()] = plugins_model(app, app.ui_locale);
        snapshot.tabs[ExtensionsTab::Marketplace.index()] = marketplace_model(app, app.ui_locale);
        snapshot.tabs[ExtensionsTab::Skills.index()] = skills_model(app, app.ui_locale);
        snapshot.tabs[ExtensionsTab::Mcp.index()] = mcp_model(app, app.ui_locale);
        snapshot
            .with_recommendations(reviewed_product_catalog(app.ui_locale), app.ui_locale)
            .with_recommended_actions(app)
    }

    #[must_use]
    pub fn with_recommendations(mut self, products: Vec<PluginProduct>, locale: Locale) -> Self {
        if !products.is_empty() {
            self.tabs[ExtensionsTab::Marketplace.index()].groups.insert(
                0,
                ExtensionGroup {
                    id: "recommended".into(),
                    label: tr(locale, MessageId::ExtensionsGroupRecommended).into_owned(),
                    items: products
                        .into_iter()
                        .map(|product| product.into_row(locale))
                        .collect(),
                },
            );
        }
        self
    }

    fn with_recommended_actions(mut self, app: &App) -> Self {
        let configured = crate::mcp::load_config_with_workspace_and_plugins(
            &app.mcp_config_path,
            &app.workspace,
            app.plugin_registry.as_ref(),
        )
        .ok();
        let Some(group) = self.tabs[ExtensionsTab::Marketplace.index()]
            .groups
            .iter_mut()
            .find(|group| group.id == "recommended")
        else {
            return self;
        };

        if configured.is_none() {
            for item in &mut group.items {
                item.action = Some(ExtensionAction::Status {
                    label: tr(app.ui_locale, MessageId::PickerActionUnavailable).into_owned(),
                });
            }
            return self;
        }

        for item in &mut group.items {
            let recommendation = match item.id.as_str() {
                "playwright-browser" => Some(("playwright", "playwright")),
                "chrome-devtools" => Some(("chrome-devtools", "chrome-devtools")),
                "cua-computer-use" => Some(("cua-driver", "cua")),
                _ => None,
            };
            if let Some((server_name, recommendation_id)) = recommendation {
                match configured
                    .as_ref()
                    .and_then(|config| config.servers.get(server_name))
                {
                    None => {
                        item.state =
                            tr(app.ui_locale, MessageId::ExtensionsStateAvailable).into_owned();
                        item.action = Some(ExtensionAction::Command {
                            label: tr(app.ui_locale, MessageId::ExtensionsActionAdd).into_owned(),
                            command: format!("/mcp add recommended {recommendation_id}"),
                        });
                    }
                    Some(server) if !server.is_enabled() => {
                        item.state =
                            tr(app.ui_locale, MessageId::HotbarSetupStatusDisabled).into_owned();
                        item.action = Some(ExtensionAction::Command {
                            label: tr(app.ui_locale, MessageId::ExtensionsActionEnable)
                                .into_owned(),
                            command: format!("/mcp enable {server_name}"),
                        });
                    }
                    Some(_) => {
                        item.state =
                            tr(app.ui_locale, MessageId::PickerActionConfigured).into_owned();
                        item.action = Some(ExtensionAction::Status {
                            label: tr(app.ui_locale, MessageId::PickerActionConfigured)
                                .into_owned(),
                        });
                    }
                }
            } else {
                item.action = Some(ExtensionAction::Status {
                    label: tr(app.ui_locale, MessageId::PickerActionUnavailable).into_owned(),
                });
            }
        }
        self
    }

    fn tab(&self, tab: ExtensionsTab) -> &ExtensionsTabModel {
        &self.tabs[tab.index()]
    }
}

/// Pinned review metadata only. These rows do not contain install commands,
/// do not fetch anything, and do not grant trust. The source-specific plugin
/// manifests produced by the packaging lane remain the installation authority.
fn reviewed_product_catalog(locale: Locale) -> Vec<PluginProduct> {
    vec![
        PluginProduct {
            id: "playwright-browser".into(),
            name: "Playwright Browser".into(),
            description: tr(locale, MessageId::ExtensionsProductPlaywrightDescription).into_owned(),
            publisher: "Microsoft".into(),
            source_reference: "microsoft/playwright-mcp".into(),
            components: vec![
                PluginProductComponent {
                    kind: PluginProductComponentKind::Mcp,
                    name: "Playwright MCP".into(),
                },
                PluginProductComponent {
                    kind: PluginProductComponentKind::BrowserDriver,
                    name: "Playwright browser driver".into(),
                },
            ],
            maturity: tr(locale, MessageId::ExtensionsStateReviewedCandidate).into_owned(),
        },
        PluginProduct {
            id: "chrome-devtools".into(),
            name: "Chrome DevTools".into(),
            description: tr(locale, MessageId::ExtensionsProductChromeDescription).into_owned(),
            publisher: "Chrome DevTools".into(),
            source_reference: "ChromeDevTools/chrome-devtools-mcp".into(),
            components: vec![
                PluginProductComponent {
                    kind: PluginProductComponentKind::Mcp,
                    name: "Chrome DevTools MCP".into(),
                },
                PluginProductComponent {
                    kind: PluginProductComponentKind::BrowserDriver,
                    name: "Chrome".into(),
                },
            ],
            maturity: tr(locale, MessageId::ExtensionsStateReviewedCandidate).into_owned(),
        },
        PluginProduct {
            id: "cua-computer-use".into(),
            name: "Cua Computer Use".into(),
            description: tr(locale, MessageId::ExtensionsProductCuaDescription).into_owned(),
            publisher: "Cua".into(),
            source_reference: "trycua/cua".into(),
            components: vec![PluginProductComponent {
                kind: PluginProductComponentKind::NativeRuntime,
                name: "Cua Driver".into(),
            }],
            maturity: tr(locale, MessageId::ExtensionsStateUnderEvaluation).into_owned(),
        },
        PluginProduct {
            id: "browser-use".into(),
            name: "Browser Use".into(),
            description: tr(locale, MessageId::ExtensionsProductBrowserUseDescription).into_owned(),
            publisher: "Browser Use".into(),
            source_reference: "browser-use/browser-use".into(),
            components: vec![
                PluginProductComponent {
                    kind: PluginProductComponentKind::Skills,
                    name: "Browser Use Skill".into(),
                },
                PluginProductComponent {
                    kind: PluginProductComponentKind::BrowserDriver,
                    name: "Browser Use runtime".into(),
                },
            ],
            maturity: tr(locale, MessageId::ExtensionsStateReviewedCandidate).into_owned(),
        },
        PluginProduct {
            id: "anthropic-sandbox-runtime".into(),
            name: "Sandbox Runtime".into(),
            description: tr(locale, MessageId::ExtensionsProductSandboxDescription).into_owned(),
            publisher: "Anthropic Experimental".into(),
            source_reference: "anthropic-experimental/sandbox-runtime".into(),
            components: vec![PluginProductComponent {
                kind: PluginProductComponentKind::SandboxRuntime,
                name: "Sandbox Runtime".into(),
            }],
            maturity: tr(locale, MessageId::ExtensionsStateBetaCandidate).into_owned(),
        },
    ]
}

fn hooks_model(app: &App, locale: Locale) -> ExtensionsTabModel {
    let config = app.hooks.config();
    let configured = config
        .hooks
        .iter()
        .enumerate()
        .map(|(index, hook)| ExtensionItem {
            id: format!("hook-{index}"),
            label: hook.name.clone().unwrap_or_else(|| {
                localize(
                    locale,
                    MessageId::ExtensionsHookFallback,
                    &[("event", hook.event.as_str())],
                )
            }),
            description: hook.event.as_str().to_string(),
            state: if config.enabled {
                tr(locale, MessageId::ExtensionsStateEnabled)
            } else {
                tr(locale, MessageId::HotbarSetupStatusDisabled)
            }
            .into_owned(),
            detail: localize(
                locale,
                MessageId::ExtensionsHookDetail,
                &[
                    ("timeout", &hook.timeout_secs.to_string()),
                    ("background", &localized_bool(locale, hook.background)),
                    (
                        "continue_on_error",
                        &localized_bool(locale, hook.continue_on_error),
                    ),
                ],
            ),
            action: None,
        })
        .collect::<Vec<_>>();
    let problems = config
        .problems
        .iter()
        .enumerate()
        .map(|(index, problem)| ExtensionItem {
            id: format!("hook-problem-{index}"),
            label: problem.name.clone().unwrap_or_else(|| {
                tr(locale, MessageId::ExtensionsHooksConfiguration).into_owned()
            }),
            description: problem.detail.clone(),
            state: if problem.rejected {
                tr(locale, MessageId::ExtensionsStateRejected)
            } else {
                tr(locale, MessageId::ExtensionsStateWarning)
            }
            .into_owned(),
            detail: problem.summary(),
            action: None,
        })
        .collect::<Vec<_>>();
    let mut groups = Vec::new();
    if !configured.is_empty() {
        groups.push(ExtensionGroup {
            id: "configured".into(),
            label: tr(locale, MessageId::ExtensionsGroupConfigured).into_owned(),
            items: configured,
        });
    }
    if !problems.is_empty() {
        groups.push(ExtensionGroup {
            id: "problems".into(),
            label: tr(locale, MessageId::ExtensionsGroupProblems).into_owned(),
            items: problems,
        });
    }
    ExtensionsTabModel {
        groups,
        problem: None,
    }
}

fn inventory_summary(
    inventory: &crate::plugins::manifest::PluginInventory,
    locale: Locale,
) -> String {
    let mut parts = Vec::new();
    if inventory.skills > 0 {
        parts.push(localize(
            locale,
            MessageId::ExtensionsInventorySkills,
            &[("count", &inventory.skills.to_string())],
        ));
    }
    if inventory.mcp_servers > 0 {
        parts.push(localize(
            locale,
            MessageId::ExtensionsInventoryMcp,
            &[("count", &inventory.mcp_servers.to_string())],
        ));
    }
    if inventory.hooks > 0 {
        parts.push(localize(
            locale,
            MessageId::ExtensionsInventoryHooks,
            &[("count", &inventory.hooks.to_string())],
        ));
    }
    if inventory.commands > 0 {
        parts.push(localize(
            locale,
            MessageId::ExtensionsInventoryCommands,
            &[("count", &inventory.commands.to_string())],
        ));
    }
    if inventory.agents > 0 {
        parts.push(localize(
            locale,
            MessageId::ExtensionsInventoryAgents,
            &[("count", &inventory.agents.to_string())],
        ));
    }
    if parts.is_empty() {
        tr(locale, MessageId::ExtensionsInventoryNone).into_owned()
    } else {
        parts.join(", ")
    }
}

fn localized_bool(locale: Locale, value: bool) -> String {
    tr(
        locale,
        if value {
            MessageId::ExtensionsValueYes
        } else {
            MessageId::ExtensionsValueNo
        },
    )
    .into_owned()
}

fn localized_plugin_state(locale: Locale, state: &str) -> String {
    let id = match state {
        "active" => MessageId::CtxInspActive,
        "disabled" => MessageId::HotbarSetupStatusDisabled,
        "enabled-untrusted" => MessageId::ExtensionsStateEnabledUntrusted,
        "unstaged" => MessageId::ExtensionsStateUnstaged,
        "inapplicable" => MessageId::ExtensionsStateInapplicable,
        "unsupported" => MessageId::ExtensionsStateUnsupported,
        "inactive" => MessageId::ExtensionsStateInactive,
        _ => return state.to_string(),
    };
    tr(locale, id).into_owned()
}

fn localized_trust(locale: Locale, trust: &str) -> String {
    let id = match trust {
        "trusted" => MessageId::ExtensionsTrustTrusted,
        "not-reviewed" => MessageId::ExtensionsTrustNotReviewed,
        "content-changed" => MessageId::ExtensionsTrustContentChanged,
        "capabilities-changed" => MessageId::ExtensionsTrustCapabilitiesChanged,
        _ => return trust.to_string(),
    };
    tr(locale, id).into_owned()
}

fn localized_compatibility(locale: Locale, compatibility: &str) -> String {
    let id = match compatibility {
        "full" => MessageId::ExtensionsCompatibilityFull,
        "partial" => MessageId::ExtensionsCompatibilityPartial,
        "unsupported" => MessageId::ExtensionsStateUnsupported,
        _ => return compatibility.to_string(),
    };
    tr(locale, id).into_owned()
}

fn localized_tier(locale: Locale, tier: &str) -> String {
    let id = match tier {
        "community" => MessageId::ExtensionsTierCommunity,
        "official" => MessageId::ExtensionsTierOfficial,
        "curated" => MessageId::ExtensionsTierCurated,
        "partner" => MessageId::ExtensionsTierPartner,
        _ => return tier.to_string(),
    };
    tr(locale, id).into_owned()
}

fn localized_skill_root(locale: Locale, kind: crate::skills::roots::SkillRootKind) -> String {
    use crate::skills::roots::SkillRootKind;

    match kind {
        SkillRootKind::CodeWhaleProject => {
            tr(locale, MessageId::ExtensionsSkillRootProject).into_owned()
        }
        SkillRootKind::CodeWhaleGlobal => {
            tr(locale, MessageId::ExtensionsSkillRootGlobal).into_owned()
        }
        SkillRootKind::CompatibleProject(harness) => localize(
            locale,
            MessageId::ExtensionsSkillRootCompatibleProject,
            &[("harness", harness.label())],
        ),
        SkillRootKind::CompatibleGlobal(harness) => localize(
            locale,
            MessageId::ExtensionsSkillRootCompatibleGlobal,
            &[("harness", harness.label())],
        ),
        SkillRootKind::Configured => {
            tr(locale, MessageId::ExtensionsSkillRootConfigured).into_owned()
        }
        SkillRootKind::BuiltIn => tr(locale, MessageId::ExtensionsGroupBuiltIn).into_owned(),
        SkillRootKind::ReviewedPluginSnapshot => {
            tr(locale, MessageId::ExtensionsSkillRootReviewedPlugin).into_owned()
        }
        SkillRootKind::RegistryCache => {
            tr(locale, MessageId::ExtensionsSkillRootRegistryCache).into_owned()
        }
    }
}

fn plugins_model(app: &App, locale: Locale) -> ExtensionsTabModel {
    let mut by_scope = [Vec::new(), Vec::new(), Vec::new()];
    for plugin in app.plugin_registry.list() {
        let scope = match plugin.scope {
            crate::plugins::types::PluginScope::Builtin => 0,
            crate::plugins::types::PluginScope::User => 1,
            crate::plugins::types::PluginScope::Workspace => 2,
        };
        let diagnostic_count = plugin.diagnostics.len();
        let has_error_diagnostics = plugin.diagnostics.iter().any(|diagnostic| {
            diagnostic.level == crate::plugins::types::PluginDiagnosticLevel::Error
        });
        let action = if has_error_diagnostics {
            ExtensionAction::Command {
                label: tr(locale, MessageId::ExtensionsActionDiagnose).into_owned(),
                command: format!("/plugin validate {}", plugin.name()),
            }
        } else if plugin.active() {
            ExtensionAction::Command {
                label: tr(locale, MessageId::LaunchHintOpen).into_owned(),
                command: format!("/plugin show {}", plugin.name()),
            }
        } else if plugin.trusted() && !plugin.enabled {
            ExtensionAction::Command {
                label: tr(locale, MessageId::ExtensionsActionEnable).into_owned(),
                command: format!("/plugin enable {}", plugin.name()),
            }
        } else {
            ExtensionAction::Command {
                label: tr(locale, MessageId::AutomationActionInspect).into_owned(),
                command: format!("/plugin trust {}", plugin.name()),
            }
        };
        by_scope[scope].push(ExtensionItem {
            id: plugin.id.as_str().to_string(),
            label: plugin.name().to_string(),
            description: plugin
                .manifest
                .plugin
                .description
                .clone()
                .unwrap_or_else(|| inventory_summary(&plugin.inventory, locale)),
            state: localized_plugin_state(locale, plugin.state_label()),
            detail: localize(
                locale,
                MessageId::ExtensionsPluginDetail,
                &[
                    ("inventory", &inventory_summary(&plugin.inventory, locale)),
                    (
                        "trust",
                        &localized_trust(locale, plugin.trust_status.as_str()),
                    ),
                    (
                        "compatibility",
                        &localized_compatibility(locale, plugin.compatibility().as_str()),
                    ),
                    ("diagnostics", &diagnostic_count.to_string()),
                ],
            ),
            action: Some(action),
        });
    }
    let labels = [
        (
            "builtin",
            tr(locale, MessageId::ExtensionsGroupBuiltIn).into_owned(),
        ),
        (
            "user",
            tr(locale, MessageId::ExtensionsGroupUser).into_owned(),
        ),
        (
            "workspace",
            tr(locale, MessageId::ExtensionsGroupWorkspace).into_owned(),
        ),
    ];
    let mut groups = labels
        .into_iter()
        .zip(by_scope)
        .filter(|(_, items)| !items.is_empty())
        .map(|((id, label), items)| ExtensionGroup {
            id: id.into(),
            label,
            items,
        })
        .collect::<Vec<_>>();
    let problems = app
        .plugin_registry
        .diagnostics()
        .iter()
        .enumerate()
        .map(|(index, diagnostic)| ExtensionItem {
            id: format!("plugin-diagnostic-{index}"),
            label: diagnostic.code.to_string(),
            description: diagnostic.message.clone(),
            state: if diagnostic.level == crate::plugins::types::PluginDiagnosticLevel::Error {
                tr(locale, MessageId::ExtensionsStateInvalid)
            } else {
                tr(locale, MessageId::ExtensionsStateWarning)
            }
            .into_owned(),
            detail: diagnostic
                .path
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| diagnostic.message.clone()),
            action: Some(ExtensionAction::Command {
                label: tr(locale, MessageId::ExtensionsActionDiagnose).into_owned(),
                command: "/plugin validate".into(),
            }),
        })
        .collect::<Vec<_>>();
    if !problems.is_empty() {
        groups.push(ExtensionGroup {
            id: "problems".into(),
            label: tr(locale, MessageId::ExtensionsGroupProblems).into_owned(),
            items: problems,
        });
    }
    ExtensionsTabModel {
        groups,
        problem: app.plugin_registry.state_error().map(ToString::to_string),
    }
}

fn marketplace_model(app: &App, locale: Locale) -> ExtensionsTabModel {
    let Some(store) = crate::plugins::marketplace::store::MarketplaceStore::open(
        app.plugin_registry.state_path(),
    ) else {
        return ExtensionsTabModel {
            groups: Vec::new(),
            problem: Some(tr(locale, MessageId::ExtensionsMarketplaceUnavailable).into_owned()),
        };
    };
    let state = match store.load() {
        Ok(state) => state,
        Err(error) => {
            return ExtensionsTabModel {
                groups: Vec::new(),
                problem: Some(error),
            };
        }
    };
    let groups = state
        .catalogs()
        .values()
        .map(|stored| {
            let catalog = &stored.catalog;
            ExtensionGroup {
                id: catalog.id.as_str().to_string(),
                label: catalog
                    .display_name
                    .clone()
                    .unwrap_or_else(|| catalog.name.clone()),
                items: catalog
                    .candidates
                    .iter()
                    .map(|candidate| ExtensionItem {
                        id: candidate.id.as_str().to_string(),
                        label: candidate
                            .display_name
                            .clone()
                            .unwrap_or_else(|| candidate.name.clone()),
                        description: candidate.description.clone().unwrap_or_default(),
                        state: if candidate.has_errors() {
                            tr(locale, MessageId::ExtensionsStateInvalid)
                        } else if candidate.install_plan.is_supported() {
                            tr(locale, MessageId::ExtensionsStateAvailable)
                        } else {
                            tr(locale, MessageId::AutomationActionInspect)
                        }
                        .into_owned(),
                        detail: {
                            let unknown = tr(locale, MessageId::CmdCostUnknownValue);
                            localize(
                                locale,
                                MessageId::ExtensionsMarketplaceDetail,
                                &[
                                    (
                                        "publisher",
                                        candidate
                                            .provenance
                                            .publisher
                                            .as_deref()
                                            .unwrap_or(unknown.as_ref()),
                                    ),
                                    (
                                        "tier",
                                        &localized_tier(locale, candidate.provenance.tier.as_str()),
                                    ),
                                    (
                                        "installable",
                                        &localized_bool(
                                            locale,
                                            candidate.install_plan.is_supported(),
                                        ),
                                    ),
                                ],
                            )
                        },
                        action: if !candidate.has_errors() && candidate.install_plan.is_supported()
                        {
                            Some(ExtensionAction::Command {
                                label: tr(locale, MessageId::ExtensionsActionAdd).into_owned(),
                                command: format!(
                                    "/plugin marketplace install {} {}",
                                    catalog.id.as_str(),
                                    candidate.name
                                ),
                            })
                        } else {
                            Some(ExtensionAction::Status {
                                label: tr(locale, MessageId::PickerActionUnavailable).into_owned(),
                            })
                        },
                    })
                    .collect(),
            }
        })
        .collect();
    ExtensionsTabModel {
        groups,
        problem: None,
    }
}

fn skills_model(app: &App, locale: Locale) -> ExtensionsTabModel {
    use crate::skills::audit::{ParserState, SkillAuditMode, scan_with_configured};

    let home = crate::config::effective_home_dir();
    let audit = scan_with_configured(
        &app.workspace,
        home.as_deref(),
        Some(&app.skills_dir),
        SkillAuditMode::OwnedOnly,
        None,
    );
    let mut groups = Vec::<ExtensionGroup>::new();
    for skill in audit.skills {
        let group_id = format!("{:?}", skill.root.kind);
        let position = groups.iter().position(|group| group.id == group_id);
        let item = ExtensionItem {
            id: format!("{}:{}", group_id, skill.id.canonical_name),
            label: skill.name,
            description: skill.description.unwrap_or_default(),
            state: match skill.parser {
                ParserState::Valid => tr(locale, MessageId::HotbarSetupStatusReady),
                ParserState::Warning(_) => tr(locale, MessageId::ExtensionsStateWarning),
                ParserState::Broken(_) | ParserState::Oversized => {
                    tr(locale, MessageId::ExtensionsStateInvalid)
                }
            }
            .into_owned(),
            detail: skill.safe_display_path,
            action: None,
        };
        if let Some(position) = position {
            groups[position].items.push(item);
        } else {
            groups.push(ExtensionGroup {
                id: group_id.clone(),
                label: localized_skill_root(locale, skill.root.kind),
                items: vec![item],
            });
        }
    }
    ExtensionsTabModel {
        groups,
        problem: None,
    }
}

fn mcp_model(app: &App, locale: Locale) -> ExtensionsTabModel {
    let configured = crate::mcp::load_config_with_workspace_and_plugins(
        &app.mcp_config_path,
        &app.workspace,
        app.plugin_registry.as_ref(),
    )
    .ok();
    let snapshot = app.mcp_snapshot.as_ref();
    // Configured names are the count authority used by the surrounding shell.
    // Snapshot data enriches those exact rows; it must never independently
    // filter the list down to only the last discovered subset.
    let names = configured.as_ref().map_or_else(
        || {
            snapshot
                .into_iter()
                .flat_map(|snapshot| snapshot.servers.iter().map(|server| server.name.clone()))
                .collect::<BTreeSet<_>>()
        },
        |config| config.servers.keys().cloned().collect::<BTreeSet<_>>(),
    );
    let total = names.len();
    let items: Vec<_> = names
        .into_iter()
        .map(|name| {
            let observed = snapshot
                .and_then(|snapshot| snapshot.servers.iter().find(|server| server.name == name));
            let config = configured
                .as_ref()
                .and_then(|configured| configured.servers.get(&name));
            let enabled = observed
                .map(|server| server.enabled)
                .or_else(|| config.map(crate::mcp::McpServerConfig::is_enabled))
                .unwrap_or(true);
            let initializing = app.mcp_initializing
                && enabled
                && observed.is_none_or(|server| !server.connected && server.error.is_none());
            let state = if !enabled {
                tr(locale, MessageId::HotbarSetupStatusDisabled)
            } else if initializing {
                Cow::Borrowed("connecting")
            } else if observed.is_some_and(|server| server.connected) {
                tr(locale, MessageId::ExtensionsStateConnected)
            } else if observed.is_some_and(|server| server.auth_required) {
                Cow::Owned(crate::tui::session_boot::mcp_auth_required_state_label())
            } else if observed.is_some_and(|server| server.error.is_some()) {
                tr(locale, MessageId::ExtensionsStateError)
            } else if observed.is_none() {
                tr(locale, MessageId::ExtensionsStateNotInspected)
            } else {
                tr(locale, MessageId::PickerActionConfigured)
            }
            .into_owned();
            let oauth_capable = config.is_some_and(crate::mcp::mcp_server_oauth_capable);
            let recovery = match observed {
                Some(server) => server.recovery_kind(oauth_capable),
                None => crate::mcp::mcp_recovery_kind(enabled, false, false, None, oauth_capable),
            };
            let action = if initializing {
                ExtensionAction::Status {
                    label: state.clone(),
                }
            } else if crate::mcp::mcp_name_is_command_safe(&name)
                || matches!(
                    recovery,
                    crate::mcp::McpRecoveryKind::Connect
                        | crate::mcp::McpRecoveryKind::Reconnect
                        | crate::mcp::McpRecoveryKind::Diagnose
                )
            {
                ExtensionAction::Command {
                    label: tr(locale, recovery.label_key()).into_owned(),
                    command: recovery.slash_command(&name),
                }
            } else {
                ExtensionAction::Command {
                    label: tr(locale, MessageId::ExtensionsActionDiagnose).into_owned(),
                    command: "/mcp validate".into(),
                }
            };
            ExtensionItem {
                id: name.clone(),
                label: name,
                description: observed.map_or_else(String::new, |server| {
                    localize(
                        locale,
                        MessageId::ExtensionsMcpSummary,
                        &[
                            ("transport", &server.transport),
                            ("tools", &server.tools.len().to_string()),
                            ("resources", &server.resources.len().to_string()),
                        ],
                    )
                }),
                state,
                // The passive snapshot can carry a command line or URL. Do
                // not mirror either into this broad inventory surface.
                detail: observed.map_or_else(
                    || tr(locale, MessageId::ExtensionsMcpNotInspected).into_owned(),
                    |server| {
                        server.error.clone().unwrap_or_else(|| {
                            localize(
                                locale,
                                MessageId::ExtensionsMcpDetail,
                                &[
                                    ("tools", &server.tools.len().to_string()),
                                    ("resources", &server.resources.len().to_string()),
                                    ("prompts", &server.prompts.len().to_string()),
                                ],
                            )
                        })
                    },
                ),
                action: Some(action),
            }
        })
        .collect();
    ExtensionsTabModel {
        groups: (!items.is_empty())
            .then(|| ExtensionGroup {
                id: "servers".into(),
                label: tr(locale, MessageId::ExtensionsGroupServers).into_owned(),
                items,
            })
            .into_iter()
            .collect(),
        problem: (configured.is_none() && app.mcp_configured_count > total).then(|| {
            localize(
                locale,
                MessageId::ExtensionsMcpRefresh,
                &[("count", &app.mcp_configured_count.to_string())],
            )
        }),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExtensionsFocus {
    Tabs,
    Search,
    List,
}

impl ExtensionsFocus {
    const fn next(self) -> Self {
        match self {
            Self::Tabs => Self::Search,
            Self::Search => Self::List,
            Self::List => Self::Tabs,
        }
    }

    const fn previous(self) -> Self {
        match self {
            Self::Tabs => Self::List,
            Self::Search => Self::Tabs,
            Self::List => Self::Search,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibleEntry<'a> {
    Group(&'a ExtensionGroup),
    Item(&'a ExtensionGroup, &'a ExtensionItem),
    Problem(&'a str),
    Empty,
}

#[derive(Default)]
struct HitAreas {
    tabs: Vec<(Rect, ExtensionsTab)>,
    search: Option<Rect>,
    rows: Vec<(Rect, usize)>,
}

pub struct ExtensionsView {
    snapshot: ExtensionsSnapshot,
    locale: Locale,
    active_tab: ExtensionsTab,
    focus: ExtensionsFocus,
    query: String,
    selected: [usize; 5],
    scroll: [usize; 5],
    folded_groups: BTreeSet<String>,
    hits: RefCell<HitAreas>,
}

impl ExtensionsView {
    #[must_use]
    pub fn new(app: &App, tab: ExtensionsTab) -> Self {
        Self::from_snapshot_with_locale(ExtensionsSnapshot::from_app(app), tab, app.ui_locale)
    }

    fn from_snapshot_with_locale(
        snapshot: ExtensionsSnapshot,
        tab: ExtensionsTab,
        locale: Locale,
    ) -> Self {
        Self {
            snapshot,
            locale,
            active_tab: tab,
            focus: ExtensionsFocus::List,
            query: String::new(),
            selected: [0; 5],
            scroll: [0; 5],
            folded_groups: BTreeSet::new(),
            hits: RefCell::new(HitAreas::default()),
        }
    }

    fn fold_key(&self, group: &ExtensionGroup) -> String {
        format!("{}:{}", self.active_tab.index(), group.id)
    }

    fn group_matches(&self, group: &ExtensionGroup, query: &str) -> bool {
        group.label.to_lowercase().contains(query)
            || group.items.iter().any(|item| item_matches(item, query))
    }

    fn visible_entries(&self) -> Vec<VisibleEntry<'_>> {
        let model = self.snapshot.tab(self.active_tab);
        let query = self.query.trim().to_lowercase();
        let searching = !query.is_empty();
        let mut entries = Vec::new();
        if let Some(problem) = model.problem.as_deref() {
            entries.push(VisibleEntry::Problem(problem));
        }
        for group in &model.groups {
            if searching && !self.group_matches(group, &query) {
                continue;
            }
            entries.push(VisibleEntry::Group(group));
            let folded = !searching && self.folded_groups.contains(&self.fold_key(group));
            if folded {
                continue;
            }
            let group_name_matches = searching && group.label.to_lowercase().contains(&query);
            entries.extend(
                group
                    .items
                    .iter()
                    .filter(|item| !searching || group_name_matches || item_matches(item, &query))
                    .map(|item| VisibleEntry::Item(group, item)),
            );
        }
        if entries.is_empty() {
            entries.push(VisibleEntry::Empty);
        }
        entries
    }

    fn clamp_selection(&mut self) {
        let len = self.visible_entries().len();
        let index = self.active_tab.index();
        self.selected[index] = self.selected[index].min(len.saturating_sub(1));
        self.scroll[index] = self.scroll[index].min(self.selected[index]);
    }

    fn move_selection(&mut self, delta: isize) {
        let len = self.visible_entries().len();
        if len == 0 {
            return;
        }
        let index = self.active_tab.index();
        self.selected[index] =
            (self.selected[index] as isize + delta).rem_euclid(len as isize) as usize;
    }

    fn activate_selected(&mut self) -> ViewAction {
        let selected = self.selected[self.active_tab.index()];
        match self.visible_entries().get(selected).copied() {
            Some(VisibleEntry::Group(group)) => {
                let group = group.clone();
                let key = self.fold_key(&group);
                if !self.folded_groups.remove(&key) {
                    self.folded_groups.insert(key);
                }
                self.clamp_selection();
                ViewAction::None
            }
            Some(VisibleEntry::Item(_, item)) => item
                .action
                .as_ref()
                .and_then(ExtensionAction::command)
                .map_or(ViewAction::None, |command| {
                    ViewAction::EmitAndClose(ViewEvent::CommandPaletteSelected {
                        action: CommandPaletteAction::ExecuteCommand {
                            command: command.to_string(),
                        },
                    })
                }),
            _ => ViewAction::None,
        }
    }

    fn set_tab(&mut self, tab: ExtensionsTab) {
        self.active_tab = tab;
        self.clamp_selection();
    }

    fn selected_status(&self) -> String {
        let index = self.selected[self.active_tab.index()];
        match self.visible_entries().get(index).copied() {
            Some(VisibleEntry::Group(group)) => localize(
                self.locale,
                MessageId::ExtensionsGroupStatus,
                &[("count", &group.items.len().to_string())],
            ),
            Some(VisibleEntry::Item(_, item)) => {
                let action = item
                    .action
                    .as_ref()
                    .map(|action| format!(" · {}", action.label()))
                    .unwrap_or_default();
                format!("{} · {}{action} · {}", item.label, item.state, item.detail)
            }
            Some(VisibleEntry::Problem(problem)) => problem.to_string(),
            Some(VisibleEntry::Empty) | None => {
                tr(self.locale, MessageId::ExtensionsNoItems).into_owned()
            }
        }
    }
}

fn item_matches(item: &ExtensionItem, query: &str) -> bool {
    item.label.to_lowercase().contains(query)
        || item.description.to_lowercase().contains(query)
        || item.state.to_lowercase().contains(query)
        || item.detail.to_lowercase().contains(query)
        || item
            .action
            .as_ref()
            .is_some_and(|action| action.label().to_lowercase().contains(query))
}

impl ModalView for ExtensionsView {
    fn kind(&self) -> ModalKind {
        ModalKind::Extensions
    }

    fn handle_key(&mut self, key: KeyEvent) -> ViewAction {
        if key.code == KeyCode::Tab {
            self.focus = if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.focus.previous()
            } else {
                self.focus.next()
            };
            return ViewAction::None;
        }
        if self.focus == ExtensionsFocus::Search {
            match key.code {
                KeyCode::Esc => {
                    if self.query.is_empty() {
                        self.focus = ExtensionsFocus::List;
                    } else {
                        self.query.clear();
                        self.clamp_selection();
                    }
                }
                KeyCode::Backspace => {
                    self.query.pop();
                    self.clamp_selection();
                }
                KeyCode::Enter | KeyCode::Down => self.focus = ExtensionsFocus::List,
                KeyCode::Char(ch)
                    if !key.modifiers.intersects(
                        KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                    ) =>
                {
                    self.query.push(ch);
                    self.clamp_selection();
                }
                _ => {}
            }
            return ViewAction::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => ViewAction::Close,
            KeyCode::Char('/') => {
                self.focus = ExtensionsFocus::Search;
                ViewAction::None
            }
            KeyCode::Left if self.focus == ExtensionsFocus::Tabs => {
                self.set_tab(self.active_tab.previous());
                ViewAction::None
            }
            KeyCode::Right if self.focus == ExtensionsFocus::Tabs => {
                self.set_tab(self.active_tab.next());
                ViewAction::None
            }
            KeyCode::Char('[') => {
                self.set_tab(self.active_tab.previous());
                ViewAction::None
            }
            KeyCode::Char(']') => {
                self.set_tab(self.active_tab.next());
                ViewAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.focus = ExtensionsFocus::List;
                self.move_selection(-1);
                ViewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.focus = ExtensionsFocus::List;
                self.move_selection(1);
                ViewAction::None
            }
            KeyCode::Enter | KeyCode::Char(' ') => {
                self.focus = ExtensionsFocus::List;
                self.activate_selected()
            }
            _ => ViewAction::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> ViewAction {
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return ViewAction::None;
        }
        let hits = self.hits.borrow();
        if let Some((_, tab)) = hits
            .tabs
            .iter()
            .find(|(rect, _)| rect.contains((mouse.column, mouse.row).into()))
            .copied()
        {
            drop(hits);
            self.focus = ExtensionsFocus::Tabs;
            self.set_tab(tab);
            return ViewAction::None;
        }
        if hits
            .search
            .is_some_and(|rect| rect.contains((mouse.column, mouse.row).into()))
        {
            drop(hits);
            self.focus = ExtensionsFocus::Search;
            return ViewAction::None;
        }
        if let Some((_, row)) = hits
            .rows
            .iter()
            .find(|(rect, _)| rect.contains((mouse.column, mouse.row).into()))
            .copied()
        {
            drop(hits);
            self.focus = ExtensionsFocus::List;
            self.selected[self.active_tab.index()] = row;
            return self.activate_selected();
        }
        ViewAction::None
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let body = render_underwater_surface(
            area,
            buf,
            tr(self.locale, MessageId::ExtensionsTitle).into_owned(),
        );
        if body.width == 0 || body.height < 5 {
            return;
        }
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(body);

        let mut hits = HitAreas::default();
        let mut x = rows[0].x;
        let available = rows[0].right();
        for tab in ExtensionsTab::ALL {
            let label = if body.width < 58 && tab == ExtensionsTab::Marketplace {
                tr(self.locale, MessageId::ExtensionsTabMarketplaceCompact).into_owned()
            } else {
                tab.label(self.locale)
            };
            let width = (label.chars().count() as u16 + 2).min(available.saturating_sub(x));
            if width == 0 {
                break;
            }
            let tab_area = Rect::new(x, rows[0].y, width, 1);
            let active = tab == self.active_tab;
            let focused = active && self.focus == ExtensionsFocus::Tabs;
            let style = if focused {
                Style::default()
                    .fg(palette::WHALE_BG)
                    .bg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD)
            } else if active {
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else {
                Style::default().fg(palette::TEXT_MUTED)
            };
            Paragraph::new(Line::from(Span::styled(format!(" {label} "), style)))
                .render(tab_area, buf);
            hits.tabs.push((tab_area, tab));
            x = x.saturating_add(width);
        }

        let search_style = if self.focus == ExtensionsFocus::Search {
            Style::default()
                .fg(palette::WHALE_ACTION)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(palette::TEXT_MUTED)
        };
        let cursor = if self.focus == ExtensionsFocus::Search {
            "_"
        } else {
            ""
        };
        Paragraph::new(Line::from(vec![
            Span::styled(
                tr(self.locale, MessageId::ExtensionsSearchLabel),
                search_style,
            ),
            Span::styled(
                format!("{}{cursor}", self.query),
                Style::default().fg(palette::TEXT_PRIMARY),
            ),
        ]))
        .render(rows[1], buf);
        hits.search = Some(rows[1]);

        let entries = self.visible_entries();
        let list_height = usize::from(rows[2].height);
        let mut scroll = self.scroll[self.active_tab.index()];
        let selected = self.selected[self.active_tab.index()];
        if selected < scroll {
            scroll = selected;
        } else if selected >= scroll.saturating_add(list_height.max(1)) {
            scroll = selected.saturating_sub(list_height.saturating_sub(1));
        }
        let spacious = area.width >= 64 && area.height >= 16;
        for (visible_offset, (entry_index, entry)) in entries
            .iter()
            .enumerate()
            .skip(scroll)
            .take(list_height)
            .enumerate()
        {
            let row_area = Rect::new(
                rows[2].x,
                rows[2].y.saturating_add(visible_offset as u16),
                rows[2].width,
                1,
            );
            let is_selected = entry_index == selected;
            let style = if is_selected && self.focus == ExtensionsFocus::List {
                Style::default()
                    .fg(palette::WHALE_BG)
                    .bg(palette::WHALE_ACTION)
            } else if is_selected {
                Style::default()
                    .fg(palette::WHALE_ACTION)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(palette::TEXT_PRIMARY)
            };
            let text = match entry {
                VisibleEntry::Group(group) => {
                    let folded = self.folded_groups.contains(&self.fold_key(group));
                    format!(
                        "{} {} ({})",
                        if folded { "▸" } else { "▾" },
                        group.label,
                        group.items.len()
                    )
                }
                VisibleEntry::Item(_, item) => {
                    let action = item
                        .action
                        .as_ref()
                        .map(|action| format!("[{}] ", action.label()))
                        .unwrap_or_default();
                    if spacious && !item.description.is_empty() {
                        format!(
                            "  {action}{} [{}] — {}",
                            item.label, item.state, item.description
                        )
                    } else {
                        format!("  {action}{} [{}]", item.label, item.state)
                    }
                }
                VisibleEntry::Problem(problem) => format!("! {problem}"),
                VisibleEntry::Empty => {
                    if self.query.is_empty() {
                        tr(self.locale, MessageId::ExtensionsNoItems).into_owned()
                    } else {
                        localize(
                            self.locale,
                            MessageId::ExtensionsNoMatches,
                            &[("query", &self.query)],
                        )
                    }
                }
            };
            Paragraph::new(Line::from(Span::styled(
                truncate_view_text(&text, usize::from(row_area.width)),
                style,
            )))
            .render(row_area, buf);
            hits.rows.push((row_area, entry_index));
        }

        let status = truncate_view_text(&self.selected_status(), usize::from(rows[3].width));
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::default().fg(palette::TEXT_MUTED),
        )))
        .render(rows[3], buf);
        let compact_hints = [
            super::ActionHint::new("Tab", tr(self.locale, MessageId::ExtensionsActionFocus)),
            super::ActionHint::new("/", tr(self.locale, MessageId::SessionsActionSearch)),
            super::ActionHint::new("Esc", tr(self.locale, MessageId::SessionsActionClose)),
        ];
        let enter_label = match entries.get(selected).copied() {
            Some(VisibleEntry::Item(_, item)) => item
                .action
                .as_ref()
                .map(|action| action.label().to_string())
                .unwrap_or_else(|| {
                    tr(self.locale, MessageId::AutomationActionInspect).into_owned()
                }),
            _ => tr(self.locale, MessageId::ExtensionsActionFold).into_owned(),
        };
        let full_hints = [
            super::ActionHint::new("Tab", tr(self.locale, MessageId::ExtensionsActionFocus)),
            super::ActionHint::new("[ ]", tr(self.locale, MessageId::ExtensionsActionTabs)),
            super::ActionHint::new("↑↓", tr(self.locale, MessageId::LaunchHintMove)),
            super::ActionHint::new("Enter", enter_label),
            super::ActionHint::new("/", tr(self.locale, MessageId::SessionsActionSearch)),
            super::ActionHint::new("Esc", tr(self.locale, MessageId::SessionsActionClose)),
        ];
        render_modal_footer(
            rows[4],
            buf,
            if rows[4].width < 64 {
                &compact_hints
            } else {
                &full_hints
            },
        );
        *self.hits.borrow_mut() = hits;
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpRecoveryKind;

    #[test]
    fn mcp_item_action_for_stale_oauth_is_login() {
        let recovery =
            crate::mcp::mcp_recovery_kind(true, true, false, Some("401 Unauthorized"), true);
        assert_eq!(recovery, McpRecoveryKind::Reauth);
        assert_eq!(recovery.slash_command("github"), "/mcp login github");
        assert_eq!(tr(Locale::En, recovery.label_key()).as_ref(), "re-auth");
    }

    #[test]
    fn mcp_item_action_for_disconnected_server_is_reconnect() {
        let recovery = crate::mcp::mcp_recovery_kind(true, true, false, None, false);
        assert_eq!(recovery, McpRecoveryKind::Reconnect);
        assert_eq!(recovery.slash_command("playwright"), "/mcp reload");
        assert_eq!(tr(Locale::En, recovery.label_key()).as_ref(), "reconnect");
    }
}
