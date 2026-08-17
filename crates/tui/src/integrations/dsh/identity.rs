//! Exact Codewhale route identity → DeepSeek Harness overlay mapping.
//!
//! The mapping carries *identity only*: provider id, model id, endpoint,
//! reasoning tier, permission posture, and the *name* of the credential
//! environment variable. It never carries a credential value, an OAuth
//! document, or a base URL that embeds userinfo/query material.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Non-secret facts about the route Codewhale is currently configured to use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CodewhaleRouteIdentity {
    /// Exact configured provider id (e.g. `deepseek`, `ollama`, `zai`, or a
    /// named custom table key).
    pub(crate) provider_id: String,
    /// Human label for the provider.
    pub(crate) provider_label: String,
    /// Exact model id put on the wire.
    pub(crate) model: String,
    /// Resolved base URL (structural; must not carry credentials).
    pub(crate) base_url: String,
    /// Wire protocol Codewhale speaks to this endpoint.
    pub(crate) protocol: WireProtocol,
    /// Canonical credential env var name for the provider, if it has one.
    pub(crate) api_key_env: Option<String>,
    /// True when the route is a keyless self-hosted endpoint (loopback
    /// Ollama/LM Studio/vLLM/SGLang): no credential reference is written.
    pub(crate) keyless_local: bool,
    /// Codewhale reasoning tier as configured (`off|low|medium|high|xhigh|max|ultra`).
    pub(crate) reasoning_effort: Option<String>,
    /// Codewhale sandbox mode (`read-only|workspace-write|danger-full-access|external-sandbox`).
    pub(crate) sandbox_mode: Option<String>,
    /// Codewhale approval policy (`suggest|auto|never`).
    pub(crate) approval_policy: Option<String>,
    pub(crate) yolo: bool,
    /// Workspace the launch is bound to.
    pub(crate) workspace: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum WireProtocol {
    ChatCompletions,
    Responses,
    AnthropicMessages,
}

/// DSH permission mode mirrored from the Codewhale posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DshPermissionMode {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl DshPermissionMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::WorkspaceWrite => "workspace-write",
            Self::DangerFullAccess => "danger-full-access",
        }
    }
}

/// Which DSH adapter carries the route.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DshAdapter {
    /// `@deepseek-ai/dsh-llm-deepseek`, route id `deepseek-official`.
    DeepseekNative,
    /// `@deepseek-ai/dsh-llm-pi-ai` hand-declared route. The route names the
    /// provider's own wire dialect (`openai-completions`, `openai-responses`,
    /// or `anthropic-messages`); see [`pi_ai_api_for`].
    PiAiOpenAiCompatible { route_id: String },
    /// DSH cannot carry this route; nothing is written for it.
    Unsupported { reason: String },
}

/// The identity as it will be written into the overlay, plus disclosures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct MappedIdentity {
    pub(crate) source: CodewhaleRouteIdentity,
    pub(crate) adapter: DshAdapter,
    /// DSH `reasoningEffort` (`off|high|max`) for the native adapter; `None`
    /// when Codewhale has no explicit tier or the adapter cannot express it.
    pub(crate) dsh_reasoning_effort: Option<String>,
    pub(crate) permission_mode: DshPermissionMode,
    /// Facts the user must know that the overlay cannot enforce.
    pub(crate) disclosures: Vec<String>,
}

impl MappedIdentity {
    pub(crate) fn mappable(&self) -> bool {
        !matches!(self.adapter, DshAdapter::Unsupported { .. })
    }

    /// The `provider` value DSH sees for this route.
    pub(crate) fn dsh_provider(&self) -> Option<&str> {
        match &self.adapter {
            DshAdapter::DeepseekNative => Some("deepseek-official"),
            DshAdapter::PiAiOpenAiCompatible { route_id } => Some(route_id),
            DshAdapter::Unsupported { .. } => None,
        }
    }
}

/// Map a Codewhale reasoning tier onto DSH's `off | high | max`.
pub(crate) fn dsh_reasoning_effort(effort: Option<&str>) -> Option<&'static str> {
    match effort.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") => None,
        Some("off" | "none" | "disabled" | "false") => Some("off"),
        Some("minimal" | "low" | "medium" | "mid" | "high" | "auto") => Some("high"),
        Some("xhigh" | "max" | "maximum" | "highest" | "ultra" | "ultracode") => Some("max"),
        Some(_) => None,
    }
}

/// Mirror the Codewhale posture. Full access is only granted when Codewhale
/// itself runs with full access *and* the caller confirmed it explicitly.
pub(crate) fn permission_mode_for(
    identity: &CodewhaleRouteIdentity,
    allow_full_access: bool,
) -> (DshPermissionMode, Option<String>) {
    let sandbox = identity
        .sandbox_mode
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase());
    let approval = identity
        .approval_policy
        .as_deref()
        .map(|s| s.trim().to_ascii_lowercase());
    let codewhale_full = identity.yolo
        || matches!(
            sandbox.as_deref(),
            Some("danger-full-access" | "external-sandbox")
        );
    if matches!(sandbox.as_deref(), Some("read-only")) {
        return (DshPermissionMode::ReadOnly, None);
    }
    if codewhale_full {
        if allow_full_access {
            return (
                DshPermissionMode::DangerFullAccess,
                Some(
                    "DSH danger-full-access mirrors Codewhale full access; DSH will not ask before file effects."
                        .to_string(),
                ),
            );
        }
        return (
            DshPermissionMode::WorkspaceWrite,
            Some(
                "Codewhale runs with full access, but the DSH overlay stays at workspace-write; pass --allow-full-access to mirror it."
                    .to_string(),
            ),
        );
    }
    let note = match approval.as_deref() {
        Some("never" | "deny" | "denied") => Some(
            "Codewhale approval policy is `never`; DSH keeps its own ask-before-effects policy at workspace-write."
                .to_string(),
        ),
        _ => None,
    };
    (DshPermissionMode::WorkspaceWrite, note)
}

fn base_url_is_structural(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("empty base URL".to_string());
    }
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err("base URL has no scheme".to_string());
    };
    if !matches!(scheme, "http" | "https") {
        return Err(format!("unsupported base URL scheme `{scheme}`"));
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.contains('@') {
        return Err("base URL embeds userinfo; refusing to copy it".to_string());
    }
    if rest.contains('?') || rest.contains('#') {
        return Err("base URL carries a query/fragment; refusing to copy it".to_string());
    }
    Ok(())
}

/// The `api:` dialect a hand-declared `dsh-llm-pi-ai` route names for a
/// Codewhale wire protocol — identity-preserving, never an approximation.
///
/// Verified against the installed `@deepseek-ai/dsh@0.1.0-rc.6`:
/// `@deepseek-ai/dsh-llm-pi-ai`'s exported profile schema accepts exactly
/// `openai-completions | openai-responses | anthropic-messages` for `api:`
/// (`lib/index.js`: `const PROTOCOLS = { "openai-completions": …,
/// "openai-responses": openAIResponsesApi, "anthropic-messages": … }`),
/// while `@deepseek-ai/dsh-llm-deepseek` (the `deepseek-official` route)
/// speaks chat completions only — its single wire call posts to
/// `<baseURL>/chat/completions` with no protocol switch — so
/// Responses-dialect DeepSeek routes (e.g. `deepseek-v4-flash`) ride the
/// pi-ai hand-declared route instead.
pub(crate) fn pi_ai_api_for(protocol: WireProtocol) -> &'static str {
    match protocol {
        WireProtocol::ChatCompletions => "openai-completions",
        WireProtocol::Responses => "openai-responses",
        WireProtocol::AnthropicMessages => "anthropic-messages",
    }
}

fn route_id_for(provider_id: &str) -> String {
    let mut out = String::from("codewhale-");
    for ch in provider_id.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

pub(crate) fn map_identity(
    identity: &CodewhaleRouteIdentity,
    allow_full_access: bool,
) -> MappedIdentity {
    let mut disclosures = Vec::new();
    let (permission_mode, note) = permission_mode_for(identity, allow_full_access);
    if let Some(note) = note {
        disclosures.push(note);
    }
    let dsh_effort = dsh_reasoning_effort(identity.reasoning_effort.as_deref()).map(str::to_string);

    if let Err(reason) = base_url_is_structural(&identity.base_url) {
        return MappedIdentity {
            source: identity.clone(),
            adapter: DshAdapter::Unsupported { reason },
            dsh_reasoning_effort: None,
            permission_mode,
            disclosures,
        };
    }

    let is_deepseek = matches!(identity.provider_id.as_str(), "deepseek" | "deepseek-cn");
    if is_deepseek && identity.protocol == WireProtocol::ChatCompletions {
        if identity.reasoning_effort.is_some() && dsh_effort.is_none() {
            disclosures.push(format!(
                "Codewhale reasoning tier `{}` has no DSH equivalent; DSH keeps its default (high).",
                identity.reasoning_effort.as_deref().unwrap_or("")
            ));
        }
        disclosures.push(
            "DSH resolves DEEPSEEK_API_KEY from its own environment or $DSH_HOME/.credentials.yaml; Codewhale does not hand over a key."
                .to_string(),
        );
        return MappedIdentity {
            source: identity.clone(),
            adapter: DshAdapter::DeepseekNative,
            dsh_reasoning_effort: dsh_effort,
            permission_mode,
            disclosures,
        };
    }

    // Every wire dialect Codewhale can speak, `dsh-llm-pi-ai` declares a
    // hand-declared route for (see `pi_ai_api_for`), so the route is carried
    // in its own dialect rather than refused or approximated as completions.
    if identity.protocol != WireProtocol::ChatCompletions {
        disclosures.push(format!(
            "Route speaks the {} dialect; DSH carries it through the `@deepseek-ai/dsh-llm-pi-ai` hand-declared route (`api: {}`) — the wire dialect is preserved, never approximated as chat completions.",
            dialect_label(identity.protocol),
            pi_ai_api_for(identity.protocol)
        ));
    }

    if identity.reasoning_effort.is_some() {
        disclosures.push(
            "Reasoning tier is not mapped for hand-declared DSH routes (per-provider wire spellings are not verified); DSH sends no effort parameter."
                .to_string(),
        );
    }
    if identity.keyless_local {
        disclosures.push(
            "Keyless local route: no credential reference is written; DSH talks to the endpoint without a key."
                .to_string(),
        );
    } else if let Some(env) = identity.api_key_env.as_deref() {
        disclosures.push(format!(
            "DSH resolves {env} from its own environment or $DSH_HOME/.credentials.yaml; Codewhale does not hand over a key."
        ));
    } else {
        disclosures.push(
            "No credential env var is known for this provider; DSH will defer to its ambient credential discovery."
                .to_string(),
        );
    }
    MappedIdentity {
        source: identity.clone(),
        adapter: DshAdapter::PiAiOpenAiCompatible {
            route_id: route_id_for(&identity.provider_id),
        },
        dsh_reasoning_effort: None,
        permission_mode,
        disclosures,
    }
}

fn dialect_label(protocol: WireProtocol) -> &'static str {
    match protocol {
        WireProtocol::ChatCompletions => "OpenAI Chat Completions",
        WireProtocol::Responses => "OpenAI Responses",
        WireProtocol::AnthropicMessages => "Anthropic Messages",
    }
}

fn yaml_str(value: &str) -> String {
    // Single-quoted YAML scalar: only `'` needs escaping.
    format!("'{}'", value.replace('\'', "''"))
}

/// Render the DSH `--patch` overlay for a mapped identity. Deterministic for
/// a given identity so its SHA-256 can detect drift.
pub(crate) fn render_overlay(mapped: &MappedIdentity) -> Option<String> {
    let src = &mapped.source;
    let mut out = String::new();
    out.push_str("# DeepSeek Harness connected through Codewhale.\n");
    out.push_str("# Generated by `codewhale integrations dsh connect`; do not edit by hand.\n");
    out.push_str("# Identity only: no API key, token, or credential document is written here.\n");
    out.push_str(&format!(
        "# codewhale.provider={} codewhale.model={} codewhale.workspace={}\n",
        src.provider_id, src.model, src.workspace
    ));
    match &mapped.adapter {
        DshAdapter::DeepseekNative => {
            out.push_str("- id: agent-default-model\n");
            out.push_str("  name: '@deepseek-ai/dsh-agent-default-model'\n");
            out.push_str("  config:\n");
            out.push_str("    provider: deepseek-official\n");
            out.push_str(&format!("    model: {}\n", yaml_str(&src.model)));
            out.push_str("- id: llm-deepseek\n");
            out.push_str("  name: '@deepseek-ai/dsh-llm-deepseek'\n");
            out.push_str("  config:\n");
            out.push_str(&format!("    baseURL: {}\n", yaml_str(&src.base_url)));
            if let Some(effort) = mapped.dsh_reasoning_effort.as_deref() {
                out.push_str(&format!("    reasoningEffort: {effort}\n"));
            }
            out.push_str("    models:\n");
            out.push_str(&format!("      - id: {}\n", yaml_str(&src.model)));
            out.push_str(&format!("        name: {}\n", yaml_str(&src.model)));
        }
        DshAdapter::PiAiOpenAiCompatible { route_id } => {
            out.push_str("- id: agent-default-model\n");
            out.push_str("  name: '@deepseek-ai/dsh-agent-default-model'\n");
            out.push_str("  config:\n");
            out.push_str(&format!("    provider: {}\n", yaml_str(route_id)));
            out.push_str(&format!("    model: {}\n", yaml_str(&src.model)));
            out.push_str("- id: llm-pi-ai\n");
            out.push_str("  name: '@deepseek-ai/dsh-llm-pi-ai'\n");
            out.push_str("  config:\n");
            out.push_str("    providers:\n");
            out.push_str(&format!("      {}:\n", yaml_str(route_id)));
            out.push_str(&format!(
                "        displayName: {}\n",
                yaml_str(&format!("{} (via Codewhale)", src.provider_label))
            ));
            if !src.keyless_local
                && let Some(env) = src.api_key_env.as_deref()
            {
                out.push_str(&format!("        apiKeyEnv: {}\n", yaml_str(env)));
            }
            out.push_str(&format!("        api: {}\n", pi_ai_api_for(src.protocol)));
            out.push_str(&format!("        baseURL: {}\n", yaml_str(&src.base_url)));
            out.push_str("        models:\n");
            out.push_str(&format!("          - id: {}\n", yaml_str(&src.model)));
            out.push_str(&format!("            name: {}\n", yaml_str(&src.model)));
        }
        DshAdapter::Unsupported { .. } => return None,
    }
    Some(out)
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
