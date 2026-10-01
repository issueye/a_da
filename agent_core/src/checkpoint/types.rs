use serde::{Deserialize, Serialize};



#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointFile {
    pub path: String,
    pub absolute: String,
    pub existed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_incomplete: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CheckpointRecord {
    pub id: String,
    pub thread_id: String,
    pub tool_call_id: String,
    pub at: i64,
    pub files: Vec<CheckpointFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RevertRecord {
    pub id: String,
    pub at: i64,
    pub checkpoint_ids: Vec<String>,
    pub scope: String, // "single" | "file" | "all"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}


#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum CheckpointEntry {
    #[serde(rename = "checkpoint")]
    Checkpoint(CheckpointRecord),
    #[serde(rename = "revert")]
    Revert(RevertRecord),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RevertOutcome {
    pub restored: Vec<String>,
    pub deleted: Vec<String>,
    pub skipped: Vec<String>,
    pub invalidated: Vec<String>,
}
