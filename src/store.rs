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

CREATE TABLE IF NOT EXISTS chunks (
    id         INTEGER PRIMARY KEY,
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
    backfill_fts(&db)?;
    Ok(db)
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

#[cfg(test)]
mod tests {
    use super::*;

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
            let f = insert_file(&db, path, "h", 10, 0).unwrap();
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
            ["/proj/src/lib.rs", "/proj/src/notes.md"]
        );
    }

    /// An absolute pattern means exactly itself: it must not pick up the
    /// `*/` anchoring a relative one gets.
    #[test]
    fn an_absolute_glob_matches_only_what_it_literally_covers() {
        let db = mixed();
        let ids = filtered_chunk_ids(&db, &filter(&["/proj/vendor/*"], &[], &[])).unwrap();
        assert_eq!(paths_of(&db, &ids), ["/proj/vendor/other.rs"]);
    }

    #[test]
    fn extensions_union_and_a_language_resolves_to_the_same_set() {
        let db = mixed();
        let by_ext = filtered_chunk_ids(&db, &filter(&[], &["rs"], &[])).unwrap();
        let by_lang = filtered_chunk_ids(&db, &filter(&[], &[], &["rust"])).unwrap();
        assert_eq!(paths_of(&db, &by_ext), paths_of(&db, &by_lang));
        assert_eq!(
            paths_of(&db, &by_ext),
            ["/proj/src/lib.rs", "/proj/vendor/other.rs"]
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
            ["/proj/README.MD", "/proj/src/notes.md"]
        );
    }

    #[test]
    fn a_path_and_an_extension_intersect() {
        let db = mixed();
        let ids = filtered_chunk_ids(&db, &filter(&["src/**"], &["md"], &[])).unwrap();
        assert_eq!(
            paths_of(&db, &ids),
            ["/proj/src/notes.md"],
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
            ["/proj/src/lib.rs", "/proj/src/notes.md"]
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
