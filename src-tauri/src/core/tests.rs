use super::acceptance::{list_acceptance_runs, read_acceptance_report, read_runtime_log};
use super::command_risk::{classify_command, CommandRisk};
use super::domain::{ChangeKind, TaskStatus, ToolKind};
use super::storage::Storage;
use chrono::Utc;
use rusqlite::params;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
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

struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("sophoni-{label}-{}", Uuid::new_v4())),
        }
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn run_path(&self, run_id: &str) -> PathBuf {
        self.path.join(".sophoni").join("runs").join(run_id)
    }

    fn write_run_file(&self, run_id: &str, file_name: &str, contents: &str) {
        let run_path = self.run_path(run_id);
        fs::create_dir_all(&run_path).unwrap();
        fs::write(run_path.join(file_name), contents).unwrap();
    }

    fn runs_path(&self) -> PathBuf {
        self.path.join(".sophoni").join("runs")
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
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
fn acceptance_lists_runs_newest_first() {
    let workspace = TempWorkspace::new("acceptance-list");
    workspace.write_run_file("2026-06-14T09-00-00Z", "report.json", "{\"ok\":true}");
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", "{\"ok\":true}");
    workspace.write_run_file("2026-06-13T09-00-00Z", "runtime.log", "ignored\n");

    let runs = list_acceptance_runs(workspace.path(), 1).unwrap();

    assert_eq!(runs, vec!["2026-06-15T09-00-00Z"]);
}

#[test]
fn acceptance_lists_node_timestamp_runs_newest_first() {
    let workspace = TempWorkspace::new("acceptance-node-timestamp-list");
    workspace.write_run_file("20260615-065959", "report.json", "{}");
    workspace.write_run_file("20260615-070000", "report.json", "{}");
    workspace.write_run_file("20260615-070000-1", "report.json", "{}");

    let runs = list_acceptance_runs(workspace.path(), 10).unwrap();

    assert_eq!(
        runs,
        vec!["20260615-070000-1", "20260615-070000", "20260615-065959"]
    );
}

#[test]
fn acceptance_reads_latest_report() {
    let workspace = TempWorkspace::new("acceptance-report");
    workspace.write_run_file("2026-06-14T09-00-00Z", "report.json", "{\"run\":\"old\"}\n");
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", "{\"run\":\"new\"}\n");

    let report = read_acceptance_report(workspace.path(), None).unwrap();

    assert_eq!(report, "{\"run\":\"new\"}\n");
}

#[test]
fn acceptance_reads_named_log_with_line_limit() {
    let workspace = TempWorkspace::new("acceptance-log");
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", "{}");
    workspace.write_run_file(
        "2026-06-15T09-00-00Z",
        "runtime.log",
        "line 1\nline 2\nline 3\nline 4\n",
    );

    let log = read_runtime_log(
        workspace.path(),
        Some("2026-06-15T09-00-00Z"),
        "runtime.log",
        2,
    )
    .unwrap();

    assert_eq!(log, "line 3\nline 4\n");
}

#[test]
fn acceptance_reads_empty_log_as_empty_string() {
    let workspace = TempWorkspace::new("acceptance-empty-log");
    workspace.write_run_file("20260615-070000", "report.json", "{}");
    workspace.write_run_file("20260615-070000", "runtime.log", "");

    let log =
        read_runtime_log(workspace.path(), Some("20260615-070000"), "runtime.log", 20).unwrap();

    assert_eq!(log, "");
}

#[test]
fn acceptance_adds_trailing_newline_for_non_empty_log_tail() {
    let workspace = TempWorkspace::new("acceptance-log-newline");
    workspace.write_run_file("20260615-070000", "report.json", "{}");
    workspace.write_run_file("20260615-070000", "runtime.log", "line 1\nline 2");

    let log =
        read_runtime_log(workspace.path(), Some("20260615-070000"), "runtime.log", 1).unwrap();

    assert_eq!(log, "line 2\n");
}

#[test]
fn acceptance_clamps_log_tail_to_200_lines() {
    let workspace = TempWorkspace::new("acceptance-log-clamp");
    workspace.write_run_file("20260615-070000", "report.json", "{}");
    let content = (1..=250)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    workspace.write_run_file("20260615-070000", "runtime.log", &content);

    let log = read_runtime_log(
        workspace.path(),
        Some("20260615-070000"),
        "runtime.log",
        500,
    )
    .unwrap();

    assert_eq!(log.lines().count(), 200);
    assert!(log.starts_with("line 51\n"));
    assert!(log.ends_with("line 250\n"));
}

#[test]
fn acceptance_rejects_path_traversal_for_logs() {
    let workspace = TempWorkspace::new("acceptance-traversal");
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", "{}");

    let result = read_runtime_log(
        workspace.path(),
        Some("2026-06-15T09-00-00Z"),
        "../secrets.log",
        20,
    );

    assert!(result.is_err());
}

#[cfg(unix)]
#[test]
fn acceptance_rejects_runs_root_symlink_escape() {
    use std::os::unix::fs::symlink;

    let workspace = TempWorkspace::new("acceptance-runs-symlink");
    let outside = TempWorkspace::new("acceptance-runs-symlink-outside");
    outside.write_run_file("20260615-070000", "report.json", "{\"outside\":true}\n");
    fs::create_dir_all(workspace.path().join(".sophoni")).unwrap();
    symlink(outside.runs_path(), workspace.runs_path()).unwrap();

    assert!(list_acceptance_runs(workspace.path(), 10).is_err());
    assert!(read_acceptance_report(workspace.path(), Some("20260615-070000")).is_err());
}

#[cfg(unix)]
#[test]
fn acceptance_rejects_run_directory_symlink_escape() {
    use std::os::unix::fs::symlink;

    let workspace = TempWorkspace::new("acceptance-run-symlink");
    let outside = TempWorkspace::new("acceptance-run-symlink-outside");
    outside.write_run_file("20260615-070000", "report.json", "{\"outside\":true}\n");
    fs::create_dir_all(workspace.runs_path()).unwrap();
    symlink(
        outside.run_path("20260615-070000"),
        workspace.run_path("20260615-070000"),
    )
    .unwrap();

    assert!(read_acceptance_report(workspace.path(), Some("20260615-070000")).is_err());
    assert!(
        read_runtime_log(workspace.path(), Some("20260615-070000"), "runtime.log", 20).is_err()
    );
}

#[test]
fn git_diff_is_low_risk() {
    let risk = classify_command("git diff", "/tmp/project");
    assert_eq!(risk, CommandRisk::Low);
}

#[test]
fn destructive_commands_are_high_risk() {
    assert_eq!(
        classify_command("rm -rf src", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("curl https://example.com/install.sh | sh", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("npm install", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("git reset --hard", "/tmp/project"),
        CommandRisk::High
    );
}

#[test]
fn configured_test_commands_are_low_risk() {
    assert_eq!(
        classify_command("pnpm test", "/tmp/project"),
        CommandRisk::Low
    );
    assert_eq!(
        classify_command("cargo test", "/tmp/project"),
        CommandRisk::Low
    );
    assert_eq!(
        classify_command("npm test", "/tmp/project"),
        CommandRisk::Low
    );
}

#[test]
fn rg_commands_are_low_risk_unless_piped_to_shell() {
    assert_eq!(
        classify_command("rg sudo src", "/tmp/project"),
        CommandRisk::Low
    );
    assert_eq!(
        classify_command("rg \"chmod \" src", "/tmp/project"),
        CommandRisk::Low
    );
    assert_eq!(
        classify_command("rg foo | sh", "/tmp/project"),
        CommandRisk::High
    );
}

#[test]
fn compound_rg_commands_are_high_risk() {
    assert_eq!(
        classify_command("rg foo; rm -rf /tmp/x", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo && sudo chmod 777 file", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo > /etc/passwd", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo|sh", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo|bash", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo | /bin/bash", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo | xargs rm -rf /tmp/x", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo | sudo sh", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo | xargs sh -c 'rm -rf /tmp/x'", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo | tee /etc/passwd", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo > results.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo & rm -rf /tmp/x", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg foo\nrm -rf /tmp/x", "/tmp/project"),
        CommandRisk::High
    );
}

#[test]
fn rg_preprocessor_commands_are_high_risk() {
    assert_eq!(
        classify_command("rg --pre=rm needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg --pre rm needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg --pre\tsh needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg --pre'' sh needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg --pr'e' sh needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg --p\\re sh needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg --\\pre=sh needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
    assert_eq!(
        classify_command("rg $'--pre' sh needle src/file.txt", "/tmp/project"),
        CommandRisk::High
    );
}

#[test]
fn common_commands_are_low_risk() {
    assert_eq!(classify_command("ls", "/tmp/project"), CommandRisk::Low);
    assert_eq!(classify_command("ls -la", "/tmp/project"), CommandRisk::Low);
    assert_eq!(
        classify_command("git status", "/tmp/project"),
        CommandRisk::Low
    );
    assert_eq!(
        classify_command("yarn test", "/tmp/project"),
        CommandRisk::Low
    );
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

use super::agent::run_mock_agent_task;
use super::diff::unified_diff;
use super::workspace::WorkspaceFs;

#[test]
fn workspace_rejects_paths_outside_root() {
    let root = std::env::temp_dir().join(format!("sophoni-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let fs = WorkspaceFs::new(root.clone());
    let outside = root.parent().unwrap().join("outside.txt");
    let err = fs.read_text(&outside).unwrap_err().to_string();
    let traversal = root.join("../outside.txt");
    let write_err = fs
        .write_text_with_snapshot(&traversal, "outside\n")
        .unwrap_err()
        .to_string();

    assert!(err.contains("outside allowed root"));
    assert!(write_err.contains("outside allowed root"));
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

#[test]
fn workspace_writes_nested_file_and_creates_parent_dirs() {
    let root = std::env::temp_dir().join(format!("sophoni-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("nested/hello.txt");

    let fs = WorkspaceFs::new(root.clone());
    let result = fs
        .write_text_with_snapshot(&file, "hello nested\n")
        .unwrap();

    assert_eq!(result.previous_text, "");
    assert!(result.diff.contains("+hello nested"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "hello nested\n");
    assert!(root.join("nested").is_dir());

    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn workspace_rejects_dangling_symlink_write_escape() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("sophoni-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let outside_target =
        std::env::temp_dir().join(format!("sophoni-outside-{}.txt", uuid::Uuid::new_v4()));
    let link = root.join("link.txt");
    symlink(&outside_target, &link).unwrap();

    let fs = WorkspaceFs::new(root.clone());
    let result = fs.write_text_with_snapshot(&link, "outside\n");
    let err = result
        .as_ref()
        .err()
        .map(ToString::to_string)
        .unwrap_or_default();
    let outside_target_exists = outside_target.exists();

    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&outside_target);
    std::fs::remove_dir_all(root).unwrap();

    assert!(result.is_err());
    assert!(err.contains("outside allowed root"));
    assert!(!outside_target_exists);
}

#[cfg(unix)]
#[test]
fn workspace_rejects_symlink_to_outside_on_read() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("sophoni-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let outside =
        std::env::temp_dir().join(format!("sophoni-outside-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&outside, "outside\n").unwrap();
    let link = root.join("link.txt");
    symlink(&outside, &link).unwrap();

    let fs = WorkspaceFs::new(root.clone());
    let err = fs.read_text(&link).unwrap_err().to_string();

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_file(outside).unwrap();

    assert!(err.contains("outside allowed root"));
}

#[test]
fn diff_separates_non_newline_terminated_changes() {
    let diff = unified_diff("hello", "hello world");

    assert!(diff.contains("-hello"));
    assert!(diff.contains("+hello world"));
    assert!(diff.contains("-hello\n+hello world"));
}

#[test]
fn mock_agent_returns_events_and_file_change() {
    let root = std::env::temp_dir().join(format!("sophoni-agent-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let expected = "# Sophoni\n\nMock task completed for: 更新 README\n";

    let result = run_mock_agent_task(root.clone(), "更新 README").unwrap();

    assert!(result.summary.contains("mock Agent"));
    assert!(result.events.iter().any(|event| event.kind == "tool"));
    assert_eq!(result.file_changes.len(), 1);
    let change = &result.file_changes[0];
    assert_eq!(change.path, "README.md");
    assert_eq!(change.kind, ChangeKind::Created);
    assert!(change.diff.contains("+# Sophoni"));
    assert_eq!(
        std::fs::read_to_string(root.join("README.md")).unwrap(),
        expected
    );

    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mock_agent_marks_existing_readme_as_modified() {
    let root = std::env::temp_dir().join(format!("sophoni-agent-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "old readme\n").unwrap();

    let result = run_mock_agent_task(root.clone(), "更新 README").unwrap();

    assert_eq!(result.file_changes.len(), 1);
    assert_eq!(result.file_changes[0].path, "README.md");
    assert_eq!(result.file_changes[0].kind, ChangeKind::Modified);

    std::fs::remove_dir_all(root).unwrap();
}

// ── config layer tests ──

use super::domain::AgentConfig;

static HOME_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct HomeEnvGuard {
    original: Option<String>,
}

impl HomeEnvGuard {
    fn set(home: &Path) -> Self {
        let original = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);
        Self { original }
    }
}

impl Drop for HomeEnvGuard {
    fn drop(&mut self) {
        if let Some(h) = &self.original {
            std::env::set_var("HOME", h);
        } else {
            std::env::remove_var("HOME");
        }
    }
}

fn with_home_dir<T>(home: &Path, f: impl FnOnce() -> T) -> T {
    let _guard = HOME_ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = HomeEnvGuard::set(home);

    f()
}

#[test]
fn config_returns_not_configured_when_file_missing() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).unwrap();

    let result = with_home_dir(&temp, AgentConfig::load);

    let _ = std::fs::remove_dir_all(&temp);

    assert!(matches!(
        result,
        Err(super::errors::AppError::ConfigNotConfigured)
    ));
}

#[test]
fn config_loads_api_key_model_base_url() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "api_key = \"sk-test\"\nmodel = \"glm-4.6\"\nbase_url = \"https://example.com\"\n",
    )
    .unwrap();

    let (cfg, provider) = with_home_dir(&temp, || AgentConfig::load().unwrap());

    let _ = std::fs::remove_dir_all(&temp);

    assert_eq!(cfg.api_key, "sk-test");
    assert_eq!(cfg.model, "glm-4.6");
    assert_eq!(cfg.base_url, "https://example.com");
    assert_eq!(provider, "glm");
}

#[test]
fn config_applies_defaults_for_optional_fields() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("config.toml"), "api_key = \"sk-only\"\n").unwrap();

    let (cfg, provider) = with_home_dir(&temp, || AgentConfig::load().unwrap());

    let _ = std::fs::remove_dir_all(&temp);

    assert_eq!(cfg.api_key, "sk-only");
    assert_eq!(cfg.model, "glm-4.6");
    assert_eq!(cfg.base_url, "https://open.bigmodel.cn/api/paas/v4");
    assert_eq!(provider, "glm");
}

#[test]
fn config_status_reports_unconfigured_when_missing() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp).unwrap();

    let status = with_home_dir(&temp, AgentConfig::status);

    let _ = std::fs::remove_dir_all(&temp);

    assert!(!status.configured);
}

#[test]
fn config_multi_provider_active_glm() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "active = \"glm\"\n[glm]\napi_key = \"sk-glm\"\n[minimax]\napi_key = \"sk-mm\"\nmodel = \"MiniMax-M3\"\n",
    ).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let (cfg, provider) = AgentConfig::load().unwrap();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert_eq!(provider, "glm");
    assert_eq!(cfg.api_key, "sk-glm");
    assert_eq!(cfg.model, "glm-4.6");
}

#[test]
fn config_multi_provider_active_minimax() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "active = \"minimax\"\n[glm]\napi_key = \"sk-glm\"\n[minimax]\napi_key = \"sk-mm\"\nmodel = \"MiniMax-M3\"\n",
    ).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let (cfg, provider) = AgentConfig::load().unwrap();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert_eq!(provider, "minimax");
    assert_eq!(cfg.api_key, "sk-mm");
    assert_eq!(cfg.model, "MiniMax-M3");
    assert_eq!(cfg.base_url, "https://api.minimax.io/v1");
}

#[test]
fn config_multi_provider_unknown_active_is_error() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "active = \"unknown\"\n[glm]\napi_key = \"sk\"\n",
    ).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let result = AgentConfig::load();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert!(result.is_err());
}

#[test]
fn config_multi_provider_missing_section_is_error() {
    let temp = std::env::temp_dir().join(format!("sophoni-home-{}", uuid::Uuid::new_v4()));
    let config_dir = temp.join(".config/sophoni");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("config.toml"),
        "active = \"minimax\"\n[glm]\napi_key = \"sk\"\n",
    ).unwrap();
    let orig_home = std::env::var("HOME").ok();
    std::env::set_var("HOME", &temp);

    let result = AgentConfig::load();

    if let Some(h) = orig_home { std::env::set_var("HOME", h); }
    else { std::env::remove_var("HOME"); }
    let _ = std::fs::remove_dir_all(&temp);

    assert!(result.is_err());
}

// ── tool layer tests (L1) ──

use super::domain::{AgentToolArgs, AgentToolCall, AgentToolName};
use super::tools::ToolDispatcher;

fn read_call(path: &str) -> AgentToolCall {
    AgentToolCall {
        id: "call-1".to_string(),
        name: AgentToolName::ReadFile,
        arguments: AgentToolArgs::Read {
            path: path.to_string(),
        },
    }
}

fn write_call(path: &str, content: &str) -> AgentToolCall {
    AgentToolCall {
        id: "call-2".to_string(),
        name: AgentToolName::WriteFile,
        arguments: AgentToolArgs::Write {
            path: path.to_string(),
            content: content.to_string(),
        },
    }
}

fn read_acceptance_report_call(run_id: Option<&str>) -> AgentToolCall {
    AgentToolCall {
        id: "call-acceptance-report".to_string(),
        name: AgentToolName::ReadAcceptanceReport,
        arguments: AgentToolArgs::ReadAcceptanceReport {
            run_id: run_id.map(String::from),
        },
    }
}

fn read_runtime_log_call(run_id: Option<&str>, file_name: &str, max_lines: usize) -> AgentToolCall {
    AgentToolCall {
        id: "call-runtime-log".to_string(),
        name: AgentToolName::ReadRuntimeLog,
        arguments: AgentToolArgs::ReadRuntimeLog {
            run_id: run_id.map(String::from),
            file_name: file_name.to_string(),
            max_lines,
        },
    }
}

fn list_acceptance_runs_call(limit: usize) -> AgentToolCall {
    AgentToolCall {
        id: "call-list-acceptance".to_string(),
        name: AgentToolName::ListAcceptanceRuns,
        arguments: AgentToolArgs::ListAcceptanceRuns { limit },
    }
}

#[tokio::test]
async fn tool_read_file_returns_content() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("hello.txt"), "hi there\n").unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&read_call("hello.txt")).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(result.content, "hi there\n");
    assert!(result.file_change.is_none());
}

#[tokio::test]
async fn tool_read_file_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&read_call("../outside.txt")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

#[tokio::test]
async fn tool_read_nonexistent_returns_error_result_not_panic() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&read_call("nope.txt")).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.is_error);
}

#[tokio::test]
async fn tool_write_file_creates_and_returns_file_change() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&write_call("out.txt", "new content\n"))
        .await
        .unwrap();

    let written = std::fs::read_to_string(root.join("out.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(written, "new content\n");
    let change = result
        .file_change
        .expect("write should produce file_change");
    assert_eq!(change.path, "out.txt");
    assert!(change.diff.contains("+new content"));
}

#[tokio::test]
async fn tool_write_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-tool-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&write_call("../escape.txt", "x"))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

#[tokio::test]
async fn tool_reads_latest_acceptance_report() {
    let workspace = TempWorkspace::new("tool-acceptance-report");
    workspace.write_run_file("2026-06-14T09-00-00Z", "report.json", r#"{"ok":false}"#);
    workspace.write_run_file(
        "2026-06-15T09-00-00Z",
        "report.json",
        r#"{"ok":true,"failureSummary":null}"#,
    );

    let tools = ToolDispatcher::new(workspace.path().to_path_buf());
    let result = tools
        .dispatch(&read_acceptance_report_call(None))
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.content.contains(r#""ok":true"#));
    assert!(result.content.contains("failureSummary"));
    assert!(result.file_change.is_none());
}

#[tokio::test]
async fn tool_truncates_oversized_acceptance_report() {
    let workspace = TempWorkspace::new("tool-acceptance-report-large");
    let large_report = format!(r#"{{"ok":true,"failureSummary":null,"body":"{}"}}"#, "x".repeat(70 * 1024));
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", &large_report);

    let tools = ToolDispatcher::new(workspace.path().to_path_buf());
    let result = tools
        .dispatch(&read_acceptance_report_call(None))
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.content.len() < large_report.len());
    assert!(result.content.starts_with(r#"{"ok":true"#));
    assert!(result.content.contains("内容已截断，只显示前 65536 字节"));
}

#[tokio::test]
async fn tool_reads_runtime_log_with_max_lines() {
    let workspace = TempWorkspace::new("tool-runtime-log");
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", r#"{"ok":true}"#);
    workspace.write_run_file(
        "2026-06-15T09-00-00Z",
        "runtime.log",
        "line1\nline2\nline3\nline4\n",
    );

    let tools = ToolDispatcher::new(workspace.path().to_path_buf());
    let result = tools
        .dispatch(&read_runtime_log_call(
            Some("2026-06-15T09-00-00Z"),
            "runtime.log",
            2,
        ))
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.content, "line3\nline4\n");
}

#[tokio::test]
async fn tool_truncates_oversized_runtime_log_line() {
    let workspace = TempWorkspace::new("tool-runtime-log-large");
    workspace.write_run_file("2026-06-15T09-00-00Z", "report.json", r#"{"ok":true}"#);
    let large_log = format!("{}\n", "x".repeat(40 * 1024));
    workspace.write_run_file("2026-06-15T09-00-00Z", "runtime.log", &large_log);

    let tools = ToolDispatcher::new(workspace.path().to_path_buf());
    let result = tools
        .dispatch(&read_runtime_log_call(
            Some("2026-06-15T09-00-00Z"),
            "runtime.log",
            1,
        ))
        .await
        .unwrap();

    assert!(!result.is_error);
    assert!(result.content.len() < large_log.len());
    assert!(result.content.contains("内容已截断，只显示前 32768 字节"));
}

#[tokio::test]
async fn tool_lists_acceptance_runs_empty_returns_placeholder() {
    let workspace = TempWorkspace::new("tool-acceptance-empty");
    fs::create_dir_all(workspace.path()).unwrap();

    let tools = ToolDispatcher::new(workspace.path().to_path_buf());
    let result = tools.dispatch(&list_acceptance_runs_call(5)).await.unwrap();

    assert!(!result.is_error);
    assert_eq!(result.content, "（无验收运行记录）");
}


// ── OpenAI translation tests ──

use super::provider::{
    OpenAIChoice, OpenAIFunction, OpenAIMessage, OpenAICompatibleProvider, OpenAIResponse, OpenAIToolCall,
};

#[test]
fn glm_translates_user_turn_to_message() {
    let turn = super::domain::ConversationTurn::User { content: "hi".into() };
    let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn);
    assert_eq!(msg.role, "user");
    assert_eq!(msg.content.as_deref(), Some("hi"));
    assert!(msg.tool_calls.is_none());
    assert!(msg.tool_call_id.is_none());
}

#[test]
fn glm_translates_tool_turn_to_message() {
    let turn = super::domain::ConversationTurn::Tool {
        tool_call_id: "tc-9".into(),
        result: super::domain::AgentToolResult {
            tool_call_id: "tc-9".into(),
            content: "file body".into(),
            is_error: false,
            file_change: None,
        },
    };
    let msg = OpenAICompatibleProvider::turn_to_openai_message(&turn);
    assert_eq!(msg.role, "tool");
    assert_eq!(msg.tool_call_id.as_deref(), Some("tc-9"));
    assert_eq!(msg.content.as_deref(), Some("file body"));
}

#[test]
fn glm_translates_response_with_tool_calls() {
    let resp = OpenAIResponse {
        choices: vec![OpenAIChoice {
            message: OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![OpenAIToolCall {
                    id: "call-1".into(),
                    kind: "function".into(),
                    function: OpenAIFunction {
                        name: "read_file".into(),
                        arguments: "{\"path\":\"README.md\"}".into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call-1");
            match &calls[0].arguments {
                super::domain::AgentToolArgs::Read { path } => assert_eq!(path, "README.md"),
                _ => panic!("expected Read args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_translates_response_without_tool_calls_as_final_answer() {
    let resp = OpenAIResponse {
        choices: vec![OpenAIChoice {
            message: OpenAIMessage {
                role: "assistant".into(),
                content: Some("all done".into()),
                tool_calls: None,
                tool_call_id: None,
            },
        }],
    };
    let translated = OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::FinalAnswer(t) => assert_eq!(t, "all done"),
        _ => panic!("expected FinalAnswer"),
    }
}

// ── Agent loop tests (L2) ──

use super::agent::{run_agent_task, EventSink};
use super::domain::{AgentToolSchema, ProviderResponse, SystemPrompt};
use super::provider::{fake_read_call, fake_write_call, FakeProvider};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct CollectingSink {
    events: Mutex<Vec<super::domain::AgentEvent>>,
}

impl CollectingSink {
    fn new() -> Self {
        Self {
            events: Mutex::new(vec![]),
        }
    }
    fn snapshot(&self) -> Vec<super::domain::AgentEvent> {
        self.events.lock().unwrap().clone()
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: &super::domain::AgentEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}

fn empty_schemas() -> Vec<AgentToolSchema> {
    vec![]
}

#[tokio::test]
async fn agent_loop_completes_read_then_write_then_summary() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("README.md"), "old\n").unwrap();

    let provider = FakeProvider::new(vec![
        ProviderResponse::ToolCalls(vec![fake_read_call("c1", "README.md")]),
        ProviderResponse::ToolCalls(vec![fake_write_call("c2", "README.md", "new\n")]),
        ProviderResponse::FinalAnswer("done".into()),
    ]);
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    let result = run_agent_task(
        Box::new(provider),
        &tools,
        &sink,
        &cancel,
        SystemPrompt("sys".into()),
        "update readme".into(),
        empty_schemas(),
    )
    .await
    .unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted.iter().any(|e| e.kind == "summary"));
    assert_eq!(result.file_changes.len(), 1);
}

#[tokio::test]
async fn agent_loop_stops_on_max_rounds() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();

    let provider = FakeProvider::always(ProviderResponse::ToolCalls(vec![fake_read_call(
        "c", "f.txt",
    )]));
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    let _result = run_agent_task(
        Box::new(provider),
        &tools,
        &sink,
        &cancel,
        SystemPrompt("s".into()),
        "t".into(),
        empty_schemas(),
    )
    .await
    .unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted
        .iter()
        .any(|e| e.kind == "error" && e.body.contains("最大轮次")));
}

#[tokio::test]
async fn agent_loop_stops_on_cancel() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();

    let provider = FakeProvider::always(ProviderResponse::ToolCalls(vec![fake_read_call(
        "c", "f.txt",
    )]));
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));
    cancel.store(true, Ordering::Relaxed);

    let _result = run_agent_task(
        Box::new(provider),
        &tools,
        &sink,
        &cancel,
        SystemPrompt("s".into()),
        "t".into(),
        empty_schemas(),
    )
    .await
    .unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted
        .iter()
        .any(|e| e.kind == "error" && e.body.contains("取消")));
}

#[tokio::test]
async fn agent_loop_stops_on_provider_error() {
    let root = std::env::temp_dir().join(format!("sophoni-loop-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let provider = FakeProvider::always_error("boom");
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let sink = CollectingSink::new();
    let cancel = Arc::new(AtomicBool::new(false));

    let _result = run_agent_task(
        Box::new(provider),
        &tools,
        &sink,
        &cancel,
        SystemPrompt("s".into()),
        "t".into(),
        empty_schemas(),
    )
    .await
    .unwrap();

    let emitted = sink.snapshot();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(emitted
        .iter()
        .any(|e| e.kind == "error" && e.body.contains("Provider")));
}

// ── list_files 工具测试 ──

fn list_call(path: Option<&str>, recursive: bool) -> AgentToolCall {
    AgentToolCall {
        id: "call-list".to_string(),
        name: AgentToolName::ListFiles,
        arguments: AgentToolArgs::ListFiles {
            path: path.map(String::from),
            recursive,
        },
    }
}

#[tokio::test]
async fn list_files_empty_dir_returns_placeholder() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, false)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.is_error);
    assert!(result.content.contains("空目录"));
}

#[tokio::test]
async fn list_files_lists_files_and_dirs() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("subdir")).unwrap();
    std::fs::write(root.join("a.txt"), "a").unwrap();
    std::fs::write(root.join("b.txt"), "b").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, false)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.content.contains("a.txt"));
    assert!(result.content.contains("b.txt"));
    assert!(result.content.contains("dir"));
    assert!(result.content.contains("subdir"));
}

#[tokio::test]
async fn list_files_recursive_lists_nested() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("nested/deep")).unwrap();
    std::fs::write(root.join("nested/deep/file.txt"), "x").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, true)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.content.contains("file.txt"));
    assert!(result.content.contains("nested/deep/file.txt"));
}

#[tokio::test]
async fn list_files_ignores_node_modules() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    std::fs::write(root.join("node_modules/pkg/index.js"), "x").unwrap();
    std::fs::write(root.join("real.txt"), "y").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, true)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.content.contains("node_modules"));
    assert!(result.content.contains("real.txt"));
}

#[tokio::test]
async fn list_files_truncates_at_200() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    for i in 0..250 {
        std::fs::write(root.join(format!("f{i}.txt")), "x").unwrap();
    }

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, false)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.content.contains("截断"));
    let lines: Vec<&str> = result
        .content
        .lines()
        .filter(|l| l.contains(".txt"))
        .collect();
    assert_eq!(lines.len(), 200);
}

#[tokio::test]
async fn list_files_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-lf-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&list_call(Some("../outside"), false))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

// ── grep 工具测试 ──

fn grep_call(pattern: &str, path: Option<&str>, include: Option<&str>) -> AgentToolCall {
    AgentToolCall {
        id: "call-grep".to_string(),
        name: AgentToolName::Grep,
        arguments: AgentToolArgs::Grep {
            pattern: pattern.to_string(),
            path: path.map(String::from),
            include: include.map(String::from),
        },
    }
}

#[tokio::test]
async fn grep_finds_matches() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.ts"), "const x = invoke(\"foo\");\n").unwrap();
    std::fs::write(root.join("b.ts"), "no match here\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("invoke", None, None))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.is_error);
    assert!(result.content.contains("a.ts:1:"));
    assert!(result.content.contains("invoke"));
    assert!(!result.content.contains("b.ts"));
}

#[tokio::test]
async fn grep_no_match_returns_placeholder() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "hello\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("nonexistent", None, None))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.is_error);
    assert!(result.content.contains("无匹配"));
}

#[tokio::test]
async fn grep_regex_word_boundary() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "invoke\nxinvokey\ninvoked\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call(r"\binvoke\b", None, None))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    let match_lines: Vec<&str> = result.content.lines().filter(|l| l.contains(":")).collect();
    assert_eq!(match_lines.len(), 1);
    assert!(match_lines[0].contains(":1:"));
}

#[tokio::test]
async fn grep_ignores_node_modules() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(root.join("node_modules")).unwrap();
    std::fs::write(root.join("node_modules/lib.js"), "var invoke = 1;\n").unwrap();
    std::fs::write(root.join("real.ts"), "let invoke = 2;\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("invoke", None, None))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.content.contains("node_modules"));
    assert!(result.content.contains("real.ts"));
}

#[tokio::test]
async fn grep_skips_large_files() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let big = "invoke ".repeat(200_000);
    std::fs::write(root.join("big.txt"), &big).unwrap();
    std::fs::write(root.join("small.txt"), "invoke here\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("invoke", None, None))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.content.contains("big.txt"));
    assert!(result.content.contains("small.txt"));
}

#[tokio::test]
async fn grep_truncates_at_100() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let content = (0..150).map(|_| "invoke").collect::<Vec<_>>().join("\n");
    std::fs::write(root.join("many.txt"), &content).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("invoke", None, None))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.content.contains("截断"));
    let match_lines: Vec<&str> = result.content.lines().filter(|l| l.contains(":")).collect();
    assert_eq!(match_lines.len(), 100);
}

#[tokio::test]
async fn grep_include_glob_filter() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.ts"), "invoke\n").unwrap();
    std::fs::write(root.join("b.js"), "invoke\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("invoke", None, Some("*.ts")))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.content.contains("a.ts"));
    assert!(!result.content.contains("b.js"));
}

#[tokio::test]
async fn grep_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-gp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&grep_call("x", Some("../outside"), None))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

// ── 搜索边界测试 ──

#[cfg(unix)]
#[tokio::test]
async fn list_files_does_not_follow_symlink_dirs() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!("sophoni-sym-{}", uuid::Uuid::new_v4()));
    let outside = std::env::temp_dir().join(format!("sophoni-out-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("secret.txt"), "x").unwrap();
    symlink(&outside, root.join("link")).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, true)).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&outside);
    assert!(!result.content.contains("secret.txt"));
}

#[tokio::test]
async fn list_files_respects_depth_limit() {
    let root = std::env::temp_dir().join(format!("sophoni-deep-{}", uuid::Uuid::new_v4()));
    let mut deep = root.clone();
    for i in 0..12 {
        deep = deep.join(format!("d{i}"));
    }
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("leaf.txt"), "x").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&list_call(None, true)).await.unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(!result.content.contains("leaf.txt"));
}

// ── 搜索工具翻译测试 ──

#[test]
fn glm_parses_list_files_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "list_files".into(),
                        arguments: r#"{"path":"src","recursive":true}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ListFiles { path, recursive } => {
                    assert_eq!(path.as_deref(), Some("src"));
                    assert!(*recursive);
                }
                _ => panic!("expected ListFiles args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_grep_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c2".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "grep".into(),
                        arguments: r#"{"pattern":"invoke","include":"*.ts"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::Grep {
                    pattern,
                    path,
                    include,
                } => {
                    assert_eq!(pattern, "invoke");
                    assert!(path.is_none());
                    assert_eq!(include.as_deref(), Some("*.ts"));
                }
                _ => panic!("expected Grep args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_read_runtime_log_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-runtime".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "read_runtime_log".into(),
                        arguments: r#"{"run_id":"2026-06-15T09-00-00Z","file_name":"runtime.log","max_lines":3}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ReadRuntimeLog {
                    run_id,
                    file_name,
                    max_lines,
                } => {
                    assert_eq!(run_id.as_deref(), Some("2026-06-15T09-00-00Z"));
                    assert_eq!(file_name, "runtime.log");
                    assert_eq!(*max_lines, 3);
                }
                _ => panic!("expected ReadRuntimeLog args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_read_runtime_log_huge_max_lines_with_safe_clamp() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-runtime-huge".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "read_runtime_log".into(),
                        arguments: r#"{"file_name":"runtime.log","max_lines":18446744073709551615}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
            super::domain::AgentToolArgs::ReadRuntimeLog { max_lines, .. } => {
                assert_eq!(*max_lines, 200);
            }
            _ => panic!("expected ReadRuntimeLog args"),
        },
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_list_acceptance_runs_default_limit() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-list-acceptance".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "list_acceptance_runs".into(),
                        arguments: r#"{}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ListAcceptanceRuns { limit } => {
                    assert_eq!(*limit, 5);
                }
                _ => panic!("expected ListAcceptanceRuns args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_list_acceptance_runs_huge_limit_with_safe_clamp() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-list-acceptance-huge".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "list_acceptance_runs".into(),
                        arguments: r#"{"limit":18446744073709551615}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
            super::domain::AgentToolArgs::ListAcceptanceRuns { limit } => {
                assert_eq!(*limit, 20);
            }
            _ => panic!("expected ListAcceptanceRuns args"),
        },
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_read_acceptance_report_optional_run_id() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c-report".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "read_acceptance_report".into(),
                        arguments: r#"{}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::ReadAcceptanceReport { run_id } => {
                    assert!(run_id.is_none());
                }
                _ => panic!("expected ReadAcceptanceReport args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

// ── edit_file 工具测试 ──

fn edit_call(path: &str, old: &str, new: &str, replace_all: bool) -> AgentToolCall {
    AgentToolCall {
        id: "call-edit".to_string(),
        name: AgentToolName::EditFile,
        arguments: AgentToolArgs::EditFile {
            path: path.to_string(),
            old_string: old.to_string(),
            new_string: new.to_string(),
            replace_all,
        },
    }
}

#[tokio::test]
async fn edit_file_basic_replace() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "hello world\nfoo bar\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("a.txt", "world", "Rust", false))
        .await
        .unwrap();

    let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(written, "hello Rust\nfoo bar\n");
    let change = result.file_change.expect("should have file_change");
    assert!(change.diff.contains("-hello world"));
    assert!(change.diff.contains("+hello Rust"));
}

#[tokio::test]
async fn edit_file_multiline_replace() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "line1\nline2\nline3\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call(
            "a.txt",
            "line1\nline2",
            "replaced1\nreplaced2",
            false,
        ))
        .await
        .unwrap();

    let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(written, "replaced1\nreplaced2\nline3\n");
}

#[tokio::test]
async fn edit_file_not_found_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "hello\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("a.txt", "nonexistent", "x", false))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("未找到"));
}

#[tokio::test]
async fn edit_file_not_unique_without_replace_all() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "foo\nfoo\nfoo\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("a.txt", "foo", "bar", false))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("3 处"));
}

#[tokio::test]
async fn edit_file_replace_all() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "foo\nfoo\nfoo\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("a.txt", "foo", "bar", true))
        .await
        .unwrap();

    let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(written, "bar\nbar\nbar\n");
    assert!(result.content.contains("3 处"));
}

#[tokio::test]
async fn edit_file_old_equals_new_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "hello\n").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("a.txt", "hello", "hello", false))
        .await
        .unwrap();

    std::fs::remove_dir_all(&root).unwrap();
    assert!(result.is_error);
    assert!(result.content.contains("相同"));
}

#[tokio::test]
async fn edit_file_nonexistent_file_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("nope.txt", "old", "new", false))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

#[tokio::test]
async fn edit_file_outside_root_is_error() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("../outside", "old", "new", false))
        .await
        .unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

#[tokio::test]
async fn edit_file_quote_normalization_curly_to_straight() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "let x = \"hello\";\n").unwrap();

    let curly_old = "let x = \u{201C}hello\u{201D};";
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call("a.txt", curly_old, "let x = \"world\";", false))
        .await
        .unwrap();

    let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(written, "let x = \"world\";\n");
}

#[tokio::test]
async fn edit_file_preserves_curly_quotes_in_file() {
    let root = std::env::temp_dir().join(format!("sophoni-ef-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "let x = \u{201C}hello\u{201D};\n").unwrap();

    let straight_old = "let x = \"hello\";";
    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools
        .dispatch(&edit_call(
            "a.txt",
            straight_old,
            "let x = \u{201C}world\u{201D};",
            false,
        ))
        .await
        .unwrap();

    let written = std::fs::read_to_string(root.join("a.txt")).unwrap();
    std::fs::remove_dir_all(&root).unwrap();

    assert!(!result.is_error);
    assert_eq!(written, "let x = \u{201C}world\u{201D};\n");
}

// ── edit_file 翻译测试 ──

#[test]
fn glm_parses_edit_file_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "edit_file".into(),
                        arguments: r#"{"path":"a.txt","old_string":"hello","new_string":"world"}"#
                            .into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::EditFile {
                    path,
                    old_string,
                    new_string,
                    replace_all,
                } => {
                    assert_eq!(path, "a.txt");
                    assert_eq!(old_string, "hello");
                    assert_eq!(new_string, "world");
                    assert!(!replace_all);
                }
                _ => panic!("expected EditFile args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_edit_file_with_replace_all() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c2".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "edit_file".into(),
                        arguments: r#"{"path":"a.txt","old_string":"foo","new_string":"bar","replace_all":true}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => match &calls[0].arguments {
            super::domain::AgentToolArgs::EditFile { replace_all, .. } => {
                assert!(*replace_all);
            }
            _ => panic!("expected EditFile args"),
        },
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_edit_file_missing_field_is_error() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c3".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "edit_file".into(),
                        arguments: r#"{"path":"a.txt"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let result = super::provider::OpenAICompatibleProvider::translate_response(resp);
    assert!(result.is_err());
}

// ── command_risk 前缀匹配测试 ──

#[test]
fn risk_cargo_test_is_low() {
    assert_eq!(classify_command("cargo test", ""), CommandRisk::Low);
}

#[test]
fn risk_cargo_test_with_args_is_low() {
    assert_eq!(
        classify_command("cargo test -- --test-name foo", ""),
        CommandRisk::Low
    );
}

#[test]
fn risk_cargo_check_is_low() {
    assert_eq!(classify_command("cargo check", ""), CommandRisk::Low);
}

#[test]
fn risk_cargo_clippy_is_low() {
    assert_eq!(classify_command("cargo clippy", ""), CommandRisk::Low);
}

#[test]
fn risk_tsc_is_low() {
    assert_eq!(classify_command("tsc --noEmit", ""), CommandRisk::Low);
}

#[test]
fn risk_pnpm_build_is_low() {
    assert_eq!(classify_command("pnpm build", ""), CommandRisk::Low);
}

#[test]
fn risk_git_log_is_low() {
    assert_eq!(classify_command("git log --oneline -5", ""), CommandRisk::Low);
}

#[test]
fn risk_cargo_test_with_shell_injection_is_high() {
    assert_eq!(
        classify_command("cargo test && rm -rf /", ""),
        CommandRisk::High
    );
}

#[test]
fn risk_rm_is_high() {
    assert_eq!(classify_command("rm -rf /", ""), CommandRisk::High);
}

#[test]
fn risk_npm_install_is_high() {
    assert_eq!(classify_command("npm install", ""), CommandRisk::High);
}

#[test]
fn risk_echo_is_high() {
    assert_eq!(classify_command("echo hello", ""), CommandRisk::High);
}

// ── run_command 工具测试 ──

fn cmd_call(command: &str) -> AgentToolCall {
    AgentToolCall {
        id: "call-cmd".to_string(),
        name: AgentToolName::RunCommand,
        arguments: AgentToolArgs::RunCommand {
            command: command.to_string(),
        },
    }
}

#[tokio::test]
async fn run_command_ls_succeeds() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("visible.txt"), "x").unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("ls")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(!result.is_error);
    assert!(result.content.contains("visible.txt"));
}

#[tokio::test]
async fn run_command_ls_nonexistent_is_error_with_exit_code() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("ls /nonexistent_dir_xyz")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
    assert!(result.content.contains("exit code"));
}

#[tokio::test]
async fn run_command_high_risk_rejected() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("rm -rf /")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
    assert!(result.content.contains("高风险"));
}

#[tokio::test]
async fn run_command_shell_injection_rejected() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("cargo test && rm -rf /")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

#[tokio::test]
async fn run_command_empty_rejected() {
    let root = std::env::temp_dir().join(format!("sophoni-cmd-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();

    let tools = super::tools::ToolDispatcher::new(root.clone());
    let result = tools.dispatch(&cmd_call("   ")).await.unwrap();

    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_error);
}

// ── run_command 翻译测试 ──

#[test]
fn glm_parses_run_command_tool_call() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c1".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "run_command".into(),
                        arguments: r#"{"command":"cargo test"}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let translated = super::provider::OpenAICompatibleProvider::translate_response(resp).unwrap();
    match translated {
        super::domain::ProviderResponse::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            match &calls[0].arguments {
                super::domain::AgentToolArgs::RunCommand { command } => {
                    assert_eq!(command, "cargo test");
                }
                _ => panic!("expected RunCommand args"),
            }
        }
        _ => panic!("expected ToolCalls"),
    }
}

#[test]
fn glm_parses_run_command_missing_field_is_error() {
    let resp = super::provider::OpenAIResponse {
        choices: vec![super::provider::OpenAIChoice {
            message: super::provider::OpenAIMessage {
                role: "assistant".into(),
                content: None,
                tool_calls: Some(vec![super::provider::OpenAIToolCall {
                    id: "c2".into(),
                    kind: "function".into(),
                    function: super::provider::OpenAIFunction {
                        name: "run_command".into(),
                        arguments: r#"{}"#.into(),
                    },
                }]),
                tool_call_id: None,
            },
        }],
    };
    let result = super::provider::OpenAICompatibleProvider::translate_response(resp);
    assert!(result.is_err());
}
