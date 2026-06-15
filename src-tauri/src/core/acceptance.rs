use std::path::{Path, PathBuf};

use super::errors::{AppError, AppResult};

const MAX_LOG_LINES: usize = 200;

pub fn list_acceptance_runs(root: &Path, limit: usize) -> AppResult<Vec<String>> {
    let Some(runs_root) = existing_runs_root(root)? else {
        return Ok(Vec::new());
    };

    let mut runs = Vec::new();
    for entry in std::fs::read_dir(&runs_root)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if !file_type.is_dir() {
            continue;
        }

        let run_id = entry.file_name().to_string_lossy().to_string();
        if !is_safe_run_id(&run_id) {
            continue;
        }

        let report_path = entry.path().join("report.json");
        if std::fs::symlink_metadata(report_path)
            .map(|metadata| metadata.file_type().is_file())
            .unwrap_or(false)
        {
            runs.push(run_id);
        }
    }

    runs.sort_by(|a, b| b.cmp(a));
    runs.truncate(limit);
    Ok(runs)
}

pub fn read_acceptance_report(root: &Path, run_id: Option<&str>) -> AppResult<String> {
    let run_id = resolve_run_id(root, run_id)?;
    let report_path = checked_run_path(root, &run_id)?.join("report.json");
    ensure_inside_runs(root, &report_path)?;
    Ok(std::fs::read_to_string(report_path)?)
}

pub fn read_runtime_log(
    root: &Path,
    run_id: Option<&str>,
    file_name: &str,
    max_lines: usize,
) -> AppResult<String> {
    if !is_safe_file_name(file_name) {
        return Err(AppError::Message(format!(
            "invalid log file name: {file_name}"
        )));
    }

    let run_id = resolve_run_id(root, run_id)?;
    let log_path = checked_run_path(root, &run_id)?.join(file_name);
    ensure_inside_runs(root, &log_path)?;

    let content = std::fs::read_to_string(log_path)?;
    Ok(tail_lines(&content, max_lines.clamp(1, MAX_LOG_LINES)))
}

fn resolve_run_id(root: &Path, run_id: Option<&str>) -> AppResult<String> {
    match run_id {
        Some(run_id) if is_safe_run_id(run_id) => Ok(run_id.to_string()),
        Some(run_id) => Err(AppError::Message(format!(
            "invalid acceptance run id: {run_id}"
        ))),
        None => list_acceptance_runs(root, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| AppError::Message("no acceptance runs found".to_string())),
    }
}

fn checked_run_path(root: &Path, run_id: &str) -> AppResult<PathBuf> {
    if !is_safe_run_id(run_id) {
        return Err(AppError::Message(format!(
            "invalid acceptance run id: {run_id}"
        )));
    }
    Ok(require_runs_root(root)?.join(run_id))
}

fn existing_runs_root(root: &Path) -> AppResult<Option<PathBuf>> {
    if !root.exists() {
        return Ok(None);
    }

    let workspace_root = root.canonicalize()?;
    let sophoni_root = workspace_root.join(".sophoni");
    if !sophoni_root.exists() {
        return Ok(None);
    }
    ensure_not_symlink(&sophoni_root)?;

    let runs_root = sophoni_root.join("runs");
    if !runs_root.exists() {
        return Ok(None);
    }
    ensure_not_symlink(&runs_root)?;
    ensure_directory_inside_workspace(&workspace_root, &runs_root)?;

    Ok(Some(runs_root))
}

fn require_runs_root(root: &Path) -> AppResult<PathBuf> {
    existing_runs_root(root)?
        .ok_or_else(|| AppError::Message("no acceptance runs found".to_string()))
}

fn ensure_not_symlink(path: &Path) -> AppResult<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        Err(AppError::OutsideWorkspace(path.display().to_string()))
    } else {
        Ok(())
    }
}

fn ensure_directory_inside_workspace(workspace_root: &Path, runs_root: &Path) -> AppResult<()> {
    let metadata = std::fs::metadata(runs_root)?;
    if !metadata.is_dir() {
        return Err(AppError::Message(format!(
            "acceptance runs root is not a directory: {}",
            runs_root.display()
        )));
    }

    let canonical_runs_root = runs_root.canonicalize()?;
    if canonical_runs_root == runs_root && canonical_runs_root.starts_with(workspace_root) {
        Ok(())
    } else {
        Err(AppError::OutsideWorkspace(runs_root.display().to_string()))
    }
}

fn ensure_inside_runs(root: &Path, path: &Path) -> AppResult<()> {
    let runs_root = require_runs_root(root)?;
    let path = path.canonicalize()?;

    if path.starts_with(&runs_root) {
        Ok(())
    } else {
        Err(AppError::OutsideWorkspace(path.display().to_string()))
    }
}

fn is_safe_run_id(run_id: &str) -> bool {
    !run_id.is_empty()
        && !run_id.contains("..")
        && run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn is_safe_file_name(file_name: &str) -> bool {
    !file_name.is_empty()
        && !file_name.contains("..")
        && !file_name.contains('/')
        && !file_name.contains('\\')
        && Path::new(file_name)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name == file_name)
            .unwrap_or(false)
}

fn tail_lines(content: &str, max_lines: usize) -> String {
    if content.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(max_lines);
    let mut tail = lines[start..].join("\n");
    tail.push('\n');
    tail
}
