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
use std::path::Path;
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
