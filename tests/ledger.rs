//! The retrieval ledger: off unless asked for, chained so it cannot be quietly
//! edited, and free — nothing here ever asks for a key.
//!
//! ```sh
//! cargo test --test ledger -- --ignored
//! ```

use semlith::Semlith;
use std::path::Path;

/// One row's worth of arguments, so a test that cares about two of them does
/// not have to spell out the other eight.
fn row<'a>(
    client: &'a str,
    query: &'a str,
    excerpt: i64,
    whole: i64,
) -> semlith::store::NewRetrieval<'a> {
    semlith::store::NewRetrieval {
        client,
        session: "test",
        tool: "search",
        query,
        hits: 1,
        micros: 4200,
        excerpt_tokens: excerpt,
        whole_file_tokens: whole,
        stale_hits: 0,
        tokenizer: semlith::ledger::CHARS4,
    }
}

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
    semlith::store::record_retrieval(s.db(), &row("claude-code", "sourdough starter", 180, 4000))
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
        semlith::store::record_retrieval(s.db(), &row("agent", &format!("query {i}"), 5, 50))
            .unwrap();
    }
    assert!(
        semlith::store::ledger_break(s.db()).unwrap().is_none(),
        "an untouched ledger must verify"
    );

    // Somebody rewrites what the second query was. From 0.14.0 a store
    // connection refuses writes until one of the three write paths asks, so the
    // tamper is made through a connection put deliberately into the state a
    // tamperer's own `sqlite3` would already be in — what is under test here is
    // the hash chain noticing the edit, not who was able to make it.
    semlith::store::read_only(s.db(), false).unwrap();
    s.db()
        .execute(
            "UPDATE retrievals SET query = 'something else' WHERE id = 2",
            [],
        )
        .unwrap();
    semlith::store::read_only(s.db(), true).unwrap();

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

/// The release's central ledger claim, end to end: a retrieval made by an
/// agent over stdio is recorded, under the name the agent gave itself.
///
/// Until 0.15.0 this produced nothing at all. The portal's own search box was
/// the only writer, so the savings figure the product is built on counted the
/// one user who was not the point.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_search_and_a_graph_call_over_stdio_are_recorded_under_the_client_name() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    // A client that names itself `harness`, then asks two different questions.
    let script = [
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","clientInfo":{"name":"harness","version":"1"}}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"semlith_search","arguments":{"query":"sourdough","k":3}}}"#,
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"semlith_neighbors","arguments":{"name":"take"}}}"#,
    ]
    .join("\n");

    let mut fleet = semlith::fleet::Fleet::open(&[store.path().to_path_buf()]).unwrap();
    let mut out = Vec::new();
    semlith::mcp::serve(&mut fleet, script.as_bytes(), &mut out).unwrap();
    drop(fleet);

    let s = Semlith::open(store.path(), None).unwrap();
    let rows = semlith::store::retrievals(s.db(), 10).unwrap();
    assert!(
        !rows.is_empty(),
        "an agent's retrievals over stdio were recorded as nothing, which is the \
         defect this release exists to fix"
    );
    assert!(
        rows.iter().all(|r| r.client == "harness"),
        "the ledger records the name the client gave itself: {rows:?}"
    );
    assert!(
        semlith::store::ledger_break(s.db()).unwrap().is_none(),
        "the chain must verify over rows written by the new formula"
    );
}

/// A retrieval made from the command line counts for exactly as much as an
/// agent's, and is recorded under its own name.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_search_from_the_command_line_is_recorded_as_cli() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    cli(store.path(), &["search", "sourdough", "--k", "3"]);

    let s = Semlith::open(store.path(), None).unwrap();
    let rows = semlith::store::retrievals(s.db(), 10).unwrap();
    assert!(
        rows.iter().any(|r| r.client == "cli"),
        "the command line is a client like any other: {rows:?}"
    );
}

/// A store holding rows of both kinds verifies end to end.
///
/// The formula changed in 0.15.0, and the row says which one wrote it. Getting
/// this wrong in either direction is worse than having no verify: a chain that
/// fails on every pre-0.15.0 ledger cries wolf, and one that passes over a row
/// it cannot actually check is a lie with a green tick on it. So both
/// directions are asserted — first that an untouched mixed store is intact,
/// then that an edit to a row of *each* kind is found.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_store_holding_rows_of_both_formulas_verifies_and_still_catches_an_edit() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    let s = Semlith::open(store.path(), None).unwrap();
    // Row 1 and 2 in the old shape, row 3 and 4 in the new one.
    semlith::store::record_legacy_retrieval(s.db(), "claude-code", "old one", 10, 100).unwrap();
    semlith::store::record_legacy_retrieval(s.db(), "claude-code", "old two", 10, 100).unwrap();
    semlith::store::record_retrieval(s.db(), &row("cursor", "new one", 20, 200)).unwrap();
    semlith::store::record_retrieval(s.db(), &row("cursor", "new two", 20, 200)).unwrap();
    assert_eq!(rows(s.db()), 4);
    assert!(
        semlith::store::ledger_break(s.db()).unwrap().is_none(),
        "a store written by two releases must verify across the boundary"
    );

    // An edit to the older row is found.
    semlith::store::read_only(s.db(), false).unwrap();
    s.db()
        .execute("UPDATE retrievals SET query = 'tampered' WHERE id = 1", [])
        .unwrap();
    semlith::store::read_only(s.db(), true).unwrap();
    assert_eq!(
        semlith::store::ledger_break(s.db()).unwrap(),
        Some(1),
        "an edit to a pre-0.15.0 row was not detected"
    );

    // Put it back, and edit a row of the newer kind instead.
    semlith::store::read_only(s.db(), false).unwrap();
    s.db()
        .execute("UPDATE retrievals SET query = 'old one' WHERE id = 1", [])
        .unwrap();
    s.db()
        .execute("UPDATE retrievals SET stale_hits = 9 WHERE id = 3", [])
        .unwrap();
    semlith::store::read_only(s.db(), true).unwrap();
    assert_eq!(
        semlith::store::ledger_break(s.db()).unwrap(),
        Some(3),
        "an edit to one of the new columns was not detected, which would mean the \
         chain does not actually cover them"
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
