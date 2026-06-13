use std::path::{Path, PathBuf};

use super::diff::unified_diff;
use super::errors::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct WriteResult {
    pub previous_text: String,
    pub next_text: String,
    pub diff: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceFs {
    root: PathBuf,
}

impl WorkspaceFs {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn read_text(&self, path: &Path) -> AppResult<String> {
        self.ensure_inside_root(path)?;
        let metadata = std::fs::metadata(path)?;
        if metadata.len() > 1_000_000 {
            return Err(AppError::FileTooLarge(metadata.len()));
        }
        Ok(std::fs::read_to_string(path)?)
    }

    pub fn write_text_with_snapshot(&self, path: &Path, next_text: &str) -> AppResult<WriteResult> {
        self.ensure_inside_root(path)?;
        let previous_text = if path.exists() {
            self.read_text(path)?
        } else {
            String::new()
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, next_text)?;
        let diff = unified_diff(&previous_text, next_text);

        Ok(WriteResult {
            previous_text,
            next_text: next_text.to_string(),
            diff,
        })
    }

    fn ensure_inside_root(&self, path: &Path) -> AppResult<()> {
        let root = self.root.canonicalize()?;
        let canonical = if path.exists() {
            path.canonicalize()?
        } else {
            let parent = path.parent().unwrap_or(path);
            parent.canonicalize()?
        };

        if canonical.starts_with(&root) {
            Ok(())
        } else {
            Err(AppError::OutsideWorkspace(path.display().to_string()))
        }
    }
}
