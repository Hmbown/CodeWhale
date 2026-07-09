//! Models.dev catalog schema and helpers.
//!
//! Models.dev is the upstream taxonomy CodeWhale should use for model facts,
//! provider offerings, pricing, limits, and capabilities. This module is
//! intentionally network-free: callers provide JSON from a bundled snapshot,
//! live refresh, or tests. Runtime fetch/cache policy belongs above this layer.
//!
//! The important boundary is the same one Models.dev uses:
//! - `models` are provider-agnostic model facts.
//! - `providers.*.models` are provider-scoped wire offerings.
//!
//! A provider row may inline inherited facts without exposing a canonical
//! `base_model` link. CodeWhale must preserve that distinction instead of
//! inferring canonical ownership from wire IDs or namespace prefixes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::route::{ModelId, ProviderId, ProviderModelOffering, RouteLimits, WireModelId};

/// Provider catalog endpoint used by Models.dev.
pub const MODELS_DEV_API_URL: &str = "https://models.dev/api.json";
/// Provider-agnostic model metadata endpoint used by Models.dev.
pub const MODELS_DEV_MODELS_URL: &str = "https://models.dev/models.json";
/// Combined `{ models, providers }` endpoint used by Models.dev.
pub const MODELS_DEV_CATALOG_URL: &str = "https://models.dev/catalog.json";

/// Combined Models.dev catalog payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelsDevCatalog {
    /// Provider-agnostic model facts, keyed by canonical model id.
    #[serde(default)]
    pub models: BTreeMap<String, ModelsDevModel>,
    /// Provider-scoped catalogs, keyed by provider id.
    #[serde(default)]
    pub providers: BTreeMap<String, ModelsDevProvider>,
}

impl ModelsDevCatalog {
    /// Parse a Models.dev combined catalog JSON payload.
    ///
    /// # Errors
    /// Returns a serde error when the input is not valid Models.dev JSON.
    pub fn parse_json(raw: &str) -> serde_json::Result<Self> {
        serde_json::from_str(raw)
    }

    /// Look up provider-agnostic model facts by canonical model id.
    #[must_use]
    pub fn model(&self, model_id: &str) -> Option<&ModelsDevModel> {
        self.models.get(model_id.trim())
    }

    /// Look up a provider catalog by provider id.
    #[must_use]
    pub fn provider(&self, provider_id: &str) -> Option<&ModelsDevProvider> {
        self.providers.get(provider_id.trim())
    }

    /// Look up a provider-scoped wire model row.
    #[must_use]
    pub fn provider_model(
        &self,
        provider_id: &str,
        wire_model_id: &str,
    ) -> Option<&ModelsDevProviderModel> {
        self.provider(provider_id)?.models.get(wire_model_id.trim())
    }

    /// Build a route offering from a provider-scoped Models.dev row.
    ///
    /// The canonical model is set only when the row carries an explicit
    /// `base_model` id. Generated Models.dev JSON often inlines inherited facts
    /// without that link, so callers must not guess one from a prefix.
    #[must_use]
    pub fn provider_offering(
        &self,
        provider_id: &str,
        wire_model_id: &str,
    ) -> Option<ProviderModelOffering> {
        let provider_key = provider_id.trim();
        let provider = self.provider(provider_key)?;
        let model = provider.models.get(wire_model_id.trim())?;
        let provider_id = provider.effective_id(provider_key);
        Some(ProviderModelOffering {
            provider: ProviderId::from(provider_id),
            canonical_model: model.base_model.clone().map(ModelId::from),
            wire_model_id: WireModelId::from(model.id.clone()),
            endpoint_key: "chat".to_string(),
            default_for_provider: model.default_for_provider,
            limits: model
                .limit
                .as_ref()
                .map(RouteLimits::from)
                .unwrap_or_default(),
            pricing: crate::pricing::route_pricing_sku_from_cost(model.cost.as_ref()),
        })
    }

    /// Build route offerings for every normal text-chat model served by a
    /// provider.
    ///
    /// Non-chat rows (for example TTS/audio-only offerings) stay in the parsed
    /// catalog but are excluded from route resolution lists.
    #[must_use]
    pub fn provider_offerings(&self, provider_id: &str) -> Option<Vec<ProviderModelOffering>> {
        let provider_key = provider_id.trim();
        let provider = self.provider(provider_key)?;
        let provider_id = provider.effective_id(provider_key);
        Some(
            provider
                .models
                .values()
                .filter(|model| model.supports_text_chat())
                .map(|model| ProviderModelOffering {
                    provider: ProviderId::from(provider_id.clone()),
                    canonical_model: model.base_model.clone().map(ModelId::from),
                    wire_model_id: WireModelId::from(model.id.clone()),
                    endpoint_key: "chat".to_string(),
                    default_for_provider: model.default_for_provider,
                    limits: model
                        .limit
                        .as_ref()
                        .map(RouteLimits::from)
                        .unwrap_or_default(),
                    pricing: crate::pricing::route_pricing_sku_from_cost(model.cost.as_ref()),
                })
                .collect(),
        )
    }
}

/// Provider-agnostic model facts from `models.json` / `catalog.models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelsDevModel {
    /// Canonical Models.dev model id, such as `zhipuai/glm-5.2`.
    #[serde(default)]
    pub id: String,
    /// Human-friendly model name.
    #[serde(default)]
    pub name: Option<String>,
    /// Model family, such as `glm`, `gpt`, or `claude`.
    #[serde(default)]
    pub family: Option<String>,
    /// Whether attachments are accepted.
    #[serde(default)]
    pub attachment: Option<bool>,
    /// Whether the model supports reasoning.
    #[serde(default)]
    pub reasoning: Option<bool>,
    /// Whether tool calling is supported.
    #[serde(default)]
    pub tool_call: Option<bool>,
    /// Whether structured output is supported.
    #[serde(default)]
    pub structured_output: Option<bool>,
    /// Whether temperature is supported.
    #[serde(default)]
    pub temperature: Option<bool>,
    /// Whether weights are open.
    #[serde(default)]
    pub open_weights: Option<bool>,
    /// Token limits.
    #[serde(default)]
    pub limit: Option<ModelsDevLimit>,
    /// Input/output modalities.
    #[serde(default)]
    pub modalities: Option<ModelsDevModalities>,
}

impl ModelsDevModel {
    /// True when the model can be used for normal text chat.
    #[must_use]
    pub fn supports_text_chat(&self) -> bool {
        supports_text_chat(self.modalities.as_ref())
    }
}

/// Provider-scoped model row from `api.json` / `catalog.providers.*.models`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelsDevProviderModel {
    /// Provider wire model id.
    #[serde(default)]
    pub id: String,
    /// Optional explicit canonical model link from source TOML.
    #[serde(default)]
    pub base_model: Option<String>,
    /// Human-friendly model name.
    #[serde(default)]
    pub name: Option<String>,
    /// Model family as exposed for this provider row.
    #[serde(default)]
    pub family: Option<String>,
    /// Whether this is the provider's default model in a CodeWhale snapshot.
    #[serde(default, alias = "default")]
    pub default_for_provider: bool,
    /// Whether attachments are accepted.
    #[serde(default)]
    pub attachment: Option<bool>,
    /// Whether the model supports reasoning.
    #[serde(default)]
    pub reasoning: Option<bool>,
    /// Flexible reasoning-control metadata.
    #[serde(default)]
    pub reasoning_options: Vec<serde_json::Value>,
    /// Whether tool calling is supported.
    #[serde(default)]
    pub tool_call: Option<bool>,
    /// Whether structured output is supported.
    #[serde(default)]
    pub structured_output: Option<bool>,
    /// Whether temperature is supported.
    #[serde(default)]
    pub temperature: Option<bool>,
    /// Whether weights are open through this offering.
    #[serde(default)]
    pub open_weights: Option<bool>,
    /// Token limits for this provider offering.
    #[serde(default)]
    pub limit: Option<ModelsDevLimit>,
    /// Input/output modalities for this provider offering.
    #[serde(default)]
    pub modalities: Option<ModelsDevModalities>,
    /// Provider-scoped pricing.
    #[serde(default)]
    pub cost: Option<ModelsDevCost>,
    /// Interleaved reasoning field hints.
    #[serde(default)]
    pub interleaved: Option<ModelsDevInterleaved>,
}

impl ModelsDevProviderModel {
    /// True when the provider offering can be used for normal text chat.
    #[must_use]
    pub fn supports_text_chat(&self) -> bool {
        supports_text_chat(self.modalities.as_ref())
    }
}

/// Provider row from Models.dev.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelsDevProvider {
    /// Provider id, such as `zai`, `zhipuai`, or `openrouter`.
    #[serde(default)]
    pub id: String,
    /// Human-friendly provider name.
    #[serde(default)]
    pub name: Option<String>,
    /// Default API base URL, if published.
    #[serde(default)]
    pub api: Option<String>,
    /// AI SDK package identifier, useful as a protocol hint.
    #[serde(default)]
    pub npm: Option<String>,
    /// Documentation URL, if published.
    #[serde(default)]
    pub doc: Option<String>,
    /// Environment variable names for credentials.
    #[serde(default)]
    pub env: Vec<String>,
    /// Provider-scoped wire model rows.
    #[serde(default)]
    pub models: BTreeMap<String, ModelsDevProviderModel>,
}

impl ModelsDevProvider {
    /// Resolve the effective provider id for this row.
    ///
    /// Models.dev snapshots usually repeat the catalog key in the `id` field,
    /// but generated JSON can omit it. Fall back to the catalog key so callers
    /// never emit an empty [`ProviderId`].
    #[must_use]
    fn effective_id(&self, provider_key: &str) -> String {
        if self.id.trim().is_empty() {
            provider_key.to_string()
        } else {
            self.id.trim().to_string()
        }
    }
}

/// Token limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelsDevLimit {
    #[serde(default)]
    pub context: Option<u64>,
    #[serde(default)]
    pub input: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
}

impl From<&ModelsDevLimit> for RouteLimits {
    fn from(limit: &ModelsDevLimit) -> Self {
        Self {
            context_tokens: limit.context,
            input_tokens: limit.input,
            output_tokens: limit.output,
        }
    }
}

/// Input/output modalities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelsDevModalities {
    #[serde(default)]
    pub input: Vec<String>,
    #[serde(default)]
    pub output: Vec<String>,
}

/// Provider-scoped cost fields. Values are per million tokens unless a future
/// Models.dev row specifies a richer tiering object in fields CodeWhale does
/// not yet model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ModelsDevCost {
    #[serde(default)]
    pub input: Option<f64>,
    #[serde(default)]
    pub output: Option<f64>,
    #[serde(default)]
    pub cache_read: Option<f64>,
    #[serde(default)]
    pub cache_write: Option<f64>,
}

/// Interleaved reasoning metadata from a Models.dev provider row.
///
/// Live Models.dev uses two shapes for this field, verified against
/// `https://models.dev/catalog.json` on 2026-07-07:
///
/// - a bare boolean (`interleaved: true`) on ~32 provider rows, signalling the
///   provider supports interleaved reasoning without naming a wire field, and
/// - an object (`interleaved: { "field": "reasoning_content" }`) on the
///   majority of rows, naming the wire field that carries reasoning deltas.
///
/// Modeling only the object shape made `serde_json::from_str::<ModelsDevCatalog>`
/// reject every boolean row before the live catalog could be used at all
/// (#4185). This untagged enum accepts both shapes while preserving the `field`
/// hint whenever the object form supplies one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ModelsDevInterleaved {
    /// Boolean form: `interleaved: true` / `interleaved: false`.
    Enabled(bool),
    /// Object form: `interleaved: { "field": "reasoning_content" }`.
    ///
    /// `field` stays optional so an empty or partial object still parses, and
    /// unknown sibling keys are ignored rather than rejected.
    Field {
        #[serde(default)]
        field: Option<String>,
    },
}

impl ModelsDevInterleaved {
    /// Whether interleaved reasoning is enabled for this row.
    ///
    /// The boolean form reports its literal value. The object form is treated as
    /// enabled because upstream only emits the object (naming a wire field) for
    /// interleaved-capable rows.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        match self {
            Self::Enabled(enabled) => *enabled,
            Self::Field { .. } => true,
        }
    }

    /// The provider wire field carrying reasoning deltas, when upstream names
    /// one.
    ///
    /// Only the object form supplies this; the boolean form returns `None`.
    #[must_use]
    pub fn field(&self) -> Option<&str> {
        match self {
            Self::Enabled(_) => None,
            Self::Field { field } => field.as_deref(),
        }
    }
}

fn supports_text_chat(modalities: Option<&ModelsDevModalities>) -> bool {
    let Some(modalities) = modalities else {
        return true;
    };
    // Treat an empty modality list the same as absent metadata. An incomplete
    // catalog snapshot can deserialize to `Some({ input: [], output: [] })`,
    // and `Iterator::any` over an empty slice is `false` — without this guard
    // such rows would be silently dropped from chat offerings even though the
    // `None` branch above defaults them to chat-capable. Only an explicitly
    // populated, non-text list excludes the row.
    let input_ok = modalities.input.is_empty()
        || modalities
            .input
            .iter()
            .any(|modality| modality.eq_ignore_ascii_case("text"));
    let output_ok = modalities.output.is_empty()
        || modalities
            .output
            .iter()
            .any(|modality| modality.eq_ignore_ascii_case("text"));
    input_ok && output_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLM_FIXTURE: &str = r#"{
      "models": {
        "zhipuai/glm-5.2": {
          "id": "zhipuai/glm-5.2",
          "name": "GLM-5.2",
          "family": "glm",
          "reasoning": true,
          "tool_call": true,
          "structured_output": true,
          "modalities": { "input": ["text"], "output": ["text"] },
          "limit": { "context": 1000000, "output": 131072 },
          "open_weights": true
        }
      },
      "providers": {
        "zhipuai": {
          "id": "zhipuai",
          "name": "Zhipu AI",
          "api": "https://open.bigmodel.cn/api/paas/v4",
          "npm": "@ai-sdk/openai-compatible",
          "env": ["ZHIPU_API_KEY"],
          "models": {
            "glm-5.2": {
              "id": "glm-5.2",
              "name": "GLM-5.2",
              "family": "glm",
              "reasoning": true,
              "reasoning_options": [{ "type": "effort", "values": ["high", "max"] }],
              "tool_call": true,
              "structured_output": true,
              "modalities": { "input": ["text"], "output": ["text"] },
              "limit": { "context": 1000000, "output": 131072 },
              "cost": { "input": 1.4, "output": 4.4, "cache_read": 0.26 }
            }
          }
        },
        "zai": {
          "id": "zai",
          "name": "Z.AI",
          "api": "https://api.z.ai/api/paas/v4",
          "npm": "@ai-sdk/openai-compatible",
          "env": ["ZHIPU_API_KEY"],
          "models": {
            "glm-5.2": {
              "id": "glm-5.2",
              "family": "glm",
              "reasoning": true,
              "tool_call": true,
              "modalities": { "input": ["text"], "output": ["text"] },
              "cost": { "input": 1.4, "output": 4.4 }
            }
          }
        }
      }
    }"#;

    #[test]
    fn parses_models_dev_catalog_layers_without_joining_by_prefix() {
        let catalog = ModelsDevCatalog::parse_json(GLM_FIXTURE).expect("fixture parses");

        let canonical = catalog.model("zhipuai/glm-5.2").expect("canonical model");
        assert_eq!(canonical.family.as_deref(), Some("glm"));
        assert_eq!(
            canonical.limit.as_ref().and_then(|limit| limit.context),
            Some(1_000_000)
        );
        assert!(canonical.supports_text_chat());

        let provider = catalog.provider("zhipuai").expect("provider");
        assert_eq!(
            provider.api.as_deref(),
            Some("https://open.bigmodel.cn/api/paas/v4")
        );
        assert_eq!(provider.npm.as_deref(), Some("@ai-sdk/openai-compatible"));
        assert_eq!(provider.env, ["ZHIPU_API_KEY"]);

        let offering = catalog
            .provider_model("zhipuai", "glm-5.2")
            .expect("provider model");
        assert_eq!(offering.id, "glm-5.2");
        assert_eq!(offering.reasoning, Some(true));
        assert_eq!(
            offering.cost.as_ref().and_then(|cost| cost.cache_read),
            Some(0.26)
        );
        assert!(offering.supports_text_chat());
        assert_eq!(
            offering.base_model, None,
            "generated JSON does not prove a canonical join"
        );

        let route_offering = catalog
            .provider_offering("zhipuai", "glm-5.2")
            .expect("route offering");
        assert_eq!(route_offering.limits.context_tokens, Some(1_000_000));
        assert_eq!(route_offering.limits.output_tokens, Some(131_072));
    }

    #[test]
    fn provider_offering_preserves_wire_id_without_inferred_canonical_model() {
        let catalog = ModelsDevCatalog::parse_json(GLM_FIXTURE).expect("fixture parses");
        let offering = catalog
            .provider_offering("zai", "glm-5.2")
            .expect("offering");

        assert_eq!(offering.provider.as_str(), "zai");
        assert_eq!(offering.wire_model_id.as_str(), "glm-5.2");
        assert_eq!(offering.canonical_model, None);
        assert_eq!(offering.endpoint_key, "chat");
    }

    #[test]
    fn provider_offering_uses_explicit_base_model_when_present() {
        let raw = r#"{
          "providers": {
            "openrouter": {
              "id": "openrouter",
              "models": {
                "z-ai/glm-5.2": {
                  "id": "z-ai/glm-5.2",
                  "base_model": "zhipuai/glm-5.2"
                }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("fixture parses");
        let offering = catalog
            .provider_offering("openrouter", "z-ai/glm-5.2")
            .expect("offering");

        assert_eq!(
            offering.canonical_model.as_ref().map(ModelId::as_str),
            Some("zhipuai/glm-5.2")
        );
        assert_eq!(offering.wire_model_id.as_str(), "z-ai/glm-5.2");
    }

    #[test]
    fn provider_offerings_emit_chat_rows_and_skip_non_text_outputs() {
        let raw = r#"{
          "providers": {
            "zai": {
              "models": {
                "glm-5.2": {
                  "id": "glm-5.2",
                  "base_model": "zhipuai/glm-5.2",
                  "default": true,
                  "modalities": { "input": ["text"], "output": ["text"] }
                },
                "glm-voice": {
                  "id": "glm-voice",
                  "modalities": { "input": ["text"], "output": ["audio"] }
                }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("fixture parses");
        let offerings = catalog
            .provider_offerings("zai")
            .expect("provider offerings");

        assert_eq!(offerings.len(), 1);
        assert_eq!(offerings[0].provider.as_str(), "zai");
        assert_eq!(offerings[0].wire_model_id.as_str(), "glm-5.2");
        assert_eq!(
            offerings[0].canonical_model.as_ref().map(ModelId::as_str),
            Some("zhipuai/glm-5.2")
        );
        assert!(offerings[0].default_for_provider);
    }

    #[test]
    fn non_text_output_is_not_a_chat_model() {
        let model = ModelsDevProviderModel {
            id: "mimo-v2.5-tts".to_string(),
            modalities: Some(ModelsDevModalities {
                input: vec!["text".to_string()],
                output: vec!["audio".to_string()],
            }),
            ..Default::default()
        };

        assert!(!model.supports_text_chat());
    }

    #[test]
    fn empty_modalities_struct_is_chat_capable() {
        // `"modalities": {}` deserializes to Some(empty); it must default to
        // chat-capable just like absent modality metadata (the None branch),
        // otherwise rows from incomplete snapshots are silently dropped.
        let provider_model = ModelsDevProviderModel {
            modalities: Some(ModelsDevModalities::default()),
            ..Default::default()
        };
        assert!(provider_model.supports_text_chat());

        let canonical = ModelsDevModel {
            modalities: Some(ModelsDevModalities::default()),
            ..Default::default()
        };
        assert!(canonical.supports_text_chat());

        // A list populated with only non-text entries still excludes the row.
        let audio_only = ModelsDevProviderModel {
            modalities: Some(ModelsDevModalities {
                input: vec!["text".to_string()],
                output: vec!["audio".to_string()],
            }),
            ..Default::default()
        };
        assert!(!audio_only.supports_text_chat());
    }

    #[test]
    fn interleaved_boolean_true_parses_and_reports_enabled() {
        // 32 live provider rows (e.g. `vercel`, `amazon-bedrock`) send
        // `interleaved: true`; the object-only model rejected all of them.
        let raw = r#"{
          "providers": {
            "vercel": {
              "models": {
                "zai/glm-4.7": { "id": "zai/glm-4.7", "interleaved": true }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("boolean interleaved parses");
        let model = catalog
            .provider_model("vercel", "zai/glm-4.7")
            .expect("provider model");
        let interleaved = model.interleaved.as_ref().expect("interleaved present");
        assert_eq!(interleaved, &ModelsDevInterleaved::Enabled(true));
        assert!(interleaved.is_enabled());
        assert_eq!(interleaved.field(), None);
    }

    #[test]
    fn interleaved_boolean_false_parses_and_reports_disabled() {
        let raw = r#"{
          "providers": {
            "custom": {
              "models": {
                "house-model": { "id": "house-model", "interleaved": false }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("boolean interleaved parses");
        let model = catalog
            .provider_model("custom", "house-model")
            .expect("provider model");
        let interleaved = model.interleaved.as_ref().expect("interleaved present");
        assert_eq!(interleaved, &ModelsDevInterleaved::Enabled(false));
        assert!(!interleaved.is_enabled());
        assert_eq!(interleaved.field(), None);
    }

    #[test]
    fn interleaved_object_form_preserves_field_metadata() {
        // The majority of live rows use `{ "field": "reasoning_content" }`; the
        // fix must keep parsing them and surface the named wire field.
        let raw = r#"{
          "providers": {
            "alibaba-cn": {
              "models": {
                "glm-5.2": {
                  "id": "glm-5.2",
                  "interleaved": { "field": "reasoning_content" }
                }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("object interleaved parses");
        let model = catalog
            .provider_model("alibaba-cn", "glm-5.2")
            .expect("provider model");
        let interleaved = model.interleaved.as_ref().expect("interleaved present");
        assert_eq!(interleaved.field(), Some("reasoning_content"));
        assert!(interleaved.is_enabled());
    }

    #[test]
    fn interleaved_object_tolerates_empty_and_unknown_keys() {
        // An empty object and an object with only unmodeled sibling keys must
        // still parse (object form, no named field) rather than erroring.
        let raw = r#"{
          "providers": {
            "custom": {
              "models": {
                "empty-obj": { "id": "empty-obj", "interleaved": {} },
                "future-obj": {
                  "id": "future-obj",
                  "interleaved": { "future_hint": "x" }
                }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("tolerant interleaved parses");

        let empty = catalog
            .provider_model("custom", "empty-obj")
            .and_then(|m| m.interleaved.clone())
            .expect("empty object interleaved present");
        assert_eq!(empty, ModelsDevInterleaved::Field { field: None });
        assert_eq!(empty.field(), None);
        assert!(empty.is_enabled());

        let future = catalog
            .provider_model("custom", "future-obj")
            .and_then(|m| m.interleaved.clone())
            .expect("future object interleaved present");
        assert_eq!(future.field(), None);
    }

    #[test]
    fn live_ish_mixed_interleaved_sample_deserializes() {
        // A representative slice of live `catalog.json`: boolean and object
        // interleaved rows side by side, plus an unmodeled top-level provider
        // key (`doc`) and an unmodeled model key to prove unknown upstream
        // fields are ignored safely. This is the acceptance "live-ish sample".
        let raw = r#"{
          "providers": {
            "amazon-bedrock": {
              "id": "amazon-bedrock",
              "doc": "https://docs.aws.amazon.com/bedrock/",
              "models": {
                "anthropic.claude-opus": {
                  "id": "anthropic.claude-opus",
                  "reasoning": true,
                  "interleaved": true,
                  "some_future_flag": 7,
                  "modalities": { "input": ["text"], "output": ["text"] }
                }
              }
            },
            "alibaba-cn": {
              "id": "alibaba-cn",
              "models": {
                "deepseek-v4-flash": {
                  "id": "deepseek-v4-flash",
                  "interleaved": { "field": "reasoning_content" },
                  "modalities": { "input": ["text"], "output": ["text"] }
                }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("live-ish sample parses");

        let bedrock = catalog
            .provider_model("amazon-bedrock", "anthropic.claude-opus")
            .expect("bedrock row");
        assert_eq!(
            bedrock.interleaved,
            Some(ModelsDevInterleaved::Enabled(true))
        );

        let alibaba = catalog
            .provider_model("alibaba-cn", "deepseek-v4-flash")
            .expect("alibaba row");
        assert_eq!(
            alibaba.interleaved.as_ref().and_then(|i| i.field()),
            Some("reasoning_content")
        );

        // Both rows still resolve as chat offerings; interleaved does not
        // interfere with route resolution.
        assert_eq!(
            catalog
                .provider_offerings("amazon-bedrock")
                .map(|rows| rows.len()),
            Some(1)
        );
    }

    #[test]
    fn provider_offerings_keep_rows_with_empty_modalities_object() {
        // End-to-end guard for the empty-modalities case at the offering layer:
        // a custom/local provider row with `"modalities": {}` must still emit a
        // chat offering rather than being filtered out of route resolution.
        let raw = r#"{
          "providers": {
            "custom": {
              "models": {
                "house-model": { "id": "house-model", "modalities": {} }
              }
            }
          }
        }"#;
        let catalog = ModelsDevCatalog::parse_json(raw).expect("fixture parses");
        let offerings = catalog
            .provider_offerings("custom")
            .expect("provider offerings");

        assert_eq!(offerings.len(), 1);
        assert_eq!(offerings[0].wire_model_id.as_str(), "house-model");
        // `id` was omitted on the provider row → effective id is the catalog key.
        assert_eq!(offerings[0].provider.as_str(), "custom");
    }
}
