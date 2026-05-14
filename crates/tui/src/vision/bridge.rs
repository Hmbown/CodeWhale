//! Vision analysis bridge — coordinate-based region detection (0–1000 normalised).
//!
//! Ports the core ideas from openhanako's vision-bridge.js.

use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Bounding-box coordinates are normalised to `0..NORM_MAX`.
pub const NORM_MAX: u32 = 1000;

/// Normalised bounding box `[x1, y1, x2, y2]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BBox {
    pub x1: u32,
    pub y1: u32,
    pub x2: u32,
    pub y2: u32,
}

impl BBox {
    pub fn new(x1: u32, y1: u32, x2: u32, y2: u32) -> Result<Self> {
        if x1 > NORM_MAX || y1 > NORM_MAX || x2 > NORM_MAX || y2 > NORM_MAX {
            bail!("coordinates must be 0..{NORM_MAX}");
        }
        Ok(Self { x1, y1, x2, y2 })
    }
}

/// A detected visual primitive (object / region).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VisualPrimitive {
    pub id: String,
    #[serde(default)]
    #[serde(rename = "type")]
    pub type_: String,
    pub label: String,
    #[serde(rename = "box")]
    pub box_: BBox,
    pub confidence: f64,
}

/// Structured image description.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ImageNote {
    #[serde(default)]
    pub image_overview: String,
    #[serde(default)]
    pub visible_text: String,
    #[serde(default)]
    pub objects_and_layout: String,
    #[serde(default)]
    pub charts_or_data: Option<String>,
    #[serde(default)]
    pub user_request: Option<String>,
    #[serde(default)]
    pub user_request_answer: Option<String>,
    #[serde(default)]
    pub evidence: Option<String>,
    #[serde(default)]
    pub uncertainty: Option<String>,
}

/// Full analysis: text note + optional primitives.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VisionAnalysis {
    pub note: ImageNote,
    pub primitives: Vec<VisualPrimitive>,
}

// ── Prompt construction ──────────────────────────────────────────────

const NOTE_JSON_SHAPE: &str = r#"
{
  "image_overview": "basic description of what the image is",
  "visible_text": ["important OCR or readable text"],
  "objects_and_layout": "important objects, positions, counts, and relationships",
  "charts_or_data": "chart/table/data details if present; otherwise none",
  "user_request": "restate the user request in one short sentence",
  "user_request_answer": "answer the user request using the image when possible",
  "evidence": "visual evidence supporting that answer",
  "uncertainty": "anything unclear, hidden, or guessed"
}"#;

fn note_only_prompt() -> String {
    format!(
        "Analyze this image for another text-only model.\n\
         Return only one valid JSON object. Do not wrap it in Markdown.\n\
         Use this exact shape:{NOTE_JSON_SHAPE}\n\
         Do not mention that you are a tool or a separate model."
    )
}

pub fn primitives_analysis_prompt() -> String {
    format!(
        "Analyze this image for another text-only model.\n\
         Return only one valid JSON object. Do not wrap it in Markdown.\n\
         Use this exact shape:{NOTE_JSON_SHAPE}\n\
         \"visual_primitives\": [\
           {{\"id\":\"v1\",\"type\":\"box\",\"ref\":\"short label\",\"bbox_2d\":[x1,y1,x2,y2],\"confidence\":0.0}}\
         ]\n\
         }}\n\
         IMPORTANT RULES:\n\
         - visual_primitives is REQUIRED. You MUST output one entry for EVERY distinguishable object.\n\
         - NEVER group multiple people or objects into a single box. Each person gets their own box.\n\
         - For UI screenshots: one box per clickable element, text block, image, and distinct region.\n\
         - bbox_2d uses [x1,y1,x2,y2] normalised to 0-1000, (0,0) top-left, (1000,1000) bottom-right.\n\
         - Each box must tightly enclose ONLY the single detected object.\n\
         - Do not mention that you are a tool or a separate model."
    )
}

pub fn analysis_user_message(desc: &str, question: Option<&str>) -> String {
    let mut msg = format!("Analyze this image: {desc}");
    if let Some(q) = question {
        msg.push_str(&format!("\n\nUser request:\n{q}"));
    }
    msg
}

// ── Response parsing ─────────────────────────────────────────────────

pub fn parse_analysis_response(raw: &str) -> Result<VisionAnalysis> {
    let json: serde_json::Value = serde_json::from_str(&strip_markdown_fences(raw))
        .map_err(|e| anyhow::anyhow!("parse VisionAnalysis: {e}"))?;
    Ok(VisionAnalysis {
        note: parse_note_from_value(&json),
        primitives: parse_primitives_from_value(&json),
    })
}

fn parse_note_from_value(json: &serde_json::Value) -> ImageNote {
    let o = json.get("note").unwrap_or(json);
    ImageNote {
        image_overview: str_field(o, &["image_overview", "description"]),
        visible_text: str_field(o, &["visible_text", "ocr_text"]),
        objects_and_layout: str_field(o, &["objects_and_layout", "layout"]),
        charts_or_data: opt_str_field(o, &["charts_or_data"]),
        user_request: opt_str_field(o, &["user_request"]),
        user_request_answer: opt_str_field(o, &["user_request_answer", "answer"]),
        evidence: opt_str_field(o, &["evidence"]),
        uncertainty: opt_str_field(o, &["uncertainty"]),
    }
}

fn parse_primitives_from_value(json: &serde_json::Value) -> Vec<VisualPrimitive> {
    static KEYS: &[&str] = &[
        "visual_primitives",
        "visual_anchors",
        "anchors",
        "primitives",
    ];
    let items = KEYS
        .iter()
        .find_map(|k| json.get(*k).and_then(|v| v.as_array()));
    match items {
        Some(arr) => arr
            .iter()
            .enumerate()
            .filter_map(|(i, raw)| normalize_primitive(raw, i))
            .collect(),
        None => Vec::new(),
    }
}

fn normalize_primitive(raw: &serde_json::Value, index: usize) -> Option<VisualPrimitive> {
    let obj = raw.as_object()?;
    let id = obj
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(&format!("v{}", index + 1))
        .to_string();
    let type_ = obj
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("box")
        .to_string();
    let label = obj
        .get("ref")
        .or_else(|| obj.get("label"))
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    static BOX_KEYS: &[&str] = &["box", "bbox", "bbox_2d", "box_2d"];
    let raw_box = BOX_KEYS.iter().find_map(|k| obj.get(*k));
    let coords = raw_box
        .and_then(|v| v.as_array())
        .filter(|a| a.len() == 4)?;
    let nums: Vec<f64> = coords.iter().filter_map(|v| v.as_f64()).collect();
    if nums.len() != 4 {
        return None;
    }
    let clamp = |v: f64| (v.round() as i64).clamp(0, NORM_MAX as i64) as u32;
    let box_ = BBox::new(
        clamp(nums[0]),
        clamp(nums[1]),
        clamp(nums[2]),
        clamp(nums[3]),
    )
    .ok()?;
    let confidence = obj
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Some(VisualPrimitive {
        id,
        type_,
        label,
        box_,
        confidence,
    })
}

fn str_field(obj: &serde_json::Value, names: &[&str]) -> String {
    names
        .iter()
        .find_map(|n| obj.get(n).and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string()
}

fn opt_str_field(obj: &serde_json::Value, names: &[&str]) -> Option<String> {
    names
        .iter()
        .find_map(|n| obj.get(n).and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())
        .map(String::from)
}

// ── Image helpers ────────────────────────────────────────────────────

pub fn mime_type_for_path(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

pub fn build_data_url(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

// ── HTTP Vision Analysis ─────────────────────────────────────────────

pub struct VisionAnalysisParams<'a> {
    pub api_key: &'a str,
    pub base_url: &'a str,
    pub model: &'a str,
    pub max_tokens: u32,
    pub temperature: f32,
    pub timeout_secs: u64,
    pub image_data_url: &'a str,
    pub user_question: Option<&'a str>,
    /// Whether to request bounding-box primitives. Defaults to true.
    pub primitives: bool,
}

pub async fn run_vision_analysis(params: VisionAnalysisParams<'_>) -> Result<VisionAnalysis> {
    let system_prompt = if params.primitives {
        primitives_analysis_prompt()
    } else {
        note_only_prompt()
    };
    let user_text = format!(
        "{}\n\n{}",
        system_prompt,
        analysis_user_message("see attached image", params.user_question)
    );
    let mut body = serde_json::json!({
        "model": params.model,
        "messages": [
            {
                "role": "user",
                "content": [
                    { "type": "text", "text": user_text },
                    { "type": "image_url", "image_url": { "url": params.image_data_url } }
                ]
            }
        ]
    });
    if params.max_tokens > 0 {
        body["max_tokens"] = json!(params.max_tokens);
    }
    let temp = (params.temperature * 10.0).round() / 10.0;
    if temp > 0.0 {
        body["temperature"] = json!(temp);
    }
    let url = format!("{}/chat/completions", params.base_url.trim_end_matches('/'));
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    let client = CLIENT.get_or_init(reqwest::Client::new);

    let raw_body =
        tokio::time::timeout(std::time::Duration::from_secs(params.timeout_secs), async {
            let resp = client
                .post(&url)
                .header("Authorization", format!("Bearer {}", params.api_key))
                .header("Content-Type", "application/json")
                .header(
                    "User-Agent",
                    concat!(
                        "Mozilla/5.0 (compatible; deepseek-tui/",
                        env!("CARGO_PKG_VERSION"),
                        "; +https://github.com/Hmbown/DeepSeek-TUI)"
                    ),
                )
                .json(&body)
                .send()
                .await?;
            let status = resp.status();
            let text = resp.text().await?;
            if !status.is_success() {
                anyhow::bail!("Vision API HTTP {status}: {text}");
            }
            Ok::<_, anyhow::Error>(text)
        })
        .await??;
    let content: serde_json::Value = serde_json::from_str(&raw_body)?;
    let content = content["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or("");

    match parse_analysis_response(content) {
        Ok(analysis) => {
            tracing::debug!(
                primitives.count = analysis.primitives.len(),
                "Vision analysis done"
            );
            Ok(analysis)
        }
        Err(e) => {
            tracing::warn!("Vision analysis parse failed: {e}, using raw content as fallback");
            let analysis = VisionAnalysis {
                note: ImageNote {
                    image_overview: if content.is_empty() {
                        "Image processed but vision model returned empty.".into()
                    } else {
                        strip_markdown_fences(content)
                    },
                    ..Default::default()
                },
                primitives: Vec::new(),
            };
            Ok(analysis)
        }
    }
}

// ── Context formatting ───────────────────────────────────────────────

fn push_opt_tag(p: &mut Vec<String>, tag: &str, val: Option<&String>) {
    if let Some(v) = val
        && !v.is_empty()
    {
        p.push(format!("<{tag}>\n{v}\n</{tag}>"));
    }
}

pub fn format_vision_context(analysis: &VisionAnalysis) -> String {
    let mut p = vec![format!(
        "<vision-context>\n<image_overview>\n{}\n</image_overview>",
        analysis.note.image_overview
    )];
    if !analysis.note.visible_text.is_empty() {
        p.push(format!(
            "<visible_text>\n{}\n</visible_text>",
            analysis.note.visible_text
        ));
    }
    if !analysis.note.objects_and_layout.is_empty() {
        p.push(format!(
            "<objects_and_layout>\n{}\n</objects_and_layout>",
            analysis.note.objects_and_layout
        ));
    }
    push_opt_tag(
        &mut p,
        "charts_or_data",
        analysis.note.charts_or_data.as_ref(),
    );
    push_opt_tag(&mut p, "user_request", analysis.note.user_request.as_ref());
    push_opt_tag(
        &mut p,
        "user_request_answer",
        analysis.note.user_request_answer.as_ref(),
    );
    push_opt_tag(&mut p, "evidence", analysis.note.evidence.as_ref());
    p.push(r#"<visual_primitives coord="norm-1000" box_order="xyxy">"#.to_string());
    if analysis.primitives.is_empty() {
        p.push("- unavailable | reason: no valid coordinates".to_string());
    } else {
        for prim in &analysis.primitives {
            p.push(format!(
                "- {} | type: {} | box: [{},{},{},{}] | ref: {} | confidence: {:.2}",
                prim.id,
                prim.type_,
                prim.box_.x1,
                prim.box_.y1,
                prim.box_.x2,
                prim.box_.y2,
                prim.label,
                prim.confidence,
            ));
        }
    }
    p.push("</visual_primitives>".to_string());
    push_opt_tag(&mut p, "uncertainty", analysis.note.uncertainty.as_ref());
    p.push("</vision-context>".to_string());
    p.join("\n")
}

// ── Helpers ──────────────────────────────────────────────────────────

pub(crate) fn strip_markdown_fences(s: &str) -> String {
    let t = s.trim();
    if t.starts_with("```") {
        let inner = t
            .trim_start_matches("```")
            .trim_start_matches("json")
            .trim_start_matches('\n');
        if let Some(end) = inner.rfind("```") {
            return inner[..end].trim().to_string();
        }
    }
    t.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_validation() {
        assert!(BBox::new(0, 0, 1000, 1000).is_ok());
        assert!(BBox::new(1001, 0, 1000, 1000).is_err());
    }

    #[test]
    fn parse_with_box_field_names() {
        // Accepts box, bbox, bbox_2d, box_2d
        for field in ["box", "bbox", "bbox_2d", "box_2d"] {
            let raw = format!(
                r#"{{"image_overview":"T","visual_primitives":[{{"id":"v1","{field}":[10,20,300,400],"confidence":0.9}}]}}"#
            );
            let a = parse_analysis_response(&raw).unwrap();
            assert_eq!(a.primitives.len(), 1, "failed for field {field}");
            assert_eq!(a.primitives[0].box_, BBox::new(10, 20, 300, 400).unwrap());
        }
    }

    #[test]
    fn parse_array_field_names() {
        // Accepts visual_primitives, visual_anchors, anchors, primitives
        for field in [
            "visual_primitives",
            "visual_anchors",
            "anchors",
            "primitives",
        ] {
            let raw = format!(
                r#"{{"image_overview":"T","{field}":[{{"id":"p1","box":[0,0,100,100]}}]}}"#
            );
            let a = parse_analysis_response(&raw).unwrap();
            assert_eq!(a.primitives.len(), 1, "failed for field {field}");
        }
    }

    #[test]
    fn parse_label_from_ref_or_label() {
        let raw = r#"{"image_overview":"T","visual_primitives":[{"ref":"via ref","box":[0,0,10,10]},{"label":"via label","box":[0,0,10,10]}]}"#;
        let a = parse_analysis_response(raw).unwrap();
        assert_eq!(a.primitives[0].label, "via ref");
        assert_eq!(a.primitives[1].label, "via label");
    }

    #[test]
    fn parse_fenced_json() {
        let raw = "```json\n{\"image_overview\":\"x\",\"visible_text\":\"y\"}\n```";
        let a = parse_analysis_response(raw).unwrap();
        assert_eq!(a.note.image_overview, "x");
        assert!(a.primitives.is_empty());
    }

    #[test]
    fn format_context_output() {
        let a = VisionAnalysis {
            note: ImageNote {
                image_overview: "Test.".into(),
                objects_and_layout: "One box.".into(),
                visible_text: String::new(),
                evidence: Some("proof".into()),
                ..Default::default()
            },
            primitives: vec![VisualPrimitive {
                id: "v1".into(),
                type_: "box".into(),
                label: "Obj".into(),
                box_: BBox::new(10, 20, 300, 400).unwrap(),
                confidence: 0.9,
            }],
        };
        let ctx = format_vision_context(&a);
        assert!(ctx.contains(r#"<visual_primitives coord="norm-1000""#));
        assert!(ctx.contains("v1 | type: box | box: [10,20,300,400]"));
        assert!(ctx.contains("<evidence>"));
        assert!(!ctx.contains("<visible_text>"));
    }

    #[test]
    fn format_no_primitives() {
        let a = VisionAnalysis {
            note: ImageNote::default(),
            primitives: vec![],
        };
        assert!(format_vision_context(&a).contains("unavailable"));
    }

    #[test]
    fn strip_fences() {
        assert_eq!(strip_markdown_fences(r#"{"a":1}"#), r#"{"a":1}"#);
        assert_eq!(
            strip_markdown_fences("```json\n{\"a\":1}\n```"),
            r#"{"a":1}"#
        );
    }

    #[test]
    fn mime_and_data_url() {
        assert_eq!(mime_type_for_path(Path::new("x.png")), Some("image/png"));
        assert_eq!(mime_type_for_path(Path::new("x.PDF")), None);
        let url = build_data_url("image/png", b"hi");
        assert!(url.starts_with("data:image/png;base64,"));
    }
}
