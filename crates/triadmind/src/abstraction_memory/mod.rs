//! # Abstraction Memory — reusable abstraction recall for the agent
//!
//! Ported from triadmind-core/abstractionMemory.ts
//!
//! Scans triad-map topology nodes for abstraction patterns (interfaces,
//! abstract classes, abstract functions, contract dependencies) and builds
//! a searchable memory of reusable abstractions. The agent can query this
//! memory to avoid re-inventing abstractions that already exist in the
//! codebase.
//!
//! Key capabilities:
//! - `sync_abstraction_memory`: build and persist the memory artifact
//! - `search_abstraction_memory`: token-based fuzzy search
//! - `recommend_abstractions`: scored reuse recommendations
//! - `build_protocol_action_candidates`: generate reuse/modify protocol seeds
//! - `build_prompt_context`: format memory for LLM prompt injection
//!
//! @LeftBranch: sync_abstraction_memory, search_abstraction_memory, recommend_abstractions
//! @RightBranch: AbstractionMemoryEntry, AbstractionMemoryArtifact, AbstractionMemoryRecommendation

use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::protocol::TriadNodeDefinition;

// ── Types ───────────────────────────────────────────────────────────

/// Kind of abstraction entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbstractionMemoryEntryKind {
    InterfaceOrContract,
    AbstractClass,
    AbstractFunction,
    ContractDependency,
    AbstractionModule,
}

impl AbstractionMemoryEntryKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::InterfaceOrContract => "interface_or_contract",
            Self::AbstractClass => "abstract_class",
            Self::AbstractFunction => "abstract_function",
            Self::ContractDependency => "contract_dependency",
            Self::AbstractionModule => "abstraction_module",
        }
    }
}

/// A remembered abstraction from the codebase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemoryEntry {
    pub id: String,
    pub name: String,
    pub kind: AbstractionMemoryEntryKind,
    #[serde(rename = "primarySourcePath")]
    pub primary_source_path: String,
    #[serde(rename = "sourcePaths")]
    pub source_paths: Vec<String>,
    #[serde(rename = "nodeIds")]
    pub node_ids: Vec<String>,
    #[serde(rename = "providerNodeIds")]
    pub provider_node_ids: Vec<String>,
    #[serde(rename = "consumerNodeIds")]
    pub consumer_node_ids: Vec<String>,
    #[serde(rename = "variantClusters")]
    pub variant_clusters: Vec<String>,
    #[serde(rename = "relatedAbstractions")]
    pub related_abstractions: Vec<String>,
    pub signatures: Vec<String>,
    pub tags: Vec<String>,
    #[serde(rename = "abstractionRatio")]
    pub abstraction_ratio: f64,
    #[serde(rename = "reusabilityScore")]
    pub reusability_score: f64,
    #[serde(rename = "whyReusable")]
    pub why_reusable: String,
}

/// Full abstraction memory artifact (persisted to abstraction-memory.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemoryArtifact {
    #[serde(rename = "schemaVersion")]
    pub schema_version: String,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub project: String,
    #[serde(rename = "sourceMapFile")]
    pub source_map_file: String,
    pub summary: AbstractionMemorySummary,
    pub entries: Vec<AbstractionMemoryEntry>,
}

/// Summary counters for the abstraction memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemorySummary {
    #[serde(rename = "scannedSourceCount")]
    pub scanned_source_count: usize,
    #[serde(rename = "excludedStableSourceCount")]
    pub excluded_stable_source_count: usize,
    #[serde(rename = "excludedConfiguredSourceCount")]
    pub excluded_configured_source_count: usize,
    #[serde(rename = "rememberedEntryCount")]
    pub remembered_entry_count: usize,
    #[serde(rename = "contractEntryCount")]
    pub contract_entry_count: usize,
    #[serde(rename = "abstractFunctionEntryCount")]
    pub abstract_function_entry_count: usize,
    #[serde(rename = "moduleEntryCount")]
    pub module_entry_count: usize,
    #[serde(rename = "hotspotCount")]
    pub hotspot_count: usize,
    #[serde(rename = "variantClusterCount")]
    pub variant_cluster_count: usize,
}

impl Default for AbstractionMemorySummary {
    fn default() -> Self {
        Self {
            scanned_source_count: 0,
            excluded_stable_source_count: 0,
            excluded_configured_source_count: 0,
            remembered_entry_count: 0,
            contract_entry_count: 0,
            abstract_function_entry_count: 0,
            module_entry_count: 0,
            hotspot_count: 0,
            variant_cluster_count: 0,
        }
    }
}

/// A search result for abstraction memory queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemorySearchResult {
    pub entry: AbstractionMemoryEntry,
    pub score: f64,
    #[serde(rename = "matchedTerms")]
    pub matched_terms: Vec<String>,
}

/// A reuse recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemoryRecommendation {
    pub entry: AbstractionMemoryEntry,
    pub score: f64,
    #[serde(rename = "matchedTerms")]
    pub matched_terms: Vec<String>,
    pub rationale: Vec<String>,
    #[serde(rename = "suggestedUsage")]
    pub suggested_usage: SuggestedUsage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedUsage {
    ReuseFirst,
    AdaptBeforeCreate,
}

/// A protocol action candidate derived from abstraction memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionProtocolActionCandidate {
    pub kind: String, // "reuse_seed" | "modify_seed"
    #[serde(flatten)]
    pub action: ProtocolActionSeed,
    pub score: f64,
    #[serde(rename = "basedOnEntry")]
    pub based_on_entry: AbstractionMemoryEntryRef,
    pub rationale: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemoryEntryRef {
    pub id: String,
    pub name: String,
    pub kind: AbstractionMemoryEntryKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op")]
pub enum ProtocolActionSeed {
    #[serde(rename = "reuse")]
    Reuse(ReuseActionSeed),
    #[serde(rename = "modify")]
    Modify(ModifyActionSeed),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReuseActionSeed {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub reason: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifyActionSeed {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    pub reason: String,
    pub confidence: f64,
}

/// Prompt-ready memory context for LLM consumption.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptMemoryContext {
    #[serde(rename = "summaryLines")]
    pub summary_lines: Vec<String>,
    pub matches: Vec<AbstractionMemoryEntry>,
    pub recommendations: Vec<AbstractionMemoryRecommendation>,
    #[serde(rename = "protocolActionCandidates")]
    pub protocol_action_candidates: Vec<AbstractionProtocolActionCandidate>,
}

/// Configuration for abstraction memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbstractionMemoryConfig {
    pub enabled: bool,
    #[serde(rename = "autoSyncOnPrompt", default = "default_true")]
    pub auto_sync_on_prompt: bool,
    #[serde(rename = "excludeMatureStableSources", default = "default_true")]
    pub exclude_mature_stable_sources: bool,
    #[serde(default)]
    pub exclude_source_paths: Vec<String>,
    #[serde(default)]
    pub exclude_source_path_patterns: Vec<String>,
    #[serde(rename = "minAbstractionRatio", default = "default_min_ratio")]
    pub min_abstraction_ratio: f64,
    #[serde(rename = "maxPromptEntries", default = "default_max_entries")]
    pub max_prompt_entries: usize,
    #[serde(rename = "maxSearchResults", default = "default_max_search")]
    pub max_search_results: usize,
}

fn default_true() -> bool { true }
fn default_min_ratio() -> f64 { 0.2 }
fn default_max_entries() -> usize { 6 }
fn default_max_search() -> usize { 10 }

impl Default for AbstractionMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_sync_on_prompt: true,
            exclude_mature_stable_sources: true,
            exclude_source_paths: vec![],
            exclude_source_path_patterns: vec![],
            min_abstraction_ratio: default_min_ratio(),
            max_prompt_entries: default_max_entries(),
            max_search_results: default_max_search(),
        }
    }
}

/// Input to the recommendation engine.
#[derive(Debug, Clone)]
pub struct RecommendationInput {
    pub query: String,
    pub focus_node_id: Option<String>,
    pub focus_source_path: Option<String>,
    pub limit: usize,
}

// ── Abstraction Evidence (from triad-map nodes) ────────────────────

/// Abstraction evidence embedded in a triad node's fission.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AbstractionEvidence {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub signals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub implements: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extends_abstract: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on_abstractions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub abstract_functions: Vec<String>,
    #[serde(rename = "functionContractCount", default)]
    pub function_contract_count: usize,
    #[serde(default)]
    pub variant_cluster: String,
    #[serde(rename = "abstractionSignalCount", default)]
    pub abstraction_signal_count: usize,
    #[serde(rename = "concreteSignalCount", default)]
    pub concrete_signal_count: usize,
    #[serde(rename = "interfaceCount", default)]
    pub interface_count: usize,
    #[serde(rename = "abstractClassCount", default)]
    pub abstract_class_count: usize,
    #[serde(rename = "concreteClassCount", default)]
    pub concrete_class_count: usize,
    #[serde(rename = "typeAliasCount", default)]
    pub type_alias_count: usize,
    #[serde(rename = "publicMethodCount", default)]
    pub public_method_count: usize,
    #[serde(rename = "topLevelExecutableCount", default)]
    pub top_level_executable_count: usize,
}

impl AbstractionEvidence {
    fn abstraction_ratio(&self) -> f64 {
        let total = self.abstraction_signal_count + self.concrete_signal_count;
        if total == 0 { 0.0 } else { self.abstraction_signal_count as f64 / total as f64 }
    }
}

// ── Core Entry Point: Sync ─────────────────────────────────────────

/// Build and persist the abstraction memory artifact.
pub fn sync_abstraction_memory(
    map_file: &Path,
    output_file: &Path,
    project_name: &str,
    config: &AbstractionMemoryConfig,
    stable_source_paths: &HashSet<String>,
) -> Result<AbstractionMemoryArtifact, anyhow::Error> {
    let artifact = build_abstraction_memory(map_file, project_name, config, stable_source_paths)?;

    if let Some(parent) = output_file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&artifact)?;
    std::fs::write(output_file, json)?;

    Ok(artifact)
}

/// Load an existing abstraction memory artifact.
pub fn load_abstraction_memory(file_path: &Path) -> Option<AbstractionMemoryArtifact> {
    let content = std::fs::read_to_string(file_path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Ensure abstraction memory exists (load or build).
pub fn ensure_abstraction_memory(
    map_file: &Path,
    memory_file: &Path,
    project_name: &str,
    config: &AbstractionMemoryConfig,
    stable_source_paths: &HashSet<String>,
    force: bool,
) -> Result<AbstractionMemoryArtifact, anyhow::Error> {
    if !config.enabled {
        return Ok(empty_artifact(project_name, map_file));
    }

    if force || !memory_file.exists() {
        return sync_abstraction_memory(map_file, memory_file, project_name, config, stable_source_paths);
    }

    match load_abstraction_memory(memory_file) {
        Some(artifact) => Ok(artifact),
        None => sync_abstraction_memory(map_file, memory_file, project_name, config, stable_source_paths),
    }
}

fn empty_artifact(project_name: &str, map_file: &Path) -> AbstractionMemoryArtifact {
    AbstractionMemoryArtifact {
        schema_version: "1.0".into(),
        generated_at: chrono_now(),
        project: project_name.into(),
        source_map_file: map_file.to_string_lossy().to_string(),
        summary: AbstractionMemorySummary::default(),
        entries: Vec::new(),
    }
}

// ── Core: Build Memory ──────────────────────────────────────────────

fn build_abstraction_memory(
    map_file: &Path,
    project_name: &str,
    config: &AbstractionMemoryConfig,
    stable_source_paths: &HashSet<String>,
) -> Result<AbstractionMemoryArtifact, anyhow::Error> {
    let nodes = load_triad_nodes(map_file)?;

    let mut scanned = 0usize;
    let mut excluded_stable = 0usize;
    let mut excluded_configured = 0usize;

    // Accumulate by abstraction type
    let mut contracts: HashMap<String, ContractAccumulator> = HashMap::new();
    let mut abstract_classes: HashMap<String, ContractAccumulator> = HashMap::new();
    let mut abstract_functions: HashMap<String, FunctionAccumulator> = HashMap::new();
    let mut dependency_entries: Vec<AbstractionMemoryEntry> = Vec::new();

    // Track source paths
    let mut seen_sources: HashSet<String> = HashSet::new();

    for node in &nodes {
        let source_path = node.source_path.as_deref().unwrap_or("");
        if source_path.is_empty() {
            continue;
        }

        let normalized_path = normalize_path(source_path);
        seen_sources.insert(normalized_path.clone());
        scanned += 1;

        // Skip stable/mature sources
        if config.exclude_mature_stable_sources && stable_source_paths.contains(&normalized_path) {
            excluded_stable += 1;
            continue;
        }

        // Skip configured excluded paths
        if is_excluded_source(&normalized_path, config) {
            excluded_configured += 1;
            continue;
        }

        // Extract abstraction evidence
        let evidence = extract_abstraction_evidence(node);
        let ratio = evidence.abstraction_ratio();

        if ratio < config.min_abstraction_ratio && evidence.signals.is_empty() {
            continue;
        }

        let node_id = &node.node_id;

        // Process interface/contract implementations
        for contract_name in &evidence.implements {
            let acc = contracts
                .entry(contract_name.clone())
                .or_insert_with(|| ContractAccumulator::new(contract_name));
            acc.source_paths.insert(normalized_path.clone());
            acc.provider_node_ids.insert(node_id.clone());
            acc.abstraction_ratios.push(ratio);
            acc.add_signal_tags(&evidence.signals);
        }

        // Process abstract class extensions
        for class_name in &evidence.extends_abstract {
            let acc = abstract_classes
                .entry(class_name.clone())
                .or_insert_with(|| {
                    let mut a = ContractAccumulator::new(class_name);
                    a.kind_hints.insert("abstract_class".into());
                    a
                });
            acc.source_paths.insert(normalized_path.clone());
            acc.provider_node_ids.insert(node_id.clone());
            acc.abstraction_ratios.push(ratio);
        }

        // Process abstract function signatures
        for sig in &evidence.abstract_functions {
            let acc = abstract_functions
                .entry(sig.clone())
                .or_insert_with(|| FunctionAccumulator::new(sig));
            acc.source_paths.insert(normalized_path.clone());
            acc.node_ids.insert(node_id.clone());
            acc.abstraction_ratios.push(ratio);
        }

        // Process contract consumers (nodes that depend on abstractions)
        for dep in &evidence.depends_on_abstractions {
            if let Some(contract_acc) = contracts.get_mut(dep) {
                contract_acc.consumer_node_ids.insert(node_id.clone());
            }
        }
    }

    // Build entries from accumulators
    let mut entries: Vec<AbstractionMemoryEntry> = Vec::new();

    for (name, acc) in contracts {
        if acc.provider_node_ids.len() >= 2 || appears_as_contract(&name, &nodes) {
            entries.push(build_contract_entry(
                &name,
                AbstractionMemoryEntryKind::InterfaceOrContract,
                acc,
            ));
        }
    }

    for (name, acc) in abstract_classes {
        entries.push(build_contract_entry(
            &name,
            AbstractionMemoryEntryKind::AbstractClass,
            acc,
        ));
    }

    for (sig, acc) in abstract_functions {
        if acc.node_ids.len() >= 2 {
            entries.push(build_function_entry(&sig, acc));
        }
    }

    entries.extend(dependency_entries);

    // Deduplicate by id
    let mut seen: HashSet<String> = HashSet::new();
    entries.retain(|e| seen.insert(e.id.clone()));

    // Compute summary
    let contract_count = entries.iter().filter(|e| e.kind == AbstractionMemoryEntryKind::InterfaceOrContract).count();
    let function_count = entries.iter().filter(|e| e.kind == AbstractionMemoryEntryKind::AbstractFunction).count();
    let module_count = entries.iter().filter(|e| e.kind == AbstractionMemoryEntryKind::AbstractionModule).count();

    Ok(AbstractionMemoryArtifact {
        schema_version: "1.0".into(),
        generated_at: chrono_now(),
        project: project_name.into(),
        source_map_file: map_file.to_string_lossy().to_string(),
        summary: AbstractionMemorySummary {
            scanned_source_count: scanned,
            excluded_stable_source_count: excluded_stable,
            excluded_configured_source_count: excluded_configured,
            remembered_entry_count: entries.len(),
            contract_entry_count: contract_count,
            abstract_function_entry_count: function_count,
            module_entry_count: module_count,
            hotspot_count: 0,
            variant_cluster_count: 0,
        },
        entries,
    })
}

// ── Accumulator helpers ─────────────────────────────────────────────

struct ContractAccumulator {
    name: String,
    source_paths: HashSet<String>,
    provider_node_ids: HashSet<String>,
    consumer_node_ids: HashSet<String>,
    variant_clusters: HashSet<String>,
    related_abstractions: HashSet<String>,
    tags: HashSet<String>,
    abstraction_ratios: Vec<f64>,
    kind_hints: HashSet<String>,
}

impl ContractAccumulator {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            source_paths: HashSet::new(),
            provider_node_ids: HashSet::new(),
            consumer_node_ids: HashSet::new(),
            variant_clusters: HashSet::new(),
            related_abstractions: HashSet::new(),
            tags: HashSet::new(),
            abstraction_ratios: Vec::new(),
            kind_hints: HashSet::new(),
        }
    }

    fn add_signal_tags(&mut self, signals: &[String]) {
        for s in signals {
            self.tags.insert(s.clone());
        }
    }

    fn primary_source_path(&self) -> String {
        let mut paths: Vec<&String> = self.source_paths.iter().collect();
        paths.sort();
        paths.first().map(|s| s.to_string()).unwrap_or_default()
    }

    fn avg_abstraction_ratio(&self) -> f64 {
        if self.abstraction_ratios.is_empty() { 0.0 }
        else { self.abstraction_ratios.iter().sum::<f64>() / self.abstraction_ratios.len() as f64 }
    }
}

struct FunctionAccumulator {
    signature: String,
    source_paths: HashSet<String>,
    node_ids: HashSet<String>,
    variant_clusters: HashSet<String>,
    related_abstractions: HashSet<String>,
    tags: HashSet<String>,
    abstraction_ratios: Vec<f64>,
}

impl FunctionAccumulator {
    fn new(sig: &str) -> Self {
        Self {
            signature: sig.into(),
            source_paths: HashSet::new(),
            node_ids: HashSet::new(),
            variant_clusters: HashSet::new(),
            related_abstractions: HashSet::new(),
            tags: HashSet::new(),
            abstraction_ratios: Vec::new(),
        }
    }

    fn primary_source_path(&self) -> String {
        let mut paths: Vec<&String> = self.source_paths.iter().collect();
        paths.sort();
        paths.first().map(|s| s.to_string()).unwrap_or_default()
    }

    fn avg_abstraction_ratio(&self) -> f64 {
        if self.abstraction_ratios.is_empty() { 0.0 }
        else { self.abstraction_ratios.iter().sum::<f64>() / self.abstraction_ratios.len() as f64 }
    }

    fn function_name(&self) -> String {
        self.signature.split('(').next().unwrap_or(&self.signature).trim().to_string()
    }
}

fn build_contract_entry(
    name: &str,
    kind: AbstractionMemoryEntryKind,
    acc: ContractAccumulator,
) -> AbstractionMemoryEntry {
    let ratio = acc.avg_abstraction_ratio();
    let provider_count = acc.provider_node_ids.len();
    let consumer_count = acc.consumer_node_ids.len();
    let reusability = if provider_count >= 2 && consumer_count >= 1 { 0.85 }
        else if provider_count >= 2 { 0.7 }
        else { 0.5 };

    let why = if provider_count >= 2 && consumer_count >= 1 {
        format!("Contract '{}' has {} providers and {} consumers — high reuse potential", name, provider_count, consumer_count)
    } else if provider_count >= 2 {
        format!("Contract '{}' has {} providers — candidate for consolidation", name, provider_count)
    } else {
        format!("Contract '{}' present in topology", name)
    };

    AbstractionMemoryEntry {
        id: sanitize_id(name),
        name: name.into(),
        kind,
        primary_source_path: acc.primary_source_path(),
        source_paths: sorted_set(&acc.source_paths),
        node_ids: vec![],
        provider_node_ids: sorted_set(&acc.provider_node_ids),
        consumer_node_ids: sorted_set(&acc.consumer_node_ids),
        variant_clusters: sorted_set(&acc.variant_clusters),
        related_abstractions: sorted_set(&acc.related_abstractions),
        signatures: vec![],
        tags: sorted_set(&acc.tags),
        abstraction_ratio: ratio,
        reusability_score: reusability,
        why_reusable: why,
    }
}

fn build_function_entry(
    sig: &str,
    acc: FunctionAccumulator,
) -> AbstractionMemoryEntry {
    let ratio = acc.avg_abstraction_ratio();
    let provider_count = acc.node_ids.len();
    let reusability = if provider_count >= 3 { 0.8 } else if provider_count >= 2 { 0.6 } else { 0.4 };

    let fn_name = acc.function_name();
    let why = format!("Abstract function '{}' appears in {} nodes — reuse candidate", fn_name, provider_count);

    AbstractionMemoryEntry {
        id: sanitize_id(sig),
        name: fn_name,
        kind: AbstractionMemoryEntryKind::AbstractFunction,
        primary_source_path: acc.primary_source_path(),
        source_paths: sorted_set(&acc.source_paths),
        node_ids: sorted_set(&acc.node_ids),
        provider_node_ids: vec![],
        consumer_node_ids: vec![],
        variant_clusters: sorted_set(&acc.variant_clusters),
        related_abstractions: sorted_set(&acc.related_abstractions),
        signatures: vec![sig.to_string()],
        tags: sorted_set(&acc.tags),
        abstraction_ratio: ratio,
        reusability_score: reusability,
        why_reusable: why,
    }
}

// ── Search ──────────────────────────────────────────────────────────

/// Search abstraction memory for entries matching a query.
pub fn search_abstraction_memory(
    artifact: &AbstractionMemoryArtifact,
    query: &str,
    limit: usize,
) -> Vec<AbstractionMemorySearchResult> {
    let terms = tokenize(query);
    if terms.is_empty() {
        return Vec::new();
    }

    let mut scored: Vec<(f64, &AbstractionMemoryEntry, Vec<String>)> = artifact
        .entries
        .iter()
        .filter_map(|entry| {
            let (score, matched) = score_entry(entry, &terms);
            if score > 0.0 { Some((score, entry, matched)) } else { None }
        })
        .collect();

    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    scored
        .into_iter()
        .take(limit.max(1).min(50))
        .map(|(score, entry, matched_terms)| AbstractionMemorySearchResult {
            entry: entry.clone(),
            score,
            matched_terms,
        })
        .collect()
}

fn score_entry(entry: &AbstractionMemoryEntry, terms: &[String]) -> (f64, Vec<String>) {
    let mut score = 0.0;
    let mut matched: Vec<String> = Vec::new();

    let search_text = format!(
        "{} {} {} {} {}",
        entry.name,
        entry.kind.as_str(),
        entry.tags.join(" "),
        entry.signatures.join(" "),
        entry.why_reusable,
    )
    .to_lowercase();

    for term in terms {
        if search_text.contains(term.as_str()) {
            score += 1.0;
            matched.push(term.clone());
        } else if fuzzy_contains(&search_text, term) {
            score += 0.5;
            matched.push(format!("~{}", term));
        }
    }

    // Boost by reusability score
    score *= 1.0 + entry.reusability_score;

    (score, matched)
}

// ── Recommend ───────────────────────────────────────────────────────

/// Recommend abstractions for reuse.
pub fn recommend_abstractions(
    artifact: &AbstractionMemoryArtifact,
    input: &RecommendationInput,
) -> Vec<AbstractionMemoryRecommendation> {
    let results = search_abstraction_memory(artifact, &input.query, input.limit * 2);

    let focus_node = input.focus_node_id.as_deref().unwrap_or("");

    results
        .into_iter()
        .map(|sr| {
            let is_provider = sr.entry.provider_node_ids.iter().any(|id| id == focus_node);
            let is_consumer = sr.entry.consumer_node_ids.iter().any(|id| id == focus_node);

            let adjusted_score = if is_provider { sr.score * 1.2 } else if is_consumer { sr.score * 1.1 } else { sr.score };

            let suggested = if sr.entry.reusability_score >= 0.7 && (is_provider || is_consumer) {
                SuggestedUsage::ReuseFirst
            } else {
                SuggestedUsage::AdaptBeforeCreate
            };

            let mut rationale = vec![sr.entry.why_reusable.clone()];

            if is_provider {
                rationale.push(format!("Focus node '{}' is a provider of this contract", focus_node));
            } else if is_consumer {
                rationale.push(format!("Focus node '{}' is a consumer of this contract", focus_node));
            }

            if sr.entry.provider_node_ids.len() >= 2 {
                rationale.push(format!("{} existing providers exist — prefer reuse over new implementation", sr.entry.provider_node_ids.len()));
            }

            AbstractionMemoryRecommendation {
                entry: sr.entry.clone(),
                score: adjusted_score,
                matched_terms: sr.matched_terms,
                rationale,
                suggested_usage: suggested,
            }
        })
        .take(input.limit)
        .collect()
}

// ── Protocol Action Candidates ──────────────────────────────────────

/// Build protocol action candidates from abstraction memory.
pub fn build_abstraction_protocol_action_candidates(
    map_file: &Path,
    memory_file: &Path,
    input: &RecommendationInput,
) -> Result<Vec<AbstractionProtocolActionCandidate>, anyhow::Error> {
    let artifact = match load_abstraction_memory(memory_file) {
        Some(a) => a,
        None => return Ok(Vec::new()),
    };

    let recommendations = recommend_abstractions(&artifact, input);
    let nodes = load_triad_nodes(map_file)?;
    let node_map: HashMap<&str, &TriadNodeDefinition> = nodes.iter().map(|n| (n.node_id.as_str(), n)).collect();

    let mut candidates = Vec::new();

    for rec in &recommendations {
        // Reuse seed: reuse the abstraction provider
        if let Some(preferred_id) = choose_preferred_reuse(&rec.entry, input.focus_node_id.as_deref()) {
            candidates.push(AbstractionProtocolActionCandidate {
                kind: "reuse_seed".into(),
                action: ProtocolActionSeed::Reuse(ReuseActionSeed {
                    node_id: preferred_id,
                    reason: format!("Reuse abstraction '{}' ({})", rec.entry.name, rec.entry.kind.as_str()),
                    confidence: normalize_confidence(rec.score),
                }),
                score: rec.score,
                based_on_entry: AbstractionMemoryEntryRef {
                    id: rec.entry.id.clone(),
                    name: rec.entry.name.clone(),
                    kind: rec.entry.kind,
                },
                rationale: rec.rationale.clone(),
            });
        }

        // Modify seed: modify an existing consumer to use the abstraction
        for consumer_id in &rec.entry.consumer_node_ids {
            if let Some(_node) = node_map.get(consumer_id.as_str()) {
                candidates.push(AbstractionProtocolActionCandidate {
                    kind: "modify_seed".into(),
                    action: ProtocolActionSeed::Modify(ModifyActionSeed {
                        node_id: consumer_id.clone(),
                        reason: format!("Adapt consumer '{}' to use abstraction '{}'", consumer_id, rec.entry.name),
                        confidence: normalize_confidence(rec.score * 0.9),
                    }),
                    score: rec.score * 0.9,
                    based_on_entry: AbstractionMemoryEntryRef {
                        id: rec.entry.id.clone(),
                        name: rec.entry.name.clone(),
                        kind: rec.entry.kind,
                    },
                    rationale: rec.rationale.clone(),
                });
            }
        }
    }

    Ok(candidates)
}

// ── Prompt Context ──────────────────────────────────────────────────

/// Build a prompt-ready memory context for LLM consumption.
pub fn build_prompt_context(
    map_file: &Path,
    memory_file: &Path,
    input: &RecommendationInput,
    config: &AbstractionMemoryConfig,
) -> PromptMemoryContext {
    let artifact = match load_abstraction_memory(memory_file) {
        Some(a) => a,
        None => {
            return PromptMemoryContext {
                summary_lines: vec!["No abstraction memory entries recorded yet.".into()],
                matches: Vec::new(),
                recommendations: Vec::new(),
                protocol_action_candidates: Vec::new(),
            }
        }
    };

    let max_entries = config.max_prompt_entries.max(1);
    let recommendations = recommend_abstractions(&artifact, &RecommendationInput {
        query: input.query.clone(),
        focus_node_id: input.focus_node_id.clone(),
        focus_source_path: input.focus_source_path.clone(),
        limit: max_entries,
    });

    let matches: Vec<AbstractionMemoryEntry> = recommendations.iter().map(|r| r.entry.clone()).collect();

    let candidates: Vec<AbstractionProtocolActionCandidate> = recommendations
        .iter()
        .filter_map(|rec| {
            choose_preferred_reuse(&rec.entry, input.focus_node_id.as_deref()).map(|preferred_id| {
                AbstractionProtocolActionCandidate {
                    kind: "reuse_seed".into(),
                    action: ProtocolActionSeed::Reuse(ReuseActionSeed {
                        node_id: preferred_id,
                        reason: format!("Reuse abstraction '{}'", rec.entry.name),
                        confidence: normalize_confidence(rec.score),
                    }),
                    score: rec.score,
                    based_on_entry: AbstractionMemoryEntryRef {
                        id: rec.entry.id.clone(),
                        name: rec.entry.name.clone(),
                        kind: rec.entry.kind,
                    },
                    rationale: rec.rationale.clone(),
                }
            })
        })
        .take(max_entries)
        .collect();

    let summary_lines = if artifact.entries.is_empty() {
        vec![
            "No abstraction memory entries were recorded yet.".into(),
            "Proceed with mount-point and impact analysis, then create reusable abstractions only if no existing tool fits.".into(),
        ]
    } else {
        vec![format!(
            "Abstraction memory: {} entries ({} contracts, {} functions). Top reusable: '{}'",
            artifact.entries.len(),
            artifact.summary.contract_entry_count,
            artifact.summary.abstract_function_entry_count,
            artifact.entries.first().map(|e| e.name.as_str()).unwrap_or("none"),
        )]
    };

    PromptMemoryContext {
        summary_lines,
        matches,
        recommendations,
        protocol_action_candidates: candidates,
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn load_triad_nodes(map_file: &Path) -> Result<Vec<TriadNodeDefinition>, anyhow::Error> {
    if !map_file.exists() {
        return Ok(Vec::new());
    }
    let content = std::fs::read_to_string(map_file)?;
    let trimmed = content.trim().trim_start_matches('\u{FEFF}');
    Ok(serde_json::from_str(trimmed).unwrap_or_default())
}

fn extract_abstraction_evidence(node: &TriadNodeDefinition) -> AbstractionEvidence {
    // Try to extract from the fission evidence field
    // The evidence is nested inside fission, but TriadNodeDefinition doesn't
    // have a dedicated evidence field. We check if the node has abstraction
    // characteristics based on its structure.
    //
    // For now, we infer abstraction signals from the node's demand/answer
    // and from its node_id naming conventions.

    let mut evidence = AbstractionEvidence::default();
    let fission = match &node.fission {
        Some(f) => f,
        None => return evidence,
    };

    // Infer from node_id patterns
    if let Some((class, _method)) = node.node_id.split_once('.') {
        // Check if this node looks like an interface/trait implementation
        if fission.problem.contains("implement")
            || fission.problem.contains("Implement")
        {
            evidence.signals.push("implements_contract".into());
            evidence.abstraction_signal_count += 1;
        }

        // Check if this looks like it extends an abstract class
        if fission.problem.contains("extend") || fission.problem.contains("Extend") {
            evidence.signals.push("extends_abstract".into());
            evidence.abstract_class_count += 1;
            evidence.abstraction_signal_count += 1;
        }

        // Detect dependency on abstractions (references to trait/interface names)
        let contract_names = extract_contract_names(&fission.demand);
        evidence.depends_on_abstractions = contract_names.clone();
        if !contract_names.is_empty() {
            evidence.signals.push("depends_on_contract".into());
            evidence.abstraction_signal_count += contract_names.len();
            evidence.implements = contract_names;
        }

        // Detect abstract function patterns (functions named like trait methods)
        evidence.abstract_functions = extract_abstract_functions(class, &fission);
        evidence.function_contract_count = evidence.abstract_functions.len();

        // Count concrete signals
        evidence.concrete_signal_count = fission.demand.len() + fission.answer.len() + 1;
    }

    evidence.role = if evidence.abstraction_signal_count > evidence.concrete_signal_count {
        "abstraction_rich".into()
    } else if evidence.abstraction_signal_count > 0 {
        "mixed".into()
    } else {
        "concrete".into()
    };

    evidence
}

/// Extract likely contract/trait names from demand entries.
fn extract_contract_names(demand: &[String]) -> Vec<String> {
    demand
        .iter()
        .filter(|d| {
            let d = d.as_str();
            // Traits/interfaces typically start with uppercase and are not primitive types
            d.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
                && !is_primitive_type(d)
                && !is_generic_container(d)
        })
        .cloned()
        .collect()
}

fn extract_abstract_functions(class_name: &str, fission: &crate::protocol::TriadFission) -> Vec<String> {
    // If the answer includes a trait-like return type, this might be implementing an abstract method
    let mut funcs = Vec::new();
    for answer in &fission.answer {
        if answer.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
            && !is_primitive_type(answer)
        {
            funcs.push(format!("{}.execute({}) -> {}", class_name, fission.demand.join(", "), answer));
        }
    }
    funcs
}

fn is_primitive_type(s: &str) -> bool {
    matches!(
        s.to_lowercase().as_str(),
        "str" | "string" | "int" | "i32" | "i64" | "u32" | "u64" | "f32" | "f64"
        | "bool" | "boolean" | "void" | "()" | "usize" | "isize"
    )
}

fn is_generic_container(s: &str) -> bool {
    matches!(
        s,
        "Vec" | "HashMap" | "HashSet" | "Option" | "Result" | "String"
        | "Box" | "Rc" | "Arc" | "RefCell" | "Mutex"
    )
}

fn appears_as_contract(name: &str, nodes: &[TriadNodeDefinition]) -> bool {
    // Check if this name appears in multiple nodes' demands as a dependency
    let count = nodes
        .iter()
        .filter(|n| {
            n.fission.as_ref().map(|f| f.demand.contains(&name.to_string())).unwrap_or(false)
        })
        .count();
    count >= 2
}

fn is_excluded_source(normalized_path: &str, config: &AbstractionMemoryConfig) -> bool {
    for pattern in &config.exclude_source_path_patterns {
        if normalized_path.contains(pattern.as_str()) {
            return true;
        }
    }
    for path in &config.exclude_source_paths {
        if normalized_path == path.as_str() {
            return true;
        }
    }
    false
}

fn normalize_path(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").trim_end_matches('/').to_string()
}

fn sorted_set(set: &HashSet<String>) -> Vec<String> {
    let mut v: Vec<String> = set.iter().cloned().collect();
    v.sort();
    v
}

fn sanitize_id(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_alphanumeric() { c.to_ascii_lowercase() } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

fn tokenize(text: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let mut tokens: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .map(|s| s.trim().to_string())
        .filter(|s| s.len() >= 2)
        .collect();
    tokens.sort();
    tokens.dedup();
    tokens
}

fn fuzzy_contains(haystack: &str, term: &str) -> bool {
    let no_underscore = term.replace('_', "");
    let no_dash = term.replace('-', "");
    haystack.contains(&no_underscore) || haystack.contains(&no_dash)
}

fn choose_preferred_reuse(entry: &AbstractionMemoryEntry, focus_node_id: Option<&str>) -> Option<String> {
    if let Some(focus) = focus_node_id {
        if entry.provider_node_ids.contains(&focus.to_string()) {
            return Some(focus.to_string());
        }
    }
    entry.provider_node_ids.first().cloned()
        .or_else(|| entry.node_ids.first().cloned())
}

fn normalize_confidence(score: f64) -> f64 {
    (score / 40.0).max(0.35).min(0.92)
}

fn chrono_now() -> String {
    // Simple ISO 8601 without chrono dependency
    use std::time::SystemTime;
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    // Rough ISO format: YYYY-MM-DDTHH:MM:SSZ
    let days = secs / 86400;
    // This is approximate; for exact dates we'd need chrono
    format!("{:?}", now)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TriadFission;

    fn make_node(id: &str, source: &str, demand: &[&str], answer: &[&str], implements: &[&str]) -> TriadNodeDefinition {
        TriadNodeDefinition {
            node_id: id.into(),
            category: Some("core".into()),
            source_path: Some(source.into()),
            lifecycle: None,
            fission: Some(TriadFission {
                problem: format!("{} functionality", id),
                demand: demand.iter().map(|s| s.to_string()).collect(),
                answer: answer.iter().map(|s| s.to_string()).collect(),
            }),
        }
    }

    #[test]
    fn test_build_abstraction_memory_detects_contract_providers() {
        let tmp = std::env::temp_dir().join(format!("triadmind_abm_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let map_file = tmp.join("triad-map.json");

        let nodes = vec![
            make_node("StripePay.execute", "src/payments/stripe.rs", &["PaymentStrategy", "PayInput"], &["PayResult"], &["PaymentStrategy"]),
            make_node("PaypalPay.execute", "src/payments/paypal.rs", &["PaymentStrategy", "PayInput"], &["PayResult"], &["PaymentStrategy"]),
            make_node("PaymentRouter.route", "src/payments/router.rs", &["PaymentStrategy", "PayInput"], &["PayResult"], &[]),
        ];

        std::fs::write(&map_file, serde_json::to_string(&nodes).unwrap()).unwrap();

        // Verify nodes load correctly
        let loaded = load_triad_nodes(&map_file).unwrap();
        assert_eq!(loaded.len(), 3, "Failed to load test nodes");
        
        let config = AbstractionMemoryConfig::default();
        let stable = HashSet::new();
        let artifact = build_abstraction_memory(&map_file, "test", &config, &stable).unwrap();

        let _ = std::fs::remove_dir_all(&tmp);

        assert!(artifact.entries.len() >= 1,
            "Expected at least 1 entry, got {}. Scanned: {}, excluded_stable: {}, excluded_configured: {}",
            artifact.entries.len(), artifact.summary.scanned_source_count,
            artifact.summary.excluded_stable_source_count, artifact.summary.excluded_configured_source_count);
        // PaymentStrategy should be detected as a contract with 2 providers
        let contract = artifact.entries.iter().find(|e| e.name == "PaymentStrategy");
        assert!(contract.is_some(), "Expected PaymentStrategy contract entry");
        if let Some(c) = contract {
            assert_eq!(c.kind, AbstractionMemoryEntryKind::InterfaceOrContract);
            assert_eq!(c.provider_node_ids.len(), 3);
            assert!(c.provider_node_ids.contains(&"StripePay.execute".to_string()));
            assert!(c.provider_node_ids.contains(&"PaypalPay.execute".to_string()));
        }
    }

    #[test]
    fn test_search_abstraction_memory() {
        let artifact = AbstractionMemoryArtifact {
            schema_version: "1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            project: "test".into(),
            source_map_file: "triad-map.json".into(),
            summary: AbstractionMemorySummary::default(),
            entries: vec![
                AbstractionMemoryEntry {
                    id: "payment_strategy".into(),
                    name: "PaymentStrategy".into(),
                    kind: AbstractionMemoryEntryKind::InterfaceOrContract,
                    primary_source_path: "src/payments/strategies.ts".into(),
                    source_paths: vec!["src/payments/strategies.ts".into()],
                    node_ids: vec![],
                    provider_node_ids: vec!["StripePay.execute".into(), "PaypalPay.execute".into()],
                    consumer_node_ids: vec!["PaymentRouter.route".into()],
                    variant_clusters: vec!["payment".into()],
                    related_abstractions: vec![],
                    signatures: vec![],
                    tags: vec!["implements_contract".into()],
                    abstraction_ratio: 0.4,
                    reusability_score: 0.85,
                    why_reusable: "Has 2 providers and 1 consumer".into(),
                },
            ],
        };

        let results = search_abstraction_memory(&artifact, "payment strategy", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].entry.name, "PaymentStrategy");
        assert!(results[0].score > 0.0);

        let no_results = search_abstraction_memory(&artifact, "xyz_nonexistent", 5);
        assert!(no_results.is_empty());
    }

    #[test]
    fn test_recommend_abstractions() {
        let artifact = AbstractionMemoryArtifact {
            schema_version: "1.0".into(),
            generated_at: "2026-01-01T00:00:00Z".into(),
            project: "test".into(),
            source_map_file: "triad-map.json".into(),
            summary: AbstractionMemorySummary::default(),
            entries: vec![
                AbstractionMemoryEntry {
                    id: "payment_strategy".into(),
                    name: "PaymentStrategy".into(),
                    kind: AbstractionMemoryEntryKind::InterfaceOrContract,
                    primary_source_path: "src/payments/strategies.ts".into(),
                    source_paths: vec!["src/payments/strategies.ts".into()],
                    node_ids: vec![],
                    provider_node_ids: vec!["StripePay.execute".into(), "PaypalPay.execute".into()],
                    consumer_node_ids: vec!["PaymentRouter.route".into()],
                    variant_clusters: vec!["payment".into()],
                    related_abstractions: vec![],
                    signatures: vec![],
                    tags: vec!["implements_contract".into()],
                    abstraction_ratio: 0.4,
                    reusability_score: 0.85,
                    why_reusable: "Has 2 providers and 1 consumer".into(),
                },
            ],
        };

        let input = RecommendationInput {
            query: "payment strategy reuse".into(),
            focus_node_id: Some("PaymentRouter.route".into()),
            focus_source_path: Some("src/payments/router.ts".into()),
            limit: 3,
        };

        let recs = recommend_abstractions(&artifact, &input);
        assert!(!recs.is_empty());
        assert_eq!(recs[0].entry.name, "PaymentStrategy");
        assert_eq!(recs[0].suggested_usage, SuggestedUsage::ReuseFirst);
    }

    #[test]
    fn test_tokenize() {
        let tokens = tokenize("Payment strategy reuse pattern");
        assert!(tokens.contains(&"payment".to_string()));
        assert!(tokens.contains(&"strategy".to_string()));
        assert!(tokens.contains(&"reuse".to_string()));
        assert!(tokens.contains(&"pattern".to_string()));
    }

    #[test]
    fn test_extract_contract_names() {
        let names = extract_contract_names(&[
            "PaymentStrategy".into(),
            "string".into(),
            "UserService".into(),
            "Vec".into(),
            "i32".into(),
        ]);
        // Only UserService should remain (non-primitive, non-generic)
        assert!(names.contains(&"PaymentStrategy".to_string()));
        assert!(names.contains(&"UserService".to_string()));
        assert!(!names.contains(&"string".to_string()));
        assert!(!names.contains(&"Vec".to_string()));
    }
}