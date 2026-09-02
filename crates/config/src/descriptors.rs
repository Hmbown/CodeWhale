//! OMP-style provider descriptors: how to talk to a host.
//!
//! Model ids are **not** compiled here. A descriptor names the wire, URL, env
//! var, and whether authenticated `GET /v1/models` is the catalog authority
//! for that host. Offerings come from the Codewhale catalog layers and live
//! provider `/models` refreshes.

use std::sync::OnceLock;

use serde::Deserialize;

const DESCRIPTORS_JSON: &str = include_str!("../assets/provider_descriptors.json");

/// How this host's model list is discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DescriptorDiscovery {
    /// Authenticated `GET {base_url}/models` is authoritative for this credential.
    ModelsEndpoint,
    /// No live discovery; only catalog/config rows.
    None,
}

/// Transport used to send turns. Not a brand enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DescriptorWire {
    OpenaiCompatible,
    AnthropicMessages,
}

#[derive(Debug, Deserialize)]
struct DescriptorFile {
    descriptors: Vec<ProviderDescriptor>,
}

/// Data row describing a hosted OpenAI-compatible (or Anthropic Messages) gateway.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ProviderDescriptor {
    pub id: String,
    pub label: String,
    pub wire: DescriptorWire,
    pub base_url: String,
    pub api_key_env: String,
    pub default_model: String,
    pub discovery: DescriptorDiscovery,
    #[serde(default)]
    pub docs_url: Option<String>,
    #[serde(default)]
    pub credential_url: Option<String>,
    #[serde(default)]
    pub guidance: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

impl ProviderDescriptor {
    #[must_use]
    pub fn matches(&self, needle: &str) -> bool {
        let needle = needle.trim().to_ascii_lowercase().replace('_', "-");
        if needle.is_empty() {
            return false;
        }
        self.id == needle
            || self
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(&needle))
    }
}

static DESCRIPTORS: OnceLock<Vec<ProviderDescriptor>> = OnceLock::new();

/// Bundled compatible-host descriptors. Panics only if the committed JSON is invalid.
#[must_use]
pub fn bundled_provider_descriptors() -> &'static [ProviderDescriptor] {
    DESCRIPTORS
        .get_or_init(|| {
            let file: DescriptorFile = serde_json::from_str(DESCRIPTORS_JSON)
                .expect("committed provider_descriptors.json must parse");
            file.descriptors
        })
        .as_slice()
}

#[must_use]
pub fn provider_descriptor(id: &str) -> Option<&'static ProviderDescriptor> {
    bundled_provider_descriptors()
        .iter()
        .find(|descriptor| descriptor.matches(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptors_parse_and_command_code_is_a_row_not_a_kind() {
        let rows = bundled_provider_descriptors();
        assert!(
            rows.len() >= 6,
            "expected compatible hosts plus command-code and dashscope"
        );
        for row in rows {
            assert!(row.base_url.starts_with("https://"), "{}", row.id);
            assert!(!row.api_key_env.is_empty(), "{}", row.id);
            assert!(!row.default_model.is_empty(), "{}", row.id);
            assert_eq!(row.discovery, DescriptorDiscovery::ModelsEndpoint);
            assert_eq!(row.wire, DescriptorWire::OpenaiCompatible);
        }
        let cmd = provider_descriptor("command-code").expect("command-code");
        assert_eq!(cmd.base_url, "https://api.commandcode.ai/provider/v1");
        assert_eq!(cmd.api_key_env, "COMMAND_CODE_API_KEY");
        assert_eq!(
            provider_descriptor("cmd-code").map(|row| row.id.as_str()),
            Some("command-code")
        );
        // Alibaba Model Studio is a data-driven row: live /v1/models is the
        // Qwen model authority, never a compiled roster.
        let dashscope = provider_descriptor("dashscope").expect("dashscope");
        assert_eq!(
            dashscope.base_url,
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1"
        );
        assert_eq!(dashscope.api_key_env, "DASHSCOPE_API_KEY");
        assert_eq!(
            provider_descriptor("qwen").map(|row| row.id.as_str()),
            Some("dashscope"),
            "the founder's `qwen` name resolves to the DashScope row"
        );
    }

    #[test]
    fn descriptors_do_not_embed_model_rosters() {
        let raw = DESCRIPTORS_JSON;
        assert!(
            !raw.contains("moonshotai/Kimi-K2.7-Code"),
            "do not compile a Baseten/Kimi roster into descriptors"
        );
        assert!(
            !raw.contains("openai/gpt-oss-120b"),
            "do not compile a Groq roster into descriptors"
        );
    }
}
