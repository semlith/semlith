//! What `semlith watch` promises: a file saved in an editor is searchable a
//! moment later, without anyone running `index`.
//!
//! These drive real filesystem events against a real store, so they are slow
//! and they download an embedding model on first run:
//!
//! ```sh
//! cargo test --test watch -- --ignored
//! ```

use semlith::Semlith;
use std::fs;
use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long a test waits for an edit to show up in search results. Generous:
/// the watcher debounces, then embeds, and a loaded CI machine is slow.
const APPEAR_TIMEOUT: Duration = Duration::from_secs(60);

/// Debounce used by the tests. Short enough to keep them quick, long enough to
/// still coalesce the burst a single `fs::write` produces.
const DEBOUNCE: Duration = Duration::from_millis(200);

#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_edit_is_searchable_without_running_index() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();

    write(
        corpus.path(),
        "bread.md",
        "Sourdough needs flour and water.",
    );
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    // The sentence exists nowhere in the store until this write lands.
    write(
        corpus.path(),
        "rust.md",
        "The borrow checker proves aliasing rules at compile time.",
    );

    let hit = wait_for(store.path(), "compile time aliasing proof", "rust.md");
    assert!(
        hit,
        "the edit never became searchable within {APPEAR_TIMEOUT:?}"
    );

    watcher.stop();
}

/// A store has one writer. A watcher is a writer that lives for hours, so the
/// refusal has to name it rather than read as "busy, try later".
#[test]
#[ignore = "downloads an embedding model on first run"]
fn indexing_a_watched_store_is_refused_and_names_the_watcher() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "bread.md",
        "Sourdough needs flour and water.",
    );
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    let mut second = Semlith::open(store.path(), None).unwrap();
    second.quiet = true;
    let err = second
        .index_paths(&[corpus.path().to_path_buf()], |_| {})
        .unwrap_err()
        .to_string();
    assert!(err.contains("being indexed by"), "unhelpful error: {err}");
    assert!(
        err.contains(&format!("pid {}", std::process::id())),
        "the error does not name the holder: {err}"
    );
    drop(second);

    watcher.stop();

    // The store survived the refusal: it still opens and still answers.
    let mut after = Semlith::open(store.path(), None).unwrap();
    after.quiet = true;
    assert!(!after.search("sourdough flour", 3).unwrap().is_empty());
}

/// The two halves of keeping a store current that are not "a file changed":
/// a file that did not exist before, and one that no longer exists.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_new_file_is_indexed_and_a_deleted_one_is_dropped() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "bread.md",
        "Sourdough needs flour and water.",
    );
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    write(corpus.path(), "cells.md", "Mitochondria power the cell.");
    assert!(
        wait_until(store.path(), |p| holds(p, "cells.md")),
        "a new file never reached the store"
    );

    fs::remove_file(corpus.path().join("bread.md")).unwrap();
    assert!(
        wait_until(store.path(), |p| !holds(p, "bread.md")),
        "a deleted file is still indexed"
    );

    watcher.stop();

    // Gone from search, not merely absent from the file list.
    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let hits = s.search("sourdough flour water", 5).unwrap();
    assert!(
        !hits.iter().any(|h| h.path.ends_with("bread.md")),
        "the deleted file still has vectors: {hits:#?}"
    );
}

/// A rename is a delete and a create that must both land, or the store keeps
/// answering with a path that no longer exists.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_rename_moves_the_file_in_the_store() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "before.md",
        "Ferments need salt, time and a warm shelf.",
    );
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    fs::rename(
        corpus.path().join("before.md"),
        corpus.path().join("after.md"),
    )
    .unwrap();

    assert!(
        wait_until(store.path(), |p| holds(p, "after.md")
            && !holds(p, "before.md")),
        "the rename did not move the file in the store"
    );

    watcher.stop();

    let mut s = Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let hits = s.search("salt time warm shelf ferment", 5).unwrap();
    assert!(
        hits.iter().any(|h| h.path.ends_with("after.md")),
        "the renamed file is not searchable under its new path: {hits:#?}"
    );
    assert!(!hits.iter().any(|h| h.path.ends_with("before.md")));
}

/// How Vim and most editors save: write a temp file, rename it over the
/// target. The remove event that arrives first must not evict the file the
/// user just saved.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_atomic_save_is_an_edit_not_a_deletion() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "notes.md",
        "Mitochondria are the powerhouse of the cell.",
    );
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    let tmp = corpus.path().join(".notes.md.swp");
    fs::write(
        &tmp,
        "The borrow checker proves aliasing rules at compile time.",
    )
    .unwrap();
    fs::rename(&tmp, corpus.path().join("notes.md")).unwrap();

    assert!(
        wait_for(store.path(), "compile time aliasing proof", "notes.md"),
        "the atomically saved contents never became searchable"
    );

    watcher.stop();

    let paths = {
        let s = Semlith::open(store.path(), None).unwrap();
        semlith::store::all_paths(s.db()).unwrap()
    };
    assert!(holds(&paths, "notes.md"), "the saved file was evicted");
    assert!(
        !holds(&paths, ".notes.md.swp"),
        "the editor's temp file was indexed: {paths:#?}"
    );
}

/// A save is many events, and an editor with format-on-save is many more. If
/// each one cost an embed, leaving the watcher running would cost more than
/// re-indexing by hand.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_burst_of_saves_costs_one_re_embed() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "notes.md", "First draft of the notes.");
    index(store.path(), corpus.path());

    // Long enough that ten writes cannot straddle two quiet periods.
    let debounce = Duration::from_secs(1);
    let watcher = spawn_with(store.path(), corpus.path(), debounce);

    for i in 0..10 {
        write(
            corpus.path(),
            "notes.md",
            &format!("Draft number {i} of the notes."),
        );
    }

    assert!(
        wait_for(store.path(), "draft number 9 of the notes", "notes.md"),
        "the last save never became searchable"
    );
    settle(debounce);

    assert_eq!(
        watcher.embedded(),
        1,
        "ten saves inside one debounce window cost {} re-embeds",
        watcher.embedded()
    );
    watcher.stop();
}

/// An event does not mean the bytes changed. `touch`, a chmod, a formatter
/// that rewrites a file identically — all of them arrive as events, and none
/// of them is worth an embed.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_event_on_unchanged_contents_re_embeds_nothing() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(corpus.path(), "notes.md", "Mitochondria power the cell.");
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    // Same bytes, new events: rewrite the file with its own contents.
    for _ in 0..3 {
        write(corpus.path(), "notes.md", "Mitochondria power the cell.");
        thread::sleep(DEBOUNCE * 3);
    }
    settle(DEBOUNCE);

    assert_eq!(
        watcher.embedded(),
        0,
        "unchanged contents were re-embedded {} times",
        watcher.embedded()
    );
    watcher.stop();
}

/// The watcher must ignore what the indexer ignores — including the store's
/// own directory, whose every write would otherwise queue the next re-embed,
/// forever.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn ignored_hidden_and_store_paths_are_not_re_embedded() {
    let corpus = tempfile::tempdir().unwrap();
    write(corpus.path(), ".gitignore", "build/\n");
    fs::create_dir(corpus.path().join("build")).unwrap();
    write(corpus.path(), "notes.md", "Mitochondria power the cell.");

    // The store lives inside the watched tree, which is the default layout:
    // `.semlith` beside what is indexed.
    let store = corpus.path().join(".semlith");
    index(&store, corpus.path());

    let watcher = spawn(&store, corpus.path());

    write(
        &corpus.path().join("build"),
        "artifact.md",
        "Generated output nobody asked to search.",
    );
    write(
        corpus.path(),
        ".hidden.md",
        "A dotfile nobody asked to search.",
    );
    fs::write(store.join("scratch.txt"), "the store's own scribbles").unwrap();
    settle(DEBOUNCE);

    assert_eq!(
        watcher.embedded(),
        0,
        "an ignored, hidden or store-internal path was embedded"
    );
    watcher.stop();

    let s = Semlith::open(&store, None).unwrap();
    let paths = semlith::store::all_paths(s.db()).unwrap();
    assert!(holds(&paths, "notes.md"));
    assert!(!holds(&paths, "artifact.md"), "gitignored: {paths:#?}");
    assert!(!holds(&paths, ".hidden.md"), "hidden: {paths:#?}");
    assert!(!holds(&paths, "scratch.txt"), "store internals: {paths:#?}");
}

/// The roadmap outcome is stated for an agent, and an agent starts its MCP
/// server once and keeps it. A store kept fresh by a watcher the agent cannot
/// see is worth nothing to it.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn a_running_mcp_server_sees_an_edit_without_restarting() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "bread.md",
        "Sourdough needs flour and water.",
    );
    index(store.path(), corpus.path());

    let watcher = spawn(store.path(), corpus.path());

    // A real server process over real stdio, started before the edit exists.
    let mut server = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = server.stdin.take().unwrap();
    let mut stdout = std::io::BufReader::new(server.stdout.take().unwrap());

    rpc(
        &mut stdin,
        &mut stdout,
        1,
        "initialize",
        r#"{"protocolVersion":"2024-11-05"}"#,
    );

    let before = search_over_mcp(&mut stdin, &mut stdout, 2);
    assert!(
        !before.contains("ownership"),
        "the corpus already contained the edit: {before}"
    );

    write(
        corpus.path(),
        "rust.md",
        "Ownership means each value has a single owner, and the compiler \
         frees it when that owner goes out of scope.",
    );
    // Wait on the vector count, not on the file list: a file's row is written
    // before its vectors are durable, so `all_paths` would say yes while the
    // dense half still knows nothing about it.
    assert!(
        wait_for_vectors(store.path(), 2),
        "the watcher never embedded the edit"
    );

    // Same server process, no restart, no re-initialize.
    let after = search_over_mcp(&mut stdin, &mut stdout, 3);
    assert!(
        after.contains("ownership") || after.contains("Ownership"),
        "the running server is still answering from the index it started with: {after}"
    );

    drop(stdin);
    let _ = server.wait();
    watcher.stop();
}

/// Deliberately shares no term with the file it should find: the keyword half
/// of the search reads SQLite, which is fresh across processes for free, so a
/// query it can answer proves nothing about the vector index being reloaded.
/// Only the dense half can return this one.
fn search_over_mcp(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut impl BufRead,
    id: u64,
) -> String {
    rpc(
        stdin,
        stdout,
        id,
        "tools/call",
        r#"{"name":"semlith_search","arguments":{"query":"automatic memory reclamation without garbage collection","k":3}}"#,
    )
}

/// One JSON-RPC round trip, returning the raw response line.
fn rpc(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut impl BufRead,
    id: u64,
    method: &str,
    params: &str,
) -> String {
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":{id},"method":"{method}","params":{params}}}"#
    )
    .unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert!(!line.is_empty(), "the server closed instead of answering");
    line
}

/// A watcher is ended by Ctrl-C, every time. If the ordinary way to stop it
/// can strand a temp index or leave a file half-indexed, the release makes
/// stores less trustworthy than not running it at all.
#[cfg(unix)]
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_interrupted_watcher_leaves_the_store_whole() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "bread.md",
        "Sourdough needs flour and water.",
    );
    index(store.path(), corpus.path());

    for (label, busy) in [("idle", false), ("mid re-embed", true)] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_semlith"))
            .arg("--store")
            .arg(store.path())
            .arg("watch")
            .arg(corpus.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        // Started means the model is loaded and the catch-up pass is done.
        assert!(
            wait_for_vectors(store.path(), 1),
            "{label}: the watcher never got going"
        );
        thread::sleep(Duration::from_secs(2));

        if busy {
            // Enough text that the embed is still running when the signal
            // lands, which is the case worth proving.
            let big = "Ownership and borrowing, explained again and again. ".repeat(4000);
            fs::write(corpus.path().join("long.md"), big).unwrap();
            thread::sleep(Duration::from_millis(700));
        }

        let status = Command::new("kill")
            .arg("-INT")
            .arg(child.id().to_string())
            .status()
            .unwrap();
        assert!(status.success(), "{label}: could not signal the watcher");

        let exit = child.wait().unwrap();
        assert!(exit.success(), "{label}: watcher exited {exit}");

        assert!(
            !store.path().join("index.tv.tmp").exists(),
            "{label}: a half-written index was left behind"
        );

        // Whatever was in flight is either fully in or fully absent after the
        // next run — never a file with chunks and no vectors.
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_| {})
            .unwrap();
        let (_, chunks, _) = s.stats().unwrap();
        assert_eq!(
            chunks as usize,
            s.len(),
            "{label}: {chunks} chunks against {} vectors after re-indexing",
            s.len()
        );
    }
}

// ---- harness ------------------------------------------------------------

/// A watcher running in its own thread, stoppable from the test, counting
/// what it re-embedded so a test can prove a burst cost one pass and not ten.
struct Watcher {
    stop: Arc<AtomicBool>,
    embedded: Arc<AtomicUsize>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    fn embedded(&self) -> usize {
        self.embedded.load(Ordering::Relaxed)
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}

/// Start a watcher over `corpus` and return once it is actually watching —
/// otherwise the test's first write races the watcher's registration and is
/// lost.
fn spawn(store: &Path, corpus: &Path) -> Watcher {
    spawn_with(store, corpus, DEBOUNCE)
}

fn spawn_with(store: &Path, corpus: &Path, debounce: Duration) -> Watcher {
    let stop = Arc::new(AtomicBool::new(false));
    let embedded = Arc::new(AtomicUsize::new(0));
    let (ready_tx, ready_rx) = mpsc::channel();

    let (store_path, roots, flag, counter) = (
        store.to_path_buf(),
        vec![corpus.to_path_buf()],
        Arc::clone(&stop),
        Arc::clone(&embedded),
    );
    let handle = thread::spawn(move || {
        let mut s = Semlith::open(&store_path, None).unwrap();
        s.quiet = true;
        semlith::watch::run(&mut s, &roots, debounce, &flag, |progress| {
            use semlith::watch::Progress;
            match progress {
                Progress::Ready { .. } => {
                    let _ = ready_tx.send(());
                }
                // Counted from the batch report rather than per file, because
                // what is being proven is how many re-embeds happened.
                Progress::Batch(report, _) => {
                    counter.fetch_add(report.indexed, Ordering::Relaxed);
                }
                _ => {}
            }
        })
        .unwrap();
    });

    ready_rx
        .recv_timeout(APPEAR_TIMEOUT)
        .expect("watcher never reported itself ready");

    Watcher {
        stop,
        embedded,
        handle: Some(handle),
    }
}

/// Give the watcher time to do the wrong thing. Used by the tests that assert
/// nothing happened — those cannot poll for an outcome that never arrives.
fn settle(debounce: Duration) {
    thread::sleep(debounce * 8 + Duration::from_secs(1));
}

/// Poll until the store's file list satisfies `done`, or time runs out.
fn wait_until(store: &Path, done: impl Fn(&[String]) -> bool) -> bool {
    let deadline = Instant::now() + APPEAR_TIMEOUT;
    while Instant::now() < deadline {
        let s = Semlith::open(store, None).unwrap();
        let paths = semlith::store::all_paths(s.db()).unwrap();
        if done(&paths) {
            return true;
        }
        drop(s);
        thread::sleep(Duration::from_millis(300));
    }
    false
}

/// Poll until the store holds at least `at_least` vectors. The honest signal
/// that an edit is fully in: chunks land in SQLite before `index.tv` is
/// rewritten, so the file list runs ahead of the vectors.
fn wait_for_vectors(store: &Path, at_least: usize) -> bool {
    let deadline = Instant::now() + APPEAR_TIMEOUT;
    while Instant::now() < deadline {
        if Semlith::open(store, None).unwrap().len() >= at_least {
            return true;
        }
        thread::sleep(Duration::from_millis(300));
    }
    false
}

fn holds(paths: &[String], name: &str) -> bool {
    paths.iter().any(|p| p.ends_with(name))
}

/// Poll a fresh reader until `query` returns `name` best, or time runs out.
fn wait_for(store: &Path, query: &str, name: &str) -> bool {
    let deadline = Instant::now() + APPEAR_TIMEOUT;
    while Instant::now() < deadline {
        let mut s = Semlith::open(store, None).unwrap();
        s.quiet = true;
        if let Ok(hits) = s.search(query, 3)
            && hits.iter().any(|h| h.path.ends_with(name))
        {
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
    s.index_paths(&[corpus.to_path_buf()], |_| {}).unwrap();
}

fn write(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).unwrap();
}
