//! Keep a store current while its corpus is being edited.
//!
//! The watcher is an event source in front of the indexer that already exists:
//! filesystem events name candidate paths, and everything after that — the
//! content hash that skips unchanged files, chunk eviction, the batched embed,
//! the atomic `index.tv` write — is the same code `semlith index` runs.
//!
//! Two decisions worth knowing about:
//!
//! - **The store lock is held for the watcher's whole life.** A store has one
//!   writer, and `Semlith` holds its index in memory: a second writer would
//!   land rows the watcher then overwrites from a stale in-memory copy. So a
//!   concurrent `semlith index` is refused, by design, and told who has it.
//! - **Ignore rules come from the same walk `index` uses**, not from a second
//!   matcher. An event is acted on only if the path is one the walk would
//!   have yielded, so a watched tree and an indexed tree are the same set of
//!   files by construction rather than by two rules agreeing.

use crate::{IndexReport, Semlith, canonical, lock, walk};
use anyhow::{Context, Result};
use notify::{RecursiveMode, Watcher};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

/// Set by the signal handler; the loop leaves at the next safe point.
pub static STOP: AtomicBool = AtomicBool::new(false);

/// Ask Ctrl-C and SIGTERM to stop the watcher rather than kill it.
///
/// A watcher is ended by a signal every single day — that is how a foreground
/// process finishes — so the ordinary exit has to be the safe one. The handler
/// only sets a flag; the loop notices it between batches, which means an
/// in-flight re-embed finishes and `index.tv` is written whole or not at all.
///
/// A second signal restores the default and kills the process, so a stuck
/// embed can still be escaped.
///
/// Windows is not covered: its console handlers are a different mechanism, and
/// leaving it on the default means Ctrl-C there terminates mid-write, exactly
/// as `semlith index` already does.
#[cfg(unix)]
pub fn stop_on_signal() {
    extern "C" fn handler(sig: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
        // Restoring the default is async-signal-safe, and is what keeps a
        // second Ctrl-C meaningful during a long embed.
        unsafe { libc::signal(sig, libc::SIG_DFL) };
    }

    unsafe {
        libc::signal(libc::SIGINT, handler as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as *const () as libc::sighandler_t);
    }
}

#[cfg(not(unix))]
pub fn stop_on_signal() {}

/// Default quiet period after the last event before a batch is indexed. One
/// editor save is several events — write, chmod, rename — and a formatter on
/// save is several more.
///
/// Measured, not guessed: at 500 ms a burst of ten rewrites of one file costs
/// one embed and one index write, and the edit is searchable about a second
/// after the save — against the five seconds the release budgets. One second
/// was tried and bought nothing. `--debounce` moves it either way.
pub const DEBOUNCE: Duration = Duration::from_millis(500);

/// Longest a batch may keep growing before it is indexed anyway. Without it a
/// steady stream of events (a `git checkout`, a build writing into the tree)
/// would restart the quiet period forever and nothing would ever be indexed.
const MAX_BATCH_WAIT: Duration = Duration::from_secs(5);

/// How often the loop wakes to notice `stop` while no events are arriving.
/// Ctrl-C should not have to wait for a filesystem event to be honoured.
const IDLE_TICK: Duration = Duration::from_millis(250);

/// What the watcher is doing, for whoever is watching the watcher.
///
/// `Ready` matters beyond display: it fires once the watchers are registered
/// and the catch-up pass is done, which is the first moment an edit is
/// guaranteed to be seen. A caller that writes a file and then expects it
/// indexed has to wait for it.
pub enum Progress<'a> {
    Ready {
        catch_up: IndexReport,
        files: i64,
        chunks: i64,
    },
    File(&'a Path),
    /// One file the re-index could not read, with what failed.
    ///
    /// Separate from [`Progress::Error`] because it is not the watcher's
    /// error: the loop keeps running and the store keeps being watched. Before
    /// 0.19.0 a save that failed to embed came back as `Err` from
    /// `index_changed` and ended the watch thread, so every later save in that
    /// tree was silently never indexed.
    Failed {
        path: &'a Path,
        why: String,
    },
    Batch(IndexReport, Duration),
    /// A non-fatal backend error. Reported rather than swallowed: an exhausted
    /// inotify watch limit leaves a process that is running and no longer
    /// watching anything.
    Error(String),
}

/// Watch `roots` until `stop` is set.
pub fn run(
    store: &mut Semlith,
    roots: &[PathBuf],
    debounce: Duration,
    stop: &AtomicBool,
    progress: impl FnMut(Progress),
) -> Result<()> {
    // Taken before anything is watched: if the store is busy, say so now
    // rather than after a catch-up pass has already embedded half a corpus.
    let _lock = lock::StoreLock::acquire(store.dir())?;
    // Nothing queues behind `semlith watch`: it is the only writer there is.
    run_held(
        store,
        roots,
        debounce,
        stop,
        Held {
            catch_up: true,
            waiting: &|| false,
            defer: &|_| false,
        },
        progress,
        |_| Ok(Vec::new()),
    )
}

/// What a caller that owns the writer decides about the watcher's own work.
pub struct Held<'a> {
    /// Whether the watcher catches the store up itself when it starts.
    /// `semlith watch` does; the daemon admits its catch-up as a run instead,
    /// so the concurrency limit bounds it and the page shows it.
    pub catch_up: bool,
    /// Asked between files during the catch-up, so a request that arrives
    /// while a cold store is being walked waits milliseconds rather than
    /// minutes.
    pub waiting: &'a dyn Fn() -> bool,
    /// Offered each batch of events before it is indexed. `true` means the
    /// caller has taken it — the daemon admits a large batch like a run — and
    /// the watcher leaves it alone.
    pub defer: &'a dyn Fn(&[PathBuf]) -> bool,
}

/// [`run`] for a caller that already holds the store's write lock and has work
/// of its own to give the writer.
///
/// `semlith start` is that caller. It owns the lock for the daemon's life, and
/// `pump` is how a portal or a forwarded `semlith_index` reaches the one thread
/// allowed to write — called between batches, so a queued job and a filesystem
/// event never overlap and there is still exactly one writer.
pub fn run_held(
    store: &mut Semlith,
    roots: &[PathBuf],
    debounce: Duration,
    stop: &AtomicBool,
    held: Held<'_>,
    mut progress: impl FnMut(Progress),
    mut pump: impl FnMut(&mut Semlith) -> Result<Vec<PathBuf>>,
) -> Result<()> {
    let roots: Vec<PathBuf> = roots.iter().map(|r| canonical(r)).collect();

    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res| {
        // A send failure means the loop is gone, which is a shutdown, not an
        // error worth reporting from inside the backend's thread.
        let _ = tx.send(res);
    })
    .context("starting the filesystem watcher")?;

    for root in &roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .with_context(|| format!("watching {}", root.display()))?;
    }

    // Catch up on whatever changed while nothing was watching. It is the same
    // incremental pass `semlith index` runs, so an unchanged tree costs a walk
    // and a hash per file, and nothing else.
    // Yielding keeps everything the pass already did: the loop below walks
    // again once the queue is empty, and an unchanged file costs a hash.
    let waiting = held.waiting;
    let step_aside = || {
        // Shutdown counts as something waiting: a catch-up that only looked at
        // the queue kept the process alive until it had walked the whole tree.
        if waiting() || stop.load(Ordering::Relaxed) {
            crate::Flow::Yield
        } else {
            crate::Flow::Run
        }
    };
    let catch_up = if held.catch_up {
        store.index_walk_under(&roots, &step_aside, |path, _| {
            progress(Progress::File(path))
        })?
    } else {
        IndexReport::default()
    };
    let (files, chunks, _) = store.stats()?;
    let mut behind = catch_up.remaining > 0;
    progress(Progress::Ready {
        catch_up,
        files,
        chunks,
    });

    while !stop.load(Ordering::Relaxed) {
        // Before waiting on the filesystem, not after: a request that arrived
        // while the last batch was embedding should not sit for another idle
        // tick behind a tree nobody is editing.
        let stopped = match pump(store) {
            Ok(stopped) => stopped,
            Err(e) => {
                progress(Progress::Error(e.to_string()));
                Vec::new()
            }
        };

        // A run that was stopped owns the events that queued up while it held
        // the writer: they are mostly its own corpus being written, and a stop
        // means "leave it". Indexing them here re-embedded the very files the
        // stop had just undone. Events already waiting for paths under the
        // stopped run's roots are dropped; the rest are indexed as usual.
        if !stopped.is_empty() {
            let stopped: Vec<PathBuf> = stopped.iter().map(|r| canonical(r)).collect();
            let mut waiting_events: BTreeSet<PathBuf> = BTreeSet::new();
            while let Ok(event) = rx.try_recv() {
                match event {
                    Ok(event) => collect(&mut waiting_events, event.paths),
                    Err(e) => progress(Progress::Error(e.to_string())),
                }
            }
            waiting_events.retain(|path| !stopped.iter().any(|root| path.starts_with(root)));
            let paths = admissible(&roots, waiting_events);
            if !paths.is_empty() && !(held.defer)(&paths) {
                let started = Instant::now();
                let report =
                    store.index_changed(paths, |path, p| progress(file_progress(path, &p)))?;
                if report.indexed > 0 || report.removed > 0 {
                    progress(Progress::Batch(report, started.elapsed()));
                }
            }
            continue;
        }

        // Whatever the catch-up stepped aside from, once the queue is clear.
        // Without this a store that was interrupted would only finish catching
        // up when a file happened to change.
        if behind && !waiting() {
            let started = Instant::now();
            let more = store.index_walk_under(&roots, &step_aside, |path, p| {
                progress(file_progress(path, &p))
            })?;
            behind = more.remaining > 0;
            if more.indexed > 0 || more.removed > 0 {
                // Its real elapsed time. `Duration::ZERO` here printed every
                // catch-up as "in 0.0s".
                progress(Progress::Batch(more, started.elapsed()));
            }
            continue;
        }

        let first = match rx.recv_timeout(IDLE_TICK) {
            Ok(Ok(event)) => event,
            Ok(Err(e)) => {
                progress(Progress::Error(e.to_string()));
                continue;
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => break,
        };

        let mut batch: BTreeSet<PathBuf> = BTreeSet::new();
        collect(&mut batch, first.paths);
        drain(&rx, &mut batch, debounce, &mut progress);

        let paths = admissible(&roots, batch);
        if paths.is_empty() || (held.defer)(&paths) {
            continue;
        }

        let started = Instant::now();
        let report = store.index_changed(paths, |path, p| progress(file_progress(path, &p)))?;
        if report.indexed > 0 || report.removed > 0 {
            progress(Progress::Batch(report, started.elapsed()));
        }
    }

    Ok(())
}

/// One file's line on the watcher's feed.
///
/// A failure is named; everything else is the file alone. The whole point is
/// that a failure is a line rather than the end of the thread.
fn file_progress<'a>(path: &'a Path, p: &crate::IndexProgress) -> Progress<'a> {
    match p.outcome {
        crate::FileOutcome::Failed => Progress::Failed {
            path,
            why: p.why.clone().unwrap_or_else(|| "failed".to_string()),
        },
        _ => Progress::File(path),
    }
}

/// Keep taking events until the stream goes quiet for `debounce`, so one save
/// costs one re-embed. Capped by `MAX_BATCH_WAIT` so a storm still gets
/// indexed rather than deferred forever.
fn drain(
    rx: &mpsc::Receiver<notify::Result<notify::Event>>,
    batch: &mut BTreeSet<PathBuf>,
    debounce: Duration,
    progress: &mut impl FnMut(Progress),
) {
    let deadline = Instant::now() + MAX_BATCH_WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        match rx.recv_timeout(debounce.min(left)) {
            Ok(Ok(event)) => collect(batch, event.paths),
            Ok(Err(e)) => progress(Progress::Error(e.to_string())),
            Err(RecvTimeoutError::Timeout) => return,
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn collect(batch: &mut BTreeSet<PathBuf>, paths: Vec<PathBuf>) {
    for path in paths {
        batch.insert(resolve(&path));
    }
}

/// Which of a batch's paths the indexer should be given: the ones the walk
/// would yield, plus the ones that have gone from disk — a deletion has no
/// walk entry, and is the other half of keeping a store current.
///
/// ponytail: the walk runs once per batch rather than per path, and only when
/// events fired. If it ever shows up in a profile, the upgrade is a
/// `ignore::gitignore` matcher built once at startup — at the cost of two
/// definitions of "which files count" that have to agree.
fn admissible(roots: &[PathBuf], batch: BTreeSet<PathBuf>) -> Vec<PathBuf> {
    let gone: Vec<PathBuf> = batch.iter().filter(|p| !p.exists()).cloned().collect();
    if batch.len() == gone.len() {
        return gone;
    }

    let visible: BTreeSet<PathBuf> = walk(roots).files.into_iter().collect();
    batch
        .into_iter()
        .filter(|p| visible.contains(p) || gone.contains(p))
        .collect()
}

/// Canonicalize an event path, including one whose file no longer exists —
/// store keys are canonical, and on macOS a path under `/var` is stored under
/// `/private/var`, so an uncanonicalized deletion would match nothing.
fn resolve(path: &Path) -> PathBuf {
    if path.exists() {
        return canonical(path);
    }
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) if dir.exists() => canonical(dir).join(name),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deleted path must survive canonicalization, or a deletion event names
    /// a key the store has never heard of and the chunks stay forever.
    #[test]
    fn a_missing_file_still_resolves_under_its_canonical_parent() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("gone.md");
        std::fs::write(&real, "x").unwrap();
        let while_present = resolve(&real);
        std::fs::remove_file(&real).unwrap();

        assert_eq!(
            resolve(&real),
            while_present,
            "the same file resolves to two different keys depending on whether it exists"
        );
    }

    /// Nothing on disk means nothing to walk: a batch of pure deletions must
    /// not pay for a tree walk to learn that.
    #[test]
    fn a_batch_of_deletions_needs_no_walk() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("never-existed.md");
        let batch: BTreeSet<PathBuf> = [missing.clone()].into_iter().collect();

        assert_eq!(
            admissible(&[dir.path().to_path_buf()], batch),
            vec![missing]
        );
    }

    /// The store's own directory is not corpus. Without this the watcher's own
    /// writes to index.tv would queue another re-embed, forever.
    #[test]
    fn the_store_directory_is_not_admissible() {
        let dir = tempfile::tempdir().unwrap();
        let store = dir.path().join(".semlith");
        std::fs::create_dir(&store).unwrap();
        let index = store.join("index.tv");
        std::fs::write(&index, "not really an index").unwrap();
        std::fs::write(dir.path().join("real.md"), "corpus").unwrap();

        let batch: BTreeSet<PathBuf> = [canonical(&index), canonical(&dir.path().join("real.md"))]
            .into_iter()
            .collect();
        let admitted = admissible(&[dir.path().to_path_buf()], batch);

        assert_eq!(admitted, vec![canonical(&dir.path().join("real.md"))]);
    }
}
