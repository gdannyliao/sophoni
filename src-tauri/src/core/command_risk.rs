use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRisk {
    Low,
    High,
}

pub fn classify_command(command: &str, _workspace_root: &str) -> CommandRisk {
    let trimmed = command.trim();
    if matches!(trimmed, "git diff" | "git status" | "ls") || trimmed.starts_with("rg ") {
        CommandRisk::Low
    } else {
        CommandRisk::High
    }
}
