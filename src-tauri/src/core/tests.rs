use super::command_risk::{classify_command, CommandRisk};
use super::domain::{TaskStatus, ToolKind};
use super::storage::Storage;
use chrono::Utc;
use rusqlite::params;
use std::path::{Path, PathBuf};
use uuid::Uuid;

struct TempDb {
    path: PathBuf,
}

impl TempDb {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("sophoni-{label}-{}.sqlite", Uuid::new_v4())),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

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

#[test]
fn storage_persists_workspaces_to_file() {
    let db = TempDb::new("persist-workspace");
    let workspace = {
        let storage = Storage::open(db.path()).unwrap();
        storage.create_workspace("Demo", "/tmp/demo").unwrap()
    };

    let storage = Storage::open(db.path()).unwrap();
    let loaded = storage.list_workspaces().unwrap();

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, workspace.id);
    assert_eq!(loaded[0].name, "Demo");
    assert_eq!(loaded[0].path, "/tmp/demo");
}

#[test]
fn storage_returns_error_for_invalid_workspace_uuid() {
    let db = TempDb::new("invalid-workspace-uuid");
    {
        let storage = Storage::open(db.path()).unwrap();
        drop(storage);
    }

    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute(
        "INSERT INTO workspaces (id, name, path, last_opened_at) VALUES (?1, ?2, ?3, ?4)",
        params!["not-a-uuid", "Demo", "/tmp/demo", Utc::now().to_rfc3339()],
    )
    .unwrap();
    drop(conn);

    let storage = Storage::open(db.path()).unwrap();
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| storage.list_workspaces()));

    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
}

#[test]
fn storage_returns_error_for_invalid_workspace_timestamp() {
    let db = TempDb::new("invalid-workspace-timestamp");
    {
        let storage = Storage::open(db.path()).unwrap();
        drop(storage);
    }

    let conn = rusqlite::Connection::open(db.path()).unwrap();
    conn.execute(
        "INSERT INTO workspaces (id, name, path, last_opened_at) VALUES (?1, ?2, ?3, ?4)",
        params![
            Uuid::new_v4().to_string(),
            "Demo",
            "/tmp/demo",
            "not-a-date"
        ],
    )
    .unwrap();
    drop(conn);

    let storage = Storage::open(db.path()).unwrap();
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| storage.list_workspaces()));

    assert!(result.is_ok());
    assert!(result.unwrap().is_err());
}

#[test]
fn storage_enables_foreign_keys() {
    let storage = Storage::open_in_memory().unwrap();

    assert!(storage.foreign_keys_enabled().unwrap());
}

use super::workspace::WorkspaceFs;

#[test]
fn workspace_rejects_paths_outside_root() {
    let root = std::env::temp_dir().join(format!("sophoni-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let fs = WorkspaceFs::new(root.clone());
    let outside = root.parent().unwrap().join("outside.txt");
    let err = fs.read_text(&outside).unwrap_err().to_string();

    assert!(err.contains("outside allowed root"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn workspace_writes_file_and_returns_diff() {
    let root = std::env::temp_dir().join(format!("sophoni-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("hello.txt");
    std::fs::write(&file, "hello\n").unwrap();

    let fs = WorkspaceFs::new(root.clone());
    let result = fs.write_text_with_snapshot(&file, "hello world\n").unwrap();

    assert_eq!(result.previous_text, "hello\n");
    assert!(result.diff.contains("-hello"));
    assert!(result.diff.contains("+hello world"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello world\n");

    std::fs::remove_dir_all(root).unwrap();
}
