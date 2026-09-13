//! SQLite side of the store: file bookkeeping and chunk payloads.
//!
//! The vector index only ever holds `(chunk id, quantized vector)`. Everything
//! a caller actually wants back — the text, the path, the line span — lives
//! here and is looked up by chunk id after the search returns.

use anyhow::Result;
use rusqlite::types::Value;
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

-- AUTOINCREMENT, not a bare rowid: a sharded index routes a chunk id to a
-- shard by the range its file name claims, so SQLite handing a deleted row's
-- id to a new chunk would put one id inside two shards at once.
CREATE TABLE IF NOT EXISTS chunks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    ord        INTEGER NOT NULL,
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    text       TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS chunks_file_id ON chunks(file_id);

-- Keyword half of the search. `content='chunks'` means FTS5 keeps only its
-- index, not a second copy of every chunk, so the store barely grows.
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text,
    content='chunks',
    content_rowid='id',
    tokenize='unicode61'
);

-- External-content FTS5 does not track its source table by itself. These keep
-- the two in step; a delete has to hand back the original text so FTS5 can
-- find the terms it needs to remove. Foreign-key cascades fire them too, which
-- is what keeps `delete_file` correct.
CREATE TRIGGER IF NOT EXISTS chunks_fts_insert AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;

CREATE TRIGGER IF NOT EXISTS chunks_fts_delete AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES('delete', old.id, old.text);
END;

-- Structure half of the store, from 0.12.0. Additive: an older store grows
-- these tables on its next open and they stay empty until something indexes
-- into it, which is why the format version does not move for them. See
-- `docs/compatibility.md`.
--
-- `chunk_id` is nullable on purpose. A symbol's defining line always falls
-- inside some chunk, but a file whose chunks were rewritten between extraction
-- and insertion would otherwise have to fail rather than record the symbol.
CREATE TABLE IF NOT EXISTS symbols (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    chunk_id   INTEGER,
    kind       TEXT NOT NULL,
    name       TEXT NOT NULL,
    qualified  TEXT NOT NULL,
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS symbols_file_id ON symbols(file_id);
CREATE INDEX IF NOT EXISTS symbols_name ON symbols(name);

-- An edge is owned by the file its *source* is in: `src` is a symbol id, and
-- the cascade above deletes an edge when its source file is re-indexed or
-- forgotten.
--
-- `dst` is a symbol NAME, not an id, and that asymmetry is the point. Ids are
-- reissued every time a file is re-extracted, so an id here would mean that
-- re-indexing `b.rs` silently deleted every edge pointing into it from `a.rs`
-- — the graph would rot from the one operation this release exists to make
-- safe. A name is resolved against `symbols_name` at query time instead, so an
-- edge is always as current as both of its ends, and an edge to something not
-- indexed (a standard-library call) is still recorded and simply resolves to
-- nothing.
--
-- `confidence` is `extracted` when an import in the source file named where the
-- target came from, and `inferred` when the target was matched by bare name.
-- Nothing that displays an edge may present the second as the first.
CREATE TABLE IF NOT EXISTS edges (
    src        INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    dst        TEXT NOT NULL,
    kind       TEXT NOT NULL,
    confidence TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS edges_src ON edges(src);
CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst);

-- The retrieval ledger, from 0.12.0. Additive and empty unless recording is
-- switched on, which it is not by default.
--
-- `prev` is the hash of the row before it and `hash` covers this row's own
-- fields plus `prev`, so the table is a chain: editing or deleting a row
-- breaks every hash after it and `semlith ledger` can say so. That is what
-- makes this an audit record rather than a log file, and it costs one blake3
-- of a short string per query.
--
-- `excerpt_tokens` is what the agent actually read. `whole_file_tokens` is
-- what reading those files whole would have cost — the honest denominator for
-- a savings number, measured rather than claimed.
CREATE TABLE IF NOT EXISTS retrievals (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    at                INTEGER NOT NULL,
    client            TEXT NOT NULL,
    query             TEXT NOT NULL,
    hits              INTEGER NOT NULL,
    micros            INTEGER NOT NULL,
    excerpt_tokens    INTEGER NOT NULL,
    whole_file_tokens INTEGER NOT NULL,
    prev              TEXT NOT NULL,
    hash              TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS retrievals_at ON retrievals(at);
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
    check_format(&db)?;
    backfill_fts(&db)?;
    Ok(db)
}

/// The store layout this binary understands.
///
/// Written into a store when one is created, and absent from every store
/// written before 0.6.0 — which is what makes 1 the right reading of an absent
/// key rather than an unknown.
///
/// 1 is a single `index.tv`. 2 is a directory of shards, written by 0.7.0 and
/// later; see [`crate::index`]. A binary understands every format up to its
/// own, so this one reads both and creates the newer.
pub const FORMAT_VERSION: u32 = 2;

/// The first format that keeps its vectors in shards.
pub const SHARDED_FORMAT: u32 = 2;

/// Which layout this store's vectors are in.
///
/// An absent key is format 1: every store written before 0.6.0 has no key, and
/// reading it as anything else would refuse the whole installed base.
pub fn format(db: &Connection) -> Result<u32> {
    Ok(get_meta(db, FORMAT_KEY)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(1))
}

pub const FORMAT_KEY: &str = "format_version";

/// Refuse a store a later semlith wrote, rather than misreading it.
///
/// Nothing is written here. A store from before this key existed is format 1
/// and stays exactly as it is: a read command that quietly rewrote the store
/// it was only asked to search would be a worse surprise than the one this
/// guards against.
fn check_format(db: &Connection) -> Result<()> {
    let Some(recorded) = get_meta(db, FORMAT_KEY)? else {
        return Ok(());
    };
    let found: u32 = recorded.parse().map_err(|_| {
        anyhow::anyhow!("store records {FORMAT_KEY} {recorded:?}, which is not a version number")
    })?;
    if found > FORMAT_VERSION {
        anyhow::bail!(
            "store is format {found}, and semlith {} understands format {FORMAT_VERSION}; \
             upgrade semlith to read this store",
            env!("CARGO_PKG_VERSION"),
        );
    }
    Ok(())
}

/// Meta key recording that the keyword index has been built for this store.
///
/// Counting rows in `chunks_fts` cannot answer this. An external-content FTS5
/// table reads through to its content table, so `COUNT(*)` returns the number
/// of chunks whether or not a single term has been indexed — a guard built on
/// it would skip the backfill on exactly the stores that need it.
const FTS_BUILT: &str = "fts_built";

/// Populate the keyword index for a store written before it existed.
///
/// A 0.1.0 store has chunks but no FTS index, and the triggers only fire on new
/// writes. Rebuilding from the text already in SQLite costs no embedding and
/// leaves the vectors untouched.
fn backfill_fts(db: &Connection) -> Result<()> {
    if get_meta(db, FTS_BUILT)?.is_some() {
        return Ok(());
    }
    // FTS5's own command for external content: discard the index and rebuild
    // it from the content table.
    db.execute_batch("INSERT INTO chunks_fts(chunks_fts) VALUES('rebuild');")?;
    set_meta(db, FTS_BUILT, "1")?;
    Ok(())
}

/// `AND`ed groups of `OR`ed `GLOB` patterns as a SQL fragment, plus the values
/// to bind. An empty group list yields `1` — a predicate that selects
/// everything, so callers need no special case.
///
/// Patterns are matched against `lower(files.path)`: filtering is
/// case-insensitive, because `README.MD` and `readme.md` are the same file to
/// anyone typing `--ext md`, and no filesystem this runs on treats a path's
/// case as load-bearing.
fn glob_predicate(groups: &[Vec<String>]) -> (String, Vec<String>) {
    let mut binds = Vec::new();
    let mut clauses = Vec::new();
    for group in groups {
        let ors: Vec<&str> = group
            .iter()
            .map(|p| {
                binds.push(p.clone());
                "lower(f.path) GLOB ?"
            })
            .collect();
        clauses.push(format!("({})", ors.join(" OR ")));
    }
    if clauses.is_empty() {
        return ("1".to_string(), binds);
    }
    (clauses.join(" AND "), binds)
}

/// Ids of every chunk whose file matches `groups`.
///
/// This is the single source of the subset. The vector index gets these ids as
/// an allowlist and [`keyword_search`] applies the same predicate, so the two
/// halves of a hybrid search can never rank a chunk the other was forbidden to
/// see.
pub fn filtered_chunk_ids(db: &Connection, groups: &[Vec<String>]) -> Result<Vec<u64>> {
    let (predicate, binds) = glob_predicate(groups);
    let sql =
        format!("SELECT c.id FROM chunks c JOIN files f ON f.id = c.file_id WHERE {predicate}");
    let mut stmt = db.prepare(&sql)?;
    let args = binds.into_iter().map(Value::Text);
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
    Ok(rows
        .collect::<Result<Vec<i64>, _>>()?
        .into_iter()
        .map(|i| i as u64)
        .collect())
}

/// How many indexed files `groups` selects.
///
/// Zero means the filter itself is wrong — a typo in a glob, a subsystem that
/// was never indexed — which is a different thing to tell a user than "the
/// corpus does not contain this".
pub fn matching_files(db: &Connection, groups: &[Vec<String>]) -> Result<i64> {
    let (predicate, binds) = glob_predicate(groups);
    let sql = format!("SELECT COUNT(*) FROM files f WHERE {predicate}");
    let args = binds.into_iter().map(Value::Text);
    Ok(db.query_row(&sql, rusqlite::params_from_iter(args), |r| r.get(0))?)
}

/// The keyword statement for a search with no filter.
///
/// Byte-identical to the one 0.2.0 issued: no join to `chunks` and `files`, so
/// a query that uses no filter pays nothing for the filtering feature. A test
/// pins the text, because the cheapest way to regress the common path is to
/// "tidy" the two branches into one.
const UNFILTERED_SQL: &str =
    "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ? ORDER BY rank LIMIT ?";

/// Chunk ids matching `query` as keywords, best first, restricted to files
/// matching `groups`.
///
/// The query is reduced to bare terms before it reaches FTS5. A raw query would
/// be parsed as FTS5 syntax, where a stray quote or `*` is a syntax error and a
/// bare `AND` is an operator — a search for "index AND search" would silently
/// mean something the user did not type, and a search for `foo(` would fail
/// outright.
pub fn keyword_search(
    db: &Connection,
    query: &str,
    limit: usize,
    groups: &[Vec<String>],
) -> Result<Vec<u64>> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let match_expr = terms.join(" OR ");

    let (sql, mut args) = if groups.is_empty() {
        (UNFILTERED_SQL.to_string(), vec![Value::Text(match_expr)])
    } else {
        let (predicate, binds) = glob_predicate(groups);
        let mut args = vec![Value::Text(match_expr)];
        args.extend(binds.into_iter().map(Value::Text));
        (
            format!(
                "SELECT x.rowid FROM chunks_fts x
                 JOIN chunks c ON c.id = x.rowid
                 JOIN files f ON f.id = c.file_id
                 WHERE x.chunks_fts MATCH ? AND {predicate}
                 ORDER BY x.rank LIMIT ?"
            ),
            args,
        )
    };
    // Bound last because `LIMIT` is the final placeholder in either statement.
    args.push(Value::Integer(limit as i64));

    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
    Ok(rows
        .collect::<Result<Vec<i64>, _>>()?
        .into_iter()
        .map(|i| i as u64)
        .collect())
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

/// One row per indexed file: path, bytes, chunks, the last line any chunk of
/// it covers, and when it was last indexed.
#[derive(Debug, Clone)]
pub struct FileRow {
    pub path: String,
    pub bytes: i64,
    pub chunks: i64,
    pub lines: i64,
    pub indexed_at: i64,
}

/// The column a Files listing is ordered by.
///
/// An enum rather than a string spliced into the SQL: the sort column arrives
/// from a query parameter, and the only safe way to put a caller's value in an
/// `ORDER BY` — which cannot be bound as a parameter — is to never put their
/// bytes there at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSort {
    Path,
    Bytes,
    Chunks,
    Lines,
    Indexed,
}

impl FileSort {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "path" => Some(Self::Path),
            "bytes" => Some(Self::Bytes),
            "chunks" => Some(Self::Chunks),
            "lines" => Some(Self::Lines),
            "indexed" => Some(Self::Indexed),
            _ => None,
        }
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Path => "f.path",
            Self::Bytes => "f.bytes",
            Self::Chunks => "COUNT(c.id)",
            Self::Lines => "COALESCE(MAX(c.end_line), 0)",
            Self::Indexed => "f.indexed_at",
        }
    }
}

/// How many indexed files match `groups`.
///
/// Separate from the rows because the Files table pages: the page is bounded
/// and the total is not, and returning every row to count them is what the old
/// silent truncation was hiding.
pub fn file_count(db: &Connection, groups: &[Vec<String>]) -> Result<i64> {
    let (predicate, binds) = glob_predicate(groups);
    let sql = format!("SELECT COUNT(*) FROM files f WHERE {predicate}");
    let mut stmt = db.prepare(&sql)?;
    let args = binds.into_iter().map(Value::Text);
    Ok(stmt.query_row(rusqlite::params_from_iter(args), |r| r.get(0))?)
}

/// The first `take` matching files in the requested order.
///
/// The Files view's row, in one query. Asking per file would be one statement
/// per file, and a corpus is tens of thousands of them. The ordering is done
/// here rather than in the browser so that sorting orders the whole store
/// rather than the page of it that happens to be loaded — with several stores
/// open the caller takes `offset + limit` from each and merges, which is why
/// this takes a count rather than an offset.
pub fn file_rows(
    db: &Connection,
    groups: &[Vec<String>],
    sort: FileSort,
    desc: bool,
    take: i64,
) -> Result<Vec<FileRow>> {
    let (predicate, binds) = glob_predicate(groups);
    let direction = if desc { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT f.path, f.bytes, COUNT(c.id), COALESCE(MAX(c.end_line), 0), f.indexed_at \
         FROM files f LEFT JOIN chunks c ON c.file_id = f.id \
         WHERE {predicate} GROUP BY f.id ORDER BY {} {direction}, f.path ASC LIMIT ?",
        sort.sql()
    );
    let mut stmt = db.prepare(&sql)?;
    let mut args: Vec<Value> = binds.into_iter().map(Value::Text).collect();
    args.push(Value::Integer(take.max(0)));
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
        Ok(FileRow {
            path: r.get(0)?,
            bytes: r.get(1)?,
            chunks: r.get(2)?,
            lines: r.get(3)?,
            indexed_at: r.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// What a listing spans: the distinct file extensions in it, and the total
/// number of lines across every file.
///
/// Both are things the Stores and Files pages state and neither `stats()` nor
/// a page of rows can answer — a page knows about its own rows, and a header
/// that reports "9 formats" from the fifteen rows on screen is reporting the
/// page rather than the store.
#[derive(Debug, Default, Clone)]
pub struct Facets {
    pub extensions: Vec<String>,
    pub lines: i64,
}

pub fn file_facets(db: &Connection, groups: &[Vec<String>]) -> Result<Facets> {
    let (predicate, binds) = glob_predicate(groups);

    // The extension is taken in Rust rather than in SQL. SQLite has no
    // right-hand search, and the usual `rtrim`/`replace` idiom for it reads a
    // dot in a directory name as the start of an extension — which is a wrong
    // answer in a column that exists to say what the corpus is made of.
    let sql = format!("SELECT f.path FROM files f WHERE {predicate}");
    let mut stmt = db.prepare(&sql)?;
    let args = binds.clone().into_iter().map(Value::Text);
    let paths = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?;
    let mut extensions: Vec<String> = paths
        .collect::<Result<Vec<String>, _>>()?
        .iter()
        .filter_map(|p| {
            std::path::Path::new(p)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
        })
        .collect();
    extensions.sort();
    extensions.dedup();

    let sql = format!(
        "SELECT COALESCE(SUM(n), 0) FROM ( \
           SELECT COALESCE(MAX(c.end_line), 0) AS n \
           FROM files f LEFT JOIN chunks c ON c.file_id = f.id \
           WHERE {predicate} GROUP BY f.id)"
    );
    let mut stmt = db.prepare(&sql)?;
    let args = binds.into_iter().map(Value::Text);
    let lines: i64 = stmt.query_row(rusqlite::params_from_iter(args), |r| r.get(0))?;

    Ok(Facets { extensions, lines })
}

pub fn all_paths(db: &Connection) -> Result<Vec<String>> {
    filtered_paths(db, &[])
}

/// Indexed paths whose file matches `groups`, in path order.
///
/// The same predicate the search halves are built on, so "which files would
/// this filter reach" and "which files did this filter search" cannot drift
/// apart and give an agent two different answers about one store.
pub fn filtered_paths(db: &Connection, groups: &[Vec<String>]) -> Result<Vec<String>> {
    let (predicate, binds) = glob_predicate(groups);
    let sql = format!("SELECT f.path FROM files f WHERE {predicate} ORDER BY f.path");
    let mut stmt = db.prepare(&sql)?;
    let args = binds.into_iter().map(Value::Text);
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Chunks whose file has a committed hash — the ones whose vectors are on disk.
///
/// A file is recorded with the [`PENDING`](crate::PENDING) empty hash until its
/// vectors are durable, so this is exactly what the vector index holds at rest,
/// and it costs one indexed count instead of reading the index to find out.
pub fn durable_chunks(db: &Connection) -> Result<i64> {
    Ok(db.query_row(
        "SELECT COUNT(*) FROM chunks c JOIN files f ON f.id = c.file_id WHERE f.hash != ''",
        [],
        |r| r.get(0),
    )?)
}

/// Apply the schema to a bare in-memory connection, for tests in other
/// modules that need somewhere to put symbols and edges.
#[cfg(test)]
pub fn prepare_for_tests(db: &Connection) {
    db.execute_batch(SCHEMA).unwrap();
    db.pragma_update(None, "foreign_keys", "ON").unwrap();
}

/// A symbol row joined with its file's path — what a graph answer resolves to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SymbolRow {
    pub id: i64,
    pub path: String,
    pub kind: String,
    pub name: String,
    pub qualified: String,
    pub start_line: u32,
    pub end_line: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunk_id: Option<i64>,
    /// Which store this came from, when more than one is open. Absent for a
    /// single store, so one store's output is what it would have been before
    /// stores could be combined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// One end of a traversal: the symbol reached, and the edge that reached it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EdgeEnd {
    #[serde(flatten)]
    pub symbol: SymbolRow,
    pub kind: String,
    pub confidence: String,
}

const SYMBOL_COLUMNS: &str =
    "s.id, f.path, s.kind, s.name, s.qualified, s.start_line, s.end_line, s.chunk_id";

fn symbol_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<SymbolRow> {
    Ok(SymbolRow {
        id: r.get(0)?,
        path: r.get(1)?,
        kind: r.get(2)?,
        name: r.get(3)?,
        qualified: r.get(4)?,
        start_line: r.get(5)?,
        end_line: r.get(6)?,
        chunk_id: r.get(7)?,
        store: None,
    })
}

pub fn insert_symbol(
    db: &Connection,
    file_id: i64,
    chunk_id: Option<i64>,
    symbol: &crate::graph::Symbol,
) -> Result<i64> {
    db.execute(
        "INSERT INTO symbols (file_id, chunk_id, kind, name, qualified, start_line, end_line)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            file_id,
            chunk_id,
            symbol.kind,
            symbol.name,
            symbol.qualified,
            symbol.start_line,
            symbol.end_line
        ],
    )?;
    Ok(db.last_insert_rowid())
}

pub fn insert_edge(
    db: &Connection,
    src: i64,
    dst: &str,
    kind: &str,
    confidence: &str,
) -> Result<()> {
    db.execute(
        "INSERT INTO edges (src, dst, kind, confidence) VALUES (?1, ?2, ?3, ?4)",
        params![src, dst, kind, confidence],
    )?;
    Ok(())
}

/// Every definition of `name`, across the whole store.
///
/// Exact match, not a glob: `semlith symbol acquire` asking about `acquire` and
/// being handed `acquire_timeout` as well would make the answer something the
/// caller has to re-filter.
pub fn symbols_named(db: &Connection, name: &str, limit: usize) -> Result<Vec<SymbolRow>> {
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.name = ?1 ORDER BY f.path, s.start_line LIMIT ?2"
    );
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map(params![name, limit as i64], symbol_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Symbols to draw for a scope, newest-defined first within each file.
///
/// `prefix` narrows to a directory or a file the way the Files view does.
/// Module symbols are left out: every file has one, so they would fill the
/// budget with nodes that say only "this file exists".
pub fn symbols_scoped(
    db: &Connection,
    prefix: Option<&str>,
    limit: usize,
) -> Result<Vec<SymbolRow>> {
    let (predicate, binds) = match prefix {
        Some(p) => {
            let pattern = format!("*{}*", p.to_lowercase());
            ("lower(f.path) GLOB ?".to_string(), vec![pattern])
        }
        None => ("1".to_string(), Vec::new()),
    };
    // Busiest first. A scope filled with symbols that touch nothing draws a
    // field of dots: technically the graph, and of no use to anyone looking at
    // it. Ordering by how connected a symbol is puts the shape on the screen.
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE {predicate} AND s.kind != 'module'
         ORDER BY (
           (SELECT COUNT(*) FROM edges e WHERE e.src = s.id)
           + (SELECT COUNT(*) FROM edges e WHERE e.dst = s.name)
         ) DESC, f.path, s.start_line LIMIT ?"
    );
    let mut stmt = db.prepare(&sql)?;
    let mut args: Vec<Value> = binds.into_iter().map(Value::Text).collect();
    args.push(Value::Integer(limit as i64));
    let rows = stmt.query_map(rusqlite::params_from_iter(args), symbol_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Symbols defined in files matching `groups` whose name is one of `names`.
///
/// The resolution step every traversal shares: an edge names its target, and
/// this is what turns that name back into rows. The filter is threaded through
/// so graph expansion obeys the same chunk-id set as the other two search
/// halves rather than reaching outside it.
pub fn symbols_by_names(
    db: &Connection,
    names: &[String],
    groups: &[Vec<String>],
) -> Result<Vec<SymbolRow>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let (predicate, mut binds) = glob_predicate(groups);
    let holes = vec!["?"; names.len()].join(", ");
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS} FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE {predicate} AND s.name IN ({holes}) ORDER BY f.path, s.start_line"
    );
    binds.extend(names.iter().cloned());
    let mut stmt = db.prepare(&sql)?;
    let args = binds.into_iter().map(Value::Text);
    let rows = stmt.query_map(rusqlite::params_from_iter(args), symbol_row)?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// What the symbols named `name` point at: callees, imports, references out.
///
/// One hop. `kinds` empty means every edge kind.
pub fn edges_out(db: &Connection, name: &str, kinds: &[String]) -> Result<Vec<EdgeEnd>> {
    let filter = kind_predicate(kinds, "e.kind");
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS}, e.kind, e.confidence
         FROM symbols src
         JOIN edges e ON e.src = src.id
         JOIN symbols s ON s.name = e.dst
         JOIN files f ON f.id = s.file_id
         WHERE src.name = ?1 AND {filter}
         ORDER BY f.path, s.start_line"
    );
    let mut stmt = db.prepare(&sql)?;
    let mut binds: Vec<Value> = vec![Value::Text(name.to_string())];
    binds.extend(kinds.iter().map(|k| Value::Text(k.clone())));
    let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok(EdgeEnd {
            symbol: symbol_row(r)?,
            kind: r.get(8)?,
            confidence: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// What points at `name`: callers, importers, references in.
///
/// The direction `impact` walks, and the reason `edges_dst` exists.
pub fn edges_in(db: &Connection, name: &str, kinds: &[String]) -> Result<Vec<EdgeEnd>> {
    let filter = kind_predicate(kinds, "e.kind");
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS}, e.kind, e.confidence
         FROM edges e
         JOIN symbols s ON s.id = e.src
         JOIN files f ON f.id = s.file_id
         WHERE e.dst = ?1 AND {filter}
         ORDER BY f.path, s.start_line"
    );
    let mut stmt = db.prepare(&sql)?;
    let mut binds: Vec<Value> = vec![Value::Text(name.to_string())];
    binds.extend(kinds.iter().map(|k| Value::Text(k.clone())));
    let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok(EdgeEnd {
            symbol: symbol_row(r)?,
            kind: r.get(8)?,
            confidence: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

fn kind_predicate(kinds: &[String], column: &str) -> String {
    if kinds.is_empty() {
        return "1".to_string();
    }
    let holes = vec!["?"; kinds.len()].join(", ");
    format!("{column} IN ({holes})")
}

/// The names of every symbol whose defining lines overlap one of `chunk_ids`.
///
/// The seed of graph expansion: search returns chunks, the graph knows symbols,
/// and this is the join between them.
pub fn symbols_in_chunks(db: &Connection, chunk_ids: &[u64]) -> Result<Vec<String>> {
    if chunk_ids.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; chunk_ids.len()].join(", ");
    let sql = format!("SELECT DISTINCT name FROM symbols WHERE chunk_id IN ({holes})");
    let mut stmt = db.prepare(&sql)?;
    let args = chunk_ids.iter().map(|i| Value::Integer(*i as i64));
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// One recorded retrieval.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Retrieval {
    pub id: i64,
    pub at: i64,
    pub client: String,
    pub query: String,
    pub hits: i64,
    pub micros: i64,
    pub excerpt_tokens: i64,
    pub whole_file_tokens: i64,
    pub hash: String,
}

/// Append one retrieval, chained to the row before it.
///
/// The chain is the point: a row cannot be quietly edited or removed without
/// every hash after it failing to recompute.
pub fn record_retrieval(
    db: &Connection,
    client: &str,
    query: &str,
    hits: i64,
    micros: i64,
    excerpt_tokens: i64,
    whole_file_tokens: i64,
) -> Result<()> {
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let prev: String = db
        .query_row(
            "SELECT hash FROM retrievals ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default();
    let hash = chain_hash(
        &prev,
        at,
        client,
        query,
        hits,
        micros,
        excerpt_tokens,
        whole_file_tokens,
    );
    db.execute(
        "INSERT INTO retrievals
         (at, client, query, hits, micros, excerpt_tokens, whole_file_tokens, prev, hash)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            at,
            client,
            query,
            hits,
            micros,
            excerpt_tokens,
            whole_file_tokens,
            prev,
            hash
        ],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn chain_hash(
    prev: &str,
    at: i64,
    client: &str,
    query: &str,
    hits: i64,
    micros: i64,
    excerpt: i64,
    whole: i64,
) -> String {
    let payload = format!(
        "{prev}\u{1f}{at}\u{1f}{client}\u{1f}{query}\u{1f}{hits}\u{1f}{micros}\u{1f}{excerpt}\u{1f}{whole}"
    );
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

/// The most recent `limit` retrievals, newest first.
pub fn retrievals(db: &Connection, limit: usize) -> Result<Vec<Retrieval>> {
    let mut stmt = db.prepare(
        "SELECT id, at, client, query, hits, micros, excerpt_tokens, whole_file_tokens, hash
         FROM retrievals ORDER BY id DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map(params![limit as i64], |r| {
        Ok(Retrieval {
            id: r.get(0)?,
            at: r.get(1)?,
            client: r.get(2)?,
            query: r.get(3)?,
            hits: r.get(4)?,
            micros: r.get(5)?,
            excerpt_tokens: r.get(6)?,
            whole_file_tokens: r.get(7)?,
            hash: r.get(8)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `(queries, clients, excerpt tokens, whole-file tokens)` over the whole
/// ledger — what the Ledger page shows.
pub fn ledger_totals(db: &Connection) -> Result<(i64, i64, i64, i64)> {
    Ok(db.query_row(
        "SELECT COUNT(*), COUNT(DISTINCT client), COALESCE(SUM(excerpt_tokens), 0),
                COALESCE(SUM(whole_file_tokens), 0) FROM retrievals",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?)
}

/// Re-walk the chain and return the id of the first row that does not verify.
///
/// `None` means the ledger is intact.
pub fn ledger_break(db: &Connection) -> Result<Option<i64>> {
    let mut stmt = db.prepare(
        "SELECT id, at, client, query, hits, micros, excerpt_tokens, whole_file_tokens, prev, hash
         FROM retrievals ORDER BY id",
    )?;
    let mut rows = stmt.query([])?;
    let mut expected = String::new();
    while let Some(r) = rows.next()? {
        let (id, at): (i64, i64) = (r.get(0)?, r.get(1)?);
        let client: String = r.get(2)?;
        let query: String = r.get(3)?;
        let (hits, micros, excerpt, whole): (i64, i64, i64, i64) =
            (r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?);
        let prev: String = r.get(8)?;
        let hash: String = r.get(9)?;
        if prev != expected {
            return Ok(Some(id));
        }
        let recomputed = chain_hash(&prev, at, &client, &query, hits, micros, excerpt, whole);
        if recomputed != hash {
            return Ok(Some(id));
        }
        expected = hash;
    }
    Ok(None)
}

/// `(symbols, edges)` — the graph's size, for `stats` and the portal.
pub fn graph_stats(db: &Connection) -> Result<(i64, i64)> {
    let symbols: i64 = db.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get(0))?;
    let edges: i64 = db.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get(0))?;
    Ok((symbols, edges))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str) -> crate::graph::Symbol {
        crate::graph::Symbol {
            kind: "function".to_string(),
            name: name.to_string(),
            qualified: name.to_string(),
            start_line: 1,
            end_line: 2,
        }
    }

    fn at(mut s: crate::graph::Symbol, start: u32, end: u32) -> crate::graph::Symbol {
        s.start_line = start;
        s.end_line = end;
        s
    }

    /// Build a store with one file, one chunk and one symbol, and return the
    /// connection plus the symbol's id.
    fn one_symbol(db: &Connection, path: &str, name: &str) -> i64 {
        db.execute_batch(SCHEMA).unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        let file_id = insert_file(db, path, "h", 1, 0).unwrap();
        let chunk_id = insert_chunk(db, file_id, 0, 1, 2, name).unwrap();
        insert_symbol(db, file_id, Some(chunk_id), &sym(name)).unwrap()
    }

    /// Forgetting a file takes its symbols and its outgoing edges with it.
    ///
    /// The cascade does this, not the caller — which is why it is worth a test:
    /// a missing `ON DELETE CASCADE` would leave rows pointing at a file id
    /// SQLite is free to reissue.
    #[test]
    fn deleting_a_file_deletes_its_symbols_and_its_outgoing_edges() {
        let db = Connection::open_in_memory().unwrap();
        let caller = one_symbol(&db, "a.rs", "caller");
        insert_edge(&db, caller, "callee", "calls", "inferred").unwrap();
        assert_eq!(graph_stats(&db).unwrap(), (1, 1));

        delete_file(&db, "a.rs").unwrap();
        assert_eq!(
            graph_stats(&db).unwrap(),
            (0, 0),
            "the file's symbols and the edges leaving them both cascade"
        );
    }

    /// Re-indexing the file an edge points *into* must not delete the edge.
    ///
    /// This is the whole reason `edges.dst` is a name and not an id. With an id
    /// there, re-extracting `b.rs` would reissue its symbol ids and orphan
    /// every edge from `a.rs` into it — the graph would rot on exactly the
    /// operation this release exists to make safe.
    #[test]
    fn re_indexing_the_target_file_leaves_edges_into_it_intact() {
        let db = Connection::open_in_memory().unwrap();
        let caller = one_symbol(&db, "a.rs", "caller");
        insert_edge(&db, caller, "callee", "calls", "inferred").unwrap();
        let b = insert_file(&db, "b.rs", "h", 1, 0).unwrap();
        insert_symbol(&db, b, None, &sym("callee")).unwrap();
        assert_eq!(edges_out(&db, "caller", &[]).unwrap().len(), 1);

        // b.rs changes: its symbols are dropped and re-extracted with new ids.
        delete_file(&db, "b.rs").unwrap();
        let b = insert_file(&db, "b.rs", "h2", 1, 0).unwrap();
        insert_symbol(&db, b, None, &at(sym("callee"), 9, 10)).unwrap();

        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(
            out.len(),
            1,
            "the edge survived its target being re-indexed"
        );
        assert_eq!(
            out[0].symbol.start_line, 9,
            "and now points at the new rows"
        );
    }

    /// Both directions resolve, and the edge kind filter applies to each.
    #[test]
    fn edges_resolve_in_both_directions_and_filter_by_kind() {
        let db = Connection::open_in_memory().unwrap();
        let caller = one_symbol(&db, "a.rs", "caller");
        let b = insert_file(&db, "b.rs", "h", 1, 0).unwrap();
        insert_symbol(&db, b, None, &sym("callee")).unwrap();
        insert_edge(&db, caller, "callee", "calls", "extracted").unwrap();
        insert_edge(&db, caller, "callee", "references", "inferred").unwrap();

        assert_eq!(edges_out(&db, "caller", &[]).unwrap().len(), 2);
        let calls = edges_out(&db, "caller", &["calls".to_string()]).unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].confidence, "extracted");

        let inbound = edges_in(&db, "callee", &["calls".to_string()]).unwrap();
        assert_eq!(inbound.len(), 1);
        assert_eq!(
            inbound[0].symbol.name, "caller",
            "edges_in reports the source"
        );
    }

    /// An edge to something the corpus does not contain is still recorded, and
    /// simply resolves to nothing. A call into the standard library is the
    /// common case, and dropping it would lose the fact that the call is there.
    #[test]
    fn an_edge_to_an_unindexed_target_resolves_to_nothing_without_erroring() {
        let db = Connection::open_in_memory().unwrap();
        let caller = one_symbol(&db, "a.rs", "caller");
        insert_edge(&db, caller, "println", "calls", "inferred").unwrap();
        assert!(edges_out(&db, "caller", &[]).unwrap().is_empty());
        assert_eq!(graph_stats(&db).unwrap().1, 1, "but the edge row is there");
    }

    /// Every store written before 0.6.0 lacks the key entirely. Reading that as
    /// anything but format 1 would refuse the whole installed base.
    #[test]
    fn a_store_without_the_key_is_the_format_we_understand() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        assert!(get_meta(&db, FORMAT_KEY).unwrap().is_none());
        check_format(&db).expect("a pre-0.6.0 store is readable");
        // And still has no key: opening a store must not rewrite it.
        assert!(get_meta(&db, FORMAT_KEY).unwrap().is_none());
    }

    /// The point of writing the format down: a later semlith's store is refused
    /// with both numbers, instead of read as though its layout were this one.
    #[test]
    fn a_newer_format_is_refused_naming_both_numbers() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let newer = FORMAT_VERSION + 1;
        set_meta(&db, FORMAT_KEY, &newer.to_string()).unwrap();

        let refused = check_format(&db).expect_err("a newer format must not be read");
        let said = refused.to_string();
        assert!(said.contains(&newer.to_string()), "{said}");
        assert!(
            said.contains(&FORMAT_VERSION.to_string()),
            "the error must name what this binary understands: {said}"
        );

        // The format this binary wrote is of course readable.
        set_meta(&db, FORMAT_KEY, &FORMAT_VERSION.to_string()).unwrap();
        check_format(&db).unwrap();
    }

    fn seeded() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let f = insert_file(&db, "/a/lib.rs", "h", 10, 0).unwrap();
        insert_chunk(&db, f, 0, 1, 5, "const EMBED_BATCH: usize = 32;").unwrap();
        insert_chunk(
            &db,
            f,
            1,
            6,
            9,
            "a paragraph about retry backoff and jitter",
        )
        .unwrap();
        db
    }

    /// A corpus with the same word in four files: two under `src`, two not,
    /// two Rust, two Markdown. Enough to tell a union from an intersection.
    /// A store holds the paths its own platform produces, and `filter::anchor`
    /// builds patterns to match those — backslashes on Windows, forward
    /// slashes everywhere else. A fixture written with one separator therefore
    /// tests nothing on the other platform, so these are spelled in the
    /// separator the code under test is going to use.
    fn native(path: &str) -> String {
        path.replace('/', std::path::MAIN_SEPARATOR_STR)
    }

    fn mixed() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.pragma_update(None, "foreign_keys", "ON").unwrap();
        db.execute_batch(SCHEMA).unwrap();
        for path in [
            "/proj/src/lib.rs",
            "/proj/src/notes.md",
            "/proj/vendor/other.rs",
            "/proj/README.MD",
        ] {
            let f = insert_file(&db, &native(path), "h", 10, 0).unwrap();
            insert_chunk(&db, f, 0, 1, 5, "retry backoff and jitter").unwrap();
        }
        db
    }

    /// `groups()` output for a filter, so the tests exercise the same patterns
    /// the CLI produces rather than hand-written globs that could drift from it.
    fn filter(paths: &[&str], exts: &[&str], langs: &[&str]) -> Vec<Vec<String>> {
        let own = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        crate::filter::Filter::new(&own(paths), &own(exts), &own(langs))
            .unwrap()
            .groups()
            .to_vec()
    }

    fn paths_of(db: &Connection, ids: &[u64]) -> Vec<String> {
        let mut out: Vec<String> = ids
            .iter()
            .map(|&i| chunk(db, i).unwrap().unwrap().path)
            .collect();
        out.sort();
        out
    }

    #[test]
    fn keyword_search_finds_an_exact_identifier() {
        let db = seeded();
        let hits = keyword_search(&db, "EMBED_BATCH", 10, &[]).unwrap();
        assert_eq!(hits, vec![1], "expected the defining chunk, got {hits:?}");
    }

    #[test]
    fn a_path_glob_selects_only_what_is_under_it() {
        let db = mixed();
        let ids = filtered_chunk_ids(&db, &filter(&["src/**"], &[], &[])).unwrap();
        assert_eq!(
            paths_of(&db, &ids),
            [native("/proj/src/lib.rs"), native("/proj/src/notes.md")]
        );
    }

    /// An absolute pattern means exactly itself: it must not pick up the
    /// `*/` anchoring a relative one gets.
    #[test]
    fn an_absolute_glob_matches_only_what_it_literally_covers() {
        let db = mixed();
        let ids = filtered_chunk_ids(&db, &filter(&["/proj/vendor/*"], &[], &[])).unwrap();
        assert_eq!(paths_of(&db, &ids), [native("/proj/vendor/other.rs")]);
    }

    #[test]
    fn extensions_union_and_a_language_resolves_to_the_same_set() {
        let db = mixed();
        let by_ext = filtered_chunk_ids(&db, &filter(&[], &["rs"], &[])).unwrap();
        let by_lang = filtered_chunk_ids(&db, &filter(&[], &[], &["rust"])).unwrap();
        assert_eq!(paths_of(&db, &by_ext), paths_of(&db, &by_lang));
        assert_eq!(
            paths_of(&db, &by_ext),
            [native("/proj/src/lib.rs"), native("/proj/vendor/other.rs")]
        );

        let both = filtered_chunk_ids(&db, &filter(&[], &["rs", "md"], &[])).unwrap();
        assert_eq!(both.len(), 4, "repeating --ext must union, not intersect");
    }

    /// `README.MD` is Markdown. Extension matching folds case, so a filter
    /// written the way anyone would write it does not miss it.
    #[test]
    fn extension_matching_is_case_insensitive() {
        let db = mixed();
        let ids = filtered_chunk_ids(&db, &filter(&[], &["md"], &[])).unwrap();
        assert_eq!(
            paths_of(&db, &ids),
            [native("/proj/README.MD"), native("/proj/src/notes.md")]
        );
    }

    #[test]
    fn a_path_and_an_extension_intersect() {
        let db = mixed();
        let ids = filtered_chunk_ids(&db, &filter(&["src/**"], &["md"], &[])).unwrap();
        assert_eq!(
            paths_of(&db, &ids),
            [native("/proj/src/notes.md")],
            "the README is Markdown but is not under src"
        );
    }

    /// The whole point of the release: the keyword half must see the same
    /// subset the vector half is given, or fusion ranks a chunk one side was
    /// never allowed to return.
    #[test]
    fn the_keyword_half_honours_the_same_filter() {
        let db = mixed();
        let unfiltered = keyword_search(&db, "retry backoff", 10, &[]).unwrap();
        assert_eq!(unfiltered.len(), 4);

        let scoped =
            keyword_search(&db, "retry backoff", 10, &filter(&["src/**"], &[], &[])).unwrap();
        assert_eq!(
            paths_of(&db, &scoped),
            [native("/proj/src/lib.rs"), native("/proj/src/notes.md")]
        );
    }

    #[test]
    fn a_filter_matching_nothing_selects_nothing() {
        let db = mixed();
        let groups = filter(&["nowhere/**"], &[], &[]);
        assert_eq!(matching_files(&db, &groups).unwrap(), 0);
        assert!(filtered_chunk_ids(&db, &groups).unwrap().is_empty());
        assert!(
            keyword_search(&db, "retry", 10, &groups)
                .unwrap()
                .is_empty()
        );
    }

    /// An unfiltered search must cost exactly what it cost in 0.2.0. The
    /// statement is the whole of that promise: adding the join unconditionally
    /// would tax every query that uses no filter, and would not fail any other
    /// test in this file.
    #[test]
    fn an_unfiltered_search_issues_the_0_2_0_statement_verbatim() {
        assert_eq!(
            UNFILTERED_SQL,
            "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ? ORDER BY rank LIMIT ?"
        );
        // And an empty filter must take that branch, not the general one.
        assert_eq!(glob_predicate(&[]), ("1".to_string(), vec![]));
    }

    #[test]
    fn no_filter_counts_every_file() {
        let db = mixed();
        assert_eq!(matching_files(&db, &[]).unwrap(), 4);
    }

    #[test]
    fn fts_syntax_in_a_query_is_not_executed_as_syntax() {
        let db = seeded();
        // Each of these is either an FTS5 syntax error or an FTS5 operator if
        // passed through raw. All must come back as ordinary searches.
        for query in [
            "EMBED_BATCH AND retry",
            "unbalanced \" quote",
            "call_me(",
            "prefix*",
            "NEAR(a b)",
            "-negated",
        ] {
            let result = keyword_search(&db, query, 10, &[]);
            assert!(
                result.is_ok(),
                "query {query:?} errored: {:?}",
                result.err()
            );
        }
    }

    #[test]
    fn a_query_with_no_terms_matches_nothing() {
        let db = seeded();
        assert!(keyword_search(&db, "!!! ???", 10, &[]).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_file_removes_its_chunks_from_the_keyword_index() {
        let db = seeded();
        assert!(
            !keyword_search(&db, "EMBED_BATCH", 10, &[])
                .unwrap()
                .is_empty()
        );
        delete_file(&db, "/a/lib.rs").unwrap();
        assert!(
            keyword_search(&db, "EMBED_BATCH", 10, &[])
                .unwrap()
                .is_empty(),
            "cascade delete left the keyword index stale"
        );
    }

    #[test]
    fn an_old_store_gets_its_keyword_index_backfilled() {
        // A 0.1.0 store: chunks written with no FTS table in the schema.
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
             CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE,
                 hash TEXT NOT NULL, bytes INTEGER NOT NULL, indexed_at INTEGER NOT NULL);
             CREATE TABLE chunks (id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL
                 REFERENCES files(id) ON DELETE CASCADE, ord INTEGER NOT NULL,
                 start_line INTEGER NOT NULL, end_line INTEGER NOT NULL, text TEXT NOT NULL);
             INSERT INTO files VALUES (1, '/a/lib.rs', 'h', 10, 0);
             INSERT INTO chunks VALUES (1, 1, 0, 1, 5, 'const EMBED_BATCH: usize = 32;');",
        )
        .unwrap();

        // Opening it with the current schema must index what is already there.
        db.execute_batch(SCHEMA).unwrap();
        backfill_fts(&db).unwrap();
        assert_eq!(
            keyword_search(&db, "EMBED_BATCH", 10, &[]).unwrap(),
            vec![1]
        );
    }
}
