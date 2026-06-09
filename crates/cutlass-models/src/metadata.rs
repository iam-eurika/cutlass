use serde::{Deserialize, Serialize};

/// User-facing metadata attached to a [`Project`](crate::Project).
///
/// Kept separate from timeline/media state so saves and agent edits can update
/// notes without touching the edit graph.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMetadata {
    /// Free-form description or notes about this edit.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Optional creator / author label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
}
