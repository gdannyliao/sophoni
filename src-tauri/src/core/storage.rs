use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::domain::Workspace;
use super::errors::AppResult;

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: impl AsRef<std::path::Path>) -> AppResult<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    pub fn open_in_memory() -> AppResult<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let storage = Self { conn };
        storage.migrate()?;
        Ok(storage)
    }

    fn migrate(&self) -> AppResult<()> {
        self.conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS workspaces (
                id TEXT PRIMARY KEY NOT NULL,
                name TEXT NOT NULL,
                path TEXT NOT NULL UNIQUE,
                last_opened_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS conversations (
                id TEXT PRIMARY KEY NOT NULL,
                workspace_id TEXT NOT NULL,
                title TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(workspace_id) REFERENCES workspaces(id)
            );

            CREATE TABLE IF NOT EXISTS task_runs (
                id TEXT PRIMARY KEY NOT NULL,
                conversation_id TEXT NOT NULL,
                status TEXT NOT NULL,
                summary TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY(conversation_id) REFERENCES conversations(id)
            );

            CREATE TABLE IF NOT EXISTS tool_calls (
                id TEXT PRIMARY KEY NOT NULL,
                task_run_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                input_json TEXT NOT NULL,
                output_summary TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(task_run_id) REFERENCES task_runs(id)
            );

            CREATE TABLE IF NOT EXISTS file_changes (
                id TEXT PRIMARY KEY NOT NULL,
                task_run_id TEXT NOT NULL,
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                diff TEXT NOT NULL,
                created_at TEXT NOT NULL,
                FOREIGN KEY(task_run_id) REFERENCES task_runs(id)
            );
            "#,
        )?;
        Ok(())
    }

    pub fn create_workspace(&self, name: &str, path: &str) -> AppResult<Workspace> {
        let workspace = Workspace {
            id: Uuid::new_v4(),
            name: name.to_string(),
            path: path.to_string(),
            last_opened_at: Utc::now(),
        };

        self.conn.execute(
            "INSERT INTO workspaces (id, name, path, last_opened_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                workspace.id.to_string(),
                workspace.name,
                workspace.path,
                workspace.last_opened_at.to_rfc3339()
            ],
        )?;

        Ok(workspace)
    }

    pub fn list_workspaces(&self) -> AppResult<Vec<Workspace>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, path, last_opened_at FROM workspaces ORDER BY last_opened_at DESC",
        )?;
        let mut rows = stmt.query([])?;

        let mut workspaces = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let last_opened_at: String = row.get(3)?;
            workspaces.push(Workspace {
                id: Uuid::parse_str(&id)?,
                name: row.get(1)?,
                path: row.get(2)?,
                last_opened_at: DateTime::parse_from_rfc3339(&last_opened_at)?.with_timezone(&Utc),
            });
        }
        Ok(workspaces)
    }

    #[cfg(test)]
    pub fn foreign_keys_enabled(&self) -> AppResult<bool> {
        let enabled: i64 = self
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        Ok(enabled == 1)
    }
}
