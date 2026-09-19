//! What a symbol used to be.
//!
//! From 0.23.0 a re-index copies a file's definitions into `symbols_past`
//! before the file row takes them with it, stamped with the content hash they
//! were true for. These tests build real stores and embed real text, so they
//! are slow and they download an embedding model on first run:
//!
//! ```sh
//! cargo test --test history -- --ignored
//! ```

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const FIRST: &str = r#"
/// Decide whether a stored file still looks the way it did when it was read.
pub fn is_stale(recorded_size: u64, size_now: u64) -> bool {
    recorded_size != size_now
}
"#;

const SECOND: &str = r#"
/// Decide whether a stored file still looks the way it did when it was read.
///
/// Now also compares the modification time, because a file rewritten to the
/// same length is the case a size alone cannot see.
pub fn is_stale(recorded_size: u64, size_now: u64, recorded_at: i64, now: i64) -> bool {
    recorded_size != size_now || recorded_at != now
}
"#;

const THIRD: &str = r#"
/// Decide whether a stored file still looks the way it did when it was read.
pub fn is_stale(recorded: (u64, i64), now: (u64, i64)) -> bool {
    recorded != now
}
"#;

// ---------------------------------------------------------------- T11

/// A store written before this release opens unchanged, answers a query, and is
/// not migrated by being opened -- it simply starts keeping history at its next
/// index pass.
///
/// The older shape is made by dropping the table, which is exactly what a
/// 0.22.0 store has: the schema is additive and `IF NOT EXISTS`, so the format
/// version never moved and an older binary's store differs from a newer one's
/// only by this table's absence.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_from_before_this_release_opens_unchanged_and_starts_keeping_history() {
    let corpus = corpus(&[("drift.rs", FIRST)]);
    let store = index(&corpus);

    // Make it look like a store 0.22.0 wrote.
    let db = semlith::store::open(&store.join("store.db")).unwrap();
    db.execute_batch("PRAGMA query_only = OFF; DROP TABLE symbols_past;")
        .unwrap();
    drop(db);

    // Opening answers a query, and does not fail on the missing table.
    let hits = search(&store, "how does a file look stale");
    assert!(
        !hits.as_array().unwrap().is_empty(),
        "a store from before this release stopped answering: {hits}"
    );
    assert_eq!(
        retired(&store),
        0,
        "opening the store invented history it never had"
    );

    // One edit, one pass, and the definition it replaced is recorded.
    write(&corpus, "drift.rs", SECOND);
    reindex(&corpus, &store);

    let past = history(&store, "is_stale");
    let rows = past.as_array().unwrap();
    assert_eq!(
        rows.len(),
        1,
        "one edit should retire one definition: {past}"
    );
    assert_eq!(rows[0]["name"], "is_stale");
    assert!(
        rows[0]["content_hash"]
            .as_str()
            .is_some_and(|h| !h.is_empty()),
        "a retired definition with no content hash is not bi-temporal: {past}"
    );
    assert!(
        rows[0]["start_line"].as_u64().is_some(),
        "a retired definition with no span: {past}"
    );
}

/// The current answer is what a caller that asks for nothing still gets. Every
/// existing caller depends on it, which is why it is a test rather than a note.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_symbol_asked_for_with_no_history_answers_exactly_as_before() {
    let corpus = corpus(&[("drift.rs", FIRST)]);
    let store = index(&corpus);
    let before = symbol(&store, "is_stale");

    write(&corpus, "drift.rs", SECOND);
    reindex(&corpus, &store);
    let after = symbol(&store, "is_stale");

    // The definition moved, so the spans differ; what must not differ is the
    // shape of the answer or the fact that it is the *current* one.
    let definitions = after["definitions"].as_array().unwrap();
    assert_eq!(
        definitions.len(),
        1,
        "the current answer now carries more than the live definition: {after}"
    );
    assert_eq!(
        definitions[0]["name"], "is_stale",
        "the current answer stopped being about the symbol asked for: {after}"
    );
    assert!(
        before["definitions"].as_array().unwrap().len() == 1,
        "the answer before the edit was already wrong: {before}"
    );
    assert!(
        after.get("past").is_none(),
        "history leaked into the default answer: {after}"
    );
}

// ---------------------------------------------------------------- NON05

/// One row per changed symbol per pass, and not one more.
///
/// Three passes with an edit between each. The bound is the number of
/// definitions the file actually held -- a rewritten file replaces all of them,
/// including the module symbol every file carries -- so the count is read from
/// the store rather than written into the test, which is what makes this a
/// measurement of the growth rather than a restatement of it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn history_grows_by_one_row_per_changed_symbol_per_pass() {
    let corpus = corpus(&[("drift.rs", FIRST)]);
    let store = index(&corpus);
    assert_eq!(retired(&store), 0, "the first pass retired something");

    let per_pass = live_symbols(&store);
    assert!(per_pass > 0, "the fixture holds no symbols at all");

    write(&corpus, "drift.rs", SECOND);
    reindex(&corpus, &store);
    assert_eq!(
        retired(&store),
        per_pass,
        "the second pass retired something other than the {per_pass} definitions it replaced"
    );

    write(&corpus, "drift.rs", THIRD);
    reindex(&corpus, &store);
    assert_eq!(
        retired(&store),
        2 * per_pass,
        "the third pass retired something other than the definitions it replaced"
    );

    // A pass over an unchanged file rewrites nothing, so it retires nothing.
    reindex(&corpus, &store);
    assert_eq!(
        retired(&store),
        2 * per_pass,
        "a pass over an unchanged file retired a definition anyway"
    );
}

// ---------------------------------------------------------------- T05

/// The sidecar is written beside the codes and `stats` says so, because a store
/// without one is ranked by codes alone and that is invisible from the answers.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_carries_the_full_precision_sidecar_and_says_so() {
    let corpus = corpus(&[("drift.rs", FIRST)]);
    let store = index(&corpus);

    assert!(
        store.join("exact.f32").exists(),
        "the index pass wrote no f32 sidecar"
    );
    let stats = run(&store, &["stats"]);
    assert!(
        stats.contains("exact") && stats.contains("full-precision"),
        "stats does not say the store carries a sidecar:\n{stats}"
    );
    assert!(
        stats.contains("history"),
        "stats does not say whether the store keeps symbol history:\n{stats}"
    );

    // Every vector, and nothing but: one fixed-size record per chunk.
    let vectors: u64 = stats
        .lines()
        .find_map(|l| l.strip_prefix("vectors")?.trim().parse().ok())
        .expect("a vector count in stats");
    let bytes = fs::metadata(store.join("exact.f32")).unwrap().len();
    assert_eq!(
        bytes % vectors,
        0,
        "the sidecar is not a whole number of fixed-size records: {bytes} over {vectors}"
    );
}

// ---------------------------------------------------------------- helpers

fn corpus(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("history")
        .tempdir()
        .unwrap();
    let inner = dir.path().join("src");
    fs::create_dir_all(&inner).unwrap();
    for (file, body) in files {
        fs::write(inner.join(file), body).unwrap();
    }
    dir
}

fn write(corpus: &tempfile::TempDir, file: &str, body: &str) {
    fs::write(corpus.path().join("src").join(file), body).unwrap();
}

fn index(corpus: &tempfile::TempDir) -> PathBuf {
    let inner = corpus.path().join("src");
    let store = inner.join(".semlith");
    reindex(corpus, &store);
    store
}

fn reindex(corpus: &tempfile::TempDir, store: &Path) {
    let inner = corpus.path().join("src");
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .arg("index")
        .arg(&inner)
        .arg("--quiet")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "indexing failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn run(store: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "semlith {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn json(store: &Path, args: &[&str]) -> Value {
    let text = run(store, args);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON: {e}: {text}"))
}

fn search(store: &Path, query: &str) -> Value {
    json(store, &["search", query, "--json"])
}

fn symbol(store: &Path, name: &str) -> Value {
    json(store, &["symbol", name, "--json"])
}

fn history(store: &Path, name: &str) -> Value {
    json(store, &["symbol", name, "--history", "--json"])
}

/// How many live definitions the store holds, counted through the database the
/// store itself opens rather than through a second copy of the schema.
fn live_symbols(store: &Path) -> u64 {
    let db = semlith::store::open(&store.join("store.db")).unwrap();
    db.query_row("SELECT COUNT(*) FROM symbols", [], |r| r.get::<_, i64>(0))
        .unwrap() as u64
}

/// How many definitions the store has retired, read off `stats`.
fn retired(store: &Path) -> u64 {
    run(store, &["stats"])
        .lines()
        .find_map(|l| {
            l.strip_prefix("history")?
                .split_whitespace()
                .next()?
                .parse()
                .ok()
        })
        .expect("a history line in stats")
}
