//! The retrieval ledger: off unless asked for, chained so it cannot be quietly
//! edited, and free — nothing here ever asks for a key.
//!
//! ```sh
//! cargo test --test ledger -- --ignored
//! ```

use semlith::Semlith;
use std::path::Path;

/// Recording is off by default, and a search with it off writes nothing.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn nothing_is_recorded_unless_recording_was_asked_for() {
    let (corpus, store) = corpus();
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    s.search("sourdough", 3).unwrap();

    assert_eq!(
        rows(s.db()),
        0,
        "a search recorded a row without anyone switching recording on"
    );
    assert!(semlith::store::ledger_break(s.db()).unwrap().is_none());
}

/// With recording on, one retrieval is one row, and the CLI prints it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn one_recorded_retrieval_is_one_row_the_cli_prints() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    let s = Semlith::open(store.path(), None).unwrap();
    semlith::store::record_retrieval(
        s.db(),
        "claude-code",
        "sourdough starter",
        3,
        4200,
        180,
        4000,
    )
    .unwrap();
    assert_eq!(rows(s.db()), 1);

    let (queries, clients, excerpt, whole) = semlith::store::ledger_totals(s.db()).unwrap();
    assert_eq!((queries, clients, excerpt, whole), (1, 1, 180, 4000));
    drop(s);

    let printed = cli(store.path(), &["ledger", "--last", "5"]);
    assert!(printed.contains("sourdough starter"), "{printed}");
    assert!(printed.contains("claude-code"), "{printed}");
}

/// The chain is the point: a row cannot be edited or removed unnoticed.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn editing_a_row_breaks_the_chain_and_the_break_is_found() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    let s = Semlith::open(store.path(), None).unwrap();
    for i in 0..4 {
        semlith::store::record_retrieval(s.db(), "agent", &format!("query {i}"), 1, 10, 5, 50)
            .unwrap();
    }
    assert!(
        semlith::store::ledger_break(s.db()).unwrap().is_none(),
        "an untouched ledger must verify"
    );

    // Somebody rewrites what the second query was.
    s.db()
        .execute(
            "UPDATE retrievals SET query = 'something else' WHERE id = 2",
            [],
        )
        .unwrap();

    assert_eq!(
        semlith::store::ledger_break(s.db()).unwrap(),
        Some(2),
        "the edited row was not detected"
    );
}

/// A store that has never recorded says so, rather than erroring or implying
/// the feature is missing.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_empty_ledger_explains_itself() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }
    let printed = cli(store.path(), &["ledger"]);
    assert!(printed.contains("nothing recorded"), "{printed}");
    assert!(
        printed.contains("ever leaves this machine"),
        "the empty case should still say where the data would live: {printed}"
    );
}

/// A 0.11.0 store has no `retrievals` table; opening it under this binary adds
/// one, empty, and changes nothing else.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_older_store_gains_an_empty_ledger_and_keeps_its_format() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    // What an older binary leaves behind: the tables this release adds, gone.
    {
        let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
        db.execute_batch("DROP TABLE retrievals; DROP TABLE edges; DROP TABLE symbols;")
            .unwrap();
    }

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    assert_eq!(rows(s.db()), 0, "the ledger came back empty");
    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        2,
        "opening a store must not move its format version"
    );
    assert!(
        !s.search("sourdough", 3).unwrap().is_empty(),
        "the store still searches with an empty graph and an empty ledger"
    );
}

// ----------------------------------------------------------------- helpers

fn corpus() -> (tempfile::TempDir, tempfile::TempDir) {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    std::fs::write(
        corpus.path().join("bread.md"),
        "Sourdough starter needs flour and water and a warm shelf.",
    )
    .unwrap();
    std::fs::write(
        corpus.path().join("lock.rs"),
        "fn hold() {}\nfn take() { hold(); }\n",
    )
    .unwrap();
    (corpus, store)
}

fn rows(db: &rusqlite::Connection) -> i64 {
    db.query_row("SELECT COUNT(*) FROM retrievals", [], |r| r.get(0))
        .unwrap()
}

fn cli(store: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .args(args)
        .output()
        .expect("the binary runs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}
