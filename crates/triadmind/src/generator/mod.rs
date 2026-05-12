//! # Generator — UpgradeProtocol → source code scaffolding
//!
//! Ported from triadmind-core/generator.ts
//!
//! Translates an UpgradeProtocol into concrete code changes:
//! - `reuse` actions: verify existing nodes
//! - `modify` actions: generate code diffs for existing files
//! - `create_child` actions: generate new source files with skeleton code
//!
//! Currently supports Rust and TypeScript code generation.
//!
//! @LeftBranch: apply_protocol, generate_code_for_action
//! @RightBranch: GeneratorOptions, GeneratedFile, GenerationResult

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::{ProtocolAction, TriadOp, UpgradeProtocol};

// TriadNodeDefinition used in create_child test below
#[cfg(test)]
use crate::protocol::TriadNodeDefinition;

// ── Generator Options ───────────────────────────────────────────────

/// Options controlling code generation behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratorOptions {
    /// Target language for generated code.
    pub language: String,
    /// Whether to actually write files (false = dry run).
    #[serde(default = "default_true")]
    pub write_files: bool,
    /// Base directory for generated files.
    #[serde(default)]
    pub output_dir: Option<PathBuf>,
}

fn default_true() -> bool {
    true
}

impl Default for GeneratorOptions {
    fn default() -> Self {
        Self {
            language: "rust".into(),
            write_files: true,
            output_dir: None,
        }
    }
}

// ── Generated File ──────────────────────────────────────────────────

/// A single generated source file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneratedFile {
    /// Relative file path.
    pub path: String,
    /// Generated source code content.
    pub content: String,
    /// Whether this is a new file (true) or a modification (false).
    pub is_new: bool,
    /// The action that triggered this generation.
    #[serde(rename = "sourceAction")]
    pub source_action: String,
}

// ── Generation Result ───────────────────────────────────────────────

/// Result of a code generation run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationResult {
    /// Project root directory.
    #[serde(rename = "projectRoot")]
    pub project_root: String,
    /// Protocol version used.
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    /// Generated or modified files.
    pub files: Vec<GeneratedFile>,
    /// Number of reuse actions (no code changes).
    #[serde(rename = "reuseCount")]
    pub reuse_count: usize,
    /// Number of modify actions.
    #[serde(rename = "modifyCount")]
    pub modify_count: usize,
    /// Number of create_child actions.
    #[serde(rename = "createCount")]
    pub create_count: usize,
}

// ── Core Entry Point ────────────────────────────────────────────────

/// Apply an upgrade protocol to generate/modify source files.
///
/// Walks through each action in the protocol and generates corresponding
/// code changes. Reuse actions are no-ops (verified existing nodes).
pub fn apply_protocol(
    project_root: &Path,
    protocol: &UpgradeProtocol,
    options: &GeneratorOptions,
) -> Result<GenerationResult, anyhow::Error> {
    let mut files: Vec<GeneratedFile> = Vec::new();
    let mut reuse_count = 0usize;
    let mut modify_count = 0usize;
    let mut create_count = 0usize;

    for action in &protocol.actions {
        match action.op {
            TriadOp::Reuse => {
                reuse_count += 1;
                // Reuse is a verification-only action — no code changes
            }
            TriadOp::Modify => {
                modify_count += 1;
                if let Some(file) = generate_modify_code(action, options) {
                    files.push(file);
                }
            }
            TriadOp::CreateChild => {
                create_count += 1;
                if let Some(file) = generate_create_child_code(action, options) {
                    files.push(file);
                }
            }
        }
    }

    // Write files if not dry run
    if options.write_files {
        let output_base = options
            .output_dir
            .as_deref()
            .unwrap_or(project_root);
        for file in &files {
            let dest = output_base.join(&file.path);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &file.content)?;
        }
    }

    Ok(GenerationResult {
        project_root: project_root.to_string_lossy().to_string(),
        protocol_version: protocol.protocol_version.clone(),
        files,
        reuse_count,
        modify_count,
        create_count,
    })
}

// ── Code Generation Helpers ─────────────────────────────────────────

/// Generate code for a `modify` action.
fn generate_modify_code(
    action: &ProtocolAction,
    options: &GeneratorOptions,
) -> Option<GeneratedFile> {
    let node_id = action.node_id.as_deref()?;
    let fission = action.fission.as_ref()?;

    let content = match options.language.as_str() {
        "rust" => generate_rust_method_modification(node_id, fission),
        "typescript" | "javascript" => generate_ts_method_modification(node_id, fission),
        _ => return None,
    };

    // Modify actions target existing files — we output a diff-style comment
    let path = format!(
        "src/{}.{}",
        node_id.split('.').next().unwrap_or("generated").to_lowercase(),
        if options.language == "rust" { "rs" } else { "ts" }
    );

    Some(GeneratedFile {
        path,
        content,
        is_new: false,
        source_action: format!("modify:{}", node_id),
    })
}

/// Generate code for a `create_child` action.
fn generate_create_child_code(
    action: &ProtocolAction,
    options: &GeneratorOptions,
) -> Option<GeneratedFile> {
    let node = action.node.as_ref()?;
    let fission = node.fission.as_ref()?;

    let content = match options.language.as_str() {
        "rust" => generate_rust_function(&node.node_id, fission),
        "typescript" | "javascript" => generate_ts_function(&node.node_id, fission),
        _ => return None,
    };

    let file_name = node
        .node_id
        .split('.')
        .next()
        .unwrap_or("generated")
        .to_lowercase();
    let path = format!(
        "src/{}.{}",
        file_name,
        if options.language == "rust" { "rs" } else { "ts" }
    );

    Some(GeneratedFile {
        path,
        content,
        is_new: true,
        source_action: format!("create_child:{}", node.node_id),
    })
}

// ── Rust Code Generation ────────────────────────────────────────────

/// Generate a Rust function from a node definition.
fn generate_rust_function(
    node_id: &str,
    fission: &crate::protocol::TriadFission,
) -> String {
    let fn_name = node_id.split('.').last().unwrap_or(node_id);
    let params: Vec<String> = fission
        .demand
        .iter()
        .enumerate()
        .map(|(i, d)| format!("    arg{}: {}", i, d))
        .collect();
    let return_type = fission.answer.first().cloned().unwrap_or_else(|| "()".into());

    let mut lines = Vec::new();

    // Doc comment
    if !fission.problem.is_empty() {
        lines.push(format!("/// {}", fission.problem));
    }

    lines.push(format!(
        "pub fn {}({}) -> {} {{",
        fn_name,
        if params.is_empty() {
            String::new()
        } else {
            format!("\n{}\n", params.join(",\n"))
        },
        return_type
    ));

    // Body: return default value
    lines.push(format!("    // TODO: implement {}", fission.problem));
    lines.push("    todo!(\"Generated by TriadMind — implement this function\")".into());
    lines.push("}".into());

    lines.join("\n")
}

/// Generate a modification comment for an existing Rust method.
fn generate_rust_method_modification(
    node_id: &str,
    fission: &crate::protocol::TriadFission,
) -> String {
    format!(
        "// TriadMind: modify `{}` to handle: {}\n// New params: {:?}\n// New return: {:?}\n",
        node_id,
        fission.problem,
        fission.demand,
        fission.answer,
    )
}

// ── TypeScript Code Generation ──────────────────────────────────────

/// Generate a TypeScript function from a node definition.
fn generate_ts_function(
    node_id: &str,
    fission: &crate::protocol::TriadFission,
) -> String {
    let fn_name = node_id.split('.').last().unwrap_or(node_id);
    let params: Vec<String> = fission
        .demand
        .iter()
        .enumerate()
        .map(|(i, d)| format!("arg{}: {}", i, d))
        .collect();
    let return_type = fission.answer.first().cloned().unwrap_or_else(|| "void".into());

    let mut lines = Vec::new();

    // JSDoc
    if !fission.problem.is_empty() {
        lines.push("/**".into());
        lines.push(format!(" * {}", fission.problem));
        lines.push(" */".into());
    }

    lines.push(format!(
        "export function {}({}) {} {{",
        fn_name,
        params.join(", "),
        if return_type == "void" || return_type == "()" {
            String::new()
        } else {
            format!(": {}", return_type)
        }
    ));

    lines.push(format!("    // TODO: implement {}", fission.problem));
    lines.push("    throw new Error('Not implemented: generated by TriadMind');".into());
    lines.push("}".into());

    lines.join("\n")
}

/// Generate a modification comment for an existing TypeScript method.
fn generate_ts_method_modification(
    node_id: &str,
    fission: &crate::protocol::TriadFission,
) -> String {
    format!(
        "// TriadMind: modify `{}` to handle: {}\n// New params: {:?}\n// New return: {:?}\n",
        node_id,
        fission.problem,
        fission.demand,
        fission.answer,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_protocol_empty() {
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test".into(),
            upgrade_policy: crate::protocol::UpgradePolicy {
                allowed_ops: vec!["reuse".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![],
        };
        let tmp = std::env::temp_dir().join("triadmind_gen_empty");
        let _ = std::fs::create_dir_all(&tmp);
        let opts = GeneratorOptions::default();
        let result = apply_protocol(&tmp, &protocol, &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.files.len(), 0);
    }

    #[test]
    fn test_generate_rust_function() {
        let fission = crate::protocol::TriadFission {
            problem: "Process incoming data".into(),
            demand: vec!["String".into(), "Config".into()],
            answer: vec!["Result".into()],
        };
        let code = generate_rust_function("Service.process", &fission);
        assert!(code.contains("/// Process incoming data"));
        assert!(code.contains("pub fn process"));
        assert!(code.contains("arg0: String"));
        assert!(code.contains("arg1: Config"));
        assert!(code.contains("-> Result"));
    }

    #[test]
    fn test_generate_ts_function() {
        let fission = crate::protocol::TriadFission {
            problem: "Handle user login".into(),
            demand: vec!["string".into(), "string".into()],
            answer: vec!["boolean".into()],
        };
        let code = generate_ts_function("AuthService.login", &fission);
        assert!(code.contains("* Handle user login"));
        assert!(code.contains("export function login"));
        assert!(code.contains("arg0: string"));
        assert!(code.contains(": boolean"));
    }

    #[test]
    fn test_apply_protocol_with_actions() {
        let protocol = UpgradeProtocol {
            protocol_version: "1.0".into(),
            project: "test".into(),
            map_source: "triad-map.json".into(),
            user_demand: "test".into(),
            upgrade_policy: crate::protocol::UpgradePolicy {
                allowed_ops: vec!["create_child".into()],
                principle: "reuse_first".into(),
            },
            macro_split: None,
            meso_split: None,
            micro_split: None,
            actions: vec![
                ProtocolAction {
                    op: TriadOp::Reuse,
                    node_id: Some("Existing.run".into()),
                    parent_node_id: None,
                    node: None,
                    fission: None,
                    reuse: vec![],
                    reason: None,
                    confidence: None,
                },
                ProtocolAction {
                    op: TriadOp::CreateChild,
                    node_id: None,
                    parent_node_id: Some("Existing.run".into()),
                    node: Some(TriadNodeDefinition {
                        node_id: "NewFeature.process".into(),
                        category: Some("core".into()),
                        source_path: None,
                        lifecycle: None,
                        fission: Some(crate::protocol::TriadFission {
                            problem: "New feature".into(),
                            demand: vec!["Input".into()],
                            answer: vec!["Output".into()],
                        }),
                    }),
                    fission: None,
                    reuse: vec![],
                    reason: None,
                    confidence: None,
                },
            ],
        };

        let tmp = std::env::temp_dir().join("triadmind_gen_actions");
        let _ = std::fs::create_dir_all(&tmp);
        let opts = GeneratorOptions {
            write_files: false, // dry run
            ..Default::default()
        };
        let result = apply_protocol(&tmp, &protocol, &opts);
        let _ = std::fs::remove_dir_all(&tmp);
        assert!(result.is_ok());
        let r = result.unwrap();
        assert_eq!(r.reuse_count, 1);
        assert_eq!(r.create_count, 1);
        assert_eq!(r.files.len(), 1);
    }
}
