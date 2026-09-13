//! What the code graph promises: that it is extracted from the same pass that
//! re-embeds a file, and so is never a build artifact that has gone stale.
//!
//! These drive a real store, so they download an embedding model on first run:
//!
//! ```sh
//! cargo test --test graph -- --ignored
//! ```
//!
//! The extractor's own behaviour — which languages, which edge kinds, how
//! confidence is decided — is unit-tested in `src/graph.rs` where it needs no
//! model. What is proven here is the wiring: that indexing writes the rows,
//! that changing a file rewrites its rows and nobody else's, and that the
//! watcher's path inherits all of it.

use rusqlite::Connection;
use semlith::Semlith;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

const APPEAR_TIMEOUT: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_millis(200);

/// Every language that carries edges, in one corpus, each calling a helper it
/// defines itself so there is an edge to look for.
fn polyglot(dir: &Path) {
    write(
        dir,
        "lock.rs",
        "use std::fs::File;\n\
         fn helper() {}\n\
         fn acquire() { helper(); }\n",
    );
    write(
        dir,
        "app.ts",
        "import {x} from './m';\n\
         function helper(){}\n\
         function acquire(){ helper(); }\n",
    );
    write(
        dir,
        "run.py",
        "import os\n\
         def helper():\n    pass\n\
         def acquire():\n    helper()\n",
    );
    write(
        dir,
        "serve.go",
        "package m\nimport \"fmt\"\n\
         func helper() {}\n\
         func acquire() { helper() }\n",
    );
    write(
        dir,
        "Main.java",
        "import java.util.List;\n\
         class Main { void helper(){} void acquire(){ helper(); } }\n",
    );
    write(
        dir,
        "main.c",
        "#include <stdio.h>\n\
         int helper(){ return 1; }\n\
         int acquire(){ return helper(); }\n",
    );
    // Prose, which carries no edges and must not be an error.
    write(dir, "notes.md", "The lock is acquired before the write.");
}

/// Indexing a polyglot repository fills the graph for the six languages that
/// carry edges and says nothing about the file that does not.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn indexing_fills_the_graph_for_every_advertised_language() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    polyglot(corpus.path());
    index(store.path(), corpus.path());

    let s = Semlith::open(store.path(), None).unwrap();
    for file in [
        "lock.rs",
        "app.ts",
        "run.py",
        "serve.go",
        "Main.java",
        "main.c",
    ] {
        assert!(
            symbols_in(s.db(), file) > 0,
            "{file} produced no symbols at all"
        );
        assert!(
            calls_from(s.db(), "acquire").contains(&"helper".to_string()),
            "{file}: acquire -> helper was not extracted; calls found: {:?}",
            calls_from(s.db(), "acquire")
        );
    }

    assert_eq!(
        symbols_in(s.db(), "notes.md"),
        0,
        "a language with no grammar must contribute no symbols, not an error"
    );

    // Every edge is one of the two confidences, never null and never a third.
    let bad: i64 = s
        .db()
        .query_row(
            "SELECT COUNT(*) FROM edges WHERE confidence NOT IN ('extracted', 'inferred')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bad, 0, "an edge carries a confidence outside the two");
}

/// Re-indexing one changed file rewrites that file's rows and leaves every
/// other file's alone — and leaves nothing behind pointing at a dead file id.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn editing_a_file_re_extracts_that_file_and_only_that_file() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    polyglot(corpus.path());
    index(store.path(), corpus.path());

    let before: Vec<(String, i64)> = {
        let s = Semlith::open(store.path(), None).unwrap();
        per_file_counts(s.db())
    };

    // `acquire` stops calling `helper` and starts calling `release`.
    write(
        corpus.path(),
        "lock.rs",
        "use std::fs::File;\n\
         fn helper() {}\n\
         fn release() {}\n\
         fn acquire() { release(); }\n",
    );
    index(store.path(), corpus.path());

    let s = Semlith::open(store.path(), None).unwrap();
    let after = per_file_counts(s.db());

    for (path, count) in &before {
        if path.ends_with("lock.rs") {
            continue;
        }
        let now = after
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| *c)
            .unwrap_or(-1);
        assert_eq!(
            *count, now,
            "{path} was re-extracted by an edit to another file"
        );
    }

    let calls = calls_from_in(s.db(), "acquire", "lock.rs");
    assert!(
        calls.contains(&"release".to_string()),
        "the new call was not extracted: {calls:?}"
    );
    assert!(
        !calls.contains(&"helper".to_string()),
        "the old call survived the edit: {calls:?}"
    );

    let orphans: i64 = s
        .db()
        .query_row(
            "SELECT COUNT(*) FROM symbols s LEFT JOIN files f ON f.id = s.file_id
             WHERE f.id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        orphans, 0,
        "symbols survived the file they were extracted from"
    );
}

/// Forgetting a file takes its symbols and its outgoing edges with it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn forgetting_a_file_takes_its_symbols_and_edges() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    polyglot(corpus.path());
    index(store.path(), corpus.path());

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    assert!(symbols_in(s.db(), "lock.rs") > 0);
    let elsewhere = symbols_in(s.db(), "run.py");

    s.forget(&corpus.path().join("lock.rs")).unwrap();

    assert_eq!(
        symbols_in(s.db(), "lock.rs"),
        0,
        "symbols outlived the file"
    );
    assert_eq!(
        symbols_in(s.db(), "run.py"),
        elsewhere,
        "forgetting one file disturbed another"
    );
    let dangling: i64 = s
        .db()
        .query_row(
            "SELECT COUNT(*) FROM edges e LEFT JOIN symbols s ON s.id = e.src
             WHERE s.id IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dangling, 0, "edges outlived the symbols they leave");
}

/// The claim the whole release rests on: an edit under a running watcher
/// updates the graph in the same pass that updates the vectors. No separate
/// command, no build step, nothing that can be out of date.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_edit_under_the_watcher_updates_the_graph_with_the_vectors() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "lock.rs",
        "fn helper() {}\nfn acquire() { helper(); }\n",
    );
    index(store.path(), corpus.path());

    let stop = Arc::new(AtomicBool::new(false));
    let handle = {
        let (store_path, roots, flag) = (
            store.path().to_path_buf(),
            vec![corpus.path().to_path_buf()],
            Arc::clone(&stop),
        );
        thread::spawn(move || {
            let mut s = Semlith::open(&store_path, None).unwrap();
            s.quiet = true;
            semlith::watch::run(&mut s, &roots, DEBOUNCE, &flag, |_| {}).unwrap();
        })
    };

    // Nobody runs `index`. The file is simply saved, as an editor would.
    write(
        corpus.path(),
        "lock.rs",
        "fn helper() {}\nfn release() {}\nfn acquire() { release(); }\n",
    );

    let updated = wait_until(store.path(), |db| {
        calls_from_in(db, "acquire", "lock.rs").contains(&"release".to_string())
    });
    assert!(
        updated,
        "the watcher re-embedded the file but the graph still describes the old text"
    );

    // And the old edge is gone, not merely outnumbered.
    let s = Semlith::open(store.path(), None).unwrap();
    assert!(
        !calls_from_in(s.db(), "acquire", "lock.rs").contains(&"helper".to_string()),
        "the edge the edit removed is still in the graph"
    );
    drop(s);

    stop.store(true, Ordering::Relaxed);
    handle.join().unwrap();
}

/// The three graph commands answer, and `neighbors` agrees with a full-text
/// sweep of the corpus: every direct caller, and nothing a sweep does not find.
///
/// This asserted the same property through `impact` until 0.13.0 removed it
/// from the free product. The property is the one that matters — that the
/// graph's answer about who calls a symbol matches what is written in the
/// file — and `neighbours` is where it now lives.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_three_commands_answer_and_neighbours_agrees_with_a_sweep() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "lock.rs",
        "fn acquire() { hold(); }\n\
         fn hold() { release(); }\n\
         fn release() {}\n\
         fn alone() {}\n",
    );
    index(store.path(), corpus.path());

    let out = cli(store.path(), &["symbol", "hold"]);
    assert!(out.contains("lock.rs"), "symbol found nothing: {out}");

    let out = cli(store.path(), &["neighbors", "hold"]);
    assert!(out.contains("acquire"), "hold's caller is missing: {out}");
    assert!(out.contains("release"), "hold's callee is missing: {out}");

    let out = cli(store.path(), &["path", "acquire", "release"]);
    assert!(out.contains("acquire --calls--> hold"), "{out}");
    assert!(out.contains("hold --calls--> release"), "{out}");

    let out = cli(store.path(), &["path", "acquire", "alone"]);
    assert!(out.contains("no chain"), "unconnected must say so: {out}");

    // Depth 1 against the sweep: `release` is called once, by `hold`.
    let body = fs::read_to_string(corpus.path().join("lock.rs")).unwrap();
    let callers: Vec<&str> = ["acquire", "hold", "alone"]
        .into_iter()
        .filter(|f| {
            let start = body.find(&format!("fn {f}()")).unwrap();
            body[start..]
                .split_once('}')
                .is_some_and(|(b, _)| b.contains("release("))
        })
        .collect();
    assert_eq!(callers, ["hold"], "the sweep itself is wrong");

    let s = Semlith::open(store.path(), None).unwrap();
    let around = semlith::graph::neighbours(s.db(), "release", &[]).unwrap();
    let names: Vec<String> = around
        .callers
        .iter()
        .map(|e| e.symbol.name.clone())
        .collect();
    assert_eq!(
        names,
        ["hold"],
        "neighbours disagrees with a full-text sweep"
    );
    drop(s);
}

/// `semlith impact` is gone from the free product, and says so rather than
/// doing something else. Removed in 0.13.0; reverse reachability returns in
/// 0.14.0 as a paid surface.
#[test]
fn the_impact_command_no_longer_exists() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["impact", "anything"])
        .output()
        .expect("the binary runs");
    assert!(!out.status.success(), "`semlith impact` still succeeds");
    let said = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        said.contains("unrecognized subcommand") || said.contains("unrecognised subcommand"),
        "an unknown command must say so: {said}"
    );
}

/// Every graph answer is the same whether it came from the library or the
/// command line, so an agent and a person are never told different things.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_json_output_matches_what_the_library_returns() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "lock.rs", "fn a() { b(); }\nfn b() {}\n");
    index(store.path(), corpus.path());

    let printed: serde_json::Value =
        serde_json::from_str(&cli(store.path(), &["path", "a", "b", "--json"])).unwrap();
    let s = Semlith::open(store.path(), None).unwrap();
    let direct =
        serde_json::to_value(semlith::graph::shortest_path(s.db(), "a", "b", 6).unwrap()).unwrap();
    assert_eq!(printed, direct);
}

/// The third list earns its place: a chunk that shares no vocabulary with the
/// query is still returned, because a symbol in the top hit points at it — and
/// it says it arrived through the graph rather than passing as a match.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_chunk_reachable_only_through_the_graph_is_returned_and_badged() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    // The query's words are all in `mixing.rs`. `proving.rs` shares none of
    // them, and is reachable only because `knead` calls `autolyse`.
    write(
        corpus.path(),
        "mixing.rs",
        "/// Combine flour and water for the sourdough loaf.\n\
         fn knead() { autolyse(); }\n",
    );
    write(
        corpus.path(),
        "proving.rs",
        "fn autolyse() { let _ = 1; }\n",
    );
    index(store.path(), corpus.path());

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let hits = s
        .search("combine flour and water for the sourdough loaf", 8)
        .unwrap();

    let reached = hits
        .iter()
        .find(|h| h.path.ends_with("proving.rs"))
        .unwrap_or_else(|| {
            panic!(
                "the graph-reached chunk is missing; got {:?}",
                hits.iter().map(|h| (&h.path, &h.lists)).collect::<Vec<_>>()
            )
        });
    assert!(
        reached.lists.contains(&"graph"),
        "the chunk came back without saying the graph found it: {:?}",
        reached.lists
    );

    // Every hit says how it was found, and the top hit is a real match.
    assert!(hits.iter().all(|h| !h.lists.is_empty()));
    assert!(
        hits[0].path.ends_with("mixing.rs"),
        "expansion outranked the actual match"
    );
}

/// The filter gates all three lists, not two. A chunk outside it must not
/// arrive through the graph by the back door.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_filter_constrains_the_graph_list_too() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "mixing.rs",
        "/// Combine flour and water for the sourdough loaf.\n\
         fn knead() { autolyse(); }\n",
    );
    write(
        corpus.path(),
        "proving.rs",
        "fn autolyse() { let _ = 1; }\n",
    );
    index(store.path(), corpus.path());

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let filter = semlith::filter::Filter::new(&["**/mixing.rs".to_string()], &[], &[]).unwrap();
    let hits = s
        .search_filtered("combine flour and water for the sourdough loaf", 8, &filter)
        .unwrap();

    assert!(
        hits.iter().all(|h| h.path.ends_with("mixing.rs")),
        "graph expansion reached outside the filter: {:?}",
        hits.iter().map(|h| &h.path).collect::<Vec<_>>()
    );
}

// ----------------------------------------------------------------- helpers

/// Run the built binary against `store` and return its stdout and stderr.
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

fn symbols_in(db: &Connection, name: &str) -> i64 {
    db.query_row(
        "SELECT COUNT(*) FROM symbols s JOIN files f ON f.id = s.file_id
         WHERE f.path LIKE '%' || ?1",
        rusqlite::params![name],
        |r| r.get(0),
    )
    .unwrap()
}

fn per_file_counts(db: &Connection) -> Vec<(String, i64)> {
    let mut stmt = db
        .prepare(
            "SELECT f.path, COUNT(s.id) FROM files f LEFT JOIN symbols s ON s.file_id = f.id
             GROUP BY f.id ORDER BY f.path",
        )
        .unwrap();
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap()
}

fn calls_from(db: &Connection, from: &str) -> Vec<String> {
    let mut stmt = db
        .prepare(
            "SELECT e.dst FROM edges e JOIN symbols s ON s.id = e.src
                  WHERE s.name = ?1 AND e.kind = 'calls'",
        )
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![from], |r| r.get::<_, String>(0))
        .unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap()
}

fn calls_from_in(db: &Connection, from: &str, file: &str) -> Vec<String> {
    let mut stmt = db
        .prepare(
            "SELECT e.dst FROM edges e
             JOIN symbols s ON s.id = e.src
             JOIN files f ON f.id = s.file_id
             WHERE s.name = ?1 AND e.kind = 'calls' AND f.path LIKE '%' || ?2",
        )
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![from, file], |r| r.get::<_, String>(0))
        .unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap()
}

/// Poll a fresh reader until the graph satisfies `done`, or time runs out.
fn wait_until(store: &Path, done: impl Fn(&Connection) -> bool) -> bool {
    let deadline = Instant::now() + APPEAR_TIMEOUT;
    while Instant::now() < deadline {
        let s = Semlith::open(store, None).unwrap();
        if done(s.db()) {
            return true;
        }
        drop(s);
        thread::sleep(Duration::from_millis(300));
    }
    false
}

fn index(store: &Path, corpus: &Path) {
    let mut s = Semlith::open(store, None).unwrap();
    s.quiet = true;
    s.index_paths(&[corpus.to_path_buf()], |_, _| {}).unwrap();
}

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}
