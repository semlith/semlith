//! What 0.34.0 promises about the ledger's usage columns: with the setting
//! off nothing is read and nothing is written; with it on, a row an AI client
//! wrote gets the model, tokens and cost of the request that made the call,
//! read from that client's own log, and a client that keeps no log says so.
//!
//! One test, because it points HOME at a scratch directory and the setting is
//! read from there; a second test in this binary would race it.

use semlith::store::{self, NewRetrieval};
use std::path::Path;

const FIXTURE_AT: i64 = 1_790_762_400; // 2026-09-30T10:00:00Z, the fixtures' clock

fn row<'a>(client: &'a str, query: &'a str, query_id: &'a str) -> NewRetrieval<'a> {
    NewRetrieval {
        client,
        client_version: "",
        session: "s",
        tool: "search",
        query,
        hits: 2,
        micros: 1_000,
        excerpt_tokens: 10,
        whole_file_tokens: 100,
        stale_hits: 0,
        tokenizer: "estimate",
        query_id,
    }
}

fn usage_of(
    db: &rusqlite::Connection,
    query_id: &str,
) -> (
    Option<String>,
    Option<i64>,
    Option<f64>,
    Option<String>,
    Option<String>,
) {
    db.query_row(
        "SELECT model, cache_read_tokens, cost_usd, cost_source, usage_source FROM retrievals WHERE query_id = ?1",
        [query_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
    )
    .unwrap()
}

#[test]
fn usage_is_read_from_the_clients_own_log_only_when_turned_on() {
    let home = tempfile::tempdir().unwrap();
    // SAFETY: the only test in this binary, and set before anything reads it.
    unsafe { std::env::set_var("HOME", home.path()) };
    unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };
    let transcript = home.path().join(".claude/projects/-tmp-proj/s.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/usage/claude-code.jsonl"),
        &transcript,
    )
    .unwrap();

    let dir = home.path().join("store");
    std::fs::create_dir_all(&dir).unwrap();
    let db = store::open(&dir.join("store.db")).unwrap();
    store::record_retrieval(&db, &row("Claude Code", "parser", "q1")).unwrap();
    store::record_retrieval(&db, &row("Zed", "parser", "q2")).unwrap();
    store::record_retrieval(&db, &row("portal", "parser", "q3")).unwrap();
    // Rows at the fixtures' time, as if written when the client called.
    {
        let _writing = store::Writing::begin(&db).unwrap();
        db.execute("UPDATE retrievals SET at = ?1", [FIXTURE_AT + 2])
            .unwrap();
    }
    let now = FIXTURE_AT + 3_600;
    // Moving `at` above is itself an edit the chain sees; what matters is that
    // filling usage changes nothing the chain covers.
    let hashes = |db: &rusqlite::Connection| -> Vec<String> {
        store::retrievals(db, 10)
            .unwrap()
            .into_iter()
            .map(|r| r.hash)
            .collect()
    };
    let chain_before = (store::ledger_break(&db).unwrap(), hashes(&db));

    // Off by default: nothing read, nothing written.
    assert!(!semlith::usage::enabled());
    assert_eq!(semlith::usage::enrich(&db, now).unwrap(), 0);
    assert_eq!(usage_of(&db, "q1"), (None, None, None, None, None));

    let mut settings = semlith::home::Settings::load();
    settings.ledger_usage = Some(true);
    settings.save().unwrap();

    // On: the Claude Code row takes its request from the transcript, the
    // Zed row says why it has none, and the portal row is not an AI client.
    assert_eq!(semlith::usage::enrich(&db, now).unwrap(), 2);
    let (model, cache_read, cost, cost_source, source) = usage_of(&db, "q1");
    assert_eq!(model.as_deref(), Some("claude-opus-5"));
    assert_eq!(cache_read, Some(51_000));
    assert!(cost.unwrap() > 0.0, "an Opus call priced at nothing");
    assert!(cost_source.unwrap().starts_with("models.dev "));
    assert!(source.unwrap().ends_with("s.jsonl#req_fixture0001"));

    let (model, _, cost, _, source) = usage_of(&db, "q2");
    assert_eq!((model, cost), (None, None));
    assert_eq!(
        source.as_deref(),
        Some("Zed keeps a per-thread total, not the request behind each call")
    );
    assert_eq!(usage_of(&db, "q3").4, None);

    // Filled rows are not looked for again, and the chain reads as it did:
    // the usage columns are outside it.
    assert_eq!(semlith::usage::enrich(&db, now).unwrap(), 0);
    assert_eq!(
        (store::ledger_break(&db).unwrap(), hashes(&db)),
        chain_before
    );
    let rows = store::retrievals(&db, 10).unwrap();
    let filled = rows.iter().find(|r| r.query_id == "q1").unwrap();
    assert_eq!(
        filled.usage.as_ref().unwrap().model.as_deref(),
        Some("claude-opus-5")
    );
}
