use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::domain::{Conversation, ConversationMemory, ConversationSummary, Workspace};
use super::errors::AppResult;

pub struct Storage {
    conn: Connection,
}

impl Storage {
    pub fn open(path: impl AsRef<std::path::Path>) -> AppResult<Self> {
        tracing::debug!(path = ?path.as_ref(), "storage: open");
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
        // migrations: 加 events_json 列（SQLite 没有 ADD COLUMN IF NOT EXISTS）
        let _ = self.conn.execute(
            "ALTER TABLE conversations ADD COLUMN events_json TEXT NOT NULL DEFAULT '[]'",
            [],
        );
        let _ = self.conn.execute(
            "ALTER TABLE conversations ADD COLUMN category TEXT",
            [],
        );
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
                last_opened_at: DateTime::parse_from_rfc3339(&last_opened_at)?
                    .with_timezone(&Utc),
            });
        }
        Ok(workspaces)
    }

    pub fn get_or_create_workspace(&self, path: &str) -> AppResult<Workspace> {
        let existing = self.conn.query_row(
            "SELECT id, name, path, last_opened_at FROM workspaces WHERE path = ?1",
            params![path],
            |row| {
                let id: String = row.get(0)?;
                let last_opened_at: String = row.get(3)?;
                Ok(Workspace {
                    id: Uuid::parse_str(&id).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            0,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?,
                    name: row.get(1)?,
                    path: row.get(2)?,
                    last_opened_at: DateTime::parse_from_rfc3339(&last_opened_at)
                        .map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                3,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?
                        .with_timezone(&Utc),
                })
            },
        );
        if let Ok(ws) = existing {
            return Ok(ws);
        }
        let name = std::path::Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string());
        self.create_workspace(&name, path)
    }

    pub fn create_conversation(
        &self,
        workspace_id: &Uuid,
        title: &str,
    ) -> AppResult<Conversation> {
        let id = Uuid::new_v4();
        let now = Utc::now();
        let now_str = now.to_rfc3339();
        self.conn.execute(
            "INSERT INTO conversations (id, workspace_id, title, events_json, created_at, updated_at) VALUES (?1, ?2, ?3, '[]', ?4, ?5)",
            params![id.to_string(), workspace_id.to_string(), title, now_str, now_str],
        )?;
        Ok(Conversation {
            id,
            workspace_id: *workspace_id,
            title: title.to_string(),
            events_json: "[]".to_string(),
            created_at: now,
            updated_at: now,
        })
    }

    pub fn list_conversations(
        &self,
        workspace_id: &Uuid,
    ) -> AppResult<Vec<ConversationSummary>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, updated_at FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at DESC",
        )?;
        let mut rows = stmt.query(params![workspace_id.to_string()])?;
        let mut conversations = Vec::new();
        while let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            let updated_at: String = row.get(2)?;
            conversations.push(ConversationSummary {
                id: Uuid::parse_str(&id).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
                })?,
                title: row.get(1)?,
                updated_at: DateTime::parse_from_rfc3339(&updated_at)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc),
            });
        }
        Ok(conversations)
    }

    pub fn get_conversation(&self, id: &Uuid) -> AppResult<Conversation> {
        self.conn
            .query_row(
                "SELECT id, workspace_id, title, events_json, created_at, updated_at FROM conversations WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    let conv_id: String = row.get(0)?;
                    let ws_id: String = row.get(1)?;
                    let created_at: String = row.get(4)?;
                    let updated_at: String = row.get(5)?;
                    Ok(Conversation {
                        id: Uuid::parse_str(&conv_id).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        workspace_id: Uuid::parse_str(&ws_id).map_err(|e| {
                            rusqlite::Error::FromSqlConversionFailure(
                                1,
                                rusqlite::types::Type::Text,
                                Box::new(e),
                            )
                        })?,
                        title: row.get(2)?,
                        events_json: row.get(3)?,
                        created_at: DateTime::parse_from_rfc3339(&created_at)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&Utc),
                        updated_at: DateTime::parse_from_rfc3339(&updated_at)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    5,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&Utc),
                    })
                },
            )
            .map_err(|e| e.into())
    }

    pub fn update_conversation_events(&self, id: &Uuid, events_json: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE conversations SET events_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![events_json, now, id.to_string()],
        )?;
        Ok(())
    }

    pub fn update_conversation_title(&self, id: &Uuid, title: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now, id.to_string()],
        )?;
        Ok(())
    }

    pub fn list_conversation_memories(
        &self,
        workspace_id: &Uuid,
    ) -> AppResult<Vec<ConversationMemory>> {
        let mut stmt = self.conn.prepare(
            "SELECT category, title, updated_at FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at ASC",
        )?;
        let mut rows = stmt.query(params![workspace_id.to_string()])?;
        let mut memories = Vec::new();
        while let Some(row) = rows.next()? {
            let category: Option<String> = row.get(0)?;
            let updated_at: String = row.get(2)?;
            memories.push(ConversationMemory {
                category,
                summary: row.get(1)?,
                updated_at: DateTime::parse_from_rfc3339(&updated_at)
                    .map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            2,
                            rusqlite::types::Type::Text,
                            Box::new(e),
                        )
                    })?
                    .with_timezone(&Utc),
            });
        }
        Ok(memories)
    }

    pub fn update_conversation_category(
        &self,
        id: &Uuid,
        category: &str,
    ) -> AppResult<()> {
        self.conn.execute(
            "UPDATE conversations SET category = ?1 WHERE id = ?2",
            params![category, id.to_string()],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: &Uuid) -> AppResult<()> {
        self.conn.execute(
            "DELETE FROM conversations WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    #[cfg(test)]
    pub fn foreign_keys_enabled(&self) -> AppResult<bool> {
        let enabled: i64 = self
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
        Ok(enabled == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::Storage;
    use super::super::test_support::TempDb;
    use chrono::Utc;
    use rusqlite::params;
    use uuid::Uuid;

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

    // ── conversations CRUD 测试 ──

    #[test]
    fn storage_conversation_crud() {
        let storage = Storage::open_in_memory().unwrap();
        let workspace = storage.get_or_create_workspace("/tmp/test-project").unwrap();
        let conv = storage
            .create_conversation(&workspace.id, "test-uuid")
            .unwrap();
        assert_eq!(conv.title, "test-uuid");
        assert_eq!(conv.events_json, "[]");

        // update events
        storage
            .update_conversation_events(&conv.id, r#"[{"kind":"summary"}]"#)
            .unwrap();
        // update title
        storage
            .update_conversation_title(&conv.id, "修复编译错误")
            .unwrap();

        // get
        let loaded = storage.get_conversation(&conv.id).unwrap();
        assert_eq!(loaded.title, "修复编译错误");
        assert_eq!(loaded.events_json, r#"[{"kind":"summary"}]"#);

        // list
        let list = storage.list_conversations(&workspace.id).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "修复编译错误");
    }

    #[test]
    fn storage_get_or_create_workspace_idempotent() {
        let storage = Storage::open_in_memory().unwrap();
        let ws1 = storage.get_or_create_workspace("/tmp/proj-a").unwrap();
        let ws2 = storage.get_or_create_workspace("/tmp/proj-a").unwrap();
        assert_eq!(ws1.id, ws2.id, "同一 path 应返回同一 workspace");
    }
}
