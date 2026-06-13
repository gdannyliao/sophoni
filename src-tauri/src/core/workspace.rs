use std::path::{Component, Path, PathBuf};

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
        self.ensure_write_target_is_not_symlink(path)?;
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

    fn ensure_write_target_is_not_symlink(&self, path: &Path) -> AppResult<()> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(AppError::OutsideWorkspace(path.display().to_string()))
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    fn ensure_inside_root(&self, path: &Path) -> AppResult<()> {
        let root = self.root.canonicalize()?;

        if path.exists() {
            let canonical = path.canonicalize()?;
            if canonical.starts_with(&root) {
                return Ok(());
            }
            return Err(AppError::OutsideWorkspace(path.display().to_string()));
        }

        let target = self.absolute_lexical_path(path, &root);
        if !target.starts_with(&root) {
            return Err(AppError::OutsideWorkspace(path.display().to_string()));
        }

        let mut existing_ancestor = target.as_path();
        while !existing_ancestor.exists() {
            existing_ancestor = existing_ancestor.parent().unwrap_or(existing_ancestor);
        }

        let canonical = existing_ancestor.canonicalize()?;
        if canonical.starts_with(&root) {
            Ok(())
        } else {
            Err(AppError::OutsideWorkspace(path.display().to_string()))
        }
    }

    fn absolute_lexical_path(&self, path: &Path, canonical_root: &Path) -> PathBuf {
        let absolute = if path.is_absolute() {
            match path.strip_prefix(&self.root) {
                Ok(relative) => canonical_root.join(relative),
                Err(_) => path.to_path_buf(),
            }
        } else {
            canonical_root.join(path)
        };

        lexical_normalize(&absolute)
    }
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
        }
    }

    normalized
}
