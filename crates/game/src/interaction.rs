use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Playbook {
    pub mode: String,
    pub freeform_allowed: bool,
    pub active_branch: Option<String>,
    pub active_ref: Option<String>,
    pub story_style: Option<StoryStyleProfile>,
    pub verbs: Vec<ActionVerb>,
    pub suggestions: Vec<ActionSuggestion>,
    pub active_node: Option<StoryNodeSummary>,
    pub visible_nodes: Vec<StoryNodeSummary>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StoryStyleProfile {
    pub id: String,
    pub title: String,
    pub pacing: String,
    pub turn_shape: String,
    pub branch_policy: String,
    pub tension_axes: Vec<String>,
    pub principles: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionVerb {
    pub command: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionSuggestion {
    pub id: String,
    pub label: String,
    pub input: String,
    pub description: String,
    pub target_node: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoryNodeSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub summary: String,
    pub gate: Option<String>,
    pub parents: Vec<String>,
    pub next_nodes: Vec<String>,
}

pub fn build_playbook(state: &Value) -> Playbook {
    let mut warnings = Vec::new();
    let mode = state
        .pointer("/interaction/mode")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("choice_and_freeform")
        .to_string();
    let freeform_allowed = state
        .pointer("/interaction/freeform_allowed")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let verbs = state
        .pointer("/interaction/verbs")
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(parse_verb).collect())
        .unwrap_or_default();
    let mut suggestions: Vec<ActionSuggestion> = state
        .pointer("/interaction/suggestions")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .enumerate()
                .filter_map(parse_suggestion)
                .collect()
        })
        .unwrap_or_default();

    let active_branch = state
        .pointer("/story/active_branch")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string);
    let active_ref = active_branch
        .as_ref()
        .map(|branch| format!("/story/branches/{branch}/head"));
    let branch_head = active_ref
        .as_deref()
        .and_then(|pointer| state.pointer(pointer))
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    if active_branch.is_some() && branch_head.is_none() {
        warnings.push("active story branch has no readable head".to_string());
    }
    let story_style = parse_story_style(state);
    let active_id = state
        .pointer("/story/active_node")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .or(branch_head)
        .map(str::to_string);
    let visible_nodes = visible_story_nodes(state);
    let active_node = active_id.as_deref().and_then(|id| {
        let node = visible_nodes
            .iter()
            .find(|node| node.id == id)
            .cloned()
            .or_else(|| parse_story_node_by_id(state, id));
        if node.is_none() {
            warnings.push(format!("active story node is missing: {id}"));
        }
        node
    });
    if active_node
        .as_ref()
        .is_some_and(|node| node.status == "sealed")
    {
        warnings.push("active story node is sealed".to_string());
    }
    if suggestions.is_empty() {
        suggestions = active_id
            .as_deref()
            .and_then(|id| state.pointer(&format!("/story/nodes/{id}/choices")))
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .enumerate()
                    .filter_map(parse_suggestion)
                    .collect()
            })
            .unwrap_or_default();
    }
    warnings.extend(validate_story_edges(
        state.pointer("/story/nodes").and_then(Value::as_object),
    ));
    warnings.extend(validate_suggestions(
        &suggestions,
        state.pointer("/story/nodes").and_then(Value::as_object),
    ));

    Playbook {
        mode,
        freeform_allowed,
        active_branch,
        active_ref,
        story_style,
        verbs,
        suggestions,
        active_node,
        visible_nodes,
        warnings,
    }
}

pub fn format_playbook(playbook: &Playbook) -> String {
    let mut lines = Vec::new();
    lines.push("How to Play".to_string());
    lines.push(String::new());
    if playbook.freeform_allowed {
        lines.push(
            "Type a numbered choice, a bracket command such as [ASK], or a custom action."
                .to_string(),
        );
    } else {
        lines.push("Type a numbered choice or one of the listed commands.".to_string());
    }
    if !playbook.verbs.is_empty() {
        lines.push(String::new());
        lines.push("Commands:".to_string());
        for verb in &playbook.verbs {
            let mut line = format!("- {} {}", verb.command, verb.label);
            if !verb.description.is_empty() {
                line.push_str(": ");
                line.push_str(&verb.description);
            }
            lines.push(line);
        }
    }
    if !playbook.suggestions.is_empty() {
        lines.push(String::new());
        lines.push("Choices:".to_string());
        for (index, suggestion) in playbook.suggestions.iter().enumerate() {
            let mut line = format!("{}. {}", index + 1, suggestion.label);
            if !suggestion.input.is_empty() {
                line.push_str(" -> ");
                line.push_str(&suggestion.input);
            }
            lines.push(line);
            if !suggestion.description.is_empty() {
                lines.push(format!("   {}", suggestion.description));
            }
        }
    }
    if let Some(style) = &playbook.story_style {
        lines.push(String::new());
        lines.push(format!("Story style: {} ({})", style.title, style.id));
        if !style.pacing.is_empty() {
            lines.push(format!("Pacing: {}", style.pacing));
        }
        if !style.turn_shape.is_empty() {
            lines.push(format!("Turn shape: {}", style.turn_shape));
        }
        if !style.branch_policy.is_empty() {
            lines.push(format!("Branch policy: {}", style.branch_policy));
        }
        if !style.tension_axes.is_empty() {
            lines.push(format!("Tension axes: {}", style.tension_axes.join(", ")));
        }
        if !style.principles.is_empty() {
            lines.push("Story principles:".to_string());
            lines.extend(
                style
                    .principles
                    .iter()
                    .map(|principle| format!("- {principle}")),
            );
        }
    }
    if let Some(node) = &playbook.active_node {
        lines.push(String::new());
        let branch = playbook
            .active_branch
            .as_deref()
            .map(|branch| format!(" on {branch}"))
            .unwrap_or_default();
        lines.push(format!(
            "Current beat{branch}: {} [{}]",
            node.title, node.status
        ));
        if !node.summary.is_empty() {
            lines.push(node.summary.clone());
        }
        if let Some(gate) = &node.gate {
            lines.push(format!("Progress gate: {gate}"));
        }
        if !node.next_nodes.is_empty() {
            lines.push(format!("Can lead to: {}", node.next_nodes.join(", ")));
        }
    }
    if !playbook.warnings.is_empty() {
        lines.push(String::new());
        lines.push("Playbook warnings:".to_string());
        lines.extend(
            playbook
                .warnings
                .iter()
                .map(|warning| format!("- {warning}")),
        );
    }
    lines.join("\n")
}

fn parse_story_style(state: &Value) -> Option<StoryStyleProfile> {
    let value = state.pointer("/story/style")?;
    if let Some(id) = value.as_str().map(str::trim).filter(|id| !id.is_empty()) {
        return Some(style_template(id));
    }

    let id = string_field(value, "id").unwrap_or_else(|| "custom".to_string());
    let template = style_template(&id);
    let title = string_field(value, "title").unwrap_or(template.title);
    let pacing = string_field(value, "pacing").unwrap_or(template.pacing);
    let turn_shape = string_field(value, "turn_shape").unwrap_or(template.turn_shape);
    let branch_policy = string_field(value, "branch_policy").unwrap_or(template.branch_policy);
    let tension_axes = non_empty_or(
        string_array_field(value, "tension_axes"),
        template.tension_axes,
    );
    let principles = non_empty_or(string_array_field(value, "principles"), template.principles);

    Some(StoryStyleProfile {
        id,
        title,
        pacing,
        turn_shape,
        branch_policy,
        tension_axes,
        principles,
    })
}

fn style_template(id: &str) -> StoryStyleProfile {
    match id {
        "deliberation_drama" => StoryStyleProfile {
            id: id.to_string(),
            title: "Deliberation drama".to_string(),
            pacing: "One room, rising social pressure, evidence beats released by earned questions.".to_string(),
            turn_shape: "Action -> juror reaction -> pressure/time shift -> visible next dilemma.".to_string(),
            branch_policy:
                "Branch by argument route: process, evidence, prejudice, indifference, or final holdout."
                    .to_string(),
            tension_axes: vec![
                "evidence doubt".to_string(),
                "procedure integrity".to_string(),
                "room pressure".to_string(),
                "juror trust".to_string(),
            ],
            principles: vec![
                "Every turn should move evidence, character, procedure, time, or vote pressure.".to_string(),
                "Hints must be diegetic: hesitation, body language, exhibit requests, or procedural friction.".to_string(),
                "A vote change needs evidence, social permission, and a juror-specific reason.".to_string(),
            ],
        },
        "emotional_reconciliation" | "romance" | "galgame" => StoryStyleProfile {
            id: id.to_string(),
            title: "Emotional reconciliation".to_string(),
            pacing: "Small gestures, subtext, and delayed vulnerability over fast plot twists.".to_string(),
            turn_shape:
                "Action -> emotional read -> boundary/respect check -> changed trust beat.".to_string(),
            branch_policy:
                "Branch by emotional posture: honesty, repair, avoidance, pressure, or farewell."
                    .to_string(),
            tension_axes: vec![
                "trust".to_string(),
                "honesty".to_string(),
                "boundary respect".to_string(),
                "fear of rejection".to_string(),
            ],
            principles: vec![
                "Make the consequence emotional before it is mechanical.".to_string(),
                "Reward specificity, accountability, and restraint.".to_string(),
                "Let silence and body language carry part of the scene.".to_string(),
            ],
        },
        "mystery" => StoryStyleProfile {
            id: id.to_string(),
            title: "Mystery".to_string(),
            pacing: "Clue discovery, contradiction pressure, false confidence, and delayed synthesis.".to_string(),
            turn_shape: "Action -> clue access check -> implication -> new question or contradiction.".to_string(),
            branch_policy:
                "Branch by theory pressure and clue graph, not by a single correct command."
                    .to_string(),
            tension_axes: vec![
                "known clues".to_string(),
                "sealed facts".to_string(),
                "suspect pressure".to_string(),
                "time risk".to_string(),
            ],
            principles: vec![
                "A clue should create a sharper question before it creates an answer.".to_string(),
                "Never reveal a sealed fact only to make narration easier.".to_string(),
                "Let wrong theories produce useful pressure, cost, or misdirection.".to_string(),
            ],
        },
        "rpg" | "adventure" => StoryStyleProfile {
            id: id.to_string(),
            title: "Adventure RPG".to_string(),
            pacing: "Explore, choose, pay costs, and let stateful consequences reshape options.".to_string(),
            turn_shape: "Intent -> capability/risk check -> world response -> updated options.".to_string(),
            branch_policy:
                "Branch by location, faction, quest state, inventory, and irreversible choices."
                    .to_string(),
            tension_axes: vec![
                "risk".to_string(),
                "resources".to_string(),
                "faction standing".to_string(),
                "quest pressure".to_string(),
            ],
            principles: vec![
                "Keep choices concrete: move, talk, inspect, use, fight, flee, or manage.".to_string(),
                "Make costs legible before punishing the player.".to_string(),
                "Use inventory and location state to make the world feel persistent.".to_string(),
            ],
        },
        _ => StoryStyleProfile {
            id: id.to_string(),
            title: id.replace('_', " "),
            pacing: "Match the cartridge's declared tension and current story node.".to_string(),
            turn_shape: "Action -> consequence -> state change -> next clear option.".to_string(),
            branch_policy:
                "Advance only when the player satisfies a visible or diegetic gate.".to_string(),
            tension_axes: Vec::new(),
            principles: vec![
                "Give each turn a meaningful change, even when the player fails.".to_string(),
                "Keep branch state explicit and player-facing hints diegetic.".to_string(),
                "Prefer consequential options over generic prose prompts.".to_string(),
            ],
        },
    }
}

fn non_empty_or(values: Vec<String>, fallback: Vec<String>) -> Vec<String> {
    if values.is_empty() { fallback } else { values }
}

fn parse_verb(value: &Value) -> Option<ActionVerb> {
    if let Some(raw) = value.as_str() {
        let command = raw.trim();
        if command.is_empty() {
            return None;
        }
        return Some(ActionVerb {
            command: command.to_string(),
            label: command.trim_matches(&['[', ']'][..]).to_ascii_lowercase(),
            description: String::new(),
        });
    }
    let command = string_field(value, "command")?;
    Some(ActionVerb {
        command: command.clone(),
        label: string_field(value, "label").unwrap_or(command),
        description: string_field(value, "description").unwrap_or_default(),
    })
}

fn parse_suggestion((index, value): (usize, &Value)) -> Option<ActionSuggestion> {
    if let Some(raw) = value.as_str() {
        let input = raw.trim();
        if input.is_empty() {
            return None;
        }
        return Some(ActionSuggestion {
            id: format!("choice_{}", index + 1),
            label: input.to_string(),
            input: input.to_string(),
            description: String::new(),
            target_node: None,
        });
    }
    let input = string_field(value, "input")?;
    Some(ActionSuggestion {
        id: string_field(value, "id").unwrap_or_else(|| format!("choice_{}", index + 1)),
        label: string_field(value, "label").unwrap_or_else(|| input.clone()),
        input,
        description: string_field(value, "description").unwrap_or_default(),
        target_node: string_field(value, "target_node"),
    })
}

fn visible_story_nodes(state: &Value) -> Vec<StoryNodeSummary> {
    let Some(nodes) = state.pointer("/story/nodes").and_then(Value::as_object) else {
        return Vec::new();
    };
    nodes
        .iter()
        .filter_map(|(id, value)| parse_story_node(id, value))
        .filter(|node| node.status != "sealed")
        .collect()
}

fn parse_story_node_by_id(state: &Value, id: &str) -> Option<StoryNodeSummary> {
    state
        .pointer(&format!("/story/nodes/{id}"))
        .and_then(|value| parse_story_node(id, value))
}

fn parse_story_node(id: &str, value: &Value) -> Option<StoryNodeSummary> {
    let status = string_field(value, "status").unwrap_or_else(|| "available".to_string());
    Some(StoryNodeSummary {
        id: id.to_string(),
        title: string_field(value, "title").unwrap_or_else(|| id.replace('_', " ")),
        status,
        summary: string_field(value, "summary").unwrap_or_default(),
        gate: string_field(value, "gate"),
        parents: string_array_field(value, "parents"),
        next_nodes: value
            .get("next")
            .and_then(Value::as_array)
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
    })
}

fn validate_story_edges(nodes: Option<&Map<String, Value>>) -> Vec<String> {
    let Some(nodes) = nodes else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for (id, value) in nodes {
        for next in string_array_field(value, "next") {
            if !nodes.contains_key(&next) {
                warnings.push(format!(
                    "story node {id} points to missing next node {next}"
                ));
            }
        }
        for parent in string_array_field(value, "parents") {
            if !nodes.contains_key(&parent) {
                warnings.push(format!(
                    "story node {id} references missing parent node {parent}"
                ));
            }
        }
    }
    warnings
}

fn validate_suggestions(
    suggestions: &[ActionSuggestion],
    nodes: Option<&Map<String, Value>>,
) -> Vec<String> {
    let Some(nodes) = nodes else {
        return Vec::new();
    };
    suggestions
        .iter()
        .filter_map(|suggestion| {
            let target = suggestion.target_node.as_deref()?;
            (!nodes.contains_key(target)).then(|| {
                format!(
                    "choice {} targets missing story node {target}",
                    suggestion.id
                )
            })
        })
        .collect()
}

fn string_array_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}
