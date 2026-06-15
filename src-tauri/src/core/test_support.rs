//! 测试基础设施：跨多个模块测试共享的临时目录 helper。
//!
//! 各模块在自己的 `#[cfg(test)] mod tests` 里 `use super::super::test_support::*`
//! 即可获得这些 helper。

#![cfg(test)]

use std::fs;
use std::path::{Path, PathBuf};

use uuid::Uuid;

/// 临时 SQLite 数据库文件，drop 时删除。供 storage 测试使用。
pub struct TempDb {
    path: PathBuf,
}

impl TempDb {
    pub fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("sophoni-{label}-{}.sqlite", Uuid::new_v4())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// 临时工作区目录，drop 时递归删除。供 acceptance / tools 等测试使用。
pub struct TempWorkspace {
    path: PathBuf,
}

impl TempWorkspace {
    pub fn new(label: &str) -> Self {
        Self {
            path: std::env::temp_dir().join(format!("sophoni-{label}-{}", Uuid::new_v4())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn run_path(&self, run_id: &str) -> PathBuf {
        self.path.join(".sophoni").join("runs").join(run_id)
    }

    pub fn write_run_file(&self, run_id: &str, file_name: &str, contents: &str) {
        let run_path = self.run_path(run_id);
        fs::create_dir_all(&run_path).unwrap();
        fs::write(run_path.join(file_name), contents).unwrap();
    }

    pub fn runs_path(&self) -> PathBuf {
        self.path.join(".sophoni").join("runs")
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
