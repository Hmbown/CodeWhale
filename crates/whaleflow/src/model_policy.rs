use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AgentType, ModelPolicy, WorkflowUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelRole {
    Planner,
    LeafReasoner,
    Implementer,
    Reviewer,
    Teacher,
    Student,
    JsonExtractor,
    StarlarkRepair,
}

impl From<AgentType> for ModelRole {
    fn from(agent_type: AgentType) -> Self {
        match agent_type {
            AgentType::General | AgentType::Explore | AgentType::ToolAgent => Self::LeafReasoner,
            AgentType::Plan => Self::Planner,
            AgentType::Review | AgentType::Verifier => Self::Reviewer,
            AgentType::Implementer => Self::Implementer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ModelCapabilities {
    #[serde(default)]
    pub tool_calls: bool,
    #[serde(default)]
    pub json_mode: bool,
    #[serde(default)]
    pub prompt_cache: bool,
    #[serde(default)]
    pub large_context: bool,
    #[serde(default)]
    pub streaming: bool,
}

impl ModelCapabilities {
    #[must_use]
    pub fn satisfies(self, required: Self) -> bool {
        (!required.tool_calls || self.tool_calls)
            && (!required.json_mode || self.json_mode)
            && (!required.prompt_cache || self.prompt_cache)
            && (!required.large_context || self.large_context)
            && (!required.streaming || self.streaming)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderModel {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub capabilities: ModelCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedModel {
    pub role: ModelRole,
    pub provider: String,
    pub model: String,
    pub capabilities: ModelCapabilities,
    pub source: ModelSelectionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSelectionSource {
    Primary,
    Fallback,
    RoleDefault,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderRegistry {
    models: BTreeMap<String, ProviderModel>,
    role_policies: BTreeMap<ModelRole, ModelPolicy>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_model(mut self, model: ProviderModel) -> Self {
        self.insert_model(model);
        self
    }

    pub fn with_role_policy(mut self, role: ModelRole, policy: ModelPolicy) -> Self {
        self.role_policies.insert(role, policy);
        self
    }

    pub fn insert_model(&mut self, model: ProviderModel) {
        self.models
            .insert(model_key(&model.provider, &model.model), model);
    }

    pub fn resolve_role(
        &self,
        role: ModelRole,
        policy: Option<&ModelPolicy>,
        required: ModelCapabilities,
    ) -> Result<ResolvedModel, ModelPolicyError> {
        let policy = match policy {
            Some(policy) => (policy, ModelSelectionSource::Primary),
            None => (
                self.role_policies
                    .get(&role)
                    .ok_or(ModelPolicyError::MissingPolicy { role })?,
                ModelSelectionSource::RoleDefault,
            ),
        };
        self.resolve_policy(role, policy.0, policy.1, required)
    }

    fn resolve_policy(
        &self,
        role: ModelRole,
        policy: &ModelPolicy,
        primary_source: ModelSelectionSource,
        required: ModelCapabilities,
    ) -> Result<ResolvedModel, ModelPolicyError> {
        let candidates = model_candidates(policy)?;
        let mut rejected = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let source = if index == 0 {
                primary_source
            } else {
                ModelSelectionSource::Fallback
            };
            let Some(model) = self
                .models
                .get(&model_key(&candidate.provider, &candidate.model))
            else {
                rejected.push(format!(
                    "{}/{}: unknown",
                    candidate.provider, candidate.model
                ));
                continue;
            };
            if model.capabilities.satisfies(required) {
                return Ok(ResolvedModel {
                    role,
                    provider: model.provider.clone(),
                    model: model.model.clone(),
                    capabilities: model.capabilities,
                    source,
                });
            }
            rejected.push(format!(
                "{}/{}: missing required capabilities",
                model.provider, model.model
            ));
        }
        Err(ModelPolicyError::NoCapableModel { role, rejected })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub role: ModelRole,
    pub prompt: String,
    #[serde(default)]
    pub require_json: bool,
    #[serde(default)]
    pub model_policy: ModelPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub text: String,
    #[serde(default)]
    pub usage: WorkflowUsage,
}

pub trait ModelProvider {
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn capabilities(&self) -> ModelCapabilities;
    fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ModelProviderError>;
}

#[derive(Debug, Clone)]
pub struct MockModelProvider {
    provider: String,
    model: String,
    capabilities: ModelCapabilities,
    response: CompletionResponse,
}

impl MockModelProvider {
    pub fn new(
        provider: impl Into<String>,
        model: impl Into<String>,
        capabilities: ModelCapabilities,
        response: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            capabilities,
            response: CompletionResponse {
                text: response.into(),
                usage: WorkflowUsage::default(),
            },
        }
    }
}

impl ModelProvider for MockModelProvider {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities
    }

    fn complete(
        &self,
        _request: &CompletionRequest,
    ) -> Result<CompletionResponse, ModelProviderError> {
        Ok(self.response.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelPolicyError {
    #[error("no model policy configured for role `{role:?}`")]
    MissingPolicy { role: ModelRole },
    #[error("model policy must include a model for role resolution")]
    MissingModel,
    #[error("fallback model `{model}` requires a provider when the primary policy has none")]
    MissingFallbackProvider { model: String },
    #[error("no configured model satisfies role `{role:?}` requirements: {rejected:?}")]
    NoCapableModel {
        role: ModelRole,
        rejected: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ModelProviderError {
    #[error("model provider `{provider}/{model}` failed: {reason}")]
    Failed {
        provider: String,
        model: String,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JsonRepairError {
    #[error("json parse failed before and after one repair pass: {reason}")]
    Parse { reason: String },
}

pub fn parse_json_with_repair<T: DeserializeOwned>(raw: &str) -> Result<T, JsonRepairError> {
    match serde_json::from_str(raw) {
        Ok(parsed) => Ok(parsed),
        Err(first) => {
            let repaired = repair_json_text_once(raw);
            serde_json::from_str(&repaired).map_err(|second| JsonRepairError::Parse {
                reason: format!("{first}; repair failed: {second}"),
            })
        }
    }
}

pub fn repair_json_text_once(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_fence = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|value| value.strip_suffix("```"))
        .map(str::trim)
        .unwrap_or(trimmed);
    let object = slice_json_payload(without_fence, '{', '}');
    let array = slice_json_payload(without_fence, '[', ']');
    object.or(array).unwrap_or(without_fence).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ModelCandidate {
    provider: String,
    model: String,
}

fn model_candidates(policy: &ModelPolicy) -> Result<Vec<ModelCandidate>, ModelPolicyError> {
    let mut candidates = Vec::new();
    let Some(primary_model) = policy.model.as_ref() else {
        return Err(ModelPolicyError::MissingModel);
    };
    candidates.push(candidate_from_model(
        policy.provider.as_deref(),
        primary_model,
    )?);
    for fallback in &policy.fallback_models {
        candidates.push(candidate_from_model(policy.provider.as_deref(), fallback)?);
    }
    Ok(candidates)
}

fn candidate_from_model(
    default_provider: Option<&str>,
    model: &str,
) -> Result<ModelCandidate, ModelPolicyError> {
    if let Some((provider, model)) = model.split_once('/') {
        return Ok(ModelCandidate {
            provider: provider.to_string(),
            model: model.to_string(),
        });
    }
    let Some(provider) = default_provider else {
        return Err(ModelPolicyError::MissingFallbackProvider {
            model: model.to_string(),
        });
    };
    Ok(ModelCandidate {
        provider: provider.to_string(),
        model: model.to_string(),
    })
}

fn model_key(provider: &str, model: &str) -> String {
    format!("{provider}/{model}")
}

fn slice_json_payload(raw: &str, open: char, close: char) -> Option<&str> {
    let start = raw.find(open)?;
    let end = raw.rfind(close)?;
    (end >= start).then_some(&raw[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(provider: &str, model: &str, capabilities: ModelCapabilities) -> ProviderModel {
        ProviderModel {
            provider: provider.to_string(),
            model: model.to_string(),
            capabilities,
        }
    }

    #[test]
    fn provider_capability_fallback() {
        let registry = ProviderRegistry::new()
            .with_model(model("mock", "plain", ModelCapabilities::default()))
            .with_model(model(
                "mock",
                "json",
                ModelCapabilities {
                    json_mode: true,
                    ..ModelCapabilities::default()
                },
            ));
        let policy = ModelPolicy {
            provider: Some("mock".to_string()),
            model: Some("plain".to_string()),
            fallback_models: vec!["json".to_string()],
        };

        let resolved = registry
            .resolve_role(
                ModelRole::JsonExtractor,
                Some(&policy),
                ModelCapabilities {
                    json_mode: true,
                    ..ModelCapabilities::default()
                },
            )
            .expect("fallback json model should satisfy the role");

        assert_eq!(resolved.model, "json");
        assert_eq!(resolved.source, ModelSelectionSource::Fallback);
    }

    #[test]
    fn role_default_policy_resolves_model() {
        let registry = ProviderRegistry::new()
            .with_model(model(
                "mock",
                "planner",
                ModelCapabilities {
                    large_context: true,
                    ..ModelCapabilities::default()
                },
            ))
            .with_role_policy(
                ModelRole::Planner,
                ModelPolicy {
                    provider: Some("mock".to_string()),
                    model: Some("planner".to_string()),
                    fallback_models: Vec::new(),
                },
            );

        let resolved = registry
            .resolve_role(
                ModelRole::Planner,
                None,
                ModelCapabilities {
                    large_context: true,
                    ..ModelCapabilities::default()
                },
            )
            .expect("role default should resolve");

        assert_eq!(resolved.role, ModelRole::Planner);
        assert_eq!(resolved.source, ModelSelectionSource::RoleDefault);
    }

    #[test]
    fn agent_type_maps_to_model_role() {
        assert_eq!(ModelRole::from(AgentType::Plan), ModelRole::Planner);
        assert_eq!(
            ModelRole::from(AgentType::Implementer),
            ModelRole::Implementer
        );
        assert_eq!(ModelRole::from(AgentType::Verifier), ModelRole::Reviewer);
    }

    #[test]
    fn json_repair_fallback() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct Payload {
            answer: String,
        }

        let parsed: Payload = parse_json_with_repair(
            r#"Here is the JSON:
```json
{"answer":"ok"}
```
"#,
        )
        .expect("repair should extract fenced JSON");

        assert_eq!(
            parsed,
            Payload {
                answer: "ok".to_string()
            }
        );
    }

    #[test]
    fn json_repair_fallback_fails_closed() {
        let err = parse_json_with_repair::<serde_json::Value>("not json")
            .expect_err("non-json text should fail closed");

        assert!(matches!(err, JsonRepairError::Parse { .. }));
    }

    #[test]
    fn mock_provider_returns_configured_response() {
        let provider = MockModelProvider::new(
            "mock",
            "fast",
            ModelCapabilities::default(),
            "mock response",
        );
        let request = CompletionRequest {
            role: ModelRole::LeafReasoner,
            prompt: "say something".to_string(),
            require_json: false,
            model_policy: ModelPolicy::default(),
        };

        let response = provider.complete(&request).expect("mock should respond");

        assert_eq!(provider.provider(), "mock");
        assert_eq!(provider.model(), "fast");
        assert_eq!(response.text, "mock response");
    }
}
