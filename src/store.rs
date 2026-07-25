//! SQLite side of the store: file bookkeeping and chunk payloads.
//!
//! The vector index only ever holds `(chunk id, quantized vector)`. Everything
//! a caller actually wants back — the text, the path, the line span — lives
//! here and is looked up by chunk id after the search returns.

use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    k TEXT PRIMARY KEY,
    v TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id         INTEGER PRIMARY KEY,
    path       TEXT NOT NULL UNIQUE,
    hash       TEXT NOT NULL,
    bytes      INTEGER NOT NULL,
    indexed_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS chunks (
    id         INTEGER PRIMARY KEY,
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    ord        INTEGER NOT NULL,
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    text       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS chunks_file_id ON chunks(file_id);
"#;

/// A chunk row joined with its file's path — what a search hit resolves to.
#[derive(Debug, Clone)]
pub struct ChunkRow {
    pub id: i64,
    pub path: String,
    pub ord: i64,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

pub fn open(path: &Path) -> Result<Connection> {
    let db = Connection::open(path)?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    db.pragma_update(None, "foreign_keys", "ON")?;
    db.execute_batch(SCHEMA)?;
    Ok(db)
}

pub fn get_meta(db: &Connection, k: &str) -> Result<Option<String>> {
    Ok(db
        .query_row("SELECT v FROM meta WHERE k = ?1", params![k], |r| r.get(0))
        .optional()?)
}

pub fn set_meta(db: &Connection, k: &str, v: &str) -> Result<()> {
    db.execute(
        "INSERT INTO meta (k, v) VALUES (?1, ?2)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![k, v],
    )?;
    Ok(())
}

/// Content hash recorded for `path`, if we have indexed it before.
pub fn file_hash(db: &Connection, path: &str) -> Result<Option<String>> {
    Ok(db
        .query_row(
            "SELECT hash FROM files WHERE path = ?1",
            params![path],
            |r| r.get(0),
        )
        .optional()?)
}

/// Drop a file and its chunks, returning the chunk ids so the caller can
/// evict them from the vector index too.
pub fn delete_file(db: &Connection, path: &str) -> Result<Vec<u64>> {
    let ids: Vec<u64> = {
        let mut stmt = db.prepare(
            "SELECT c.id FROM chunks c JOIN files f ON f.id = c.file_id WHERE f.path = ?1",
        )?;
        let rows = stmt.query_map(params![path], |r| r.get::<_, i64>(0))?;
        rows.collect::<Result<Vec<i64>, _>>()?
            .into_iter()
            .map(|i| i as u64)
            .collect()
    };
    db.execute("DELETE FROM files WHERE path = ?1", params![path])?;
    Ok(ids)
}

pub fn insert_file(db: &Connection, path: &str, hash: &str, bytes: u64, now: i64) -> Result<i64> {
    db.execute(
        "INSERT INTO files (path, hash, bytes, indexed_at) VALUES (?1, ?2, ?3, ?4)",
        params![path, hash, bytes as i64, now],
    )?;
    Ok(db.last_insert_rowid())
}

pub fn insert_chunk(
    db: &Connection,
    file_id: i64,
    ord: usize,
    start_line: u32,
    end_line: u32,
    text: &str,
) -> Result<i64> {
    db.execute(
        "INSERT INTO chunks (file_id, ord, start_line, end_line, text)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![file_id, ord as i64, start_line, end_line, text],
    )?;
    Ok(db.last_insert_rowid())
}

pub fn chunk(db: &Connection, id: u64) -> Result<Option<ChunkRow>> {
    Ok(db
        .query_row(
            "SELECT c.id, f.path, c.ord, c.start_line, c.end_line, c.text
             FROM chunks c JOIN files f ON f.id = c.file_id
             WHERE c.id = ?1",
            params![id as i64],
            |r| {
                Ok(ChunkRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    ord: r.get(2)?,
                    start_line: r.get(3)?,
                    end_line: r.get(4)?,
                    text: r.get(5)?,
                })
            },
        )
        .optional()?)
}

pub fn all_paths(db: &Connection) -> Result<Vec<String>> {
    let mut stmt = db.prepare("SELECT path FROM files ORDER BY path")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `(files, chunks, indexed bytes)`
pub fn stats(db: &Connection) -> Result<(i64, i64, i64)> {
    let files: i64 = db.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let chunks: i64 = db.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?;
    let bytes: i64 = db.query_row("SELECT COALESCE(SUM(bytes), 0) FROM files", [], |r| {
        r.get(0)
    })?;
    Ok((files, chunks, bytes))
}
