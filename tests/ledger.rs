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
        // Empty is what every row written before 0.20.2 holds, and it is what
        // keeps this helper's rows one retrieval each. The rows of one
        // cross-store search sharing an id have their own test below.
        query_id: "",
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

/// The ledger counts with the model's own tokenizer, and says which counted.
///
/// This is the test that would have caught the release's worst near-miss.
/// `Fleet` keeps its own embedder pool and never calls `Semlith::embedder`, so
/// hanging the tokenizer off `Semlith` left every real path — the CLI, MCP, the
/// portal — falling back to four characters per token while the contract, the
/// documentation and the changelog all said otherwise. The rows said `chars4`
/// and nothing else did.
///
/// Asserting the label rather than the number, because the number is the
/// tokenizer's business and the label is the ledger's promise.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_recorded_retrieval_says_the_model_counted_it() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }

    // Through the fleet, which is what every surface actually uses.
    let mut fleet = semlith::fleet::Fleet::open(&[store.path().to_path_buf()]).unwrap();
    fleet.quiet = true;
    let hits = fleet
        .search_filtered("sourdough", 3, &semlith::filter::Filter::default())
        .unwrap();
    assert!(!hits.is_empty(), "the fixture corpus should answer this");
    semlith::ledger::search(
        &fleet,
        &semlith::ledger::Who {
            client: "harness",
            session: "harness",
        },
        "sourdough",
        &hits,
        std::time::Duration::from_millis(1),
    );
    drop(fleet);

    let s = Semlith::open(store.path(), None).unwrap();
    let labels: Vec<String> = s
        .db()
        .prepare("SELECT tokenizer FROM retrievals")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert!(!labels.is_empty(), "nothing was recorded");
    assert!(
        labels.iter().all(|l| l == semlith::ledger::MODEL),
        "the ledger fell back to an estimate while claiming to count: {labels:?}"
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

/// One search across several stores is one retrieval.
///
/// Each store that answered keeps its own row — that is what makes its chain
/// its own and its token figures about its own hits — and the rows share one
/// id, so what the Ledger page counts is searches rather than open stores.
/// Before 0.20.2 a search with six stores open was counted six times.
#[test]
fn rows_sharing_a_query_id_count_as_one_retrieval() {
    let dir = tempfile::tempdir().unwrap();
    let s = Semlith::open(dir.path(), None).unwrap();

    let shared = "one-search";
    for (excerpt, whole) in [(10, 100), (20, 200)] {
        let mut r = row("portal", "sourdough", excerpt, whole);
        r.query_id = shared;
        semlith::store::record_retrieval(s.db(), &r).unwrap();
    }
    // A row from before this release, with no id: its own retrieval, as it was.
    semlith::store::record_retrieval(s.db(), &row("cli", "rye", 5, 50)).unwrap();

    let (queries, _, excerpt, whole) = semlith::store::ledger_totals(s.db()).unwrap();
    assert_eq!(queries, 2, "two searches, three rows");
    // The token sums stay sums of rows: each row holds its own store's hits.
    assert_eq!((excerpt, whole), (35, 350));

    // And the chain still walks. The id is not part of the hash — see
    // `chain_hash` — so a 0.20.1 binary, which is this release's rollback
    // path, verifies these rows too rather than reporting every one of them
    // as broken.
    assert_eq!(semlith::store::ledger_break(s.db()).unwrap(), None);
    let with_id = semlith::store::retrievals(s.db(), 10).unwrap();
    assert!(
        with_id.iter().any(|row| row.query_id == shared),
        "the id is still stored and read back, it is only left out of the hash"
    );
}
