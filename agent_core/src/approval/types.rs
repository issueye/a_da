use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalGuardConfig {
    pub auto_approve: Vec<String>,
    pub confirm_commands: Vec<String>,
    pub command_tools: Vec<String>,
}

impl Default for ApprovalGuardConfig {
    fn default() -> Self {
        Self {
            auto_approve: Vec::new(),
            command_tools: vec![
                "run_command".to_string(),
                "run_background".to_string(),
            ],
            confirm_commands: vec![
                "rm ".to_string(),
                "rmdir".to_string(),
                "del ".to_string(),
                "format".to_string(),
                "git push".to_string(),
                "git reset".to_string(),
                "git clean".to_string(),
                "npm publish".to_string(),
                "bun publish".to_string(),
                "shutdown".to_string(),
                "taskkill".to_string(),
            ],
        }
    }
}
