use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{AppHandle, Manager};

/// SQLite connection for interview RAG (managed by Tauri).
pub struct InterviewDb(pub Mutex<Connection>);

pub fn sqlite_path(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("app_data_dir: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create app_data_dir: {e}"))?;
    Ok(dir.join("assistant.sqlite3"))
}

pub fn open_connection(app: &AppHandle) -> Result<Connection, String> {
    let path = sqlite_path(app)?;
    let conn = Connection::open(path).map_err(|e| format!("sqlite open: {e}"))?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;

        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            user_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_messages_user_time ON messages(user_id, created_at DESC);

        CREATE TABLE IF NOT EXISTS summaries (
            user_id TEXT PRIMARY KEY,
            summary TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS memories (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            content TEXT NOT NULL,
            importance INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_memories_user ON memories(user_id);

        CREATE TABLE IF NOT EXISTS documents (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            doc_type TEXT NOT NULL,
            filename TEXT NOT NULL,
            bytes BLOB,
            extracted_text TEXT NOT NULL,
            created_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_documents_user ON documents(user_id);

        CREATE TABLE IF NOT EXISTS profile_items (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL DEFAULT '',
            content TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_profile_user ON profile_items(user_id);
        "#,
    )
    .map_err(|e| format!("sqlite migrate: {e}"))?;
    Ok(())
}

pub fn insert_message(
    conn: &Connection,
    user_id: &str,
    role: &str,
    content: &str,
) -> Result<i64, String> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO messages (user_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![user_id, role, content, now],
    )
    .map_err(|e| format!("insert message: {e}"))?;
    Ok(conn.last_insert_rowid())
}

pub fn recent_messages(
    conn: &Connection,
    user_id: &str,
    limit: usize,
) -> Result<Vec<(String, String, String)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT role, content, created_at FROM messages WHERE user_id = ?1 ORDER BY created_at DESC LIMIT ?2",
        )
        .map_err(|e| format!("prepare messages: {e}"))?;
    let rows = stmt
        .query_map(params![user_id, limit as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("query messages: {e}"))?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| e.to_string())?);
    }
    out.reverse();
    Ok(out)
}

pub fn get_summary(conn: &Connection, user_id: &str) -> Result<Option<String>, String> {
    let mut stmt = conn
        .prepare("SELECT summary FROM summaries WHERE user_id = ?1")
        .map_err(|e| format!("prepare summary: {e}"))?;
    stmt.query_row(params![user_id], |row| row.get(0))
        .optional()
        .map_err(|e| format!("summary: {e}"))
}

/// Replace document rows for cv/jd for this user (one row per doc_type).
pub fn upsert_document(
    conn: &Connection,
    id: &str,
    user_id: &str,
    doc_type: &str,
    filename: &str,
    bytes: Option<&[u8]>,
    extracted_text: &str,
) -> Result<(), String> {
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "DELETE FROM documents WHERE user_id = ?1 AND doc_type = ?2",
        params![user_id, doc_type],
    )
    .map_err(|e| format!("delete old doc: {e}"))?;
    conn.execute(
        "INSERT INTO documents (id, user_id, doc_type, filename, bytes, extracted_text, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![id, user_id, doc_type, filename, bytes, extracted_text, now],
    )
    .map_err(|e| format!("insert document: {e}"))?;
    Ok(())
}

/// All ingested documents for a user — (doc_type, extracted_text).
pub fn get_documents(conn: &Connection, user_id: &str) -> Result<Vec<(String, String)>, String> {
    let mut stmt = conn
        .prepare("SELECT doc_type, extracted_text FROM documents WHERE user_id = ?1")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProfileItem {
    pub id: String,
    pub user_id: String,
    /// "summary" | "story" | "strength"
    pub kind: String,
    pub title: String,
    pub content: String,
    pub updated_at: String,
}

/// Persistent candidate profile entries. Order: summary first (one per user),
/// then the rest by updated_at DESC.
pub fn list_profile_items(conn: &Connection, user_id: &str) -> Result<Vec<ProfileItem>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT id, user_id, kind, title, content, updated_at FROM profile_items
             WHERE user_id = ?1
             ORDER BY CASE kind WHEN 'summary' THEN 0 ELSE 1 END, updated_at DESC",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![user_id], |r| {
            Ok(ProfileItem {
                id: r.get(0)?,
                user_id: r.get(1)?,
                kind: r.get(2)?,
                title: r.get(3)?,
                content: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row.map_err(|e| e.to_string())?);
    }
    Ok(out)
}

/// Insert or replace a profile item. `kind == "summary"` is unique per user —
/// upserting a summary replaces any previous one regardless of id.
pub fn upsert_profile_item(conn: &Connection, item: &ProfileItem) -> Result<(), String> {
    if item.kind == "summary" {
        conn.execute(
            "DELETE FROM profile_items WHERE user_id = ?1 AND kind = 'summary' AND id != ?2",
            params![item.user_id, item.id],
        )
        .map_err(|e| format!("delete old summary: {e}"))?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO profile_items (id, user_id, kind, title, content, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            item.id,
            item.user_id,
            item.kind,
            item.title,
            item.content,
            item.updated_at
        ],
    )
    .map_err(|e| format!("upsert profile item: {e}"))?;
    Ok(())
}

pub fn delete_profile_item(conn: &Connection, user_id: &str, id: &str) -> Result<(), String> {
    conn.execute(
        "DELETE FROM profile_items WHERE user_id = ?1 AND id = ?2",
        params![user_id, id],
    )
    .map_err(|e| format!("delete profile item: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        migrate(&c).unwrap();
        c
    }

    fn item(id: &str, user: &str, kind: &str, title: &str, at: &str) -> ProfileItem {
        ProfileItem {
            id: id.into(),
            user_id: user.into(),
            kind: kind.into(),
            title: title.into(),
            content: format!("content-{id}"),
            updated_at: at.into(),
        }
    }

    #[test]
    fn profile_roundtrip_and_order() {
        let c = mem();
        upsert_profile_item(
            &c,
            &item("s1", "u", "story", "story one", "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        upsert_profile_item(
            &c,
            &item("g1", "u", "strength", "rust", "2026-01-02T00:00:00Z"),
        )
        .unwrap();
        upsert_profile_item(&c, &item("sum", "u", "summary", "", "2026-01-03T00:00:00Z")).unwrap();
        let items = list_profile_items(&c, "u").unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].kind, "summary");
        assert_eq!(items[0].content, "content-sum");
        // remaining by updated_at DESC
        assert_eq!(items[1].id, "g1");
        assert_eq!(items[2].id, "s1");
    }

    #[test]
    fn profile_single_summary_per_user() {
        let c = mem();
        upsert_profile_item(&c, &item("a", "u", "summary", "", "2026-01-01T00:00:00Z")).unwrap();
        upsert_profile_item(&c, &item("b", "u", "summary", "", "2026-01-02T00:00:00Z")).unwrap();
        let items = list_profile_items(&c, "u").unwrap();
        let summaries: Vec<_> = items.iter().filter(|i| i.kind == "summary").collect();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, "b");
    }

    #[test]
    fn profile_delete_scoped_to_user() {
        let c = mem();
        upsert_profile_item(&c, &item("s1", "u", "story", "t", "2026-01-01T00:00:00Z")).unwrap();
        delete_profile_item(&c, "other-user", "s1").unwrap();
        assert_eq!(list_profile_items(&c, "u").unwrap().len(), 1);
        delete_profile_item(&c, "u", "s1").unwrap();
        assert!(list_profile_items(&c, "u").unwrap().is_empty());
    }
}
