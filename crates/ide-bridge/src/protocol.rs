//! Wire types for the IDE bridge MCP protocol.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionChange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_url: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionRange>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRange {
    pub start: Position,
    pub end: Position,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_empty: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

pub type Selection = SelectionChange;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFolders {
    #[serde(default)]
    pub success: bool,
    #[serde(default)]
    pub folders: Vec<WorkspaceFolder>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_file: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceFolder {
    pub name: String,
    pub uri: String,
    pub path: String,
    #[serde(default)]
    pub index: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(flatten)]
    pub extra: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_camel_case() {
        let value = serde_json::json!({
            "filePath": "/tmp/example.md",
            "fileUrl": "file:///tmp/example.md",
            "text": "hello",
            "selection": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 7 },
                "isEmpty": false
            }
        });
        let parsed: SelectionChange = serde_json::from_value(value.clone()).unwrap();
        assert_eq!(parsed.file_path.as_deref(), Some("/tmp/example.md"));
        assert_eq!(parsed.text, "hello");

        let range = parsed.selection.clone().unwrap();
        assert_eq!(range.start.line, 1);
        assert_eq!(range.end.character, 7);
        assert_eq!(range.is_empty, Some(false));

        let reserialized = serde_json::to_value(&parsed).unwrap();
        assert_eq!(reserialized, value);
    }

    #[test]
    fn tolerant_to_missing_fields() {
        let parsed: SelectionChange = serde_json::from_str(r#"{"text":""}"#).unwrap();
        assert!(parsed.file_path.is_none());
        assert!(parsed.selection.is_none());
        assert!(parsed.text.is_empty());
    }
}
