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

/// The fixture corpus: every language's fixture file, copied into one
/// directory so a single index pass covers all of them.
///
/// The files live in `tests/fixtures/graph/<language>/`, and the per-language
/// assertions about what each one yields are unit tests in `src/graph.rs`,
/// where they need no embedding model. What this corpus proves is the wiring:
/// that indexing a repository actually writes those rows.
fn polyglot(dir: &Path) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph");
    for language in fs::read_dir(&root).expect("the fixture corpus is missing") {
        let language = language.unwrap().path();
        if !language.is_dir() {
            continue;
        }
        for fixture in fs::read_dir(&language).unwrap() {
            let fixture = fixture.unwrap().path();
            let name = fixture.file_name().unwrap().to_string_lossy().to_string();
            write(dir, &name, &fs::read_to_string(&fixture).unwrap());
        }
    }
    // Prose in no language at all, which carries no edges and must not be an
    // error. Markdown is a language with a graph now, so the file that used to
    // prove this had stopped proving it.
    write(dir, "notes.txt", "The lock is acquired before the write.");
}

/// Indexing a polyglot repository fills the graph for every language the
/// filter advertises, and says nothing about the file that is in none of them.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn indexing_fills_the_graph_for_every_advertised_language() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    polyglot(corpus.path());
    index(store.path(), corpus.path());

    let s = Semlith::open(store.path(), None).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/graph");
    let mut checked = 0;
    for language in fs::read_dir(&root).unwrap() {
        let language = language.unwrap().path();
        if !language.is_dir() {
            continue;
        }
        for fixture in fs::read_dir(&language).unwrap() {
            let file = fixture.unwrap().file_name().to_string_lossy().to_string();
            assert!(
                symbols_in(s.db(), &file) > 0,
                "{file} produced no symbols at all"
            );
            checked += 1;
        }
    }
    assert_eq!(
        checked,
        semlith::filter::LANGUAGES.len(),
        "every advertised language contributes a fixture to the corpus"
    );

    assert!(
        calls_from(s.db(), "acquire").contains(&"helper".to_string()),
        "acquire -> helper was not extracted; calls found: {:?}",
        calls_from(s.db(), "acquire")
    );

    assert_eq!(
        symbols_in(s.db(), "notes.txt"),
        0,
        "a file in no language at all must contribute no symbols, not an error"
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

    // Both endpoints on every hop, from 0.15.0: a hop between two bare names
    // says almost nothing when either name could mean several things.
    let out = cli(store.path(), &["path", "acquire", "release"]);
    assert!(out.contains("acquire @"), "{out}");
    assert!(out.contains("hold @"), "{out}");
    assert!(out.contains("release @"), "{out}");
    assert!(
        out.contains("2 hops"),
        "the trailer counts the chain: {out}"
    );
    assert!(
        !out.contains("hypothesis"),
        "every hop here resolves, so there is nothing to qualify: {out}"
    );

    let out = cli(store.path(), &["path", "acquire", "alone"]);
    assert!(
        out.contains("not connected"),
        "unconnected must say so: {out}"
    );

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
    // The dependency kinds, as `path` and the sweep mean them. Passing no
    // kinds asks for every edge, which includes the structural `defines` from
    // the file the symbol lives in — true, and not what "who calls this" is
    // asking.
    let kinds: Vec<String> = semlith::graph::DEPENDENCY_KINDS
        .iter()
        .map(|k| k.to_string())
        .collect();
    let around = semlith::graph::neighbours(s.db(), "release", &kinds, false).unwrap();
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
/// Reverse reachability came back in 0.26.0, free, with a page and a tool.
///
/// It was removed in 0.13.0 to be sold and never was: the monetization hold
/// of 2026-09-15 made the whole binary free, so the thing this test used to
/// assert — that the command is gone — is now the defect.
#[test]
fn the_impact_command_answers() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["impact", "--help"])
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "`semlith impact` must be a command: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let help = String::from_utf8_lossy(&out.stdout);
    for flag in ["--depth", "--all-edges", "--strict", "--json"] {
        assert!(help.contains(flag), "`semlith impact` is missing {flag}");
    }
}

/// Who reaches `finish`, and how far away each of them is.
///
/// `start` calls `middle`, `middle` calls `finish`, and `aside` calls
/// `finish` directly. Breadth first, so `aside` is one hop even though it is
/// written last, and `start` is two.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn impact_lists_every_caller_by_the_fewest_hops_it_takes() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    // File stems that do not repeat a function name: a file called
    // `middle.rs` is itself a `middle` module symbol, which makes every call
    // to `middle` ambiguous and is a fact about the fixture rather than
    // about the traversal.
    write(corpus.path(), "a.rs", "fn start() { middle(); }\n");
    write(corpus.path(), "b.rs", "fn middle() { finish(); }\n");
    write(corpus.path(), "c.rs", "fn aside() { finish(); }\n");
    write(corpus.path(), "d.rs", "fn finish() {}\n");
    index(store.path(), corpus.path());

    let out = cli(store.path(), &["impact", "finish"]);
    assert!(
        out.contains("middle"),
        "the direct caller is missing: {out}"
    );
    assert!(
        out.contains("aside"),
        "the other direct caller is missing: {out}"
    );
    assert!(
        out.contains("start"),
        "the two-hop caller is missing: {out}"
    );
    assert!(out.contains("1 hop"), "hops are not grouped: {out}");
    assert!(out.contains("2 hops"), "hops are not grouped: {out}");
    assert!(out.contains("files"), "the files block is missing: {out}");

    // A hop limit is a limit. At one hop `start` is out of reach, and saying
    // otherwise would make the control decoration.
    let one = cli(store.path(), &["impact", "finish", "--depth", "1"]);
    assert!(one.contains("middle"), "{one}");
    assert!(one.contains("aside"), "{one}");
    assert!(!one.contains("start"), "--depth 1 reached two hops: {one}");

    // Nothing reaches a leaf, and that is a sentence rather than an error.
    let none = cli(store.path(), &["impact", "start"]);
    assert!(
        none.contains("nothing in this store reaches start"),
        "{none}"
    );
}

/// Every row carries the support class of the edge that reached it. A hop
/// with no class is a hop a reader will assume was verified.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn every_impact_row_states_what_its_edge_is_worth() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "a.rs", "fn start() { middle(); }\n");
    write(corpus.path(), "b.rs", "fn middle() { finish(); }\n");
    write(corpus.path(), "c.rs", "fn finish() {}\n");
    index(store.path(), corpus.path());

    let raw = cli(store.path(), &["impact", "finish", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("--json is json");
    let reached = parsed["reached"].as_array().expect("reached is a list");
    assert!(!reached.is_empty(), "{raw}");
    for row in reached {
        let confidence = row["confidence"].as_str().unwrap_or("");
        assert!(
            ["extracted", "resolved", "inferred", "ambiguous"].contains(&confidence),
            "a row reached with no support class: {row}"
        );
        assert!(row["hop"].as_u64().unwrap_or(0) >= 1, "{row}");
        assert!(!row["via"].as_str().unwrap_or("").is_empty(), "{row}");
    }
}

/// The shape the release exists for, end to end through the binary: `start`
/// calls a `record`, there are two unrelated `record`s, and only one of them
/// reaches `finish`.
///
/// The 0.14.0 finder printed `start -> record -> finish` here, in the format a
/// real chain prints in, with nothing on the page to tell a reader it had
/// changed subject halfway through.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_path_through_a_name_with_two_definitions_is_refused_and_then_labelled() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "start.rs", "fn start() { record(); }\n");
    write(
        corpus.path(),
        "one.rs",
        "fn record() { nothing(); }\nfn nothing() {}\n",
    );
    write(
        corpus.path(),
        "two.rs",
        "fn record() { finish(); }\nfn finish() {}\n",
    );
    index(store.path(), corpus.path());

    let refused = cli(store.path(), &["path", "start", "finish"]);
    assert!(
        refused.contains("not connected"),
        "the only route crosses a name with two definitions: {refused}"
    );
    assert!(refused.contains("--all-edges"), "{refused}");

    let walked = cli(store.path(), &["path", "start", "finish", "--all-edges"]);
    assert!(walked.contains("seam"), "{walked}");
    assert!(walked.contains("record: 2 definitions"), "{walked}");
    assert!(walked.contains("A hypothesis, not a finding."), "{walked}");
    assert!(
        walked.contains("start.rs:1"),
        "every hop shows both endpoints: {walked}"
    );
}

/// A name with several definitions is one row saying so, not one row per
/// definition. Four rows saying `record` read as four calls, and the symbol
/// makes one.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn ambiguous_callees_collapse_to_one_row_and_expand_with_all() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "start.rs", "fn start() { record(); }\n");
    for (i, name) in ["one", "two", "three"].iter().enumerate() {
        write(
            corpus.path(),
            &format!("{name}.rs"),
            &format!("fn record() {{ let _ = {i}; }}\n"),
        );
    }
    index(store.path(), corpus.path());

    let collapsed = cli(store.path(), &["neighbors", "start"]);
    assert_eq!(
        collapsed.matches("record").count(),
        1,
        "one row stands for all three definitions: {collapsed}"
    );
    assert!(collapsed.contains("3 definitions"), "{collapsed}");

    let expanded = cli(store.path(), &["neighbors", "start", "--all"]);
    assert_eq!(
        expanded.matches("record").count(),
        3,
        "--all shows every definition behind the collapsed row: {expanded}"
    );
}

/// A call into something the store does not hold is left out of the answer and
/// counted, rather than left out silently. "semlith shows no callees" and
/// "everything this calls lives outside the index" are different facts.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn targets_outside_the_store_are_counted_and_shown_only_with_all() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "start.rs",
        "fn start() { somewhere_else(); }\n",
    );
    index(store.path(), corpus.path());

    let narrow = cli(store.path(), &["neighbors", "start"]);
    assert!(
        !narrow.contains("somewhere_else"),
        "not listed by default: {narrow}"
    );
    assert!(
        narrow.contains("outside this store"),
        "but the reader is told the list is short: {narrow}"
    );

    let wide = cli(store.path(), &["neighbors", "start", "--all"]);
    assert!(wide.contains("somewhere_else"), "{wide}");
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
        serde_json::to_value(semlith::graph::shortest_path(s.db(), "a", "b", 6, false).unwrap())
            .unwrap();
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

/// A re-export is the only thing between the name a caller wrote and the
/// definition it meant, so a walk that will not cross one answers "not
/// connected" about code that is connected. The chain here is only walkable
/// through `helper`, which is an alias and nothing else.
///
/// The same corpus proves the other half of 0.16.0's edge work: `neighbors`
/// names the line the call was written on, which is not the line the calling
/// function starts on.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_path_crosses_a_re_export_and_neighbours_names_the_call_site() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "alias.rs",
        "pub use real::worker as helper;\n",
    );
    write(
        corpus.path(),
        "real.rs",
        "fn worker() { finish(); }\nfn finish() {}\n",
    );
    // The call sits on line 4, four lines below the `fn` that contains it.
    write(
        corpus.path(),
        "start.rs",
        "fn start() {\n    let x = 1;\n    let _ = x;\n    helper();\n}\n",
    );
    index(store.path(), corpus.path());

    let walked = cli(store.path(), &["path", "start", "finish"]);
    assert!(
        walked.contains("aliases"),
        "the hop through the re-export must say what kind of hop it was: {walked}"
    );
    assert!(
        walked.contains("helper"),
        "the alias is a node on the chain: {walked}"
    );
    assert!(
        !walked.contains("not connected"),
        "the chain exists once aliases are crossable: {walked}"
    );

    let out = cli(store.path(), &["neighbors", "start"]);
    assert!(
        out.contains("called at") && out.contains(":4"),
        "the call site is line 4, not the line `start` begins on: {out}"
    );
}

/// The second stage of a retrieval: a locate answer says where, and this
/// returns exactly that and nothing around it. A name with several definitions
/// returns the list rather than guessing which one was meant.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn read_returns_one_span_and_refuses_to_guess_between_definitions() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "one.rs",
        "fn alpha() {\n    let marker = 1;\n}\nfn shared() {}\n",
    );
    write(corpus.path(), "two.rs", "fn shared() {}\n");
    index(store.path(), corpus.path());

    let by_name = cli(store.path(), &["read", "alpha"]);
    assert!(by_name.contains("let marker = 1;"), "{by_name}");
    assert!(
        !by_name.contains("fn shared"),
        "a read is the span and nothing around it: {by_name}"
    );

    let by_span = cli(store.path(), &["read", "one.rs:2-2"]);
    assert!(by_span.contains("let marker = 1;"), "{by_span}");
    assert!(
        !by_span.contains("fn alpha"),
        "line 2 is not line 1: {by_span}"
    );

    let ambiguous = cli(store.path(), &["read", "shared"]);
    assert!(
        ambiguous.contains("2 definitions"),
        "two definitions and nothing to choose between them: {ambiguous}"
    );

    let missing = cli(store.path(), &["read", "nosuchsymbol"]);
    assert!(missing.contains("nothing indexed"), "{missing}");
}

/// A structural question a regex cannot ask: every call whose function is a
/// bare identifier. The grammar is the one the extractor already uses, read
/// from the same table, so a language `pattern` accepts is one the graph
/// accepts.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_pattern_finds_the_shape_and_says_which_files_it_parsed() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "one.rs",
        "fn caller() {\n    helper();\n}\nfn helper() {}\n",
    );
    write(corpus.path(), "notes.md", "helper() is called here too\n");
    index(store.path(), corpus.path());

    let out = cli(
        store.path(),
        &[
            "pattern",
            "--lang",
            "rust",
            "(call_expression function: (identifier) @called)",
        ],
    );
    assert!(out.contains("@called"), "{out}");
    assert!(out.contains("helper"), "{out}");
    assert!(out.contains("one.rs:2"), "the call is on line 2: {out}");
    // The Markdown file mentions the same text and is not Rust, so it is not
    // parsed and cannot match.
    assert!(!out.contains("notes.md"), "{out}");

    // A pattern that does not compile is the caller's mistake, and saying "no
    // matches" would have them conclude the code lacks the shape.
    let broken = cli(store.path(), &["pattern", "--lang", "rust", "(unbalanced"]);
    assert!(
        broken.contains("not a valid tree-sitter pattern"),
        "{broken}"
    );

    let nolang = cli(store.path(), &["pattern", "--lang", "cobol", "(x) @y"]);
    assert!(nolang.contains("no grammar for"), "{nolang}");
}

/// Trace is the chain plus its evidence, and the two marks are not
/// interchangeable: a hop the source settled is a fact, a hop matched by bare
/// name is a candidate somebody has to check.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn trace_states_the_answer_the_chain_and_one_line_per_hop() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "a.rs", "fn start() { middle(); }\n");
    write(corpus.path(), "b.rs", "fn middle() { finish(); }\n");
    write(corpus.path(), "c.rs", "fn finish() {}\n");
    index(store.path(), corpus.path());

    let out = cli(store.path(), &["trace", "start", "finish"]);
    assert!(out.contains("answer"), "{out}");
    assert!(out.contains("chain"), "{out}");
    assert!(out.contains("supporting lines"), "{out}");
    assert!(
        out.contains("supporting fact") || out.contains("candidate"),
        "every hop carries a mark: {out}"
    );

    let raw = cli(store.path(), &["trace", "start", "finish", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("--json is json");
    let lines = parsed["lines"].as_array().expect("lines is a list");
    let hops = parsed["chain"]["steps"].as_array().expect("steps").len();
    assert_eq!(lines.len(), hops, "one supporting line per hop");
    for line in lines {
        let mark = line["mark"].as_str().unwrap_or("");
        assert!(
            mark == "supporting fact" || mark == "candidate — corroborate before use",
            "unexpected mark {mark:?}"
        );
    }

    // The block the portal's "Copy as evidence" copies is the same answer in
    // plain text, so a reviewer pasting it and a reader reading the page are
    // looking at one thing.
    let evidence = cli(store.path(), &["trace", "start", "finish", "--evidence"]);
    assert!(evidence.contains("start → finish"), "{evidence}");
    assert!(evidence.starts_with("start → finish"), "{evidence}");
    assert!(evidence.contains("answer:"), "{evidence}");
    assert!(evidence.contains("supporting lines"), "{evidence}");

    // Two symbols with nothing between them are a sentence, not an error.
    let none = cli(store.path(), &["trace", "finish", "start"]);
    assert!(none.contains("not connected"), "{none}");
}

/// Graph health on Inside the index is the same reading `semlith stats`
/// prints, from the same rows, and the page adds nothing of its own.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn graph_health_says_what_stats_says() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "a.rs",
        "fn start() { middle(); outside(); }\n",
    );
    write(corpus.path(), "b.rs", "fn middle() { finish(); }\n");
    write(corpus.path(), "c.rs", "fn finish() {}\nfn twice() {}\n");
    write(corpus.path(), "d.rs", "fn twice() {}\n");
    index(store.path(), corpus.path());

    let stats = cli(store.path(), &["stats"]);
    assert!(
        stats.contains("call targets with no definition here"),
        "stats does not report unresolved targets: {stats}"
    );
    assert!(
        stats.contains("names with several definitions"),
        "stats does not report ambiguous names: {stats}"
    );

    // The same two figures, read the way the route reads them. Equal because
    // they are one query, not two implementations that happen to agree.
    let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
    let (top, distinct) = semlith::store::unresolved_targets(&db, 5).unwrap();
    let several = semlith::store::names_with_several_definitions(&db).unwrap();
    assert!(
        stats.contains(&format!("{distinct} call targets with no definition here")),
        "stats prints a different unresolved count than the store reports ({distinct}): {stats}"
    );
    assert!(
        stats.contains(&format!("{several} names with several definitions")),
        "stats prints a different ambiguous-name count than the store reports ({several}): {stats}"
    );
    assert!(several >= 1, "`twice` is defined in two files: {several}");
    assert!(
        top.iter().any(|(name, _)| name == "outside"),
        "a call into nothing this store defines is an unresolved target: {top:?}"
    );

    // Chunks are dated by when the store read the file, and every chunk is
    // in exactly one month's bucket.
    let months = semlith::store::chunks_by_month(&db).unwrap();
    let counted: i64 = months.iter().map(|(_, n)| n).sum();
    let chunks: i64 = db
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(counted, chunks, "a chunk fell out of the month buckets");
}
