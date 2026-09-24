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

    // What an older binary leaves behind: the tables this release adds, gone,
    // and the format row where that binary left it.
    let older = semlith::store::SHARDED_FORMAT;
    {
        let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
        db.execute_batch("DROP TABLE retrievals; DROP TABLE edges; DROP TABLE symbols;")
            .unwrap();
        db.execute(
            "UPDATE meta SET v = ?1 WHERE k = ?2",
            rusqlite::params![older.to_string(), semlith::store::FORMAT_KEY],
        )
        .unwrap();
    }

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    assert_eq!(rows(s.db()), 0, "the ledger came back empty");
    assert_eq!(
        semlith::store::format(s.db()).unwrap(),
        older,
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

    let fleet = semlith::fleet::Fleet::open(&[store.path().to_path_buf()]).unwrap();
    // 0.21.0 moved the model load off the handshake, so the server takes the
    // fleet behind a lock and the store names `tools/list` prints separately.
    let labels = fleet.labels().join(", ");
    let fleet = std::sync::Mutex::new(fleet);
    let mut out = Vec::new();
    semlith::mcp::serve(&fleet, &labels, script.as_bytes(), &mut out).unwrap();
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

/// A raw read the steering hook saw becomes an uncredited row: it lowers
/// coverage, it is excluded from the saving, and the chain still verifies.
///
/// This is what turns Refunds from an estimate into a measurement. Without it
/// the ledger counts only the questions semlith was asked, which flatters every
/// ratio on the page by leaving out the ones it was not.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_raw_read_is_recorded_uncredited_and_does_not_break_the_chain() {
    let (corpus, store) = corpus();
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    drop(s);

    let fleet = semlith::fleet::Fleet::open(&[store.path().to_path_buf()]).unwrap();
    let held = corpus.path().join("bread.md").display().to_string();
    assert!(
        semlith::ledger::raw_read(&fleet, "claude-code", "s1", &held),
        "a read of an indexed file must be recorded"
    );
    // A file no store holds is recorded nowhere: Refunds counts reads semlith
    // could have answered, and nothing else.
    assert!(
        !semlith::ledger::raw_read(&fleet, "claude-code", "s1", "/nowhere/x.rs"),
        "a file no store holds must not be recorded against a store that does"
    );
    drop(fleet);

    let s = Semlith::open(store.path(), None).unwrap();
    let rows = semlith::store::retrievals(s.db(), 10).unwrap();
    let raw: Vec<_> = rows.iter().filter(|r| r.query == held).collect();
    assert_eq!(raw.len(), 1, "one raw read is one row: {rows:?}");
    assert_eq!(raw[0].hits, 0, "a raw read retrieved nothing from semlith");
    assert!(
        raw[0].whole_file_tokens > 0,
        "the row must say what reading it whole cost: {:?}",
        raw[0]
    );

    let savings = semlith::store::ledger_savings(s.db()).unwrap();
    assert_eq!(
        savings.credited, 0,
        "a raw read must never be credited with a saving"
    );
    assert_eq!(
        savings.total, 1,
        "it is still a retrieval, and a denominator"
    );
    assert_eq!(savings.coverage(), 0, "nothing was covered");
    assert!(
        semlith::store::ledger_break(s.db()).unwrap().is_none(),
        "the new row kind broke the hash chain"
    );
}

/// The block `semlith ledger` and `semlith ledger --verify` print: the number,
/// and every denominator that makes it defensible.
///
/// Both surfaces read one computation, so what this asserts is that the text a
/// person sees carries the figures that computation produced — never the saving
/// on its own.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_ledger_states_its_saving_with_coverage_refunds_and_tier() {
    let (corpus, store) = corpus();
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
        .unwrap();
    drop(s);

    let fleet = semlith::fleet::Fleet::open(&[store.path().to_path_buf()]).unwrap();
    let held = corpus.path().join("bread.md").display().to_string();
    assert!(semlith::ledger::raw_read(
        &fleet,
        "claude-code",
        "s1",
        &held
    ));
    drop(fleet);

    let s = Semlith::open(store.path(), None).unwrap();
    let savings = semlith::store::ledger_savings(s.db()).unwrap();
    let misses = semlith::store::ledger_misses(s.db()).unwrap();
    drop(s);

    for args in [vec!["ledger"], vec!["ledger", "--verify"]] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(&args)
            .arg("-s")
            .arg(store.path())
            .output()
            .expect("running semlith ledger");
        let said = String::from_utf8_lossy(&out.stdout).to_string();

        for wanted in [
            format!("coverage {} %", savings.coverage()),
            format!("tier {}", savings.tier()),
            format!("refunds {}", misses.refunds),
            format!("zero-hit {}", misses.zero_hit),
        ] {
            assert!(
                said.contains(&wanted),
                "`semlith {}` does not state {wanted:?}:\n{said}",
                args.join(" ")
            );
        }
        // The client breakdown names each client rather than counting them.
        assert!(
            said.contains("clients: claude-code"),
            "`semlith {}` does not name the clients:\n{said}",
            args.join(" ")
        );
        // The helper says what the denominator is, in the words the contract
        // fixes, so the number is never a bare multiplier.
        assert!(
            said.contains("counted with the store's tokenizer"),
            "the token helper is missing:\n{said}"
        );
    }
}

/// A session is one row, whatever it asked, and its tier is the honest one.
///
/// Two clients, two sessions, and a session counted by the four-character
/// fallback is `modelled` rather than averaged in with a measured one.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_ledger_groups_retrievals_into_sessions() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
    }
    let s = Semlith::open(store.path(), None).unwrap();
    let write = |client: &str, session: &str, query: &str, hits: i64, excerpt, whole| {
        let mut r = row(client, query, excerpt, whole);
        r.session = session;
        r.hits = hits;
        semlith::store::record_retrieval(s.db(), &r).unwrap();
    };
    write("claude-code", "s1", "how does the lock work", 3, 100, 4_000);
    write("claude-code", "s1", "who calls acquire", 2, 150, 2_000);
    // A read that found nothing is recorded and credited nothing.
    write("claude-code", "s1", "nothing at all", 0, 40, 900);
    write("cursor", "s2", "where is the parser", 1, 90, 1_200);

    let sessions = semlith::store::ledger_sessions(s.db(), 50).unwrap();
    assert_eq!(sessions.len(), 2, "two sessions: {sessions:?}");

    let one = sessions
        .iter()
        .find(|row| row.session == "s1")
        .expect("the claude-code session");
    assert_eq!(one.client, "claude-code");
    assert_eq!(one.retrievals, 3);
    assert_eq!(one.zero_hit, 1, "a zero-hit read is recorded, not dropped");
    // Net is whole less excerpt over the reads that found something: the
    // zero-hit row contributes nothing rather than a negative credit.
    assert_eq!(one.net, (4_000 - 100) + (2_000 - 150));
    assert_eq!(
        one.tier(),
        "modelled",
        "rows counted by the four-character fallback are modelled"
    );

    let two = sessions
        .iter()
        .find(|row| row.session == "s2")
        .expect("the cursor session");
    assert_eq!(two.retrievals, 1);
    assert_eq!(two.net, 1_200 - 90);
}

/// The sessions table's controls exist on the page, and its export writes
/// the columns the page shows.
#[test]
fn the_ledger_page_can_filter_sort_page_and_export_its_sessions() {
    const APP_JS: &str = include_str!("../src/portal/app.js");
    for wanted in [
        "function ledgerSessions(",
        "Filter by client",
        "Filter by tier",
        "cost at ",
        "w-sessions",
        "semlith-sessions",
    ] {
        assert!(
            APP_JS.contains(wanted),
            "the sessions table is missing {wanted:?}"
        );
    }
    // Sort and pagination come from `dataTable`, which every other table on
    // the portal uses — a second implementation for this one table would be
    // a second set of bugs.
    let block = APP_JS
        .split("function ledgerSessions(")
        .nth(1)
        .expect("ledgerSessions exists");
    assert!(
        block.contains("dataTable({"),
        "the table is not a dataTable"
    );
    // Pages at `dataTable`'s default, which every table opens at from 0.29.0.
    assert!(
        APP_JS.contains("perPage: spec.perPage || 5,"),
        "dataTable does not page at 5 by default"
    );
    assert!(block.contains("sort:"), "the table does not sort");
    // Export writes what is on screen: the same filter, the same rows.
    assert!(
        block.contains("exportRows(") && block.contains("shown()"),
        "export must write the filtered rows, not every row"
    );
    for format in ["Markdown", "CSV", "JSON"] {
        assert!(block.contains(format), "no {format} export");
    }
}

/// Session replay reads nothing until it is turned on, and says so.
#[test]
fn session_replay_is_off_until_the_privacy_page_turns_it_on() {
    // The setting's absence is off, not a default that happens to be false
    // somewhere else: what it reads belongs to another program.
    let fresh = semlith::home::Settings::default();
    assert_eq!(fresh.session_replay, None);
    assert!(!fresh.session_replay.unwrap_or(false));

    // And the reader, pointed at a directory with no transcripts, answers
    // with an empty reading rather than an error.
    let empty = tempfile::tempdir().unwrap();
    let found = semlith::replay::read(empty.path(), 10).unwrap();
    assert!(found.sessions.is_empty());
    assert_eq!(found.client, "claude-code");
}

/// A transcript with a semlith call in it is read, and what followed decides.
#[test]
fn a_transcript_marks_each_answer_with_what_the_agent_did_next() {
    let home = tempfile::tempdir().unwrap();
    let project = home.path().join("-Users-someone-work");
    std::fs::create_dir_all(&project).unwrap();
    let line = |name: &str| {
        format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"tool_use","name":"{name}"}}]}}}}"#
        )
    };
    let transcript = [
        line("mcp__semlith__semlith_search"),
        line("Read"),
        line("mcp__semlith__semlith_impact"),
        line("Edit"),
        line("mcp__semlith__semlith_search"),
        line("Grep"),
        // Not a tool_use at all, and not a panic either.
        r#"{"type":"user","message":{"content":"plain text"}}"#.to_string(),
    ]
    .join("\n");
    std::fs::write(project.join("abc123.jsonl"), transcript).unwrap();

    let found = semlith::replay::read(home.path(), 10).unwrap();
    assert_eq!(found.sessions.len(), 1, "{found:?}");
    let session = &found.sessions[0];
    assert_eq!(session.id, "abc123");
    assert_eq!(session.answers, 3);
    assert_eq!(session.refund, 1, "a whole-file read after an answer");
    assert_eq!(session.sufficed, 1, "an edit after an answer");
    assert_eq!(session.miss, 1, "a grep after an answer");
    assert_eq!(session.unknown, 0);
}
