use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::network_policy::NetworkPolicy;

use super::mutation::{PluginMutationContext, PluginMutationOutcome, PluginMutationRequest};
use super::registry::PluginRegistry;
use super::types::LoadedPlugin;

/// A lifecycle operation requested by a command or operator surface.
///
/// The controller intentionally exposes operations rather than state-file
/// fields: every caller shares the registry's validation, staging, locking,
/// and digest-bound trust rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAction {
    Install {
        spec: String,
    },
    Update {
        selector: String,
    },
    Uninstall {
        selector: String,
    },
    Trust {
        selector: String,
        review_token: String,
    },
    Enable {
        selector: String,
    },
    Disable {
        selector: String,
    },
    Revoke {
        selector: String,
    },
    /// Re-check the registry's current discovery diagnostics without
    /// modifying plugin state. A selector limits the receipt to one bundle.
    Validate {
        selector: Option<String>,
    },
    Reload,
}

/// The user-meaningful result of one [`PluginAction`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginActionOutcome {
    Installed {
        name: String,
    },
    Updated {
        name: String,
    },
    AlreadyUpToDate {
        name: String,
    },
    Uninstalled {
        name: String,
    },
    NeedsNetworkApproval {
        host: String,
    },
    NetworkDenied {
        host: String,
    },
    Trusted {
        name: String,
    },
    Enabled {
        name: String,
    },
    Disabled {
        name: String,
    },
    TrustRevoked {
        name: String,
    },
    Validated {
        selector: Option<String>,
        clean: bool,
    },
    Reloaded {
        count: usize,
    },
    /// An enable request never implicitly trusts a bundle. The operator must
    /// inspect the currently resolved capability digest and confirm it.
    ReviewRequired {
        name: String,
    },
}

/// A typed receipt shared by interactive and headless callers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginActionReceipt {
    pub outcome: PluginActionOutcome,
    /// Final installed/updated bundle path, when the mutation produced one.
    pub path: Option<PathBuf>,
    /// The host must rebuild its Skill/MCP catalogue after this operation.
    pub registry_changed: bool,
}

/// The one mutation adapter for plugin operator surfaces.
pub struct PluginController<'a> {
    registry: &'a mut Arc<PluginRegistry>,
    workspace: &'a Path,
}

impl<'a> PluginController<'a> {
    #[must_use]
    pub fn new(registry: &'a mut Arc<PluginRegistry>, workspace: &'a Path) -> Self {
        Self {
            registry,
            workspace,
        }
    }

    /// Execute one lifecycle action through the registry's established
    /// validation, staging, locking, and digest-bound trust primitives.
    pub async fn execute(
        &mut self,
        action: PluginAction,
        network: &NetworkPolicy,
    ) -> Result<PluginActionReceipt, String> {
        match action {
            PluginAction::Install { spec } => {
                let source = crate::plugins::install::PluginInstallSource::parse(&spec)
                    .map_err(|error| error.to_string())?;
                self.execute_mutation(PluginMutationRequest::Install { source }, network)
                    .await
            }
            PluginAction::Update { selector } => {
                self.execute_mutation(PluginMutationRequest::Update { selector }, network)
                    .await
            }
            PluginAction::Uninstall { selector } => {
                self.execute_mutation(PluginMutationRequest::Uninstall { selector }, network)
                    .await
            }
            PluginAction::Trust {
                selector,
                review_token: supplied,
            } => {
                let plugin = self
                    .registry
                    .get(&selector)
                    .ok_or_else(|| format!("Plugin bundle `{selector}` was not found"))?;
                if supplied != review_token(plugin) {
                    return Err(
                        "Review token does not match this bundle content and capability set; run `/plugin trust <name>` again".into(),
                    );
                }
                Arc::make_mut(self.registry).trust(&selector)?;
                Ok(PluginActionReceipt {
                    outcome: PluginActionOutcome::Trusted { name: selector },
                    path: None,
                    registry_changed: true,
                })
            }
            PluginAction::Enable { selector } => {
                let plugin = self
                    .registry
                    .get(&selector)
                    .ok_or_else(|| format!("Plugin bundle `{selector}` was not found"))?;
                if !plugin.trusted() {
                    return Ok(PluginActionReceipt {
                        outcome: PluginActionOutcome::ReviewRequired {
                            name: plugin.name().to_string(),
                        },
                        path: None,
                        registry_changed: false,
                    });
                }
                Arc::make_mut(self.registry).enable(&selector)?;
                Ok(PluginActionReceipt {
                    outcome: PluginActionOutcome::Enabled { name: selector },
                    path: None,
                    registry_changed: true,
                })
            }
            PluginAction::Disable { selector } => {
                Arc::make_mut(self.registry).disable(&selector)?;
                Ok(PluginActionReceipt {
                    outcome: PluginActionOutcome::Disabled { name: selector },
                    path: None,
                    registry_changed: true,
                })
            }
            PluginAction::Revoke { selector } => {
                Arc::make_mut(self.registry).revoke_trust(&selector)?;
                Ok(PluginActionReceipt {
                    outcome: PluginActionOutcome::TrustRevoked { name: selector },
                    path: None,
                    registry_changed: true,
                })
            }
            PluginAction::Validate { selector } => {
                let clean = if let Some(name) = selector.as_deref() {
                    let plugin = self
                        .registry
                        .get(name)
                        .ok_or_else(|| format!("Plugin bundle `{name}` was not found"))?;
                    !plugin.diagnostics.iter().any(|diagnostic| {
                        diagnostic.level == crate::plugins::types::PluginDiagnosticLevel::Error
                    })
                } else {
                    self.registry.validation_is_clean()
                };
                Ok(PluginActionReceipt {
                    outcome: PluginActionOutcome::Validated { selector, clean },
                    path: None,
                    registry_changed: false,
                })
            }
            PluginAction::Reload => {
                *self.registry = self.registry.rediscover_for_workspace(self.workspace);
                Ok(PluginActionReceipt {
                    outcome: PluginActionOutcome::Reloaded {
                        count: self.registry.len(),
                    },
                    path: None,
                    registry_changed: true,
                })
            }
        }
    }

    async fn execute_mutation(
        &mut self,
        request: PluginMutationRequest,
        network: &NetworkPolicy,
    ) -> Result<PluginActionReceipt, String> {
        let receipt = crate::plugins::mutation::execute(
            request,
            &PluginMutationContext {
                network,
                max_size: crate::plugins::install::DEFAULT_MAX_SIZE_BYTES,
            },
            Arc::make_mut(self.registry),
        )
        .await
        .map_err(|error| format!("{error:#}"))?;

        let (outcome, registry_changed) = match receipt.outcome {
            PluginMutationOutcome::Installed => (
                PluginActionOutcome::Installed {
                    name: receipt.name.clone(),
                },
                true,
            ),
            PluginMutationOutcome::Updated => (
                PluginActionOutcome::Updated {
                    name: receipt.name.clone(),
                },
                true,
            ),
            PluginMutationOutcome::NoChange => (
                PluginActionOutcome::AlreadyUpToDate {
                    name: receipt.name.clone(),
                },
                false,
            ),
            PluginMutationOutcome::Uninstalled => (
                PluginActionOutcome::Uninstalled {
                    name: receipt.name.clone(),
                },
                true,
            ),
            PluginMutationOutcome::NeedsApproval(host) => {
                (PluginActionOutcome::NeedsNetworkApproval { host }, false)
            }
            PluginMutationOutcome::NetworkDenied(host) => {
                (PluginActionOutcome::NetworkDenied { host }, false)
            }
        };
        if registry_changed {
            *self.registry = self.registry.rediscover_for_workspace(self.workspace);
        }
        Ok(PluginActionReceipt {
            outcome,
            path: receipt.path,
            registry_changed,
        })
    }
}

#[must_use]
pub fn review_token(plugin: &LoadedPlugin) -> String {
    format!("{}.{}", plugin.content_hash, plugin.capability_hash)
}

/// Resolve the on-demand network policy used by command and modal callers.
/// `App` deliberately does not retain a mutable `Config`; falling back to the
/// runtime default preserves the normal prompt gate on malformed config.
#[must_use]
pub fn active_network_policy() -> NetworkPolicy {
    crate::config::Config::load(None, None)
        .unwrap_or_default()
        .network
        .map(|policy| policy.into_runtime())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::Arc;

    use tempfile::TempDir;

    use super::{PluginAction, PluginActionOutcome, PluginController, review_token};

    fn execute(
        registry: &mut Arc<crate::plugins::PluginRegistry>,
        workspace: &std::path::Path,
        action: PluginAction,
    ) -> Result<super::PluginActionReceipt, String> {
        let network = crate::network_policy::NetworkPolicy::default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(PluginController::new(registry, workspace).execute(action, &network))
    }

    fn registry_with_demo(root: &std::path::Path) -> Arc<crate::plugins::PluginRegistry> {
        let bundle = root.join(".codewhale/plugins/demo");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("plugin.toml"),
            "schema_version = 1\n[plugin]\nname = \"demo\"\nversion = \"1.0.0\"\n",
        )
        .unwrap();
        let discovery = crate::plugins::PluginDiscoveryContext::capture_pre_dotenv();
        discovery.registry_for_workspace(root)
    }

    #[test]
    fn enabling_an_unreviewed_bundle_returns_a_review_required_receipt() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path().join("home"));
        let mut registry = registry_with_demo(temp.path());

        let receipt = execute(
            &mut registry,
            temp.path(),
            PluginAction::Enable {
                selector: "demo".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            receipt.outcome,
            PluginActionOutcome::ReviewRequired { ref name } if name == "demo"
        ));
        assert!(!receipt.registry_changed);
        assert!(!registry.is_active("demo"));
    }

    #[test]
    fn trust_rejects_any_token_other_than_the_exact_content_and_capability_digest() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path().join("home"));
        let mut registry = registry_with_demo(temp.path());

        let error = execute(
            &mut registry,
            temp.path(),
            PluginAction::Trust {
                selector: "demo".into(),
                review_token: "wrong".into(),
            },
        )
        .unwrap_err();

        assert!(error.contains("Review token does not match"));
        assert!(!registry.get("demo").unwrap().trusted());
    }

    #[test]
    fn trusted_enabled_bundle_can_be_disabled_through_the_same_controller() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path().join("home"));
        let mut registry = registry_with_demo(temp.path());
        let token = review_token(registry.get("demo").unwrap());

        let trusted = execute(
            &mut registry,
            temp.path(),
            PluginAction::Trust {
                selector: "demo".into(),
                review_token: token,
            },
        )
        .unwrap();
        assert!(matches!(
            trusted.outcome,
            PluginActionOutcome::Trusted { .. }
        ));
        execute(
            &mut registry,
            temp.path(),
            PluginAction::Enable {
                selector: "demo".into(),
            },
        )
        .unwrap();
        assert!(registry.is_active("demo"));

        let disabled = execute(
            &mut registry,
            temp.path(),
            PluginAction::Disable {
                selector: "demo".into(),
            },
        )
        .unwrap();

        assert!(matches!(
            disabled.outcome,
            PluginActionOutcome::Disabled { .. }
        ));
        assert!(disabled.registry_changed);
        assert!(!registry.is_active("demo"));
    }

    #[test]
    fn install_rejects_an_empty_source_before_attempting_any_write() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path().join("home"));
        let mut registry = registry_with_demo(temp.path());

        let error = execute(
            &mut registry,
            temp.path(),
            PluginAction::Install { spec: "   ".into() },
        )
        .unwrap_err();

        assert!(error.contains("install source must not be empty"));
        assert!(registry.get("demo").is_some());
    }

    #[test]
    fn validating_one_bundle_ignores_unrelated_discovery_errors() {
        let _lock = crate::test_support::lock_test_env();
        let temp = TempDir::new().unwrap();
        let _home =
            crate::test_support::EnvVarGuard::set("CODEWHALE_HOME", temp.path().join("home"));
        let broken = temp.path().join(".codewhale/plugins/broken");
        fs::create_dir_all(&broken).unwrap();
        fs::write(broken.join("plugin.toml"), "not valid TOML = [").unwrap();
        let mut registry = registry_with_demo(temp.path());

        let receipt = execute(
            &mut registry,
            temp.path(),
            PluginAction::Validate {
                selector: Some("demo".into()),
            },
        )
        .unwrap();

        assert!(matches!(
            receipt.outcome,
            PluginActionOutcome::Validated { clean: true, .. }
        ));
        assert!(
            !registry.validation_is_clean(),
            "the other bundle is invalid"
        );
    }
}
