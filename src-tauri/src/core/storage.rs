use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use uuid::Uuid;

use super::domain::{Conversation, ConversationMemory, ConversationSummary, ConversationTurn, ScheduledTask, Workspace};
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
        let _ = self.conn.execute(
            "ALTER TABLE conversations ADD COLUMN turns_json TEXT NOT NULL DEFAULT '[]'",
            [],
        );
        let _ = self.conn.execute(
            "CREATE TABLE IF NOT EXISTS scheduled_tasks (
                id TEXT PRIMARY KEY NOT NULL,
                prompt TEXT NOT NULL,
                hour INTEGER NOT NULL,
                minute INTEGER NOT NULL,
                enabled INTEGER NOT NULL DEFAULT 1,
                last_run_at TEXT,
                created_at TEXT NOT NULL
            )",
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
            turns_json: "[]".to_string(),
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
                "SELECT id, workspace_id, title, events_json, turns_json, created_at, updated_at FROM conversations WHERE id = ?1",
                params![id.to_string()],
                |row| {
                    let conv_id: String = row.get(0)?;
                    let ws_id: String = row.get(1)?;
                    let created_at: String = row.get(5)?;
                    let updated_at: String = row.get(6)?;
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
                        turns_json: row.get(4)?,
                        created_at: DateTime::parse_from_rfc3339(&created_at)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    5,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&Utc),
                        updated_at: DateTime::parse_from_rfc3339(&updated_at)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    6,
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

    pub fn update_conversation_turns(&self, id: &Uuid, turns_json: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE conversations SET turns_json = ?1, updated_at = ?2 WHERE id = ?3",
            params![turns_json, now, id.to_string()],
        )?;
        Ok(())
    }

    /// 读取会话历史 turns。解析失败（脏数据/旧版本）时返回空 vec，避免阻塞续聊。
    pub fn get_conversation_turns(&self, id: &Uuid) -> AppResult<Vec<ConversationTurn>> {
        let turns_json: String = self.conn.query_row(
            "SELECT turns_json FROM conversations WHERE id = ?1",
            params![id.to_string()],
            |row| row.get(0),
        )?;
        Ok(serde_json::from_str(&turns_json).unwrap_or_default())
    }

    pub fn update_conversation_title(&self, id: &Uuid, title: &str) -> AppResult<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE conversations SET title = ?1, updated_at = ?2 WHERE id = ?3",
            params![title, now, id.to_string()],
        )?;
        Ok(())
    }

    /// 列出 workspace 下的会话记忆（category + title）。
    /// `exclude` 传入当前会话 id 时，会跳过它（避免记忆自引用当前会话）。
    pub fn list_conversation_memories(
        &self,
        workspace_id: &Uuid,
        exclude: Option<&Uuid>,
    ) -> AppResult<Vec<ConversationMemory>> {
        let mut memories = Vec::new();
        match exclude {
            Some(excluded_id) => {
                let mut stmt = self.conn.prepare(
                    "SELECT category, title, updated_at FROM conversations WHERE workspace_id = ?1 AND id != ?2 ORDER BY updated_at ASC",
                )?;
                let mut rows = stmt.query(params![workspace_id.to_string(), excluded_id.to_string()])?;
                while let Some(row) = rows.next()? {
                    memories.push(row_to_memory(row)?);
                }
            }
            None => {
                let mut stmt = self.conn.prepare(
                    "SELECT category, title, updated_at FROM conversations WHERE workspace_id = ?1 ORDER BY updated_at ASC",
                )?;
                let mut rows = stmt.query(params![workspace_id.to_string()])?;
                while let Some(row) = rows.next()? {
                    memories.push(row_to_memory(row)?);
                }
            }
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

    // ── scheduled_tasks CRUD ──

    pub fn create_scheduled_task(
        &self,
        prompt: &str,
        hour: u32,
        minute: u32,
    ) -> AppResult<ScheduledTask> {
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO scheduled_tasks (id, prompt, hour, minute, enabled, created_at) VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![id.to_string(), prompt, hour, minute, now],
        )?;
        Ok(ScheduledTask {
            id,
            prompt: prompt.to_string(),
            hour,
            minute,
            enabled: true,
            last_run_at: None,
            created_at: now,
        })
    }

    pub fn list_scheduled_tasks(&self) -> AppResult<Vec<ScheduledTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, prompt, hour, minute, enabled, last_run_at, created_at FROM scheduled_tasks ORDER BY hour, minute",
        )?;
        let mut rows = stmt.query([])?;
        let mut tasks = Vec::new();
        while let Some(row) = rows.next()? {
            let id_str: String = row.get(0)?;
            tasks.push(ScheduledTask {
                id: Uuid::parse_str(&id_str).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                prompt: row.get(1)?,
                hour: row.get(2)?,
                minute: row.get(3)?,
                enabled: row.get::<_, i64>(4)? != 0,
                last_run_at: row.get(5)?,
                created_at: row.get(6)?,
            });
        }
        Ok(tasks)
    }

    pub fn update_scheduled_task_enabled(&self, id: &Uuid, enabled: bool) -> AppResult<()> {
        self.conn.execute(
            "UPDATE scheduled_tasks SET enabled = ?1 WHERE id = ?2",
            params![enabled as i64, id.to_string()],
        )?;
        Ok(())
    }

    pub fn delete_scheduled_task(&self, id: &Uuid) -> AppResult<()> {
        self.conn.execute(
            "DELETE FROM scheduled_tasks WHERE id = ?1",
            params![id.to_string()],
        )?;
        Ok(())
    }

    pub fn update_task_last_run(&self, id: &Uuid, time: &str) -> AppResult<()> {
        self.conn.execute(
            "UPDATE scheduled_tasks SET last_run_at = ?1 WHERE id = ?2",
            params![time, id.to_string()],
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

/// 把一行 (category, title, updated_at) 转成 ConversationMemory。
fn row_to_memory(row: &rusqlite::Row<'_>) -> AppResult<ConversationMemory> {
    let category: Option<String> = row.get(0)?;
    let updated_at: String = row.get(2)?;
    Ok(ConversationMemory {
        category,
        summary: row.get(1)?,
        updated_at: DateTime::parse_from_rfc3339(&updated_at)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
            })?
            .with_timezone(&Utc),
    })
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
        assert_eq!(loaded.turns_json, "[]", "新建会话 turns_json 默认应为 []");

        // list
        let list = storage.list_conversations(&workspace.id).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].title, "修复编译错误");
    }

    #[test]
    fn storage_conversation_turns_roundtrip() {
        use super::super::domain::{AgentToolName, AgentToolArgs, AgentToolCall, AgentToolResult, ConversationTurn};
        let storage = Storage::open_in_memory().unwrap();
        let workspace = storage.get_or_create_workspace("/tmp/turns-test").unwrap();
        let conv = storage.create_conversation(&workspace.id, "turns-conv").unwrap();

        let turns = vec![
            ConversationTurn::User { content: "你好".into() },
            ConversationTurn::Assistant {
                content: Some("在的".into()),
                tool_calls: vec![AgentToolCall {
                    id: "call_1".into(),
                    name: AgentToolName::ReadFile,
                    arguments: AgentToolArgs::Read { path: "README.md".into() },
                }],
            },
            ConversationTurn::Tool {
                tool_call_id: "call_1".into(),
                result: AgentToolResult {
                    tool_call_id: "call_1".into(),
                    content: "文件内容".into(),
                    is_error: false,
                    file_change: None,
                },
            },
        ];
        let json = serde_json::to_string(&turns).unwrap();
        storage.update_conversation_turns(&conv.id, &json).unwrap();

        // 读回应 round-trip 一致
        let loaded = storage.get_conversation_turns(&conv.id).unwrap();
        assert_eq!(loaded.len(), 3);
        assert!(matches!(&loaded[0], ConversationTurn::User { content } if content == "你好"));
        assert!(matches!(&loaded[2], ConversationTurn::Tool { tool_call_id, .. } if tool_call_id == "call_1"));

        // get_conversation 里 turns_json 也应同步
        let conv_full = storage.get_conversation(&conv.id).unwrap();
        assert_eq!(conv_full.turns_json, json);
    }

    #[test]
    fn storage_get_conversation_turns_dirty_data_returns_empty() {
        let storage = Storage::open_in_memory().unwrap();
        let workspace = storage.get_or_create_workspace("/tmp/dirty-test").unwrap();
        let conv = storage.create_conversation(&workspace.id, "dirty").unwrap();
        // 写入无法解析的脏数据，应容错返回空 vec 而非报错
        storage.update_conversation_turns(&conv.id, "not-json").unwrap();
        let loaded = storage.get_conversation_turns(&conv.id).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn storage_list_conversation_memories_excludes_current() {
        let storage = Storage::open_in_memory().unwrap();
        let workspace = storage.get_or_create_workspace("/tmp/mem-test").unwrap();
        let conv_a = storage.create_conversation(&workspace.id, "任务A").unwrap();
        let conv_b = storage.create_conversation(&workspace.id, "任务B").unwrap();
        storage.update_conversation_category(&conv_a.id, "编译").unwrap();
        storage.update_conversation_category(&conv_b.id, "文档").unwrap();

        // 不排除：应含两条
        let all = storage.list_conversation_memories(&workspace.id, None).unwrap();
        assert_eq!(all.len(), 2);

        // 排除 conv_a：应只剩 conv_b
        let excluded = storage
            .list_conversation_memories(&workspace.id, Some(&conv_a.id))
            .unwrap();
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].summary, "任务B");
    }

    #[test]
    fn storage_get_or_create_workspace_idempotent() {
        let storage = Storage::open_in_memory().unwrap();
        let ws1 = storage.get_or_create_workspace("/tmp/proj-a").unwrap();
        let ws2 = storage.get_or_create_workspace("/tmp/proj-a").unwrap();
        assert_eq!(ws1.id, ws2.id, "同一 path 应返回同一 workspace");
    }

    // ── scheduled_tasks 测试 ──

    #[test]
    fn scheduled_task_crud() {
        let storage = Storage::open_in_memory().unwrap();
        let task = storage
            .create_scheduled_task("跑 pnpm accept", 9, 0)
            .unwrap();
        assert_eq!(task.hour, 9);
        assert_eq!(task.minute, 0);
        assert!(task.enabled);

        let list = storage.list_scheduled_tasks().unwrap();
        assert_eq!(list.len(), 1);

        storage.update_scheduled_task_enabled(&task.id, false).unwrap();
        let list = storage.list_scheduled_tasks().unwrap();
        assert!(!list[0].enabled);

        storage
            .update_task_last_run(&task.id, "2026-06-18T09:00:00Z")
            .unwrap();
        let list = storage.list_scheduled_tasks().unwrap();
        assert_eq!(
            list[0].last_run_at.as_deref(),
            Some("2026-06-18T09:00:00Z")
        );

        storage.delete_scheduled_task(&task.id).unwrap();
        assert!(storage.list_scheduled_tasks().unwrap().is_empty());
    }
}
