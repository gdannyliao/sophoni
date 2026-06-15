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

pub(crate) fn lexical_normalize(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::WorkspaceFs;

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
}
