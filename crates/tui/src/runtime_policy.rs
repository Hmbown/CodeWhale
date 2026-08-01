//! Normalization for Runtime mode and permission-posture wires.
//!
//! Older clients can still send mode aliases and the legacy `auto_approve`
//! bit. New callers send a named posture. Keep those inputs readable, but
//! persist and execute one contract: Plan / Act / Operate plus Ask /
//! Auto-Review / Full Access.

use anyhow::{Result, bail};

use crate::tui::app::AppMode;
use crate::tui::approval::ApprovalMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimePolicyProjection {
    pub(crate) mode: AppMode,
    pub(crate) permission: ApprovalMode,
}

impl RuntimePolicyProjection {
    /// Read a persisted compatibility shape. Unknown historical values fail
    /// closed to Act + Ask unless the old bypass bit is explicitly present.
    #[must_use]
    pub(crate) fn from_persisted(
        mode: &str,
        permission_posture: Option<&str>,
        auto_approve: bool,
    ) -> Self {
        let parsed_mode = parse_runtime_mode(mode).unwrap_or(AppMode::Agent);
        let permission = permission_posture
            .and_then(ApprovalMode::from_config_value)
            .filter(|permission| *permission != ApprovalMode::Never)
            .unwrap_or_else(|| {
                if parsed_mode == AppMode::Yolo || auto_approve {
                    ApprovalMode::Bypass
                } else {
                    ApprovalMode::Suggest
                }
            });
        Self {
            mode: visible_mode(parsed_mode),
            permission,
        }
    }

    /// Validate and normalize a new Runtime request. Legacy aliases are
    /// accepted as one-way inputs, but only current values are persisted.
    pub(crate) fn from_request(
        mode: &str,
        permission_posture: Option<&str>,
        auto_approve: Option<bool>,
    ) -> Result<Self> {
        let parsed_mode = parse_runtime_mode(mode).ok_or_else(|| {
            anyhow::anyhow!("unsupported Runtime mode {mode:?}; expected plan, act, or operate")
        })?;
        let permission = match permission_posture {
            Some(value) => ApprovalMode::from_config_value(value).ok_or_else(|| {
                anyhow::anyhow!(
                    "unsupported permission posture {value:?}; expected ask, auto-review, or full-access"
                )
            })?,
            None if parsed_mode == AppMode::Yolo || auto_approve.unwrap_or(false) => {
                ApprovalMode::Bypass
            }
            None => ApprovalMode::Suggest,
        };
        if permission == ApprovalMode::Never {
            bail!("permission posture 'never' is not part of the Runtime product contract");
        }
        Ok(Self {
            mode: visible_mode(parsed_mode),
            permission,
        })
    }

    #[must_use]
    pub(crate) fn mode_setting(self) -> &'static str {
        self.mode.as_setting()
    }

    #[must_use]
    pub(crate) fn permission_wire(self) -> &'static str {
        match self.permission {
            ApprovalMode::Suggest => "ask",
            ApprovalMode::Auto => "auto_review",
            ApprovalMode::Bypass => "full_access",
            ApprovalMode::Never => "ask",
        }
    }

    #[must_use]
    pub(crate) fn auto_approve(self) -> bool {
        self.permission == ApprovalMode::Bypass
    }
}

#[must_use]
pub(crate) fn parse_runtime_mode(value: &str) -> Option<AppMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(AppMode::Agent),
        other => AppMode::parse(other),
    }
}

#[must_use]
fn visible_mode(mode: AppMode) -> AppMode {
    match mode {
        AppMode::Auto | AppMode::Yolo => AppMode::Agent,
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_inputs_project_to_current_mode_and_permission_wires() {
        for alias in ["normal", "agent", "auto"] {
            let policy = RuntimePolicyProjection::from_persisted(alias, None, false);
            assert_eq!(policy.mode_setting(), "agent", "{alias}");
            assert_eq!(policy.permission_wire(), "ask", "{alias}");
        }
        for alias in ["yolo", "bypass"] {
            let policy = RuntimePolicyProjection::from_persisted(alias, None, false);
            assert_eq!(policy.mode_setting(), "agent", "{alias}");
            assert_eq!(policy.permission_wire(), "full_access", "{alias}");
        }
    }

    #[test]
    fn named_posture_is_normalized_and_authoritative() {
        let auto = RuntimePolicyProjection::from_request("operate", Some("auto-review"), None)
            .expect("compat permission");
        assert_eq!(auto.mode_setting(), "operate");
        assert_eq!(auto.permission_wire(), "auto_review");
        assert_eq!(auto.permission, ApprovalMode::Auto);
        assert!(!auto.auto_approve());

        let full = RuntimePolicyProjection::from_request("act", Some("full_access"), None)
            .expect("current permission wire");
        assert_eq!(full.permission_wire(), "full_access");
        assert_eq!(full.permission, ApprovalMode::Bypass);
        assert!(full.auto_approve());

        let ask = RuntimePolicyProjection::from_request("yolo", Some("ask"), Some(true))
            .expect("named posture overrides legacy flags");
        assert_eq!(ask.mode_setting(), "agent");
        assert_eq!(ask.permission_wire(), "ask");
        assert_eq!(ask.permission, ApprovalMode::Suggest);
        assert!(!ask.auto_approve());
    }

    #[test]
    fn invalid_or_never_postures_are_rejected() {
        assert!(RuntimePolicyProjection::from_request("act", Some("owner"), None).is_err());
        assert!(RuntimePolicyProjection::from_request("act", Some("never"), None).is_err());
    }
}
