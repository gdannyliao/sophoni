use super::command_risk::{classify_command, CommandRisk};
use super::domain::{TaskStatus, ToolKind};
use super::storage::Storage;

#[test]
fn task_status_is_serialized_as_snake_case() {
    let value = serde_json::to_value(TaskStatus::WaitingForRiskDecision).unwrap();
    assert_eq!(value, "waiting_for_risk_decision");
}

#[test]
fn tool_kind_is_serialized_as_snake_case() {
    let value = serde_json::to_value(ToolKind::FileWrite).unwrap();
    assert_eq!(value, "file_write");
}

#[test]
fn git_diff_is_low_risk() {
    let risk = classify_command("git diff", "/tmp/project");
    assert_eq!(risk, CommandRisk::Low);
}

#[test]
fn storage_initializes_schema_and_creates_workspace() {
    let storage = Storage::open_in_memory().unwrap();
    let workspace = storage.create_workspace("Demo", "/tmp/demo").unwrap();
    let loaded = storage.list_workspaces().unwrap();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, workspace.id);
    assert_eq!(loaded[0].name, "Demo");
    assert_eq!(loaded[0].path, "/tmp/demo");
}
