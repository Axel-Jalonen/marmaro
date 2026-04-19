use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::PathBuf;

use crate::message::{ChatMessage, Conversation, Role};

/// SQLite-backed persistence for conversations and messages
pub struct Database {
    conn: Connection,
}

impl Database {
    /// Open (or create) the database at ~/.config/bedrock-chat/history.db
    pub fn open() -> Result<Self> {
        let path = Self::db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating config dir {:?}", parent))?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("opening database at {:?}", path))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                system_prompt TEXT NOT NULL DEFAULT '',
                model_id    TEXT NOT NULL,
                region      TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages (
                id              TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                role            TEXT NOT NULL,
                content         TEXT NOT NULL,
                created_at      TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_conv
                ON messages(conversation_id, created_at);
            CREATE TABLE IF NOT EXISTS config (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        Ok(Self { conn })
    }

    fn db_path() -> Result<PathBuf> {
        let home = std::env::var("HOME").context("HOME not set")?;
        Ok(PathBuf::from(home)
            .join(".config")
            .join("bedrock-chat")
            .join("history.db"))
    }

    // ── Conversations ──────────────────────────────────────────────────

    pub fn list_conversations(&self) -> Result<Vec<Conversation>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, system_prompt, model_id, region, created_at, updated_at
             FROM conversations ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                system_prompt: row.get(2)?,
                model_id: row.get(3)?,
                region: row.get(4)?,
                created_at: row
                    .get::<_, String>(5)?
                    .parse()
                    .unwrap_or_default(),
                updated_at: row
                    .get::<_, String>(6)?
                    .parse()
                    .unwrap_or_default(),
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn upsert_conversation(&self, conv: &Conversation) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conversations (id, title, system_prompt, model_id, region, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                title=excluded.title,
                system_prompt=excluded.system_prompt,
                model_id=excluded.model_id,
                region=excluded.region,
                updated_at=excluded.updated_at",
            params![
                conv.id,
                conv.title,
                conv.system_prompt,
                conv.model_id,
                conv.region,
                conv.created_at.to_rfc3339(),
                conv.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_conversation(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM conversations WHERE id = ?1", params![id])?;
        Ok(())
    }

    // ── Messages ───────────────────────────────────────────────────────

    pub fn list_messages(&self, conversation_id: &str) -> Result<Vec<ChatMessage>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, conversation_id, role, content, created_at
             FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            Ok(ChatMessage {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: Role::from_str(&row.get::<_, String>(2)?),
                content: row.get(3)?,
                created_at: row
                    .get::<_, String>(4)?
                    .parse()
                    .unwrap_or_default(),
                version: 0,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn insert_message(&self, msg: &ChatMessage) -> Result<()> {
        self.conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                msg.id,
                msg.conversation_id,
                msg.role.as_str(),
                msg.content,
                msg.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn update_message_content(&self, id: &str, content: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE messages SET content = ?1 WHERE id = ?2",
            params![content, id],
        )?;
        Ok(())
    }

    // ── Config (key-value) ─────────────────────────────────────────────

    pub fn get_config(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM config WHERE key = ?1")?;
        let mut rows = stmt.query_map(params![key], |row| row.get(0))?;
        match rows.next() {
            Some(Ok(val)) => Ok(Some(val)),
            _ => Ok(None),
        }
    }

    pub fn set_config(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO config (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}
