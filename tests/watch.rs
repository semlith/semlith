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
use std::sync::atomic::{AtomicBool, Ordering};
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

// ---- harness ------------------------------------------------------------

/// A watcher running in its own thread, stoppable from the test.
struct Watcher {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Watcher {
    fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.handle.take().unwrap().join().unwrap();
    }
}

/// Start a watcher over `corpus` and return once it is actually watching —
/// otherwise the test's first write races the watcher's registration and is
/// lost.
fn spawn(store: &Path, corpus: &Path) -> Watcher {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = mpsc::channel();

    let (store_path, roots, flag) = (
        store.to_path_buf(),
        vec![corpus.to_path_buf()],
        Arc::clone(&stop),
    );
    let handle = thread::spawn(move || {
        let mut s = Semlith::open(&store_path, None).unwrap();
        s.quiet = true;
        semlith::watch::run(&mut s, &roots, DEBOUNCE, &flag, || {
            let _ = ready_tx.send(());
        })
        .unwrap();
    });

    ready_rx
        .recv_timeout(APPEAR_TIMEOUT)
        .expect("watcher never reported itself ready");

    Watcher {
        stop,
        handle: Some(handle),
    }
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
