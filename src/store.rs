//! SQLite side of the store: file bookkeeping and chunk payloads.
//!
//! The vector index only ever holds `(chunk id, quantized vector)`. Everything
//! a caller actually wants back — the text, the path, the line span — lives
//! here and is looked up by chunk id after the search returns.

use anyhow::Result;
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub(crate) const SCHEMA: &str = r#"
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

-- What a symbol used to be, from 0.23.0. A re-index no longer simply deletes
-- the definitions of a file it is about to rewrite: it copies them here first,
-- stamped with the content hash the file had while they were true, so
-- `semlith symbol` can answer "what did this look like before".
--
-- A separate table rather than a validity column on `symbols`, because
-- `symbols.file_id` cascades from `files` and a re-index deletes the file row.
-- Making the live rows outlive their file would mean loosening that foreign
-- key, which is the one thing keeping the graph from rotting; copying them out
-- keeps the live table exactly as strict as it was.
--
-- Additive and `IF NOT EXISTS`, so `format_version` does not move, for the same
-- reason the 0.12.0 graph tables did not move it: an older binary opens the
-- store, never looks in here, and answers exactly as before. A store written
-- before 0.23.0 has an empty table and starts filling it at its next index
-- pass. See `docs/compatibility.md`.
--
-- Nothing prunes this. History is kept whole in 0.23.0 -- a row per changed
-- symbol per pass -- and a retention knob is a later release's problem, when
-- growth is something measured rather than imagined.
CREATE TABLE IF NOT EXISTS symbols_past (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    path         TEXT NOT NULL,
    kind         TEXT NOT NULL,
    name         TEXT NOT NULL,
    qualified    TEXT NOT NULL,
    start_line   INTEGER NOT NULL,
    end_line     INTEGER NOT NULL,
    -- The content hash of the file version this definition belonged to. What
    -- "invalid at" means: everything after this hash is a different file.
    content_hash TEXT NOT NULL,
    retired_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS symbols_past_name ON symbols_past(name);

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
-- `hint` is what the source text said about where `dst` lives, when it said
-- anything: the module of a scoped call, the receiver of a method call, the
-- object of a qualified one. Nullable, and NULL on every row an older binary
-- wrote, which is why the format version does not move for it. It is a lead,
-- not an address, and `edges_out` treats it as one ranking signal among
-- several — see `resolve`.
-- `line` is where the reference was written in the source file, from 0.16.0.
-- Not where either symbol is defined: "where is X invoked" is a question about
-- this column, and until it existed the closest answer was the enclosing
-- symbol's own start line. Nullable and NULL on every row an older binary
-- wrote, so the format version does not move for it either.
CREATE TABLE IF NOT EXISTS edges (
    src        INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    dst        TEXT NOT NULL,
    kind       TEXT NOT NULL,
    confidence TEXT NOT NULL,
    hint       TEXT,
    line       INTEGER
);

CREATE INDEX IF NOT EXISTS edges_src ON edges(src);
CREATE INDEX IF NOT EXISTS edges_dst ON edges(dst);

-- Images, from 0.13.0. One row per indexed image file, with the pixel size the
-- Search and Files pages show in place of a line range. The vector itself is in
-- the store's second index, under `images/`, at CLIP's 512 dimensions.
--
-- Additive and `IF NOT EXISTS`, so `format_version` does not move: an older
-- binary opens the store, never looks in here, and searches its text exactly as
-- before. Same reasoning `docs/compatibility.md` records for the graph tables.
--
-- `id` is AUTOINCREMENT for the same reason chunks are: the id addresses a
-- vector in a sharded index, and SQLite reissuing a deleted row's id would put
-- one id inside two shards at once.
CREATE TABLE IF NOT EXISTS images (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    width   INTEGER NOT NULL,
    height  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS images_file_id ON images(file_id);

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
    defensive(&db)?;
    db.execute_batch(SCHEMA)?;
    add_columns(&db)?;
    check_format(&db)?;
    backfill_fts(&db)?;
    // Everything above is the schema this binary needs in place before the
    // store is usable at all; from here the connection writes only when a
    // writer asks.
    read_only(&db, true)?;
    Ok(db)
}

/// Treat the file as data rather than as a program.
///
/// A SQLite file is a schema as well as rows, and a schema can carry views,
/// triggers and generated columns that run when the database is merely opened
/// or read. semlith opens store files it did not necessarily write — a
/// `.semlith` that a user has trusted arrived from somewhere, and `--store`
/// opens anything the user names — so both settings are on for every
/// connection:
///
/// - `trusted_schema=OFF` stops a view or a trigger calling the functions
///   SQLite marks as unsafe for a schema it does not trust;
/// - `SQLITE_DBCONFIG_DEFENSIVE` refuses writes to shadow tables and to
///   `sqlite_schema`, which is how a corrupted or crafted FTS5 index turns a
///   read into something else.
///
/// Both are cheap and neither affects a query semlith itself issues, which is
/// what `tests/measure.rs` is there to keep true.
fn defensive(db: &Connection) -> Result<()> {
    db.pragma_update(None, "trusted_schema", "OFF")?;
    db.set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    Ok(())
}

/// Make this connection refuse writes until a writer says otherwise.
///
/// `query_only` rather than `SQLITE_OPEN_READ_ONLY`, because semlith opens one
/// connection per store and writes through the same handle it reads through:
/// a read-only open would mean two connections per store and a second answer to
/// "who holds the writer", which is the one thing the store's design keeps
/// singular. The property is the same from the file's point of view — a
/// connection in this state cannot write, whatever it is asked to do — and it
/// is lifted only by [`Writing`], which the three write paths take.
pub fn read_only(db: &Connection, on: bool) -> Result<()> {
    db.pragma_update(None, "query_only", if on { "ON" } else { "OFF" })?;
    Ok(())
}

/// Permission to write, given back when it goes out of scope.
///
/// Taken by the three methods that write — indexing, forgetting, and recording
/// a retrieval in the ledger — so every other path through the store is a path
/// SQLite itself will refuse to write from.
pub struct Writing<'a>(&'a Connection);

impl<'a> Writing<'a> {
    pub fn begin(db: &'a Connection) -> Result<Self> {
        read_only(db, false)?;
        Ok(Self(db))
    }
}

impl Drop for Writing<'_> {
    fn drop(&mut self) {
        // Best effort on purpose: a connection that cannot be put back into
        // read-only mode is one whose next statement will fail anyway, and a
        // panic in a destructor would replace a clear error with a confusing
        // one.
        let _ = read_only(self.0, true);
    }
}

/// The store layout this binary understands.
///
/// Written into a store when one is created, and absent from every store
/// written before 0.6.0 — which is what makes 1 the right reading of an absent
/// key rather than an unknown.
///
/// 1 is a single `index.tv`. 2 is a directory of shards, written by 0.7.0 and
/// later; see [`crate::index`]. 3 is a store written by 0.22.0 and later,
/// whose Markdown chunks are cut at their headings and carry the heading path
/// as embedding context. 4 is a store written by 0.25.0 and later, whose code
/// chunks carry the definition they sit inside — its signature and the first
/// line of its doc comment — as embedding context. A binary understands every
/// format up to its own, so this one reads all four and creates the newest.
pub const FORMAT_VERSION: u32 = 4;

/// The first format that keeps its vectors in shards.
pub const SHARDED_FORMAT: u32 = 2;

/// The first format whose code chunks embed the definition they sit inside.
///
/// A store below this holds code chunks embedded from their own text alone.
/// They still answer — the row, the span and the bytes are identical, and only
/// what the model was shown differs — but a store half embedded one way and
/// half the other ranks its own files against each other unevenly, so the
/// first full index pass under 0.25.0 re-embeds everything it walks. `semlith
/// stats` says which rule a store is on until it has.
pub const CODE_CONTEXT: u32 = 4;

/// The first format whose Markdown chunks are cut at their headings.
///
/// A store below this holds fixed windows and still answers: a chunk is a chunk
/// whichever rule cut it, and nothing about the row or the vector changes. What
/// changes is where the boundaries fall, so a store that is half one rule and
/// half the other would rank its own files against each other unevenly — which
/// is why the first full index pass under 0.22.0 re-chunks everything it walks
/// rather than waiting for each file to be edited. `semlith stats` says which
/// rule a store is on until it has.
pub const DEFINITION_CHUNKS: u32 = 3;

/// Which chunking rule this store's chunks were cut by, in the words `stats`
/// and the portal print.
pub fn chunking(db: &Connection) -> Result<&'static str> {
    Ok(match format(db)? {
        v if v >= CODE_CONTEXT => "headings, and code with its definition",
        v if v >= DEFINITION_CHUNKS => {
            "headings; code carries no definition until it is re-indexed"
        }
        _ => "fixed windows",
    })
}

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
/// Columns added to tables that already exist in a store written by an older
/// binary.
///
/// `CREATE TABLE IF NOT EXISTS` is how every table here arrives, and it does
/// nothing at all to a table that is already there — so a column added to the
/// schema above reaches a fresh store and no other. This is the other half:
/// one `ALTER TABLE ADD COLUMN` per column, run on every open, skipped when
/// the column is present.
///
/// Every column here must be nullable with no default, which is what makes the
/// operation an O(1) catalogue edit rather than a table rewrite, and what lets
/// an older binary keep reading the store afterwards: it selects by name and
/// never sees them. That is the whole reason `format_version` does not move —
/// the same reasoning `docs/compatibility.md` records for the graph tables.
fn add_columns(db: &Connection) -> Result<()> {
    const ADDITIONS: [(&str, &str, &str); 8] = [
        ("edges", "hint", "TEXT"),
        // 0.16.0: the line the reference was written on.
        ("edges", "line", "INTEGER"),
        // The ledger's 0.15.0 columns. `session` groups the retrievals of one
        // agent conversation, `tool` says which tool was asked, `stale_hits`
        // counts the answers that came from a file edited since it was
        // indexed, and `tokenizer` names what counted the two token figures so
        // rows counted two different ways are never summed together.
        ("retrievals", "session", "TEXT"),
        ("retrievals", "tool", "TEXT"),
        ("retrievals", "stale_hits", "INTEGER"),
        ("retrievals", "tokenizer", "TEXT"),
        // 0.20.2: the id one retrieval shares across every store that
        // contributed to it. A search over six stores used to write six rows
        // and the Ledger page counted six retrievals, so every figure on it
        // scaled with how many stores happened to be open. The rows stay per
        // store — that is what keeps each store's chain verifiable and its
        // token counts its own — and this is what makes them one retrieval
        // again when they are counted.
        ("retrievals", "query_id", "TEXT"),
        // 0.25.0: what the parser made of this file — "parsed", "timeout" or
        // "none" for a language that carries no grammar. Without it a file
        // with no definitions and a file the parser gave up on look the same
        // in the per-language coverage table, which is the one question that
        // table exists to answer.
        ("files", "graph", "TEXT"),
    ];
    for (table, column, kind) in ADDITIONS {
        if has_column(db, table, column)? {
            continue;
        }
        db.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind};"))?;
    }
    Ok(())
}

fn has_column(db: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = db.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

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
        // A pattern marked with a leading `!` by `filter::anchor` excludes.
        // Exclusions apply after the inclusions of their own kind, which is
        // what keeps them inside this group rather than becoming a group of
        // their own: `--path 'src/**' --path '!src/vendor/**'` is one
        // requirement, not two intersecting ones.
        let (excluded, included): (Vec<&String>, Vec<&String>) =
            group.iter().partition(|p| p.starts_with('!'));
        let placeholder = format!("{GLOB_PATH} GLOB ?");
        let ors = vec![placeholder.clone(); included.len()].join(" OR ");
        let nots = vec![placeholder; excluded.len()].join(" OR ");

        // Binds are positional, so they are pushed in the order the clause
        // below writes their placeholders — inclusions first — rather than in
        // the order the user typed them.
        binds.extend(included.into_iter().cloned());
        binds.extend(excluded.into_iter().map(|p| p[1..].to_string()));

        clauses.push(match (ors.is_empty(), nots.is_empty()) {
            (true, true) => continue,
            (false, true) => format!("({ors})"),
            // Exclusions with nothing beside them mean everything except.
            (true, false) => format!("(NOT ({nots}))"),
            (false, false) => format!("(({ors}) AND NOT ({nots}))"),
        });
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
pub fn delete_file(db: &Connection, path: &str, now: i64) -> Result<Vec<u64>> {
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
    retire_symbols(db, path, now)?;
    db.execute("DELETE FROM files WHERE path = ?1", params![path])?;
    Ok(ids)
}

/// Copy a file's current definitions into `symbols_past` before the file row
/// takes them with it.
///
/// Stamped with the hash the file has right now, which is the hash they were
/// extracted from: `delete_file` runs before the new version is inserted, so
/// `files.hash` here is still the old content's.
///
/// A file with no symbols copies nothing, and a store opened by a binary that
/// never wrote this table simply has none to copy.
fn retire_symbols(db: &Connection, path: &str, now: i64) -> Result<()> {
    db.execute(
        "INSERT INTO symbols_past
             (path, kind, name, qualified, start_line, end_line, content_hash, retired_at)
         SELECT f.path, s.kind, s.name, s.qualified, s.start_line, s.end_line, f.hash, ?2
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path = ?1",
        params![path, now],
    )?;
    Ok(())
}

/// What a symbol used to be, newest first.
///
/// The live definitions are what `symbols_named` answers; these are the ones a
/// re-index replaced, each with the content hash of the file version it was
/// true for. An empty list means the store has never seen this name change --
/// or has never re-indexed since 0.23.0, which is the same answer from the
/// outside and is why `semlith stats` says whether the store keeps history.
pub fn symbols_past_named(db: &Connection, name: &str, limit: usize) -> Result<Vec<PastSymbol>> {
    let mut stmt = db.prepare(
        "SELECT path, kind, name, qualified, start_line, end_line, content_hash, retired_at
         FROM symbols_past WHERE name = ?1 ORDER BY retired_at DESC, id DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map(params![name, limit as i64], |r| {
        Ok(PastSymbol {
            path: r.get(0)?,
            kind: r.get(1)?,
            name: r.get(2)?,
            qualified: r.get(3)?,
            start_line: r.get(4)?,
            end_line: r.get(5)?,
            content_hash: r.get(6)?,
            retired_at: r.get(7)?,
            store: None,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// How many definitions this store has retired, over all names.
pub fn symbols_past_count(db: &Connection) -> Result<i64> {
    Ok(db.query_row("SELECT COUNT(*) FROM symbols_past", [], |r| r.get(0))?)
}

/// A definition a re-index replaced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PastSymbol {
    #[serde(serialize_with = "crate::serialize_plain")]
    pub path: String,
    pub kind: String,
    pub name: String,
    pub qualified: String,
    pub start_line: u32,
    pub end_line: u32,
    /// The content hash of the file version this definition belonged to.
    pub content_hash: String,
    pub retired_at: i64,
    /// Which store this came from, set only when more than one was searched.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

pub fn insert_file(db: &Connection, path: &str, hash: &str, bytes: u64, now: i64) -> Result<i64> {
    db.execute(
        "INSERT INTO files (path, hash, bytes, indexed_at) VALUES (?1, ?2, ?3, ?4)",
        params![path, hash, bytes as i64, now],
    )?;
    Ok(db.last_insert_rowid())
}

/// Record what the parser made of a file: `parsed`, `timeout`, or `none` for
/// a language semlith carries no grammar for.
///
/// Written beside the file row rather than derived later, because the two
/// failures it separates are invisible afterwards: a file whose parse expired
/// and a file whose language has no grammar both arrive at the store with no
/// symbols at all.
pub fn set_file_graph(db: &Connection, file_id: i64, state: &str) -> Result<()> {
    db.execute(
        "UPDATE files SET graph = ?1 WHERE id = ?2",
        params![state, file_id],
    )?;
    Ok(())
}

/// What the graph covers, per language, read from the rows themselves.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct LanguageCoverage {
    pub language: String,
    /// Files of this language the store holds.
    pub files: usize,
    /// Files whose parse expired, so the store holds their text and none of
    /// their structure.
    pub parser_failed: usize,
    pub definitions: usize,
    /// Call edges whose target was found in the file that made the call.
    pub extracted: usize,
    /// Call edges whose target name has exactly one definition in the store.
    pub resolved: usize,
    /// Call edges whose target name has several, and nothing here says which.
    pub ambiguous: usize,
    /// Call edges naming something no definition in this store satisfies —
    /// the standard library, a dependency nobody indexed, a typo.
    pub unresolved: usize,
}

impl LanguageCoverage {
    /// The share of this language's answerable call edges that settled on one
    /// definition, which is the figure the README quotes for the whole store.
    #[must_use]
    pub fn settled_share(&self) -> usize {
        let answerable = self.extracted + self.resolved + self.ambiguous;
        if answerable == 0 {
            return 0;
        }
        (self.extracted + self.resolved) * 100 / answerable
    }
}

/// Per-language coverage for every language the store holds.
///
/// Sorted by files descending then by name, so the language that dominates a
/// corpus is the first line a reader sees and two runs over one store print
/// the same table.
///
/// The edge classes are the same four the resolver reports, decided the same
/// way: `extracted` is stored on the edge, and an `inferred` edge is resolved,
/// ambiguous or unresolved according to how many definitions of its target
/// name this store holds. A second rule here would be a second answer to "what
/// share resolves", and the README quotes this one.
pub fn coverage_by_language(db: &Connection) -> Result<Vec<LanguageCoverage>> {
    use std::collections::BTreeMap;
    let mut by_language: BTreeMap<String, LanguageCoverage> = BTreeMap::new();
    let language = |path: &str| -> String {
        crate::graph::language_of(std::path::Path::new(path))
            .unwrap_or("other")
            .to_string()
    };

    let mut stmt = db.prepare("SELECT path, graph FROM files")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (path, graph) = row?;
        let entry = by_language.entry(language(&path)).or_default();
        entry.files += 1;
        if graph.as_deref() == Some("timeout") {
            entry.parser_failed += 1;
        }
    }

    let mut stmt = db.prepare(
        "SELECT f.path, COUNT(*) FROM symbols s JOIN files f ON f.id = s.file_id GROUP BY f.path",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
    for row in rows {
        let (path, count) = row?;
        by_language.entry(language(&path)).or_default().definitions +=
            usize::try_from(count).unwrap_or(0);
    }

    // One row per edge, and the definition count of its target beside it.
    let mut stmt = db.prepare(
        "SELECT f.path, e.confidence, (SELECT COUNT(*) FROM symbols d WHERE d.name = e.dst) \
         FROM edges e JOIN symbols s ON s.id = e.src JOIN files f ON f.id = s.file_id \
         WHERE e.kind = 'calls'",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
        ))
    })?;
    for row in rows {
        let (path, confidence, definitions) = row?;
        let entry = by_language.entry(language(&path)).or_default();
        if confidence == crate::graph::EXTRACTED {
            entry.extracted += 1;
        } else {
            match definitions {
                0 => entry.unresolved += 1,
                1 => entry.resolved += 1,
                _ => entry.ambiguous += 1,
            }
        }
    }

    let mut coverage: Vec<LanguageCoverage> = by_language
        .into_iter()
        .map(|(language, mut row)| {
            row.language = language;
            row
        })
        .collect();
    coverage.sort_by(|a, b| {
        b.files
            .cmp(&a.files)
            .then_with(|| a.language.cmp(&b.language))
    });
    Ok(coverage)
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

/// Indexed files whose path ends with `suffix`, most specific first.
///
/// A locate answer prints a store-relative path and a user types one, while
/// `files.path` is absolute. Rather than teach every caller to rebuild the
/// absolute form — which needs the store's roots and gets it wrong for a file
/// reached through a symlink — the suffix is matched against what was actually
/// indexed. `/` is prepended so `one.rs` cannot match `alone.rs`.
pub fn files_ending_with(db: &Connection, suffix: &str, limit: usize) -> Result<Vec<String>> {
    let suffix = suffix
        .trim_start_matches(['.', '/', '\\'])
        .replace('\\', "/");
    let mut stmt = db.prepare(&format!(
        "SELECT path FROM files WHERE path = ?1 OR {PATH_AS_TYPED} LIKE '%/' || ?1
         ORDER BY LENGTH(path) LIMIT ?2",
    ))?;
    let rows = stmt.query_map(params![suffix, limit as i64], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// `files.path` with separators in the form a person types, for the comparisons
/// that take a path from one.
///
/// A Windows store holds `C:\work\api\src\lock.rs` — or the verbatim form of
/// it — and every locator, glob and `--lang` pattern is written with `/`, which
/// is what `semlith read src/main.rs:28-40` hands back. Comparing the two
/// without this matched nothing on that platform, which is the half of #74 that
/// is not the prefix. On unix the column is used as it is: a backslash there is
/// an ordinary character in a filename and rewriting it would make
/// `a\b.rs` answer to `a/b.rs`.
#[cfg(windows)]
const PATH_AS_TYPED: &str = r#"replace(path, '\', '/')"#;
#[cfg(not(windows))]
const PATH_AS_TYPED: &str = "path";

/// The same, for the queries that alias the table as `f` and lowercase it.
#[cfg(windows)]
const GLOB_PATH: &str = r#"replace(lower(f.path), '\', '/')"#;
#[cfg(not(windows))]
const GLOB_PATH: &str = "lower(f.path)";

/// Every chunk of one file whose lines overlap `start..=end`, in file order.
///
/// The second stage of a retrieval: a locate answer says `src/store.rs:1041-1080`
/// and this is what returns those lines and nothing else. Chunks overlap by two
/// lines by design, so the caller stitches rather than concatenates — which is
/// [`crate::Span::text`]'s job, not this one's.
pub fn chunks_overlapping(
    db: &Connection,
    path: &str,
    start: u32,
    end: u32,
) -> Result<Vec<ChunkRow>> {
    let mut stmt = db.prepare(
        "SELECT c.id, f.path, c.ord, c.start_line, c.end_line, c.text
         FROM chunks c JOIN files f ON f.id = c.file_id
         WHERE f.path = ?1 AND c.start_line <= ?2 AND c.end_line >= ?3
         ORDER BY c.ord",
    )?;
    let rows = stmt.query_map(params![path, end, start], |r| {
        Ok(ChunkRow {
            id: r.get(0)?,
            path: r.get(1)?,
            ord: r.get(2)?,
            start_line: r.get(3)?,
            end_line: r.get(4)?,
            text: r.get(5)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
    /// Which store holds the row. Not a column of this table at all — it is
    /// which database the row came out of — so it is ordered by the merge.
    Store,
    /// The reader that parsed the file, derived from its extension.
    Reader,
    /// The language the file was recognised as, derived from its extension.
    Lang,
}

impl FileSort {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "path" => Some(Self::Path),
            "bytes" => Some(Self::Bytes),
            "chunks" => Some(Self::Chunks),
            "lines" => Some(Self::Lines),
            "indexed" => Some(Self::Indexed),
            "store" => Some(Self::Store),
            "reader" => Some(Self::Reader),
            "lang" => Some(Self::Lang),
            _ => None,
        }
    }

    /// Whether this ordering is over a value the database does not hold.
    ///
    /// Three of the seven columns are derived by the route from the path, or
    /// from which store answered. They cannot be an `ORDER BY`, so the rows
    /// are ordered by the merge instead — which needs every matching row
    /// rather than one page of each store's.
    pub fn derived(self) -> bool {
        matches!(self, Self::Store | Self::Reader | Self::Lang)
    }

    fn sql(self) -> &'static str {
        match self {
            Self::Path => "f.path",
            Self::Bytes => "f.bytes",
            Self::Chunks => "COUNT(c.id)",
            Self::Lines => "COALESCE(MAX(c.end_line), 0)",
            Self::Indexed => "f.indexed_at",
            // The base order under a derived sort: deterministic, and the
            // tie-break the merge uses anyway.
            Self::Store | Self::Reader | Self::Lang => "f.path",
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

/// An indexed image: its file's path and the pixel size.
#[derive(Debug, Clone)]
pub struct ImageRow {
    pub id: i64,
    pub path: String,
    pub width: u32,
    pub height: u32,
}

pub fn insert_image(db: &Connection, file_id: i64, width: u32, height: u32) -> Result<i64> {
    db.execute(
        "INSERT INTO images (file_id, width, height) VALUES (?1, ?2, ?3)",
        params![file_id, width, height],
    )?;
    Ok(db.last_insert_rowid())
}

/// One image row, joined with its file's path.
pub fn image(db: &Connection, id: i64) -> Result<Option<ImageRow>> {
    Ok(db
        .query_row(
            "SELECT i.id, f.path, i.width, i.height FROM images i \
             JOIN files f ON f.id = i.file_id WHERE i.id = ?1",
            params![id],
            |r| {
                Ok(ImageRow {
                    id: r.get(0)?,
                    path: r.get(1)?,
                    width: r.get(2)?,
                    height: r.get(3)?,
                })
            },
        )
        .optional()?)
}

/// The image ids belonging to `path`, before its row is deleted.
///
/// Read rather than returned by `delete_file`, because the cascade that removes
/// them is what makes the ids unreadable — and the vectors they address still
/// have to be evicted from the image index afterwards.
pub fn image_ids_of(db: &Connection, path: &str) -> Result<Vec<i64>> {
    let mut stmt =
        db.prepare("SELECT i.id FROM images i JOIN files f ON f.id = i.file_id WHERE f.path = ?1")?;
    let rows = stmt.query_map(params![path], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// How many images this store holds.
pub fn image_count(db: &Connection) -> Result<i64> {
    Ok(db.query_row("SELECT COUNT(*) FROM images", [], |r| r.get(0))?)
}

/// Every image id, for the ids a filtered search may consider.
pub fn image_ids(db: &Connection) -> Result<Vec<i64>> {
    let mut stmt = db.prepare("SELECT id FROM images")?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// The image ids whose file matches `groups`, for a filtered search.
pub fn filtered_image_ids(db: &Connection, groups: &[Vec<String>]) -> Result<Vec<i64>> {
    let (predicate, binds) = glob_predicate(groups);
    let sql =
        format!("SELECT i.id FROM images i JOIN files f ON f.id = i.file_id WHERE {predicate}");
    let mut stmt = db.prepare(&sql)?;
    let args = binds.into_iter().map(Value::Text);
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| r.get::<_, i64>(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

pub fn all_paths(db: &Connection) -> Result<Vec<String>> {
    filtered_paths(db, &[])
}

/// Everything this store holds for one file, its chunks in order, joined.
///
/// What `semlith scan` reads. The store's own copy rather than the file on
/// disk: the question the scan answers is what this store is holding, and a
/// file that has since been cleaned or deleted is still a credential sitting
/// in an index an agent can search.
pub fn text_of(db: &Connection, path: &str) -> Result<String> {
    let mut stmt = db.prepare(
        "SELECT c.text FROM chunks c JOIN files f ON f.id = c.file_id
         WHERE f.path = ?1 ORDER BY c.ord",
    )?;
    let rows = stmt.query_map([path], |r| r.get::<_, String>(0))?;
    let mut out = String::new();
    for row in rows {
        out.push_str(&row?);
        out.push('\n');
    }
    Ok(out)
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
    #[serde(serialize_with = "crate::serialize_plain")]
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
    /// How many definitions of this name the store holds.
    ///
    /// `1` for an edge that could only ever mean one thing. Greater than one
    /// on an `ambiguous` row, where it is the number a renderer prints instead
    /// of listing every candidate as though each were a separate call — and on
    /// a `resolved` row, where it is how many candidates the ranking had to
    /// choose between, which is the difference between "there was only one"
    /// and "there were four and the source said which".
    pub definitions: usize,
    /// The definition this edge actually leaves from.
    ///
    /// [`edges_out`] is asked about a name, and a name can have several
    /// definitions, each with its own edges. Without this the answer says only
    /// that *something* called `index` reaches here — which is how a path
    /// finder ends up walking out of one definition of a name and into
    /// another without anything on the page saying so.
    ///
    /// `None` from [`edges_in`], where the queried name is the far end of the
    /// edge and there is no single row it leaves from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_line: Option<u32>,
    /// The line the call, import or reference was written on, in the source
    /// file named by `from_path`.
    ///
    /// `None` for an edge written before 0.16.0, and for one whose source file
    /// has not been re-indexed since. A renderer says nothing rather than
    /// guessing when it is absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
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
    hint: Option<&str>,
    line: Option<u32>,
) -> Result<()> {
    db.execute(
        "INSERT INTO edges (src, dst, kind, confidence, hint, line)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![src, dst, kind, confidence, hint, line],
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
///
/// # Resolution
///
/// `edges.dst` is a name, and a name is not an address: a store of any size
/// holds four `get`s and seven `index`es. Until 0.15.0 this returned every
/// definition that shared the name, each as its own row and each looking
/// exactly like a call the source makes — which is how `semlith path` came to
/// answer "yes, connected" to questions whose honest answer is "no".
///
/// Each edge is now resolved against its candidates by [`rank`], and the
/// confidence it comes back with says how it was settled:
///
/// - `extracted` — the file named where the target came from. Stored, and
///   never overwritten here: a fact from the syntax tree outranks a ranking.
/// - `resolved` — the candidates narrowed to exactly one. Either the source
///   narrowed them (same file, the hint, an import) or the corpus did, by
///   holding only one definition of the name.
/// - `ambiguous` — several candidates survived and nothing chose between them.
///   Every candidate is returned, each marked `ambiguous` and each carrying
///   the count, so a renderer can collapse them to one row and a traversal can
///   refuse to cross them.
///
/// `inferred` is the stored value and is what [`edges_in`] and the edge census
/// still show. It does not come back from here, because by the time a target
/// has been looked up there is always something to say about it: either one
/// definition answers to the name or several do.
///
/// A resolution is against the indexed corpus, not against the world. One
/// definition of `parse` in the store does not prove the call went to it
/// rather than to a dependency that was never indexed — that is what the
/// hidden-by-default unresolved targets are about, not this.
pub fn edges_out(db: &Connection, name: &str, kinds: &[String]) -> Result<Vec<EdgeEnd>> {
    let filter = kind_predicate(kinds, "e.kind");
    // An edge's target is resolved by name, so without this a configuration key
    // named `path` is a candidate definition of every `path` any code calls.
    let target = crate::graph::not_navigational("s.kind");
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS}, e.kind, e.confidence, e.hint, srcf.path, src.start_line, e.line
         FROM symbols src
         JOIN files srcf ON srcf.id = src.file_id
         JOIN edges e ON e.src = src.id
         JOIN symbols s ON s.name = e.dst AND {target}
         JOIN files f ON f.id = s.file_id
         WHERE src.name = ?1 AND {filter}
         ORDER BY f.path, s.start_line"
    );
    let mut stmt = db.prepare(&sql)?;
    let mut binds: Vec<Value> = vec![Value::Text(name.to_string())];
    binds.extend(kinds.iter().map(|k| Value::Text(k.clone())));
    let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok(Reached {
            symbol: symbol_row(r)?,
            kind: r.get(8)?,
            confidence: r.get(9)?,
            hint: r.get(10)?,
            src_path: r.get(11)?,
            src_line: r.get(12)?,
            line: r.get(13)?,
        })
    })?;
    let reached = rows.collect::<Result<Vec<_>, _>>()?;
    resolve(db, reached)
}

/// One row of the join behind [`edges_out`]: a candidate for an edge's target,
/// with everything the ranking needs about the edge itself.
struct Reached {
    symbol: SymbolRow,
    kind: String,
    confidence: String,
    hint: Option<String>,
    src_path: String,
    src_line: u32,
    line: Option<u32>,
}

/// Turn candidate rows into resolved edges.
///
/// The rows arrive as the cross product of edges and the definitions their
/// targets could mean, so they are grouped back into edges first. Order is
/// preserved: the first time an edge is seen decides where its rows sit in the
/// answer, so a caller that used to read this list top to bottom still reads
/// it in the same order.
fn resolve(db: &Connection, reached: Vec<Reached>) -> Result<Vec<EdgeEnd>> {
    // (source file, target name, edge kind, hint) — one edge in the source.
    type Key = (String, String, String, Option<String>);
    let mut order: Vec<Key> = Vec::new();
    let mut groups: std::collections::HashMap<Key, Vec<Reached>> = std::collections::HashMap::new();
    for row in reached {
        let key = (
            row.src_path.clone(),
            row.symbol.name.clone(),
            row.kind.clone(),
            row.hint.clone(),
        );
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(row);
    }

    // The imports of each source file, read once and only when a group
    // actually needs them: the first two tiers settle most edges, and a file's
    // imports are a query this should not pay for on every hop of a traversal.
    let mut imports: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();

    let mut out = Vec::new();
    for key in order {
        let candidates = groups.remove(&key).unwrap_or_default();
        let (src_path, _, _, hint) = &key;
        let definitions = candidates.len();

        if definitions == 1 {
            // Nothing to choose between. Whatever the syntax tree said stands,
            // and a bare name that matches exactly one definition in the store
            // is as resolved as the corpus can make it.
            let row = candidates.into_iter().next().expect("one candidate");
            let confidence = if row.confidence == crate::graph::EXTRACTED {
                crate::graph::EXTRACTED
            } else {
                crate::graph::RESOLVED
            };
            out.push(EdgeEnd {
                symbol: row.symbol,
                kind: row.kind,
                confidence: confidence.to_string(),
                definitions,
                from_path: Some(row.src_path),
                from_line: Some(row.src_line),
                line: row.line,
            });
            continue;
        }

        // Rank without the imports first. If that already leaves one winner,
        // the import query is never run.
        let mut ranks: Vec<u8> = candidates
            .iter()
            .map(|c| rank(&c.symbol.path, src_path, hint.as_deref(), &[]))
            .collect();
        if survivors(&ranks) != 1 && ranks.contains(&UNRANKED) {
            let of_file = match imports.get(src_path) {
                Some(found) => found.clone(),
                None => {
                    let found = file_imports(db, src_path)?;
                    imports.insert(src_path.clone(), found.clone());
                    found
                }
            };
            ranks = candidates
                .iter()
                .map(|c| rank(&c.symbol.path, src_path, hint.as_deref(), &of_file))
                .collect();
        }

        let best = ranks.iter().copied().min().unwrap_or(UNRANKED);
        let settled = survivors(&ranks) == 1;
        for (row, rank) in candidates.into_iter().zip(&ranks) {
            if settled && *rank != best {
                continue;
            }
            let confidence = if row.confidence == crate::graph::EXTRACTED && settled {
                crate::graph::EXTRACTED
            } else if settled {
                crate::graph::RESOLVED
            } else {
                crate::graph::AMBIGUOUS
            };
            out.push(EdgeEnd {
                symbol: row.symbol,
                kind: row.kind,
                confidence: confidence.to_string(),
                definitions,
                from_path: Some(row.src_path),
                from_line: Some(row.src_line),
                line: row.line,
            });
        }
    }
    Ok(out)
}

/// The rank of a candidate that nothing placed. Sorts last, and is what says
/// "the imports are worth reading for this group".
const UNRANKED: u8 = 3;

/// How many candidates sit at the best rank.
fn survivors(ranks: &[u8]) -> usize {
    match ranks.iter().min() {
        Some(best) => ranks.iter().filter(|r| *r == best).count(),
        None => 0,
    }
}

/// Where a candidate definition sits in the ranking, lower being better.
///
/// 0. The same file as the call. A file that defines a name and calls it means
///    its own.
/// 1. A file the hint names. `store::edges_out` in the presence of
///    `src/store.rs` is not a coincidence.
/// 2. A file the calling file imports. Weaker than the hint because an import
///    list is a set of possibilities rather than a statement about this call.
/// 3. Nothing placed it.
fn rank(candidate: &str, src_path: &str, hint: Option<&str>, imports: &[String]) -> u8 {
    if candidate == src_path {
        return 0;
    }
    if hint.is_some_and(|h| names_file(h, candidate)) {
        return 1;
    }
    if imports.iter().any(|i| {
        i.split(['/', '.', ':', '\\'])
            .any(|segment| !segment.is_empty() && names_file(segment, candidate))
    }) {
        return 2;
    }
    // A same-language tier was tried here and removed
    // (US-SEMLITH-0.17.0-I01). The reasoning was sound — semlith's own portal
    // is five thousand lines of JavaScript sharing twenty-five symbol names
    // with the Rust beside it, and a Rust `calls` edge does not mean a
    // JavaScript function — but the harness scored it two hits worse: it
    // settles an ambiguous name in favour of a same-language candidate that is
    // not always the right one, and a confidently wrong edge costs more than a
    // refused one. Ambiguity across languages is a ranking problem, and it is
    // not solved by preferring the nearest guess.
    UNRANKED
}

/// Whether `word` names the file at `path` — its stem, or one of its
/// directories.
///
/// `store` names `src/store.rs` and `src/store/mod.rs` alike, which is the
/// point: a module is a file or a directory depending on how the author felt
/// that day, and a hint knows neither.
fn names_file(word: &str, path: &str) -> bool {
    let path = std::path::Path::new(path);
    if path.file_stem().is_some_and(|stem| stem == word) {
        return true;
    }
    path.parent()
        .into_iter()
        .flat_map(|parent| parent.components())
        .any(|component| component.as_os_str() == word)
}

/// Everything one file imports, as the raw strings the extractor recorded.
fn file_imports(db: &Connection, path: &str) -> Result<Vec<String>> {
    let mut stmt = db.prepare(
        "SELECT DISTINCT e.dst FROM edges e
         JOIN symbols s ON s.id = e.src
         JOIN files f ON f.id = s.file_id
         WHERE f.path = ?1 AND e.kind = 'imports'",
    )?;
    let rows = stmt.query_map(params![path], |r| r.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Targets of `name`'s edges that the store holds no definition for.
///
/// [`edges_out`] joins to `symbols`, so an edge pointing at something outside
/// the corpus — a standard-library call, a crate that was never indexed —
/// simply does not appear there. That is the right default: a list of names
/// the store knows nothing about is noise in an answer about this codebase.
/// It is not the right *only* option, because "semlith shows no callees" and
/// "everything this calls lives outside the index" are different facts, and
/// only one of them means the graph is working.
pub fn unresolved_out(db: &Connection, name: &str, kinds: &[String]) -> Result<Vec<Unresolved>> {
    let filter = kind_predicate(kinds, "e.kind");
    let sql = format!(
        "SELECT DISTINCT e.dst, e.kind, e.confidence
         FROM symbols src
         JOIN edges e ON e.src = src.id
         WHERE src.name = ?1 AND {filter}
           AND NOT EXISTS (SELECT 1 FROM symbols s WHERE s.name = e.dst)
         ORDER BY e.dst"
    );
    let mut stmt = db.prepare(&sql)?;
    let mut binds: Vec<Value> = vec![Value::Text(name.to_string())];
    binds.extend(kinds.iter().map(|k| Value::Text(k.clone())));
    let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok(Unresolved {
            name: r.get(0)?,
            kind: r.get(1)?,
            confidence: r.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// An edge whose target this store has no definition for.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Unresolved {
    pub name: String,
    pub kind: String,
    pub confidence: String,
}

/// What points at `name`: callers, importers, references in.
///
/// The direction a reverse walk takes, and the reason `edges_dst` exists —
/// `semlith neighbors` reads it to answer "what calls this".
pub fn edges_in(db: &Connection, name: &str, kinds: &[String]) -> Result<Vec<EdgeEnd>> {
    let filter = kind_predicate(kinds, "e.kind");
    let sql = format!(
        "SELECT {SYMBOL_COLUMNS}, e.kind, e.confidence, e.line
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
            // A caller is a symbol, not a name: the row came from the edge's
            // own `src` id, so there is nothing to resolve and nothing to be
            // ambiguous about. This direction is unchanged from 0.14.0.
            definitions: 1,
            from_path: None,
            from_line: None,
            line: r.get(10)?,
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
    // The one caller is the seed of the ranked walk, and the walk runs over
    // dependency edges. A heading or a configuration key has none, so seeding
    // it spends the walk's node budget to reach nothing — and where its name
    // collides with a real symbol's, it spends the seed's mass on the wrong
    // one.
    let navigational = crate::graph::not_navigational("kind");
    let sql =
        format!("SELECT DISTINCT name FROM symbols WHERE chunk_id IN ({holes}) AND {navigational}");
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
    /// Which stores answered this one retrieval, so several rows can be shown
    /// as the one search they came from. Empty for a row written before
    /// 0.20.2, which was its own retrieval.
    pub query_id: String,
}

/// Append one retrieval, chained to the row before it.
///
/// The chain is the point: a row cannot be quietly edited or removed without
/// every hash after it failing to recompute.
pub fn record_retrieval(db: &Connection, row: &NewRetrieval<'_>) -> Result<()> {
    let (client, query) = (row.client, row.query);
    let (hits, micros) = (row.hits, row.micros);
    let (excerpt_tokens, whole_file_tokens) = (row.excerpt_tokens, row.whole_file_tokens);
    // The ledger is the one write that happens on a read: a retrieval is
    // recorded by the search that answered it. It asks for the same permission
    // an index run does.
    let _writing = Writing::begin(db)?;
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
    let hash = chain_hash(&prev, at, row);
    db.execute(
        "INSERT INTO retrievals
         (at, client, query, hits, micros, excerpt_tokens, whole_file_tokens, prev, hash,
          session, tool, stale_hits, tokenizer, query_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            at,
            client,
            query,
            hits,
            micros,
            excerpt_tokens,
            whole_file_tokens,
            prev,
            hash,
            row.session,
            row.tool,
            row.stale_hits,
            row.tokenizer,
            row.query_id
        ],
    )?;
    // The one place a retrieval is written, so the one place the portal's
    // ledger counter moves — every surface that records goes through here.
    crate::daemon::changes::bump(crate::daemon::changes::Domain::Ledger);
    Ok(())
}

/// One retrieval, before it has a timestamp or a place in the chain.
///
/// A struct rather than thirteen positional arguments, because two adjacent
/// `i64`s that mean different things are a bug waiting for a refactor.
#[derive(Debug, Clone)]
pub struct NewRetrieval<'a> {
    /// Who asked: `claude-code`, `cursor`, `cli`, `portal`.
    pub client: &'a str,
    /// Which conversation, so one agent's session can be read as a unit.
    pub session: &'a str,
    /// Which tool: `search`, `neighbors`, `path`, `symbol`.
    pub tool: &'a str,
    pub query: &'a str,
    pub hits: i64,
    pub micros: i64,
    pub excerpt_tokens: i64,
    pub whole_file_tokens: i64,
    /// How many of those hits came from a file edited since it was indexed.
    pub stale_hits: i64,
    /// What counted the two token figures. Rows counted two different ways are
    /// never summed together, so the label travels with the row rather than
    /// being assumed from its age.
    pub tokenizer: &'a str,
    /// The id shared by every row one retrieval wrote.
    ///
    /// A cross-store search writes a row in each store that answered it, so
    /// each store's chain stays its own and its token figures describe its own
    /// hits. This is what says those rows are one retrieval rather than
    /// several, and it is what "queries recorded" counts.
    pub query_id: &'a str,
}

/// The hash covering one row and the one before it.
///
/// # Two formulas
///
/// A row written before 0.15.0 has four columns this one does not, and its
/// hash was computed without them. Recomputing such a row under the new
/// formula would report every 0.14.0 ledger as broken — which is the one thing
/// a verify must never do, because a verify that cries wolf is worse than no
/// verify at all.
///
/// So the formula is chosen per row, by the row itself: `tool` is NULL on
/// every row written before 0.15.0 and set on every row written since. There
/// is no version column and no migration, and a store holding rows of both
/// kinds walks end to end.
fn chain_hash(prev: &str, at: i64, row: &NewRetrieval<'_>) -> String {
    let NewRetrieval {
        client,
        query,
        hits,
        micros,
        excerpt_tokens: excerpt,
        whole_file_tokens: whole,
        ..
    } = row;
    let mut payload = format!(
        "{prev}\u{1f}{at}\u{1f}{client}\u{1f}{query}\u{1f}{hits}\u{1f}{micros}\u{1f}{excerpt}\u{1f}{whole}"
    );
    payload.push_str(&format!(
        "\u{1f}{}\u{1f}{}\u{1f}{}\u{1f}{}",
        row.session, row.tool, row.stale_hits, row.tokenizer
    ));
    // `query_id` is deliberately NOT in the payload.
    //
    // Adding it would be a third formula, and a 0.20.1 binary — which is this
    // release's own rollback path — recomputes the second one. Every row
    // written since would then fail to verify under the version somebody
    // rolled back to, which is the one thing a verify must never do: a verify
    // that cries wolf is worse than no verify at all.
    //
    // What that costs is stated rather than hidden: the id can be altered
    // without breaking the chain, so the grouping of rows into retrievals is
    // not tamper-evident. What a retrieval *was* — who asked, what they asked,
    // how many hits, what it cost — is covered exactly as before.
    blake3::hash(payload.as_bytes()).to_hex().to_string()
}

/// Append a row in the shape 0.14.0 wrote, for the test that proves a store
/// holding both kinds still verifies.
///
/// Public because that test is an integration test and cannot reach a private
/// function — and because the property it proves is worth proving. A verify
/// that reported every ledger written before this release as broken would be
/// worse than no verify at all, and nothing but a real pre-0.15.0 row in a
/// real store demonstrates that it does not.
pub fn record_legacy_retrieval(
    db: &Connection,
    client: &str,
    query: &str,
    excerpt_tokens: i64,
    whole_file_tokens: i64,
) -> Result<()> {
    let _writing = Writing::begin(db)?;
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
    let (hits, micros) = (1, 1000);
    let hash = legacy_chain_hash(
        &prev,
        at,
        client,
        query,
        hits,
        micros,
        excerpt_tokens,
        whole_file_tokens,
    );
    // The four 0.15.0 columns are left NULL, which is what makes this a row of
    // the older kind and what `ledger_break` reads to choose the formula.
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

/// The 0.14.0 formula, for rows written under it.
#[allow(clippy::too_many_arguments)]
fn legacy_chain_hash(
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
        "SELECT id, at, client, query, hits, micros, excerpt_tokens, whole_file_tokens, hash,
                query_id
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
            query_id: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Which rows of the ledger a count is about.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum LedgerScope {
    /// Every retrieval.
    All,
    /// Those that found something, which are the only ones credited with a
    /// saving.
    Credited,
    /// Those credited whose tokens were estimated rather than counted by the
    /// store's own tokenizer.
    Estimated,
}

/// The distinct retrievals this store holds, as keys that can be unioned
/// across stores.
///
/// One search over six stores writes a row in each that answered it, sharing a
/// query id. Summing six stores' own counts turns that back into six
/// retrievals, which is the multiplier the Ledger page was reporting — so the
/// portal unions these keys instead of adding counts. A row written before
/// 0.20.2 has no id and is keyed by its own row id in its own store, which is
/// what it was.
pub fn ledger_keys(db: &Connection, store: &str, scope: LedgerScope) -> Result<Vec<String>> {
    let sql = match scope {
        LedgerScope::All => "SELECT DISTINCT query_id, id FROM retrievals",
        LedgerScope::Credited => "SELECT DISTINCT query_id, id FROM retrievals WHERE hits > 0",
        LedgerScope::Estimated => {
            "SELECT DISTINCT query_id, id FROM retrievals
             WHERE hits > 0 AND (tokenizer IS NULL OR tokenizer != 'model')"
        }
    };
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map([], |r| {
        let id: Option<String> = r.get(0)?;
        let row: i64 = r.get(1)?;
        Ok(id.unwrap_or_else(|| format!("{store}:row:{row}")))
    })?;
    let mut out: Vec<String> = rows.collect::<Result<Vec<_>, _>>()?;
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// `(queries, clients, excerpt tokens, whole-file tokens)` over the whole/// `(queries, clients, excerpt tokens, whole-file tokens)` over the whole
/// ledger — what the Ledger page shows.
pub fn ledger_totals(db: &Connection) -> Result<(i64, i64, i64, i64)> {
    // Retrievals, not rows. One search across six stores writes a row in each
    // store that answered it and they share one `query_id`, so counting rows
    // here is what made every figure on the Ledger page scale with how many
    // stores happened to be open. A row written before 0.20.2 has no id and is
    // its own retrieval, which is what it was.
    Ok(db.query_row(
        "SELECT COUNT(DISTINCT COALESCE(query_id, 'row:' || id)), COUNT(DISTINCT client),
                COALESCE(SUM(excerpt_tokens), 0),
                COALESCE(SUM(whole_file_tokens), 0) FROM retrievals",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?)
}

/// The one savings figure, with the denominators that make it readable.
///
/// `net` is what the ledger says was not read: the whole-file cost of the
/// files an answer named, less what the answer itself cost. Rows that found
/// nothing are excluded from it and counted separately — a retrieval that
/// returned no hits saved nothing, and a ledger that quietly dropped those
/// rows would report a ratio that no honest denominator supports.
///
/// `tier` is `measured` when every credited row was counted with the store's
/// own tokenizer, and `modelled` when any of them was estimated at four
/// characters per token. The two are never summed: a mixed ledger reports
/// `modelled`, because that is what the weaker half makes the whole.
///
/// # What this figure does not do
///
/// It does not deduplicate files across the retrievals of one session. If an
/// agent searches twice and both answers name `src/lib.rs`, the denominator
/// counts that file twice, and the real alternative — one read — is cheaper
/// than the figure implies. Fixing it needs the file set stored per row, which
/// this release does not add. The number is therefore an upper bound on what
/// was saved, and `semlith stats` says so rather than presenting it as exact.
pub fn ledger_savings(db: &Connection) -> Result<Savings> {
    // Retrievals rather than rows on both sides of the ratio, for the reason
    // `ledger_totals` gives: the rows of one cross-store search are one search.
    // The token sums stay sums of rows, because each row holds its own store's
    // hits and nothing is double-counted by adding them.
    let (credited, net): (i64, i64) = db.query_row(
        "SELECT COUNT(DISTINCT COALESCE(query_id, 'row:' || id)),
                COALESCE(SUM(whole_file_tokens - excerpt_tokens), 0)
         FROM retrievals WHERE hits > 0",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let total: i64 = db.query_row(
        "SELECT COUNT(DISTINCT COALESCE(query_id, 'row:' || id)) FROM retrievals",
        [],
        |r| r.get(0),
    )?;
    let estimated: i64 = db.query_row(
        "SELECT COUNT(DISTINCT COALESCE(query_id, 'row:' || id)) FROM retrievals
         WHERE hits > 0 AND (tokenizer IS NULL OR tokenizer != 'model')",
        [],
        |r| r.get(0),
    )?;
    Ok(Savings {
        net: net.max(0),
        credited,
        total,
        measured: credited > 0 && estimated == 0,
    })
}

/// What the ledger adds up to.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Savings {
    /// Whole-file tokens less excerpt tokens, over rows that found something.
    pub net: i64,
    /// How many retrievals that was over.
    pub credited: i64,
    /// How many retrievals there are in total, credited or not. The
    /// denominator, so a figure is never shown without one.
    pub total: i64,
    /// Whether every credited row was counted with the store's own tokenizer.
    pub measured: bool,
}

impl Savings {
    /// `measured` or `modelled`.
    pub fn tier(&self) -> &'static str {
        if self.measured {
            "measured"
        } else {
            "modelled"
        }
    }

    /// What share of retrievals the figure covers, as a percentage.
    pub fn coverage(&self) -> i64 {
        if self.total == 0 {
            return 0;
        }
        self.credited * 100 / self.total
    }
}

/// What the ledger says about the questions semlith did *not* answer.
///
/// `refunds` counts the whole-file reads the steering hook saw on a file this
/// store holds: semlith could have answered them and was not asked. `zero_hit`
/// counts the retrievals semlith *was* asked and found nothing for. They are
/// separate figures because they call for opposite things — a refund means the
/// agent is not reaching for semlith, a zero hit means semlith is not reaching
/// the answer — and adding them together would say neither.
///
/// `measured` is false when no hook has ever written into this store, which is
/// what makes `refunds` a floor rather than a count on such a machine.
pub fn ledger_misses(db: &Connection) -> Result<Misses> {
    let refunds: i64 = db.query_row(
        "SELECT COUNT(*) FROM retrievals WHERE tool = ?1",
        params![crate::ledger::RAW_READ],
        |r| r.get(0),
    )?;
    let zero_hit: i64 = db.query_row(
        "SELECT COUNT(DISTINCT COALESCE(query_id, 'row:' || id)) FROM retrievals
         WHERE hits = 0 AND (tool IS NULL OR tool != ?1)",
        params![crate::ledger::RAW_READ],
        |r| r.get(0),
    )?;
    Ok(Misses {
        refunds,
        zero_hit,
        measured: refunds > 0,
    })
}

/// The two figures for what semlith did not answer.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct Misses {
    /// Whole-file reads of a file this store holds, seen by the steering hook.
    pub refunds: i64,
    /// Retrievals semlith answered with nothing.
    pub zero_hit: i64,
    /// Whether a hook has ever written here. Without one, `refunds` is a floor.
    pub measured: bool,
}

/// How many retrievals each client made, most first.
pub fn ledger_clients(db: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = db.prepare(
        "SELECT client, COUNT(*) FROM retrievals GROUP BY client ORDER BY COUNT(*) DESC, client",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

/// Re-walk the chain and return the id of the first row that does not verify.
///
/// `None` means the ledger is intact.
pub fn ledger_break(db: &Connection) -> Result<Option<i64>> {
    let mut stmt = db.prepare(
        "SELECT id, at, client, query, hits, micros, excerpt_tokens, whole_file_tokens, prev, hash,
                session, tool, stale_hits, tokenizer
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
        // `tool` is NULL on every row written before 0.15.0 and set on every
        // row written since, which is what says which formula wrote this hash.
        let tool: Option<String> = r.get(11)?;
        let recomputed = match tool {
            Some(tool) => {
                let session: String = r.get(10)?;
                let stale: i64 = r.get(12)?;
                let tokenizer: String = r.get(13)?;
                chain_hash(
                    &prev,
                    at,
                    &NewRetrieval {
                        client: &client,
                        session: &session,
                        tool: &tool,
                        query: &query,
                        hits,
                        micros,
                        excerpt_tokens: excerpt,
                        whole_file_tokens: whole,
                        stale_hits: stale,
                        tokenizer: &tokenizer,
                        // Not covered by the hash; see `chain_hash`.
                        query_id: "",
                    },
                )
            }
            None => legacy_chain_hash(&prev, at, &client, &query, hits, micros, excerpt, whole),
        };
        if recomputed != hash {
            return Ok(Some(id));
        }
        expected = hash;
    }
    Ok(None)
}

/// The total bytes of every file a grep for `name` would have to read.
///
/// The honest denominator for a graph answer's saving. A search's denominator
/// is the files its hits came from; a graph answer has no hits, but it does
/// have an answer that would otherwise have been assembled by grepping for the
/// name and reading what came back. These are those files: the ones that
/// define the name, and the ones whose code points at it.
pub fn grep_cost(db: &Connection, name: &str) -> Result<i64> {
    Ok(db.query_row(
        "SELECT COALESCE(SUM(bytes), 0) FROM files WHERE id IN (
            SELECT s.file_id FROM symbols s WHERE s.name = ?1
            UNION
            SELECT s.file_id FROM symbols s JOIN edges e ON e.src = s.id WHERE e.dst = ?1
         )",
        params![name],
        |r| r.get(0),
    )?)
}

/// Whether this store has `path` indexed.
///
/// Compared the way every filter compares a path — normalised separators,
/// lowercased — so a Windows store answers the same question a unix one does
/// about the same file. The steering hook's ledger row depends on it: a raw
/// read is only a refund against the store that could have answered it.
pub fn holds_path(db: &Connection, path: &str) -> Result<bool> {
    // Canonical, because that is the form a store records and a caller may hand
    // over whatever the client said — a relative path, a symlink, or on macOS a
    // `/var` that is really `/private/var`.
    let canonical = crate::canonical(std::path::Path::new(path));
    let wanted = canonical
        .to_string_lossy()
        .to_lowercase()
        .replace('\\', "/");
    let sql = format!("SELECT 1 FROM files f WHERE {GLOB_PATH} = ?1 LIMIT 1");
    Ok(db
        .query_row(&sql, params![wanted], |_| Ok(()))
        .optional()?
        .is_some())
}

/// One definition's span and identity: `(start, end, name, kind)`.
pub type SymbolSpan = (u32, u32, String, String);

/// Every symbol defined in each of `paths`.
///
/// What turns a line range into a place a person recognises. A hit that says
/// `src/store.rs:1041-1090` makes a reader open the file to find out what is
/// there; one that says `record_retrieval` does not.
///
/// Read per file rather than per hit, because eight hits in one file are one
/// question about that file.
pub fn symbols_in_files(
    db: &Connection,
    paths: &[String],
) -> Result<std::collections::HashMap<String, Vec<SymbolSpan>>> {
    let mut out: std::collections::HashMap<String, Vec<SymbolSpan>> = Default::default();
    if paths.is_empty() {
        return Ok(out);
    }
    let holes = vec!["?"; paths.len()].join(", ");
    let sql = format!(
        "SELECT f.path, s.start_line, s.end_line, s.name, s.kind
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path IN ({holes}) AND s.kind != 'module'
         ORDER BY f.path, s.start_line"
    );
    let mut stmt = db.prepare(&sql)?;
    let args = paths.iter().cloned().map(Value::Text);
    let mut rows = stmt.query(rusqlite::params_from_iter(args))?;
    while let Some(r) = rows.next()? {
        let path: String = r.get(0)?;
        out.entry(path)
            .or_default()
            .push((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?));
    }
    Ok(out)
}

/// `(bytes, indexed_at)` for each of `paths` the store knows.
///
/// The cheap half of a freshness check: what the store recorded about a file
/// when it read it. The other half is one `stat` of the file as it is now.
pub fn file_stamps(
    db: &Connection,
    paths: &[String],
) -> Result<std::collections::HashMap<String, (i64, i64)>> {
    if paths.is_empty() {
        return Ok(Default::default());
    }
    let holes = vec!["?"; paths.len()].join(", ");
    let sql = format!("SELECT path, bytes, indexed_at FROM files WHERE path IN ({holes})");
    let mut stmt = db.prepare(&sql)?;
    let args = paths.iter().cloned().map(Value::Text);
    let rows = stmt.query_map(rusqlite::params_from_iter(args), |r| {
        Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?)))
    })?;
    Ok(rows.collect::<Result<std::collections::HashMap<_, _>, _>>()?)
}

/// The newest `indexed_at` this store holds, or `None` when it holds nothing.
///
/// The store's own answer to "last write", which is what the Stores page's
/// column says it shows. The daemon used to answer it from a counter that
/// started at zero every time it launched, so a store with 438 files in it read
/// `never` until something wrote to it again — finding 1.7 of the 2026-09-17
/// drive.
pub fn last_write(db: &Connection) -> Result<Option<i64>> {
    let at: Option<i64> = db.query_row("SELECT MAX(indexed_at) FROM files", [], |r| r.get(0))?;
    Ok(at)
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

/// One session's whole ledger, as the per-session table reads it.
#[derive(Debug, Clone, Default)]
pub struct SessionRow {
    /// The session id the client sent at handshake. Empty for rows written
    /// before 0.15.0, which is said rather than guessed at.
    pub session: String,
    pub client: String,
    /// When the session's first and last recorded retrieval happened.
    pub first: i64,
    pub last: i64,
    pub retrievals: i64,
    /// Retrievals that found nothing. Recorded and credited nothing — a
    /// ledger that remembers only its successes is a marketing document.
    pub zero_hit: i64,
    pub excerpt_tokens: i64,
    pub whole_file_tokens: i64,
    /// Whole-file less excerpt, over the rows that found something.
    pub net: i64,
    /// Rows counted with the store's own tokenizer rather than by the
    /// four-characters fallback. The tier of this row's figures.
    pub measured: i64,
}

impl SessionRow {
    /// `measured` when every credited row in the session was counted by a
    /// model's own tokenizer, `modelled` otherwise. Never averaged: two rows
    /// counted two ways are not one number.
    pub fn tier(&self) -> &'static str {
        if self.measured == self.retrievals && self.retrievals > 0 {
            "measured"
        } else {
            "modelled"
        }
    }
}

/// Every session this store recorded, newest last-seen first.
///
/// Grouped in SQL rather than in the page, for the reason the totals are:
/// one search over six stores writes a row in each, and a browser adding
/// them up is a second opinion about a number the store can state.
pub fn ledger_sessions(db: &Connection, limit: usize) -> Result<Vec<SessionRow>> {
    let mut stmt = db.prepare(
        "SELECT COALESCE(session, ''), client,
                MIN(at), MAX(at), COUNT(*),
                SUM(CASE WHEN hits = 0 THEN 1 ELSE 0 END),
                SUM(excerpt_tokens), SUM(whole_file_tokens),
                SUM(CASE WHEN hits > 0 THEN whole_file_tokens - excerpt_tokens ELSE 0 END),
                SUM(CASE WHEN tokenizer IS NOT NULL AND tokenizer <> 'chars4' THEN 1 ELSE 0 END)
         FROM retrievals
         GROUP BY COALESCE(session, ''), client
         ORDER BY MAX(at) DESC
         LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(SessionRow {
            session: r.get(0)?,
            client: r.get(1)?,
            first: r.get(2)?,
            last: r.get(3)?,
            retrievals: r.get(4)?,
            zero_hit: r.get(5)?,
            excerpt_tokens: r.get(6)?,
            whole_file_tokens: r.get(7)?,
            net: r.get(8)?,
            measured: r.get(9)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Warm query latency as the ledger recorded it: p50 and p95, in microseconds.
///
/// From the rows themselves rather than from a benchmark, so the figure is
/// what this machine actually served rather than what it can serve.
pub fn ledger_latency(db: &Connection) -> Result<(i64, i64)> {
    let mut stmt = db.prepare("SELECT micros FROM retrievals ORDER BY micros")?;
    let rows = stmt.query_map([], |r| r.get::<_, i64>(0))?;
    let all: Vec<i64> = rows.collect::<rusqlite::Result<_>>()?;
    if all.is_empty() {
        return Ok((0, 0));
    }
    let at = |q: f64| all[((all.len() as f64 - 1.0) * q).round() as usize];
    Ok((at(0.50), at(0.95)))
}

/// The hash of the newest ledger row: what `--verify` checks the chain to.
pub fn ledger_head(db: &Connection) -> Result<Option<String>> {
    let hash: Option<String> = db
        .query_row(
            "SELECT hash FROM retrievals ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(hash)
}

/// Queries that found nothing, commonest first.
///
/// The gap list: a question an agent asked that this corpus could not answer
/// is either a missing document or a name nobody uses in the words they
/// searched with.
pub fn ledger_zero_hit_queries(db: &Connection, limit: usize) -> Result<Vec<(String, i64)>> {
    let mut stmt = db.prepare(
        "SELECT query, COUNT(*) AS n FROM retrievals WHERE hits = 0
         GROUP BY query ORDER BY n DESC, query LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Files this store read since `since`, newest first, with when it read them.
pub fn files_indexed_since(
    db: &Connection,
    since: i64,
    limit: usize,
) -> Result<Vec<(String, i64)>> {
    let mut stmt = db.prepare(
        "SELECT path, indexed_at FROM files WHERE indexed_at >= ?1
         ORDER BY indexed_at DESC LIMIT ?2",
    )?;
    let rows = stmt.query_map([since, limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// One settled dependency edge, by name, for the community pass.
///
/// Names rather than ids, because `edges.dst` is a name and the whole point
/// of the pass is to group names. Only edges whose target the store actually
/// defines are returned: an edge into a dependency that was never indexed
/// cannot join two communities of this corpus.
pub struct NamedEdge {
    pub from: String,
    pub to: String,
}

/// Every `calls` and `imports` edge whose target this store defines, up to
/// `limit`, with the total so a reader is told when the list was cut.
///
/// This is the one whole-graph read in the crate and it is bounded for that
/// reason. Everything else walks one hop at a time to keep peak memory flat
/// as a corpus grows; communities are a property of the whole graph and
/// cannot be computed a hop at a time, so the bound is the substitute — past
/// it the panel says `Shown N of M` rather than quietly clustering a slice.
pub fn community_edges(db: &Connection, limit: usize) -> Result<(Vec<NamedEdge>, i64)> {
    let target = crate::graph::not_navigational("s.kind");
    // `extracted` and `inferred` are the stored values; `resolved` and
    // `ambiguous` are computed per query and never written, so "settled" here
    // means an extracted edge or a bare-name match onto exactly one
    // definition. A name with several definitions is not evidence that two
    // subsystems are joined.
    let sql = format!(
        "SELECT src.name, s.name
         FROM symbols src
         JOIN edges e ON e.src = src.id
         JOIN symbols s ON s.name = e.dst AND {target}
         WHERE e.kind IN ('calls', 'imports')
           AND src.name <> s.name
           AND (SELECT COUNT(*) FROM symbols d WHERE d.name = e.dst AND {}) = 1
         LIMIT ?1",
        crate::graph::not_navigational("d.kind")
    );
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(NamedEdge {
            from: r.get(0)?,
            to: r.get(1)?,
        })
    })?;
    let edges: Vec<NamedEdge> = rows.collect::<rusqlite::Result<_>>()?;
    let total: i64 = db.query_row(
        "SELECT COUNT(*) FROM edges WHERE kind IN ('calls', 'imports')",
        [],
        |r| r.get(0),
    )?;
    Ok((edges, total))
}

/// Where each of these names is defined, for the hub lines under a community.
pub fn where_defined(db: &Connection, names: &[String]) -> Result<Vec<(String, String, u32)>> {
    if names.is_empty() {
        return Ok(Vec::new());
    }
    let holes = vec!["?"; names.len()].join(", ");
    let sql = format!(
        "SELECT s.name, f.path, s.start_line
         FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE s.name IN ({holes})
         ORDER BY f.path, s.start_line"
    );
    let mut stmt = db.prepare(&sql)?;
    let binds: Vec<Value> = names.iter().map(|n| Value::Text(n.clone())).collect();
    let rows = stmt.query_map(rusqlite::params_from_iter(binds), |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Call targets no definition in this store satisfies, commonest first.
///
/// The names a reader has to recognise as "outside this corpus" before they
/// read an absent edge as a missing call. Counted rather than listed in full:
/// on a store that indexes application code every standard-library call is
/// one of these, and the list is long and uninteresting past the first few.
pub fn unresolved_targets(db: &Connection, limit: usize) -> Result<(Vec<(String, i64)>, i64)> {
    let target = crate::graph::not_navigational("d.kind");
    let sql = format!(
        "SELECT e.dst, COUNT(*) AS n
         FROM edges e
         WHERE e.kind IN ('calls', 'imports')
           AND NOT EXISTS (SELECT 1 FROM symbols d WHERE d.name = e.dst AND {target})
         GROUP BY e.dst
         ORDER BY n DESC, e.dst
         LIMIT ?1"
    );
    let mut stmt = db.prepare(&sql)?;
    let rows = stmt.query_map([limit as i64], |r| Ok((r.get(0)?, r.get(1)?)))?;
    let top: Vec<(String, i64)> = rows.collect::<rusqlite::Result<_>>()?;
    let distinct: i64 = db.query_row(
        &format!(
            "SELECT COUNT(*) FROM (SELECT e.dst FROM edges e
             WHERE e.kind IN ('calls', 'imports')
               AND NOT EXISTS (SELECT 1 FROM symbols d WHERE d.name = e.dst AND {target})
             GROUP BY e.dst)"
        ),
        [],
        |r| r.get(0),
    )?;
    Ok((top, distinct))
}

/// How many names this store defines more than once.
///
/// The population of every ambiguous edge: a walk refuses to cross one of
/// these, so the count is what a reader needs to know before reading "not
/// connected" as "nothing calls it".
pub fn names_with_several_definitions(db: &Connection) -> Result<i64> {
    let target = crate::graph::not_navigational("s.kind");
    db.query_row(
        &format!(
            "SELECT COUNT(*) FROM (SELECT s.name FROM symbols s WHERE {target}
             GROUP BY s.name HAVING COUNT(*) > 1)"
        ),
        [],
        |r| r.get(0),
    )
    .map_err(Into::into)
}

/// Chunks by the month their file was indexed, oldest first.
///
/// Indexed rather than written: the store records when it read a file, not
/// when somebody wrote it, and labelling this "chunks per month" without
/// saying which would be a claim about a repository's history that the store
/// cannot make.
pub fn chunks_by_month(db: &Connection) -> Result<Vec<(String, i64)>> {
    let mut stmt = db.prepare(
        "SELECT strftime('%Y-%m', f.indexed_at, 'unixepoch') AS month, COUNT(c.id)
         FROM chunks c JOIN files f ON f.id = c.file_id
         GROUP BY month ORDER BY month",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
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
        insert_edge(&db, caller, "callee", "calls", "inferred", None, None).unwrap();
        assert_eq!(graph_stats(&db).unwrap(), (1, 1));

        delete_file(&db, "a.rs", 0).unwrap();
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
        insert_edge(&db, caller, "callee", "calls", "inferred", None, None).unwrap();
        let b = insert_file(&db, "b.rs", "h", 1, 0).unwrap();
        insert_symbol(&db, b, None, &sym("callee")).unwrap();
        assert_eq!(edges_out(&db, "caller", &[]).unwrap().len(), 1);

        // b.rs changes: its symbols are dropped and re-extracted with new ids.
        delete_file(&db, "b.rs", 0).unwrap();
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

    /// A store with one caller and several definitions of the name it calls.
    ///
    /// Returns the connection; `paths` are the files each definition of
    /// `callee` lives in, and the caller is always `src/caller.rs`.
    fn many_definitions(paths: &[&str]) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        one_symbol(&db, "src/caller.rs", "caller");
        for (i, path) in paths.iter().enumerate() {
            // The caller's own file is already there, and a definition in it
            // is exactly the case the same-file rank is about.
            let file = db
                .query_row("SELECT id FROM files WHERE path = ?1", params![path], |r| {
                    r.get(0)
                })
                .optional()
                .unwrap()
                .unwrap_or_else(|| insert_file(&db, path, &format!("h{i}"), 1, 0).unwrap());
            insert_symbol(&db, file, None, &sym("callee")).unwrap();
        }
        db
    }

    fn call(db: &Connection, hint: Option<&str>) {
        let src: i64 = db
            .query_row("SELECT id FROM symbols WHERE name = 'caller'", [], |r| {
                r.get(0)
            })
            .unwrap();
        insert_edge(db, src, "callee", "calls", "inferred", hint, None).unwrap();
    }

    /// One definition of the name is one answer, whatever the hint says.
    #[test]
    fn a_name_with_one_definition_resolves() {
        let db = many_definitions(&["src/other.rs"]);
        call(&db, None);
        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, crate::graph::RESOLVED);
        assert_eq!(out[0].definitions, 1);
    }

    /// Four definitions and nothing to choose between them. Every candidate
    /// comes back marked `ambiguous` and carrying the count, so a renderer can
    /// say "4 definitions" instead of printing four calls the source never
    /// made.
    #[test]
    fn a_name_nothing_narrows_is_ambiguous_and_keeps_every_candidate() {
        let db = many_definitions(&["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs"]);
        call(&db, None);
        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out.len(), 4);
        assert!(out.iter().all(|e| e.confidence == crate::graph::AMBIGUOUS));
        assert!(out.iter().all(|e| e.definitions == 4));
    }

    /// The hint picks the definition whose file it names, and the other three
    /// are not returned at all.
    #[test]
    fn a_hint_that_names_a_file_resolves_to_that_definition() {
        let db = many_definitions(&["src/a.rs", "src/store.rs", "src/c.rs"]);
        call(&db, Some("store"));
        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].symbol.path, "src/store.rs");
        assert_eq!(out[0].confidence, crate::graph::RESOLVED);
        assert_eq!(
            out[0].definitions, 3,
            "the count is what the ranking chose between, not what survived"
        );
    }

    /// A hint naming a directory works the same way: a module is a file or a
    /// directory depending on how the author felt that day.
    #[test]
    fn a_hint_names_a_directory_as_readily_as_a_file() {
        let db = many_definitions(&["src/a.rs", "src/store/mod.rs"]);
        call(&db, Some("store"));
        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].symbol.path, "src/store/mod.rs");
    }

    /// The calling file's own definition wins over one the hint names. A file
    /// that defines a name and calls it means its own.
    #[test]
    fn the_calling_file_outranks_the_hint() {
        let db = many_definitions(&["src/caller.rs", "src/store.rs"]);
        call(&db, Some("store"));
        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].symbol.path, "src/caller.rs");
    }

    /// With no hint, what the calling file imports breaks the tie.
    #[test]
    fn an_import_resolves_what_the_hint_cannot() {
        let db = many_definitions(&["src/a.rs", "src/b.rs"]);
        let src: i64 = db
            .query_row("SELECT id FROM symbols WHERE name = 'caller'", [], |r| {
                r.get(0)
            })
            .unwrap();
        insert_edge(
            &db,
            src,
            "crate::b::callee",
            "imports",
            "extracted",
            None,
            None,
        )
        .unwrap();
        call(&db, None);
        let out = edges_out(&db, "caller", &["calls".to_string()]).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].symbol.path, "src/b.rs");
        assert_eq!(out[0].confidence, crate::graph::RESOLVED);
    }

    /// The hint outranks the import list: an import is a set of possibilities,
    /// a hint is a statement about this call.
    #[test]
    fn the_hint_outranks_an_import() {
        let db = many_definitions(&["src/a.rs", "src/b.rs"]);
        let src: i64 = db
            .query_row("SELECT id FROM symbols WHERE name = 'caller'", [], |r| {
                r.get(0)
            })
            .unwrap();
        insert_edge(
            &db,
            src,
            "crate::b::callee",
            "imports",
            "extracted",
            None,
            None,
        )
        .unwrap();
        call(&db, Some("a"));
        let out = edges_out(&db, "caller", &["calls".to_string()]).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(out[0].symbol.path, "src/a.rs");
    }

    /// An edge the syntax tree settled is never downgraded by a ranking.
    #[test]
    fn an_extracted_edge_stays_extracted() {
        let db = many_definitions(&["src/store.rs"]);
        let src: i64 = db
            .query_row("SELECT id FROM symbols WHERE name = 'caller'", [], |r| {
                r.get(0)
            })
            .unwrap();
        insert_edge(
            &db,
            src,
            "callee",
            "calls",
            "extracted",
            Some("store"),
            None,
        )
        .unwrap();
        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out[0].confidence, crate::graph::EXTRACTED);
    }

    /// A store written by 0.14.0 has a NULL hint on every row. It must answer
    /// exactly as it did then: one definition is the answer, several are a
    /// question.
    #[test]
    fn a_null_hint_behaves_as_it_did_before_hints_existed() {
        let db = many_definitions(&["src/a.rs"]);
        call(&db, None);
        let one = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].symbol.path, "src/a.rs");

        let db = many_definitions(&["src/a.rs", "src/b.rs"]);
        call(&db, None);
        let two = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(
            two.len(),
            2,
            "both candidates are still reachable, now labelled rather than asserted"
        );
    }

    /// Callers are unchanged: the row came from the edge's own `src` id, so
    /// there is nothing to resolve and the stored confidence stands.
    #[test]
    fn callers_keep_the_stored_confidence() {
        let db = many_definitions(&["src/a.rs", "src/b.rs"]);
        call(&db, None);
        let inbound = edges_in(&db, "callee", &[]).unwrap();
        assert_eq!(inbound.len(), 1);
        assert_eq!(inbound[0].confidence, crate::graph::INFERRED);
        assert_eq!(inbound[0].definitions, 1);
    }

    /// Both directions resolve, and the edge kind filter applies to each.
    #[test]
    fn edges_resolve_in_both_directions_and_filter_by_kind() {
        let db = Connection::open_in_memory().unwrap();
        let caller = one_symbol(&db, "a.rs", "caller");
        let b = insert_file(&db, "b.rs", "h", 1, 0).unwrap();
        insert_symbol(&db, b, None, &sym("callee")).unwrap();
        insert_edge(&db, caller, "callee", "calls", "extracted", None, None).unwrap();
        insert_edge(&db, caller, "callee", "references", "inferred", None, None).unwrap();

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
        insert_edge(&db, caller, "println", "calls", "inferred", None, None).unwrap();
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

    /// An exclusion on its own means everything except, so it needs no
    /// inclusion beside it to be a filter.
    #[test]
    fn an_exclusion_alone_means_everything_except() {
        let db = mixed();
        let f = filter(&["!**/vendor/**"], &[], &[]);
        assert_eq!(matching_files(&db, &f).unwrap(), 3);
    }

    /// The exclusion applies after the inclusions of its own kind, so the two
    /// intersect rather than the later one replacing the earlier.
    #[test]
    fn an_exclusion_narrows_the_inclusions_of_its_kind() {
        let db = mixed();
        let f = filter(&["proj/**", "!**/vendor/**"], &[], &[]);
        assert_eq!(matching_files(&db, &f).unwrap(), 3);

        let f = filter(&["proj/src/**", "!**/notes.md"], &[], &[]);
        assert_eq!(matching_files(&db, &f).unwrap(), 1);
    }

    /// The bind values are positional, so an exclusion written before an
    /// inclusion must still bind to the `?` the clause puts it against.
    #[test]
    fn an_exclusion_written_first_still_binds_to_its_own_placeholder() {
        let db = mixed();
        let written_first = filter(&["!**/vendor/**", "proj/**"], &[], &[]);
        let written_last = filter(&["proj/**", "!**/vendor/**"], &[], &[]);
        assert_eq!(
            matching_files(&db, &written_first).unwrap(),
            matching_files(&db, &written_last).unwrap(),
        );
        assert_eq!(matching_files(&db, &written_first).unwrap(), 3);
    }

    /// Extensions negate the way paths do, and case-insensitively: a store
    /// holding `README.MD` loses it to `--ext '!md'`.
    #[test]
    fn an_excluded_extension_removes_every_spelling_of_it() {
        let db = mixed();
        let f = filter(&[], &["!md"], &[]);
        assert_eq!(matching_files(&db, &f).unwrap(), 2);
    }

    /// A group holding only exclusions still selects from everything, and the
    /// two kinds still intersect across groups.
    #[test]
    fn exclusions_of_different_kinds_intersect() {
        let db = mixed();
        let f = filter(&["!**/vendor/**"], &["!md"], &[]);
        assert_eq!(matching_files(&db, &f).unwrap(), 1);
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
        delete_file(&db, "/a/lib.rs", 0).unwrap();
        assert!(
            keyword_search(&db, "EMBED_BATCH", 10, &[])
                .unwrap()
                .is_empty(),
            "cascade delete left the keyword index stale"
        );
    }

    /// A store written by 0.15.0 has no `edges.line`. Opening it must add the
    /// column without touching the rows, and every edge already in it must
    /// report no call site rather than a wrong one.
    #[test]
    fn a_pre_0_16_store_opens_and_its_edges_report_no_call_site() {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE,
                 hash TEXT NOT NULL, bytes INTEGER NOT NULL, indexed_at INTEGER NOT NULL);
             CREATE TABLE symbols (id INTEGER PRIMARY KEY AUTOINCREMENT,
                 file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
                 chunk_id INTEGER, kind TEXT NOT NULL, name TEXT NOT NULL,
                 qualified TEXT NOT NULL, start_line INTEGER NOT NULL, end_line INTEGER NOT NULL);
             CREATE TABLE edges (src INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
                 dst TEXT NOT NULL, kind TEXT NOT NULL, confidence TEXT NOT NULL, hint TEXT);
             INSERT INTO files VALUES (1, '/a/lib.rs', 'h', 10, 0);
             INSERT INTO symbols VALUES (1, 1, NULL, 'function', 'caller', 'm::caller', 1, 9);
             INSERT INTO symbols VALUES (2, 1, NULL, 'function', 'callee', 'm::callee', 20, 24);
             CREATE TABLE retrievals (id INTEGER PRIMARY KEY AUTOINCREMENT, at INTEGER NOT NULL,
                 client TEXT NOT NULL, query TEXT NOT NULL, hits INTEGER NOT NULL,
                 micros INTEGER NOT NULL, excerpt_tokens INTEGER NOT NULL,
                 whole_file_tokens INTEGER NOT NULL);
             INSERT INTO edges VALUES (1, 'callee', 'calls', 'inferred', NULL);",
        )
        .unwrap();

        add_columns(&db).unwrap();

        let out = edges_out(&db, "caller", &[]).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!(
            out[0].line, None,
            "a 0.15.0 row must not invent a call site"
        );
        assert_eq!(
            crate::graph::call_site(&out[0], &crate::graph::verbatim),
            "",
            "nothing is rendered for an edge with no line"
        );

        let into = edges_in(&db, "callee", &[]).unwrap();
        assert_eq!(into.len(), 1, "{into:?}");
        assert_eq!(into[0].line, None);
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
