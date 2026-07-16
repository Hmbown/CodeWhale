//! Named fleet roster files for dogfood lanes (#4178).
//!
//! Format: TOML at `fleets/<name>.toml` (workspace) or
//! `$CODEWHALE_HOME/fleets/<name>.toml`.
//!
//! Fleet resolves roles → profile ids only. Runtime owns tmux/worktrees.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Parsed named fleet file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NamedFleet {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// role name → AgentProfile id
    pub roles: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NamedFleetError {
    #[error("fleet file not found: {0}")]
    NotFound(String),
    #[error("failed to read fleet file {path}: {message}")]
    Io { path: String, message: String },
    #[error("failed to parse fleet file {path}: {message}")]
    Parse { path: String, message: String },
    #[error("fleet `{fleet}` is missing required role `{role}`")]
    MissingRole { fleet: String, role: String },
    #[error("fleet name mismatch: file declares `{declared}`, expected `{expected}`")]
    NameMismatch { declared: String, expected: String },
}

/// Required roles for the stopship dogfood fleet (#4178).
pub const STOPSHIP_REQUIRED_ROLES: &[&str] = &[
    "scout",
    "implementer",
    "reviewer",
    "verifier",
    "release_lead",
];

/// Parse a fleet TOML document.
pub fn parse_named_fleet(toml_text: &str) -> Result<NamedFleet, NamedFleetError> {
    // Minimal TOML subset without adding a toml dep to workflow:
    // accept JSON as well for tests; for TOML use a tiny hand parser for
    // the documented shape, or serde via json for unit tests.
    // Prefer JSON if the text looks like JSON; otherwise use line-oriented TOML.
    let trimmed = toml_text.trim();
    if trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).map_err(|e| NamedFleetError::Parse {
            path: "<memory>".into(),
            message: e.to_string(),
        });
    }
    parse_fleet_toml_minimal(trimmed)
}

fn parse_fleet_toml_minimal(text: &str) -> Result<NamedFleet, NamedFleetError> {
    let mut name = None;
    let mut description = None;
    let mut roles = BTreeMap::new();
    let mut section = "";
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').to_string();
        match section {
            "" => match key {
                "name" => name = Some(value),
                "description" => description = Some(value),
                _ => {}
            },
            "roles" => {
                roles.insert(key.to_string(), value);
            }
            _ => {}
        }
    }
    let name = name.ok_or_else(|| NamedFleetError::Parse {
        path: "<memory>".into(),
        message: "missing name".into(),
    })?;
    Ok(NamedFleet {
        name,
        description,
        roles,
    })
}

/// Load fleet by name from search paths (first hit wins).
pub fn load_named_fleet(
    name: &str,
    search_roots: &[PathBuf],
) -> Result<NamedFleet, NamedFleetError> {
    let file_name = format!("{name}.toml");
    for root in search_roots {
        let path = root.join("fleets").join(&file_name);
        if path.is_file() {
            return load_named_fleet_file(&path, Some(name));
        }
    }
    Err(NamedFleetError::NotFound(name.to_string()))
}

pub fn load_named_fleet_file(
    path: &Path,
    expect_name: Option<&str>,
) -> Result<NamedFleet, NamedFleetError> {
    let text = std::fs::read_to_string(path).map_err(|e| NamedFleetError::Io {
        path: path.display().to_string(),
        message: e.to_string(),
    })?;
    let fleet = parse_named_fleet(&text).map_err(|e| match e {
        NamedFleetError::Parse { message, .. } => NamedFleetError::Parse {
            path: path.display().to_string(),
            message,
        },
        other => other,
    })?;
    if let Some(expected) = expect_name
        && fleet.name != expected
    {
        return Err(NamedFleetError::NameMismatch {
            declared: fleet.name,
            expected: expected.to_string(),
        });
    }
    Ok(fleet)
}

impl NamedFleet {
    /// Resolve a role name to a profile id.
    pub fn resolve(&self, role: &str) -> Result<&str, NamedFleetError> {
        let key = role.trim().to_ascii_lowercase();
        self.roles
            .get(&key)
            .or_else(|| {
                self.roles
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(role))
                    .map(|(_, v)| v)
            })
            .map(String::as_str)
            .ok_or_else(|| NamedFleetError::MissingRole {
                fleet: self.name.clone(),
                role: role.to_string(),
            })
    }

    /// Ensure all required stopship roles are present.
    pub fn validate_stopship_roles(&self) -> Result<(), NamedFleetError> {
        for role in STOPSHIP_REQUIRED_ROLES {
            self.resolve(role)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STOPSHIP_TOML: &str = r#"
name = "stopship"
description = "Stopship dogfood fleet"

[roles]
scout = "scout"
implementer = "builder"
reviewer = "reviewer"
verifier = "verifier"
release_lead = "manager"
"#;

    #[test]
    fn stopship_fleet_resolves_all_five_roles() {
        let fleet = parse_named_fleet(STOPSHIP_TOML).expect("parse");
        assert_eq!(fleet.name, "stopship");
        fleet.validate_stopship_roles().expect("all roles");
        assert_eq!(fleet.resolve("scout").unwrap(), "scout");
        assert_eq!(fleet.resolve("implementer").unwrap(), "builder");
        assert_eq!(fleet.resolve("reviewer").unwrap(), "reviewer");
        assert_eq!(fleet.resolve("verifier").unwrap(), "verifier");
        assert_eq!(fleet.resolve("release_lead").unwrap(), "manager");
    }

    #[test]
    fn unknown_role_fails_clearly() {
        let fleet = parse_named_fleet(STOPSHIP_TOML).unwrap();
        let err = fleet.resolve("wizard").unwrap_err();
        assert!(matches!(err, NamedFleetError::MissingRole { .. }));
    }

    #[test]
    fn loads_workspace_fleet_file() {
        // Relative to crate CARGO_MANIFEST_DIR → repo root fleets/
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let fleet = load_named_fleet("stopship", &[root]).expect("load workspace fleet");
        fleet.validate_stopship_roles().unwrap();
    }
}
