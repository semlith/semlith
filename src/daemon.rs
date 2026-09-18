//! `semlith start` — one process that owns every store.
//!
//! Before this, a developer could have a watcher keeping a store current or an
//! agent holding it open, but not both: a store has one writer, so the second
//! one was refused. The daemon removes that structurally rather than by
//! relaxing the rule. It *is* the writer, for its whole life, and everything
//! else — the portal, a forwarded `semlith_index` — is a client of it.
//!
//! Three things run at once:
//!
//! - **A watcher thread per store**, holding that store's write lock and its
//!   own [`Semlith`], re-embedding files as they are saved. It is the same
//!   loop `semlith watch` runs.
//! - **A reader [`Fleet`]** over the same stores, which every read route
//!   answers from. Opening a store twice in one process is what the store
//!   format already supports — a reader notices the writer's work through the
//!   index generation and reloads — so reads never wait behind an embed.
//! - **The HTTP server**, on 127.0.0.1 and nowhere else.
//!
//! Work that writes does not go to the reader. It goes on the store's queue,
//! and the watcher thread — the lock holder — performs it between batches.
//! That is why a portal index and a file save can never interleave.

use crate::fleet::Fleet;
use crate::home::{self, Registry};
use crate::http::{self, Refusal, Server};
use crate::lock::StoreLock;
use crate::{Semlith, watch};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, SystemTime};

/// Written beside the store's lock while a daemon holds it, so `semlith mcp`
/// can find the daemon and forward to it instead of fighting for the lock.
///
/// Beside the lock rather than in the home, because the question it answers is
/// "who is writing *this* store", which is exactly what the lock file is about.
pub const DISCOVERY_FILE: &str = "daemon.json";

/// How many watcher events the Stores view can show. A ring, not a log: this
/// is the "is it actually running" signal, and the last twenty answer it.
const EVENT_HISTORY: usize = 20;

/// How long one queued index slice may work before it yields the writer back
/// to the watcher. The same budget `semlith_index` has always had.
const SLICE: Duration = Duration::from_secs(45);

/// How many of a run's log lines the daemon keeps for a page to catch up on.
///
/// A ring rather than a log, for the same reason [`EVENT_HISTORY`] is one: the
/// question a returning tab asks is "what has happened since sequence N", and
/// five hundred lines is more than any tab that was away for a poll or two has
/// missed. A tab away for longer sees the gap in the sequence numbers rather
/// than a quietly shortened history.
const LOG_HISTORY: usize = 500;

/// How many of a store's runs are kept.
///
/// Enough that a session's cards are all there; bounded so a daemon left up
/// for a week does not accumulate them without limit. Only finished runs are
/// ever dropped.
const RUN_HISTORY: usize = 24;

/// The run id a job that is not an index run carries.
///
/// Ids are minted from 1, so 0 names nothing and folding an event into it is a
/// no-op — which is what a forget or a fetch wants.
const NO_RUN: u64 = 0;

/// How long a freshly made store's watcher waits for the run that is about to
/// index it before deciding none is coming.
const EXPECTED_RUN_GRACE_SECS: usize = 60;

/// What `semlith mcp` reads to find a running daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Discovery {
    pub pid: u32,
    pub port: u16,
    /// The daemon's current token. A rotate rewrites this file, so a forwarding
    /// `semlith mcp` picks the new one up on its next call.
    pub token: String,
    pub version: String,
}

impl Discovery {
    /// The daemon to forward to, if this file names one and is one semlith
    /// wrote.
    ///
    /// This file decides where `semlith mcp` sends every call, so a repository
    /// that carried one would be choosing the port an agent's questions and
    /// answers travel through. Four things have to hold, and each of them is a
    /// way the file could have come from somewhere else:
    ///
    /// - the store it sits in is one this user trusts, so a cloned `.semlith`
    ///   never gets this far;
    /// - on unix it is mode `0600` and owned by this user, so another account
    ///   on the machine did not write it;
    /// - the pid it names is alive, so a stale file from a dead daemon is
    ///   ignored rather than followed to whatever now holds that port;
    /// - the token is 64 hex characters, so what is about to be put in a header
    ///   is a token rather than whatever was in the file.
    ///
    /// Every failure falls back to opening the store directly, which is what
    /// this function's `None` has always meant.
    pub fn read(store_dir: &Path) -> Option<Self> {
        if !home::Registry::load().ok()?.trusts(store_dir) {
            return None;
        }
        let path = store_dir.join(DISCOVERY_FILE);
        if worth_warning_about(&path) {
            eprintln!(
                "semlith: ignoring {} — it is not a file this user wrote privately",
                path.display()
            );
            return None;
        }
        if !owner_only(&path) {
            return None;
        }
        let text = std::fs::read_to_string(&path).ok()?;
        let found: Self = serde_json::from_str(&text).ok()?;
        if found.token.len() != 64 || !found.token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        if !alive(found.pid) {
            return None;
        }
        Some(found)
    }

    fn write(&self, store_dir: &Path) -> Result<()> {
        let path = store_dir.join(DISCOVERY_FILE);
        // The mode is set as the file is created rather than afterwards. The
        // session token is in this file, and a file that is world-readable for
        // the microsecond between the two is a file that was world-readable —
        // the same reason `home::write_agent_key` opens with a mode.
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(home::FILE_MODE);
        }
        let mut file = options
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        use std::io::Write;
        file.write_all((serde_json::to_string_pretty(self)? + "\n").as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
        // An existing file keeps its own mode through `truncate`, so one left
        // loose by an older semlith is narrowed here too.
        home::tighten_file(&path);
        Ok(())
    }

    fn remove(store_dir: &Path) {
        let _ = std::fs::remove_file(store_dir.join(DISCOVERY_FILE));
    }
}

/// Whether a discovery file is one worth complaining about.
///
/// `owner_only` answers `false` for a file that is not there, because
/// `metadata` fails and there is no mode to read. Routing absence through it
/// meant a store with no daemon running — the ordinary case, and the case on
/// every one of six stores on this machine — was reported as a privacy
/// rejection. Every `semlith mcp` start printed six lines of "it is not a file
/// this user wrote privately" naming six paths that answer `No such file or
/// directory`, and those six lines were the first thing in a client's log for
/// anyone trying to work out why no server had appeared. A missing file is
/// missing; only a file that exists and fails the check is worth a line.
fn worth_warning_about(path: &Path) -> bool {
    path.exists() && !owner_only(path)
}

/// Whether a file is one this user wrote and nobody else can read.
///
/// On Windows there is no mode to read: the home sits inside the user's
/// profile, whose default ACL grants that user alone, and semlith carries no
/// API for reading an ACL. Stated rather than silently assumed, as
/// `home::check_key_mode` already does for the agent key.
#[cfg(unix)]
fn owner_only(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    // SAFETY: `getuid` reads this process's own id and cannot fail.
    let me = unsafe { libc::getuid() };
    meta.uid() == me && meta.permissions().mode() & 0o177 == 0
}

#[cfg(not(unix))]
fn owner_only(path: &Path) -> bool {
    path.exists()
}

/// Whether a process id names something running.
///
/// `kill(pid, 0)` asks the kernel without sending anything. A pid this user
/// does not own answers `EPERM`, which still means it is alive — and a daemon
/// belonging to somebody else is one this file should not have named, which
/// `owner_only` has already decided.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 delivers nothing; this is the documented liveness probe.
    unsafe {
        libc::kill(pid as i32, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(not(unix))]
fn alive(_pid: u32) -> bool {
    true
}

/// Something the watcher thread should do next, on behalf of a request.
/// What an index job has left to do.
///
/// A slice yields the writer back to the watcher when its budget runs out and
/// the rest is re-queued behind whatever the watcher had waiting, so one
/// logical run is several `Index` jobs on one report channel. What a
/// continuation carries is the *remainder of the walk*, not the roots: walking
/// the roots again meant re-opening and re-hashing every file the run had
/// already done, once per slice, to be told each time that it was unchanged.
enum Work {
    /// The first slice of a run: the roots to walk.
    Roots(Vec<PathBuf>),
    /// Every later slice: exactly the files the previous one did not reach.
    Rest(Vec<PathBuf>),
}

/// What a run has done so far, across every slice of it.
///
/// Carried between slices because each slice's report is its own: the `done`
/// summary used to be whatever the last slice happened to do, so a run of 35
/// files that took three slices ended by announcing the four the third slice
/// reached. Every one of these is the run's, which is what the summary claims
/// to be.
#[derive(Default, Clone, Copy)]
struct Tally {
    indexed: u64,
    unchanged: u64,
    skipped: u64,
    removed: u64,
    chunks: u64,
    images: u64,
    /// Symbols extracted, carried for the same reason the chunks are: a
    /// slice's own count starts at zero and the page is drawing one run.
    symbols: u64,
}

impl Tally {
    fn add(&mut self, report: &crate::IndexReport) {
        self.indexed += report.indexed as u64;
        self.unchanged += report.unchanged as u64;
        self.skipped += report.skipped as u64;
        self.removed += report.removed as u64;
        self.chunks += report.chunks as u64;
        self.images += report.images as u64;
        self.symbols += report.symbols as u64;
    }
}

struct Indexing {
    work: Work,
    /// Every file this run has embedded, across every slice, so a stop undoes
    /// the run rather than the slice that happened to be going.
    already: Vec<String>,
    /// What the run had scanned before this slice, and how many files the walk
    /// found in total. Carried so the progress a page draws belongs to the run:
    /// a slice's own counters restart at one, which a page faithfully redrew as
    /// a run that had begun again.
    scanned: u64,
    total: u64,
    /// What the earlier slices of this run got through.
    tally: Tally,
}

enum Job {
    Index(Indexing),
    Forget(PathBuf),
}

/// A job and the channel its progress goes back on.
struct Queued {
    /// Which run this job belongs to.
    ///
    /// A store used to hold one run, so the events a job emitted could be
    /// folded into "the run" without saying which. It holds several now — a
    /// second run against the same store is its own card with its own log —
    /// and an event that does not name its run is an event the wrong card
    /// shows.
    run: u64,
    job: Job,
    report: mpsc::Sender<serde_json::Value>,
}

/// One watcher event, for the Stores view's feed.
#[derive(Debug, Clone, Serialize)]
pub struct Event {
    /// Unix seconds.
    pub at: u64,
    pub text: String,
}

/// Where an index run has got to.
///
/// `stopping` is told apart from `stopped` because undoing what a run embedded
/// takes as long as the embedding did on a large corpus, and a card that jumps
/// straight to "stopped" would be claiming the store was already back to what
/// it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Queued,
    Running,
    Paused,
    Stopping,
    Done,
    Stopped,
    Failed,
}

impl RunStatus {
    /// Whether this run is over. An admission is what a finished run releases,
    /// so this is the predicate the queue moves on.
    pub fn finished(self) -> bool {
        matches!(self, Self::Done | Self::Stopped | Self::Failed)
    }
}

/// One index run, as the daemon knows it and a page reads it.
///
/// This is the whole of what 0.20.0 changes about a run's lifetime. Until now
/// a run existed only as the events travelling down one HTTP response, so the
/// tab that started it was the only thing that knew it was happening and
/// leaving the page threw that away. The run lives here instead: the same
/// `say` closure that emits every event writes this, so there is one path and
/// the snapshot cannot disagree with the stream.
pub struct RunState {
    pub id: u64,
    pub paths: Vec<PathBuf>,
    pub status: RunStatus,
    /// Unix seconds the run was submitted.
    pub submitted: u64,
    /// Where this run's clock started, and what it has spent held.
    ///
    /// The clock belongs to the run, not to the slice. A run yields the writer
    /// back to the watcher every [`SLICE`] and comes back as a fresh
    /// `Job::Index`, so a duration measured inside `perform` restarted from
    /// zero every forty-five seconds — which a page faithfully redrew as the
    /// run having just begun. Measured here it spans every slice, survives the
    /// page being closed, and stops while the run is paused: held time is not
    /// time anything is happening.
    origin: std::time::Instant,
    paused_at: Option<std::time::Instant>,
    paused_total: Duration,
    /// The reading frozen at the moment the run ended, so a finished card
    /// keeps its total rather than counting on.
    ended: Option<Duration>,
    pub scanned: u64,
    pub total: u64,
    pub indexed: u64,
    pub chunks: u64,
    pub symbols: u64,
    /// What the run is doing when it is not reading a file.
    ///
    /// Every two hundred files a run flushes its batch and rewrites the
    /// shards, which on a large corpus is twenty seconds with no counter
    /// moving and nothing said — indistinguishable from a hang. Carried on the
    /// snapshot rather than only on the log, so a page opened during one of
    /// those twenty seconds sees it too.
    pub phase: Option<String>,
    /// The `done` event as it was sent, with its refused, failed and
    /// skipped-by-reason lists.
    pub summary: Option<serde_json::Value>,
    /// The last [`LOG_HISTORY`] events, each carrying the sequence number a
    /// client reads after.
    log: VecDeque<serde_json::Value>,
    next_seq: u64,
}

impl RunState {
    fn new(id: u64, paths: Vec<PathBuf>) -> Self {
        Self {
            id,
            paths,
            status: RunStatus::Queued,
            submitted: now(),
            // From submission, not from the writer taking it: the wait for a
            // writer is time the person is waiting.
            origin: std::time::Instant::now(),
            paused_at: None,
            paused_total: Duration::ZERO,
            ended: None,
            scanned: 0,
            total: 0,
            indexed: 0,
            chunks: 0,
            symbols: 0,
            phase: None,
            summary: None,
            log: VecDeque::new(),
            next_seq: 0,
        }
    }

    /// How long this run has been going, not counting time it spent held.
    ///
    /// Monotonic, so it never goes backwards: a page that polls this can tick
    /// between polls and be corrected by each one without the reading ever
    /// jumping back — which is what a per-slice measurement did.
    fn elapsed(&self) -> Duration {
        if let Some(ended) = self.ended {
            return ended;
        }
        // One instant, both spans measured from it. Sampling `Instant::now()`
        // twice — once inside the held total and once for the run's own — puts
        // the gap between the two calls into the answer, so a *held* clock
        // crept upward by a few hundred nanoseconds per read and crossed a
        // millisecond boundary often enough to be visible.
        let now = std::time::Instant::now();
        let held = self.paused_total
            + self
                .paused_at
                .map(|at| now.saturating_duration_since(at))
                .unwrap_or_default();
        now.saturating_duration_since(self.origin)
            .saturating_sub(held)
    }

    /// Stop counting for as long as the run is held.
    fn hold(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(std::time::Instant::now());
        }
    }

    /// Start counting again, keeping what was already counted.
    fn unhold(&mut self) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total += at.elapsed();
        }
    }

    /// Fold one event into the run and put it on the log ring.
    fn absorb(&mut self, event: &serde_json::Value) {
        let num = |key: &str| event.get(key).and_then(serde_json::Value::as_u64);
        match event.get("event").and_then(serde_json::Value::as_str) {
            Some("started") => {
                self.status = RunStatus::Running;
                self.unhold();
            }
            Some("file") => {
                self.status = RunStatus::Running;
                // A `writing` line is the run saying it has stopped reading
                // files for a moment; the next line of any other outcome ends
                // the phase.
                self.phase = match event.get("outcome").and_then(serde_json::Value::as_str) {
                    Some("writing") => event
                        .get("why")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                        .or_else(|| Some("writing the index to disk".to_string())),
                    _ => None,
                };
                // Taken rather than added: every one of these is the run's
                // own running total, so summing them would count each file
                // once per event it appeared in.
                self.scanned = num("scanned").unwrap_or(self.scanned);
                self.total = num("total").unwrap_or(self.total);
                self.indexed = num("indexed").unwrap_or(self.indexed);
                self.chunks = num("chunks").unwrap_or(self.chunks);
                self.symbols = num("symbols").unwrap_or(self.symbols);
            }
            Some("paused") => {
                self.status = RunStatus::Paused;
                self.hold();
            }
            Some("resumed" | "slice") => {
                self.status = RunStatus::Running;
                self.unhold();
            }
            Some("done") => {
                let stopped = event
                    .get("stopped")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                self.status = if stopped {
                    RunStatus::Stopped
                } else {
                    RunStatus::Done
                };
                self.phase = None;
                self.indexed = num("indexed").unwrap_or(self.indexed);
                self.chunks = num("chunks").unwrap_or(self.chunks);
                self.unhold();
                self.ended = Some(self.elapsed());
                self.summary = Some(event.clone());
            }
            Some("error") => {
                self.status = RunStatus::Failed;
                self.unhold();
                self.ended = Some(self.elapsed());
            }
            _ => {}
        }

        let mut line = event.clone();
        if let Some(object) = line.as_object_mut() {
            object.insert("seq".into(), serde_json::json!(self.next_seq));
        }
        self.next_seq += 1;
        if self.log.len() == LOG_HISTORY {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }
}

/// Everything about one open store that a route can ask about.
pub struct Store {
    pub name: String,
    pub dir: PathBuf,
    /// Roots as the registry records them, including any that no longer exist.
    pub roots: Vec<PathBuf>,
    /// The subset of `roots` that is actually on disk and being watched.
    pub watched: Vec<PathBuf>,
    queue: Mutex<VecDeque<Queued>>,
    events: Mutex<VecDeque<Event>>,
    /// This store's runs, oldest first.
    ///
    /// Kept after a run ends so a page opened afterwards shows what it did
    /// rather than an empty card. A list rather than a slot because a store
    /// can be indexed twice: the second run used to replace the first one's
    /// record, so the card kept the old log under the new header and the only
    /// button a finished run offered was Stop — which then latched the store
    /// and killed whatever ran next.
    runs: Mutex<Vec<RunState>>,
    /// False once the watcher thread has returned, so the portal can say a
    /// store stopped being kept current rather than showing a stale count.
    pub watching: AtomicBool,
    /// Set to stop this store's watcher and release its lock, so the store can
    /// be deleted while the daemon keeps running. The daemon's own shutdown
    /// sets it on every open store alongside the global stop.
    pub stop: AtomicBool,
    /// A queued index run holds here between files while this is set.
    pub paused: AtomicBool,
    /// A queued index run gives up and undoes itself when this is set.
    pub cancelled: AtomicBool,
    /// Unix seconds of the last write this daemon made to the store.
    ///
    /// A cache in front of the store's own `MAX(indexed_at)`, not the answer:
    /// a store written before this daemon started has a last write and this
    /// counter does not know it. `/api/stores` reads the store.
    pub last_write: AtomicUsize,
    /// Rows dropped when this store was opened because they sat outside every
    /// root it is registered against. Shown for the session, so a store that
    /// was contaminated says so rather than quietly correcting itself.
    pub pruned: AtomicUsize,
    /// Unix seconds until which this store's watcher holds off its catch-up,
    /// because a run that is about to arrive is what should index it.
    ///
    /// A store the portal creates is opened and then indexed, in that order,
    /// and the watcher used to win the race: it embedded the corpus itself,
    /// and the run that followed correctly found every file unchanged and
    /// reported "1 indexed, 2 unchanged, 1 chunks" for a store that had just
    /// gained three files. A deadline rather than a flag, so a run that never
    /// arrives costs the watcher a minute rather than the session.
    pub expecting_run_until: AtomicUsize,
}

impl Store {
    fn note(&self, text: String) {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if events.len() == EVENT_HISTORY {
            events.pop_front();
        }
        events.push_back(Event { at: now(), text });
        changes::bump(changes::Domain::Events);
    }

    /// Start a run's record, before it is admitted to the writer.
    ///
    /// Appended: a second run against this store is a second card with its own
    /// header, its own log and its own buttons. The oldest finished runs are
    /// dropped once there are more than [`RUN_HISTORY`] of them, so a daemon
    /// left up for a week does not accumulate cards forever; a run that has
    /// not finished is never dropped.
    fn begin_run(&self, id: u64, paths: Vec<PathBuf>) {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        runs.push(RunState::new(id, paths));
        while runs.len() > RUN_HISTORY {
            match runs.iter().position(|run| run.status.finished()) {
                Some(at) => {
                    runs.remove(at);
                }
                None => break,
            }
        }
        drop(runs);
        self.runs_changed();
    }

    /// The one place the runs counter moves.
    ///
    /// Four things change what `/api/index/runs` would answer — a run begins,
    /// an event lands, a card is removed, the finished cards are cleared — and
    /// a counter bumped from four places is four sources of truth. See
    /// `one_bump_site_per_domain`.
    fn runs_changed(&self) {
        changes::bump(changes::Domain::Runs);
    }

    /// Fold one of this run's events into the snapshot.
    ///
    /// Called from `perform`'s `say`, so the snapshot and the stream carry the
    /// same events in the same order and neither can be the newer of the two.
    /// Addressed by run id rather than by store: a store with a run going and
    /// another queued behind it has two records, and an event folded into the
    /// wrong one is a card describing work it did not do.
    fn record(&self, id: u64, event: &serde_json::Value) {
        if let Some(run) = self
            .runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .find(|run| run.id == id)
        {
            run.absorb(event);
        }
        self.runs_changed();
    }

    /// Do something with the run that has not finished, if there is one.
    fn with_live<T>(&self, body: impl FnOnce(&mut RunState) -> T) -> Option<T> {
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .find(|run| !run.status.finished())
            .map(body)
    }

    /// Say that a stop has been asked for.
    ///
    /// Told apart from `stopped` because undoing what a run embedded takes as
    /// long as the embedding did on a large corpus, and a card that jumped
    /// straight to "stopped" would be claiming the store was already back to
    /// what it was. A run that had already finished is left alone: a stop that
    /// arrived late did not stop anything.
    pub fn mark_stopping(&self) {
        self.with_live(|run| run.status = RunStatus::Stopping);
    }

    /// How long a run has been going, in milliseconds.
    ///
    /// The one clock for a run, read by the events it emits as well as by the
    /// snapshot, so the two cannot disagree — and it spans every slice, which
    /// is the whole of what was wrong with measuring inside `perform`.
    fn run_elapsed_ms(&self, id: u64) -> u64 {
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|run| run.id == id)
            .map(|run| run.elapsed().as_millis() as u64)
            .unwrap_or(0)
    }

    /// The id of this store's unfinished run, if it has one.
    fn current_run(&self) -> Option<u64> {
        self.runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|run| !run.status.finished())
            .map(|run| run.id)
    }

    /// The paths and the committed count of a run that is still going.
    ///
    /// Read at shutdown, which is the only moment it means anything: a run
    /// that has not finished when the process ends is one that will not, and
    /// this is what the next start has to say about it.
    fn interrupted(&self) -> Option<(Vec<PathBuf>, u64)> {
        self.with_live(|run| (run.paths.clone(), run.indexed))
    }

    /// Whether this store has a run that has not finished.
    pub fn run_live(&self) -> bool {
        self.current_run().is_some()
    }

    /// Drop one finished run's card.
    ///
    /// Only a finished one: Remove is how a card that has nothing left to say
    /// is dismissed, and removing a live run's record would leave the run
    /// going with nothing watching it.
    pub fn remove_run(&self, id: u64) -> bool {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let Some(at) = runs
            .iter()
            .position(|run| run.id == id && run.status.finished())
        else {
            return false;
        };
        runs.remove(at);
        drop(runs);
        self.runs_changed();
        true
    }

    /// Drop every finished run's card, and say how many went.
    pub fn clear_finished_runs(&self) -> usize {
        let mut runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let before = runs.len();
        runs.retain(|run| !run.status.finished());
        let gone = before - runs.len();
        drop(runs);
        if gone > 0 {
            self.runs_changed();
        }
        gone
    }

    /// This store's runs as `/api/index/runs` reports them, oldest first, with
    /// the queue position a caller works out from the admission queue.
    ///
    /// The position belongs to whichever run is still waiting, so a finished
    /// card is never labelled with a place in a queue it has already left.
    pub fn run_snapshots(&self, position: Option<usize>) -> Vec<serde_json::Value> {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        runs.iter()
            .map(|run| {
                let position = (!run.status.finished()).then_some(position).flatten();
                self.snapshot_of(run, position)
            })
            .collect()
    }

    fn snapshot_of(&self, run: &RunState, position: Option<usize>) -> serde_json::Value {
        serde_json::json!({
            "id": run.id,
            "store": self.name,
            "paths": run.paths.iter()
                .map(|p| crate::plain(&p.display().to_string()))
                .collect::<Vec<_>>(),
            "status": run.status,
            "position": position,
            "submitted": run.submitted,
            "elapsed_ms": run.elapsed().as_millis() as u64,
            // Whether the clock should be ticking in the page between polls.
            // A finished or held run keeps its reading; nothing else should
            // make a page decide that for itself.
            "ticking": matches!(run.status, RunStatus::Running | RunStatus::Queued | RunStatus::Stopping),
            "scanned": run.scanned,
            "total": run.total,
            "indexed": run.indexed,
            "chunks": run.chunks,
            "symbols": run.symbols,
            "phase": run.phase,
            "summary": run.summary,
            // What a page's log cursor should be if it has never read this
            // run: the oldest line still on the ring, minus one.
            "log_from": run.log.front()
                .and_then(|line| line.get("seq"))
                .and_then(serde_json::Value::as_u64)
                .map(|first| first.saturating_sub(1)),
            "log_to": run.next_seq.saturating_sub(1),
        })
    }

    /// Every log line one run emitted after `seq`.
    ///
    /// A cursor rather than an offset, so two clients reading the same run
    /// through their own cursors each see every line exactly once and neither
    /// is affected by what the other has read.
    ///
    /// The run is named, because a store can have two. With no id the live run
    /// answers, or the last one if none is live — which is what a caller that
    /// knows only a store name means.
    pub fn log_after(&self, id: Option<u64>, seq: Option<u64>) -> Vec<serde_json::Value> {
        let runs = self.runs.lock().unwrap_or_else(|e| e.into_inner());
        let run = match id {
            Some(id) => runs.iter().find(|run| run.id == id),
            None => runs
                .iter()
                .find(|run| !run.status.finished())
                .or_else(|| runs.last()),
        };
        run.map(|run| {
            run.log
                .iter()
                .filter(|line| match seq {
                    None => true,
                    Some(after) => line
                        .get("seq")
                        .and_then(serde_json::Value::as_u64)
                        .is_some_and(|n| n > after),
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default()
    }

    /// The watcher's recent events, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// Hand a job to the writer and read its progress back as it happens.
    ///
    /// The receiver is what a streaming route writes chunks from, so the
    /// browser sees an index run while it is running rather than when it ends.
    /// Queue a job and hand back the channel its progress arrives on.
    ///
    /// `notice` is sent before the job is queued, for a caller that streams
    /// and would otherwise hear nothing while the writer finishes what it is
    /// doing. A caller that takes the first message as its answer — forget —
    /// passes `None`, because a notice would be that answer.
    fn submit(
        &self,
        job: Job,
        notice: Option<serde_json::Value>,
    ) -> mpsc::Receiver<serde_json::Value> {
        let (report, progress) = mpsc::channel();
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(mut notice) = notice {
            if let Some(object) = notice.as_object_mut() {
                object.insert("store".into(), serde_json::json!(self.name));
                object.insert("ahead".into(), serde_json::json!(queue.len()));
            }
            let _ = report.send(notice);
        }
        // Run ids start at 1, so 0 is "no run": a forget or a fetch is a job
        // the writer performs and not a run with a card, and its events belong
        // to no run's log.
        queue.push_back(Queued {
            run: NO_RUN,
            job,
            report,
        });
        progress
    }

    /// Put an admitted index run on this store's writer queue.
    ///
    /// Apart from [`Store::submit`] because the channel belongs to the run
    /// rather than to this call: the run was minted when it was queued for
    /// admission, possibly minutes ago, and whoever asked for it may already
    /// have been answered.
    fn submit_index(
        &self,
        run: u64,
        paths: Vec<PathBuf>,
        report: mpsc::Sender<serde_json::Value>,
        mut notice: serde_json::Value,
    ) {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(object) = notice.as_object_mut() {
            object.insert("store".into(), serde_json::json!(self.name));
            object.insert("ahead".into(), serde_json::json!(queue.len()));
        }
        // The snapshot before the stream, so a page that polls between these
        // two lines sees the notice rather than nothing.
        self.record(run, &notice);
        let _ = report.send(notice);
        queue.push_back(Queued {
            run,
            job: Job::Index(Indexing {
                work: Work::Roots(paths),
                already: Vec::new(),
                scanned: 0,
                total: 0,
                tally: Tally::default(),
            }),
            report,
        });
    }

    /// Whether the writer has anything waiting — the Index view's queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Put a slice's remainder back, behind whatever is waiting.
    fn requeue(&self, queued: Queued) {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_back(queued);
    }

    /// Drop every index job that has not started, answering each as stopped.
    ///
    /// A stop asked for while the job is still waiting its turn used to sit
    /// until the writer reached it and then be cleared, so the page said
    /// "stopping…" for as long as the queue took. Nothing was embedded, so
    /// there is nothing to undo and the answer is immediate.
    /// Take this store's waiting index jobs off its queue, and say which runs
    /// they were.
    ///
    /// The ids are the point. A run cancelled here has already been admitted —
    /// it is on the store's queue precisely because the admission queue let it
    /// through — so its place among the runs that may be going at once has to
    /// be given back, or the daemon loses a slot every time someone stops a
    /// run before its writer reaches it. Three stops left `runs at once` at
    /// three with nothing running and everything queued behind them forever.
    pub fn cancel_queued(&self) -> Vec<u64> {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let mut dropped = Vec::new();
        queue.retain(|queued| {
            if !matches!(queued.job, Job::Index(..)) {
                return true;
            }
            let answer = serde_json::json!({
                "event": "done",
                "indexed": 0,
                "unchanged": 0,
                "skipped": 0,
                "removed": 0,
                "chunks": 0,
                "images": 0,
                "remaining": 0,
                "stopped": true,
            });
            let _ = queued.report.send(answer);
            dropped.push(queued.run);
            false
        });
        drop(queue);
        // Folded into each run's own record, so a page that polls the snapshot
        // rather than reading the stream sees the run end too.
        for run in &dropped {
            self.record(
                *run,
                &serde_json::json!({
                    "event": "done",
                    "indexed": 0,
                    "unchanged": 0,
                    "skipped": 0,
                    "removed": 0,
                    "chunks": 0,
                    "images": 0,
                    "remaining": 0,
                    "stopped": true,
                }),
            );
        }
        dropped
    }
}

/// A run waiting for its store's writer, and the channel its progress goes
/// back on once it is admitted.
struct Pending {
    run: u64,
    store: Arc<Store>,
    paths: Vec<PathBuf>,
    report: mpsc::Sender<serde_json::Value>,
}

/// The one queue in front of every store's writer.
///
/// Each store's writer takes the next job on its own queue with nothing above
/// it, so with N stores open that is N runs at once regardless of what the
/// machine can carry — which is exactly what indexing several folders at once
/// would have produced. A run is admitted to its writer only while fewer than
/// `limit` are running; past that it waits here, in one daemon-wide FIFO
/// ordered by submission.
///
/// The watcher's own re-embeds do not pass through here. They are small, they
/// already interleave with runs through slices, and holding a file save behind
/// eleven queued repositories would make the watcher useless exactly when the
/// machine is busy.
pub struct Admission {
    queue: Mutex<VecDeque<Pending>>,
    /// Run ids admitted and not yet finished. A count would not do: a finish
    /// arriving for a run that was already removed would decrement the next
    /// run's place in the world.
    running: Mutex<Vec<u64>>,
    limit: AtomicUsize,
    next: AtomicU64,
}

impl Admission {
    pub fn new(limit: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            running: Mutex::new(Vec::new()),
            limit: AtomicUsize::new(limit.max(1)),
            next: AtomicU64::new(1),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Relaxed)
    }

    /// Change how many runs may be on at once.
    ///
    /// Lowering it admits nothing new and stops nothing that is already going:
    /// a run holds the writer and undoing it would cost the work it has done,
    /// so the running ones finish and the queue simply waits longer. Raising it
    /// admits the head immediately, which is what makes the field feel like a
    /// control rather than a preference.
    pub fn set_limit(&self, limit: usize) {
        self.limit.store(limit.max(1), Ordering::Relaxed);
        self.pump();
    }

    pub fn running(&self) -> usize {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Queue a run and hand back its id and the channel it reports on.
    ///
    /// The id is minted here rather than by a caller, so two routes submitting
    /// at the same moment cannot agree on one.
    pub fn submit(
        &self,
        store: &Arc<Store>,
        paths: Vec<PathBuf>,
    ) -> (u64, mpsc::Receiver<serde_json::Value>) {
        let run = self.next.fetch_add(1, Ordering::Relaxed);
        let (report, progress) = mpsc::channel();
        store.begin_run(run, paths.clone());
        {
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            store.record(
                run,
                &serde_json::json!({
                    "event": "submitted",
                    "run": run,
                    "ahead": queue.len(),
                }),
            );
            queue.push_back(Pending {
                run,
                store: Arc::clone(store),
                paths,
                report,
            });
        }
        self.pump();
        (run, progress)
    }

    /// Admit as many waiting runs as the limit allows.
    ///
    /// Called on every submission, every finish and every change of the limit,
    /// which is the whole of the queue's movement: there is no timer, so a
    /// queue that is not moving is one where nothing has finished.
    pub fn pump(&self) {
        loop {
            let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
            if running.len() >= self.limit() {
                return;
            }
            let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
            let Some(next) = queue.pop_front() else {
                return;
            };
            running.push(next.run);
            drop(queue);
            drop(running);
            // A store that was stopped keeps no memory of it into the next
            // run. `cancelled` is the store's flag rather than the run's, and
            // nothing used to clear it, so a Stop pressed on one run killed
            // the next one at 0% — with the file fetched, written to disk and
            // never indexed, and no error anywhere.
            next.store.cancelled.store(false, Ordering::Relaxed);
            next.store.paused.store(false, Ordering::Relaxed);
            next.store.submit_index(
                next.run,
                next.paths,
                next.report,
                serde_json::json!({ "event": "queued" }),
            );
        }
    }

    /// Release a run's place and admit whatever was waiting behind it.
    pub fn finish(&self, run: u64) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id| *id != run);
        self.pump();
    }

    /// Where each waiting run stands, oldest first.
    pub fn waiting(&self) -> Vec<(u64, String, Vec<PathBuf>)> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|p| (p.run, p.store.name.clone(), p.paths.clone()))
            .collect()
    }

    /// This store's place in the queue, counted from 1, or `None` when it is
    /// not waiting.
    pub fn position_of(&self, store: &str) -> Option<usize> {
        self.queue
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .position(|p| p.store.name == store)
            .map(|at| at + 1)
    }

    /// Take a waiting run out of the queue.
    ///
    /// Answered at once and with nothing undone, because nothing of it was
    /// embedded — which is the whole difference between removing a queued
    /// folder and stopping a running one. The runs behind it move up by having
    /// been behind it; there is no numbering to rewrite.
    pub fn dequeue(&self, store: &str) -> usize {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let mut dropped = 0;
        queue.retain(|pending| {
            if pending.store.name != store {
                return true;
            }
            let answer = serde_json::json!({
                "event": "done",
                "indexed": 0,
                "unchanged": 0,
                "skipped": 0,
                "removed": 0,
                "chunks": 0,
                "images": 0,
                "remaining": 0,
                "stopped": true,
                "dequeued": true,
            });
            pending.store.record(pending.run, &answer);
            let _ = pending.report.send(answer);
            dropped += 1;
            false
        });
        dropped
    }
}

/// One monotonic counter per data domain the portal reads.
///
/// This is the whole of what makes the portal live. A page polls
/// `/api/changes` once a second, compares six integers with what it last saw,
/// and refetches only the domains that moved — so a quiet daemon with a tab
/// open costs one small request a second and nothing else, and there is one
/// clock in the page rather than one per feature.
///
/// Process-global rather than a field of [`State`]: two of the six are bumped
/// by code that has no `State` in hand — the store's own event and log writers,
/// and the ledger's one record call — and threading a handle into them purely
/// to count would be a worse cost than a static. There is one daemon per
/// process by construction: it binds a port and holds every store's lock.
///
/// **A counter is bumped where its domain is written, once, and nowhere else.**
/// A second bump site is a second source of truth, and the failure it produces
/// is the worst one available — one page stale while the others are live, so
/// the portal is trusted everywhere and wrong in one place.
pub mod changes {
    use std::sync::atomic::{AtomicU64, Ordering};

    /// The domains a page can watch. Adding one means adding its bump site in
    /// the one function that writes it, and a row in the per-domain test.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Domain {
        Stores,
        Runs,
        Clients,
        Ledger,
        Events,
        Privacy,
    }

    pub const DOMAINS: [Domain; 6] = [
        Domain::Stores,
        Domain::Runs,
        Domain::Clients,
        Domain::Ledger,
        Domain::Events,
        Domain::Privacy,
    ];

    impl Domain {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Stores => "stores",
                Self::Runs => "runs",
                Self::Clients => "clients",
                Self::Ledger => "ledger",
                Self::Events => "events",
                Self::Privacy => "privacy",
            }
        }

        fn cell(self) -> &'static AtomicU64 {
            static STORES: AtomicU64 = AtomicU64::new(0);
            static RUNS: AtomicU64 = AtomicU64::new(0);
            static CLIENTS: AtomicU64 = AtomicU64::new(0);
            static LEDGER: AtomicU64 = AtomicU64::new(0);
            static EVENTS: AtomicU64 = AtomicU64::new(0);
            static PRIVACY: AtomicU64 = AtomicU64::new(0);
            match self {
                Self::Stores => &STORES,
                Self::Runs => &RUNS,
                Self::Clients => &CLIENTS,
                Self::Ledger => &LEDGER,
                Self::Events => &EVENTS,
                Self::Privacy => &PRIVACY,
            }
        }
    }

    /// Say that this domain was written.
    pub fn bump(domain: Domain) {
        domain.cell().fetch_add(1, Ordering::Relaxed);
    }

    /// Notice a registry that another process wrote.
    ///
    /// `semlith index ~/work/new` in a terminal writes the registry and this
    /// daemon opens that store on the next read of `/api/stores`, which is
    /// what 0.19.0's reconciliation is for. But the page only reads that route
    /// when the stores counter moves, and nothing *in this process* moved it —
    /// so the counter waited for the open and the open waited for the counter,
    /// and the row appeared on the next navigation rather than by itself.
    /// That circle is the whole of "my new repository is not in the list".
    ///
    /// One `stat` of the registry against the last one seen, on the poll that
    /// is already happening. Cheaper than reconciling per poll, which would
    /// take the registry lock and try to open every unopened store once a
    /// second; this only says "something changed", and the existing route does
    /// the work exactly once in response.
    pub fn notice_registry() {
        use std::sync::Mutex;
        use std::time::SystemTime;
        static SEEN: Mutex<Option<SystemTime>> = Mutex::new(None);

        let Ok(path) = crate::home::registry_path() else {
            return;
        };
        let Ok(at) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
            return;
        };
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        match *seen {
            // The first poll is a baseline, not a change: whatever the page
            // drew on load already covers the registry as it stands.
            None => *seen = Some(at),
            Some(was) if was != at => {
                *seen = Some(at);
                bump(Domain::Stores);
            }
            Some(_) => {}
        }
    }

    /// What each counter stands at.
    pub fn read(domain: Domain) -> u64 {
        domain.cell().load(Ordering::Relaxed)
    }
}

/// How many runs may be on at once, when the environment is what says so.
///
/// An explicit variable is an instruction from whoever started the process, so
/// it wins over the saved setting and the page cannot raise past it — the same
/// standing [`crate::embed::THREADS_ENV`] and [`crate::index::INDEX_MEMORY_ENV`]
/// already have over their own values.
pub const PARALLEL_ENV: &str = "SEMLITH_INDEX_PARALLEL";

/// Where a value in force came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Derived,
    Saved,
    Environment,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Self::Derived => "derived",
            Self::Saved => "saved",
            Self::Environment => "set by the environment",
        }
    }
}

/// One of the three settings: what is in force, where it came from, and what
/// this machine would have chosen, with the one-line reason it chose it.
///
/// The derivation travels with the value rather than being recomputed by
/// whoever draws it, so the page's warning about a field set above the derived
/// value is comparing against the number the daemon actually derived.
#[derive(Debug, Clone, Serialize)]
pub struct Limit {
    pub value: usize,
    pub source: Source,
    pub derived: usize,
    pub reason: String,
}

impl Limit {
    fn new(derived: crate::system::Derivation, saved: Option<usize>, variable: &str) -> Self {
        let from_env = std::env::var(variable)
            .ok()
            .and_then(|raw| raw.parse::<usize>().ok())
            .filter(|n| *n > 0);
        let (value, source) = match (from_env, saved.filter(|n| *n > 0)) {
            (Some(n), _) => (n, Source::Environment),
            (None, Some(n)) => (n, Source::Saved),
            (None, None) => (derived.value, Source::Derived),
        };
        Self {
            value,
            source,
            derived: derived.value,
            reason: derived.reason,
        }
    }
}

/// The three settings in force, and the machine they were derived from.
#[derive(Debug, Clone, Serialize)]
pub struct Limits {
    pub runs_at_once: Limit,
    pub embed_threads: Limit,
    pub index_memory_mb: Limit,
    /// The reading the three were derived from, as the page shows it.
    ///
    /// Carried as JSON rather than by deriving `Serialize` on the reading
    /// itself: `system` answers a question about this machine and owes nothing
    /// to a wire format, and the one place that puts it on a wire is here.
    pub machine: serde_json::Value,
}

impl Limits {
    /// Read the machine, derive the three defaults, and let the saved settings
    /// and then the environment override them.
    ///
    /// Re-read rather than cached, because one of the numbers behind the
    /// derivation is how much memory is free *now* — a figure taken at start
    /// and shown an hour later is a figure about a machine that has since done
    /// other things.
    pub fn in_force() -> Self {
        let machine = crate::system::read();
        let derived = crate::system::derive(&machine, crate::system::PER_RUN_PEAK_MB);
        let saved = home::Settings::load();
        let runs_at_once = Limit::new(derived.runs_at_once, saved.runs_at_once, PARALLEL_ENV);
        // Derived from the runs actually in force, not from the runs this
        // machine would have chosen. The two differ exactly when someone has
        // changed the setting, which is the moment the sentence under the
        // field is being read.
        let threads_derived = crate::system::threads_for(&machine, runs_at_once.value);
        Self {
            runs_at_once,
            embed_threads: Limit::new(
                threads_derived,
                saved.embed_threads,
                crate::embed::THREADS_ENV,
            ),
            index_memory_mb: Limit::new(
                derived.index_memory_mb,
                saved.index_memory_mb,
                crate::index::INDEX_MEMORY_ENV,
            ),
            machine: serde_json::json!({
                "logical_cores": machine.logical_cores,
                "physical_cores": machine.physical_cores,
                "total_memory_mb": machine.total_memory_mb,
                "available_memory_mb": machine.available_memory_mb,
                "reserve_mb": crate::system::RESERVE_MB,
                "per_run_peak_mb": crate::system::PER_RUN_PEAK_MB,
            }),
        }
    }

    /// The line `semlith start` prints: all three values with their source.
    pub fn line(&self) -> String {
        format!(
            "indexing: {} run(s) at once ({}), {} embedder thread(s) each ({}), {} MiB of vectors per store ({})",
            self.runs_at_once.value,
            self.runs_at_once.source.as_str(),
            self.embed_threads.value,
            self.embed_threads.source.as_str(),
            self.index_memory_mb.value,
            self.index_memory_mb.source.as_str(),
        )
    }
}

/// What every route is handed.
pub struct State {
    pub server: Arc<Server>,
    /// One reader over every open store.
    ///
    /// ponytail: one lock for every read route. A search is milliseconds and
    /// there is one browser, so contention is theoretical; if it ever is not,
    /// the upgrade is a reader per store rather than a shared `Fleet`.
    pub fleet: Mutex<Option<Fleet>>,
    /// The open stores.
    ///
    /// Behind a lock because the set is no longer fixed at startup: indexing a
    /// folder on a machine with no store yet has to be able to make one and
    /// serve it in the same breath, without the developer restarting the
    /// daemon they only just started. Readers take a snapshot rather than hold
    /// the guard, so a slow search never blocks a store being opened.
    stores: RwLock<Vec<Arc<Store>>>,
    /// The one queue in front of every store's writer, and the only thing that
    /// decides how many runs are on at once.
    pub admission: Arc<Admission>,
    pub airgap: bool,
    pub started: SystemTime,
    /// How long a watcher waits for a file to stop changing. Kept so a store
    /// opened later gets the same debounce as the ones opened at startup.
    debounce: Duration,
    /// The daemon's log line, so a store opened at runtime reports itself the
    /// way the startup ones do.
    report: Arc<dyn Fn(&str) + Send + Sync>,
    /// Refusals by class, for the line the daemon logs on shutdown.
    pub refusals: Mutex<BTreeMap<&'static str, u64>>,
    /// `semlith mcp` processes forwarding here: pid to the unix second it was
    /// last heard from. A proxy has no disconnect to observe — its client may
    /// simply stop asking — so recency is the only honest answer to "how many
    /// are connected".
    pub proxies: Mutex<BTreeMap<u32, u64>>,
    /// What each connected MCP client said it was, keyed by session.
    ///
    /// Held here and lost when the daemon exits, which is the honest lifetime:
    /// there is no disconnect to observe over HTTP either, so a client is
    /// "connected" for as long as it has been heard from recently.
    pub clients: Mutex<BTreeMap<String, Client>>,
    /// A second reader, for forwarded MCP calls, opened on first use.
    ///
    /// Separate from `fleet` on purpose: a forwarded `semlith_index` blocks its
    /// caller until the writer has run it, and sharing one lock would mean an
    /// agent's index freezing the portal for as long as the slice lasts. A
    /// reader that is never used costs a SQLite handle and no vectors.
    pub mcp_fleet: Mutex<Option<Fleet>>,
    /// Whether retrievals are recorded into each store's `retrievals` table.
    ///
    /// On unless `--no-ledger` or `SEMLITH_LEDGER=0` says otherwise, which
    /// reverses what 0.12.0 to 0.14.0 did. The principle those releases stated
    /// was that a local tool which starts logging without being told to is not
    /// different from one that phones home; the principle that actually holds
    /// is narrower and checkable:
    ///
    /// - the rows never leave the store they were written into, which is still
    ///   provable with a packet capture;
    /// - the daemon says on every start that it is recording, and names the
    ///   flag that stops it;
    /// - erasing every row is one `DELETE`.
    ///
    /// What the old default cost was the thing the record is for. A ledger
    /// nobody switched on measured nobody, so the savings figure had no
    /// denominator and the audit trail had no rows — and the one client whose
    /// retrievals it did record was the portal's own search box, which is not
    /// who the product is for.
    pub ledger: bool,
}

/// How recently a proxy must have called to count as connected.
const PROXY_FRESH: u64 = 120;

/// Each open store's directory beside the name the daemon knows it by.
///
/// The fleet derives a label from the store directory's basename, which is the
/// registered name only for a store living in the store home. Everywhere else
/// the two differ, and the portal — which draws its chips from `/api/stores` —
/// ended up naming stores the search route said were not open.
fn named(stores: &[Arc<Store>]) -> Vec<(PathBuf, String)> {
    stores
        .iter()
        .map(|s| (s.dir.clone(), s.name.clone()))
        .collect()
}

/// One MCP client, as the Agents page shows it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Client {
    /// What the client called itself in `initialize`, or the transport when it
    /// never said. Invented names are worse than an honest "unnamed client".
    pub name: String,
    pub transport: String,
    /// The protocol revision that was negotiated, as the client asked for it.
    pub revision: String,
    /// How many tool calls it has made through this daemon.
    pub queries: u64,
    pub seen: u64,
}

/// A registered store the daemon could not open, and why.
///
/// Reported on the Stores page rather than swallowed: a store that is in the
/// registry and not in the list is a store the user will look for and not
/// find, and "another process is writing it" and "it is not there any more"
/// are two different things to do about it.
#[derive(Debug, Clone)]
pub struct Unopened {
    pub name: String,
    pub dir: PathBuf,
    pub why: String,
    /// Whether nothing is at that path at all, as against a store that is
    /// there and could not be opened.
    ///
    /// The portal draws the two differently and must: a store another process
    /// is writing comes back by itself on the next read, and a registry entry
    /// for a directory that does not exist never will. Offering "add to
    /// alpha" for the second is the same mechanism that put 262 of one store's
    /// files into another.
    pub missing: bool,
}

impl State {
    /// Every open store, as a snapshot.
    ///
    /// Cloned out rather than handed back under the guard: an `Arc` clone is a
    /// counter bump, and holding a read lock across a search would stop a new
    /// store being opened for as long as the search took.
    pub fn stores(&self) -> Vec<Arc<Store>> {
        self.stores.read().expect("the stores lock").clone()
    }

    /// A store by name, opening it from the registry if this daemon has not
    /// got it open yet.
    ///
    /// The miss is what triggers the reconciliation, not a timer: `semlith
    /// index ~/work/new-project` from a second process writes a store this
    /// daemon has never heard of, and until 0.19.0 every agent asking for it
    /// by name got "no store called new-project is open" until the daemon was
    /// restarted.
    pub fn store(self: &Arc<Self>, name: &str) -> Option<Arc<Store>> {
        if let Some(open) = self.stores().into_iter().find(|s| s.name == name) {
            return Some(open);
        }
        self.reconcile();
        self.stores().into_iter().find(|s| s.name == name)
    }

    /// Open every registered store this daemon does not have open, and report
    /// the ones it could not.
    ///
    /// Called on a by-name miss and on every `/api/stores` read, which is the
    /// cheap place to notice: both are already the slow path, and neither runs
    /// in the indexing loop.
    ///
    /// A directory another process holds the lock on is left alone and
    /// reported as being written, never forced: `open_store` takes the lock
    /// first, so the daemon cannot become a second writer of one store. The
    /// next read tries again, which is what makes a `semlith index` that is
    /// still running appear by itself when it finishes.
    pub fn reconcile(self: &Arc<Self>) -> Vec<Unopened> {
        let Ok(registry) = Registry::load() else {
            return Vec::new();
        };
        let open: Vec<PathBuf> = self.stores().iter().map(|s| s.dir.clone()).collect();
        let mut out = Vec::new();
        for name in registry.stores.keys() {
            let Ok(dir) = Registry::dir_of(name) else {
                continue;
            };
            let dir = crate::canonical(&dir);
            if open.contains(&dir) {
                continue;
            }
            if !dir.exists() {
                out.push(Unopened {
                    name: name.clone(),
                    dir,
                    why: "registered, but there is nothing at that path".to_string(),
                    missing: true,
                });
                continue;
            }
            if let Err(e) = self.open_store(&dir, false) {
                out.push(Unopened {
                    name: name.clone(),
                    dir,
                    why: format!("{e:#}"),
                    missing: false,
                });
            }
        }
        out
    }

    /// The one store a write means, or an error naming the alternatives.
    pub fn writable(self: &Arc<Self>, name: Option<&str>) -> Result<Arc<Store>> {
        // A write naming a store this daemon has not opened is the same miss
        // as a read naming one, and gets the same reconciliation.
        if let Some(name) = name
            && let Some(open) = self.store(name)
        {
            return Ok(open);
        }
        if name.is_none() && self.stores().is_empty() {
            self.reconcile();
        }
        let stores = self.stores();
        match (name, stores.as_slice()) {
            (Some(name), _) => self
                .store(name)
                .with_context(|| format!("no store called {name} is open")),
            (None, [one]) => Ok(Arc::clone(one)),
            (None, []) => bail!("no store is open"),
            (None, many) => bail!(
                "this daemon has {} stores open, so a write has to name one: {}",
                many.len(),
                many.iter()
                    .map(|s| s.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    /// Queue an index run, and hand back its id and the channel it reports on.
    ///
    /// The run goes to the daemon-wide admission queue rather than straight to
    /// the store's writer, so how many are on at once is a property of the
    /// machine rather than of how many stores happen to be open. A caller that
    /// wants the events reads the receiver; a caller that only wanted the run
    /// started drops it, and the run carries on — its progress is in the
    /// store's snapshot either way.
    pub fn index(
        &self,
        store: &Arc<Store>,
        paths: Vec<PathBuf>,
    ) -> Result<(u64, mpsc::Receiver<serde_json::Value>)> {
        Self::writer_alive(store)?;
        Ok(self.admission.submit(store, paths))
    }

    pub fn forget(
        &self,
        store: &Arc<Store>,
        path: PathBuf,
    ) -> Result<mpsc::Receiver<serde_json::Value>> {
        Self::writer_alive(store)?;
        // No notice: this caller takes the first message as the answer.
        Ok(store.submit(Job::Forget(path), None))
    }

    /// Close a store and delete everything it holds.
    ///
    /// The order is what makes this safe while the daemon runs: the watcher is
    /// told to stop and its lock is released when the thread returns, the
    /// store leaves the served set so no request can reach it, the readers are
    /// dropped so no SQLite handle is still open on the files, and only then
    /// is the directory removed and the registry entry dropped.
    ///
    /// The indexed files themselves are not touched — this deletes what
    /// semlith derived from them.
    pub fn delete_store(self: &Arc<Self>, name: &str) -> Result<PathBuf> {
        let Some(store) = self.store(name) else {
            anyhow::bail!("this daemon is not serving a store called {name}");
        };

        store.stop.store(true, Ordering::SeqCst);
        // The watcher checks the flag between filesystem events, so this is a
        // wait of one debounce, not of one filesystem event.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while store.watching.load(Ordering::Relaxed) {
            if std::time::Instant::now() > deadline {
                anyhow::bail!(
                    "{name}'s watcher did not stop, so its lock is still held;                      nothing was deleted"
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        self.stores
            .write()
            .expect("the stores lock")
            .retain(|s| s.name != name);
        // Before the files go: a reader holding this store open would keep its
        // SQLite handles alive, and on Windows an open handle refuses the
        // delete outright.
        self.reopen_readers();
        Discovery::remove(&store.dir);

        changes::bump(changes::Domain::Stores);
        crate::home::delete_store(name)
    }

    /// Refuse to queue work for a store whose writer is gone.
    ///
    /// The queue is drained by the watcher thread and by nothing else, so a
    /// job submitted after that thread has returned would never be performed
    /// and never be answered — its caller would hold an HTTP worker open until
    /// the daemon stopped. A watcher only returns on a backend failure it has
    /// already reported, so this is a real condition, not a theoretical one.
    fn writer_alive(store: &Arc<Store>) -> Result<()> {
        if store.watching.load(Ordering::Relaxed) {
            return Ok(());
        }
        bail!(
            "the writer for {} is not running, so it cannot be written to; \
             the daemon's stderr says why, and restarting it is the fix",
            store.name
        )
    }

    /// Rotate the token and tell every discovery file about it, so a
    /// forwarding `semlith mcp` is not cut off by the Privacy page's button.
    pub fn rotate(&self) -> String {
        let fresh = self.server.rotate();
        for store in self.stores() {
            let _ = discovery(self.server.port(), &fresh).write(&store.dir);
        }
        fresh
    }

    /// Record what a client said about itself, and count its tool calls.
    ///
    /// `session` is how one client is told from another: over HTTP it is the
    /// `Mcp-Session-Id` this daemon hands out at `initialize`, and for a
    /// forwarding `semlith mcp` it is the proxy's pid. A client that echoes
    /// neither is counted as one unnamed client per transport rather than as a
    /// new one on every request.
    /// The name a session gave itself when it initialized.
    ///
    /// The MCP handshake carries `clientInfo` once, and every later request on
    /// that session carries none — so the name the ledger records has to come
    /// from what was noted at the handshake rather than from the request in
    /// hand.
    pub fn client_name(&self, session: &str, transport: &str) -> Option<String> {
        let clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        clients
            .get(&format!("{transport}:{session}"))
            .map(|client| client.name.clone())
            .filter(|name| name != "unnamed client")
    }

    pub fn note_client(
        &self,
        session: &str,
        transport: &str,
        name: Option<&str>,
        revision: Option<&str>,
        query: bool,
    ) {
        let now = now();
        let mut clients = self.clients.lock().unwrap_or_else(|e| e.into_inner());
        let entry = clients
            .entry(format!("{transport}:{session}"))
            .or_insert_with(|| Client {
                name: name.unwrap_or("unnamed client").to_string(),
                transport: transport.to_string(),
                revision: revision.unwrap_or("—").to_string(),
                queries: 0,
                seen: now,
            });
        if let Some(name) = name {
            entry.name = name.to_string();
        }
        if let Some(revision) = revision {
            entry.revision = revision.to_string();
        }
        if query {
            entry.queries += 1;
        }
        entry.seen = now;
        clients.retain(|_, client| now.saturating_sub(client.seen) <= PROXY_FRESH);
        changes::bump(changes::Domain::Clients);
    }

    /// Every client heard from recently.
    pub fn clients(&self) -> Vec<Client> {
        let now = now();
        self.clients
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|client| now.saturating_sub(client.seen) <= PROXY_FRESH)
            .cloned()
            .collect()
    }

    /// Note that a forwarding `semlith mcp` is alive.
    pub fn saw_proxy(&self, pid: u32) {
        let mut proxies = self.proxies.lock().unwrap_or_else(|e| e.into_inner());
        let now = now();
        proxies.insert(pid, now);
        proxies.retain(|_, seen| now.saturating_sub(*seen) <= PROXY_FRESH);
    }

    /// How many `semlith mcp` processes are currently forwarding here.
    pub fn proxy_count(&self) -> usize {
        let now = now();
        self.proxies
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|seen| now.saturating_sub(**seen) <= PROXY_FRESH)
            .count()
    }

    /// The reader forwarded MCP calls answer from, opened on first use.
    pub fn open_mcp_fleet(&self) -> Result<()> {
        let mut fleet = self.mcp_fleet.lock().unwrap_or_else(|e| e.into_inner());
        if fleet.is_some() {
            return Ok(());
        }
        let open = self.stores();
        let dirs: Vec<PathBuf> = open.iter().map(|s| s.dir.clone()).collect();
        if dirs.is_empty() {
            bail!("this daemon has no store open");
        }
        let mut opened = Fleet::open(&dirs)?;
        // An agent scoping a tool call with `store:` names it the way the
        // portal and `/api/stores` do. See `open_fleet`.
        opened.name_from(&named(&open));
        opened.quiet = true;
        *fleet = Some(opened);
        Ok(())
    }

    /// Open a store that was not open when the daemon started, and serve it
    /// immediately.
    ///
    /// This is what makes the portal's first run work. A machine with nothing
    /// indexed starts the daemon with no stores, and the Index page then has
    /// nothing to write to — so indexing a folder there has to be able to
    /// create the store, take its lock, start watching it and put it in front
    /// of the reader without the developer restarting anything.
    ///
    /// The order is the same one `run` uses and matters for the same reason:
    /// the lock is taken before the store is announced, so nothing is offered
    /// that another process might already own.
    pub fn open_store(self: &Arc<Self>, dir: &Path, expect_run: bool) -> Result<Arc<Store>> {
        let dir = crate::canonical(dir);
        if let Some(open) = self.stores().into_iter().find(|s| s.dir == dir) {
            return Ok(open);
        }

        let lock = StoreLock::acquire(&dir)
            .with_context(|| format!("{} cannot be opened by the daemon", dir.display()))?;

        let registry = Registry::load()?;
        let (name, roots) = roots_for(&dir, &registry);
        let watched: Vec<PathBuf> = roots.iter().filter(|r| r.exists()).cloned().collect();

        let store = Arc::new(Store {
            name: name.clone(),
            dir: dir.clone(),
            roots,
            watched,
            queue: Mutex::new(VecDeque::new()),
            events: Mutex::new(VecDeque::new()),
            runs: Mutex::new(Vec::new()),
            watching: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            last_write: AtomicUsize::new(0),
            pruned: AtomicUsize::new(0),
            // Set before the watcher thread exists, which is the point: a flag
            // set afterwards is a flag the catch-up may already have run past.
            expecting_run_until: AtomicUsize::new(if expect_run {
                now() as usize + EXPECTED_RUN_GRACE_SECS
            } else {
                0
            }),
        });

        discovery(self.server.port(), &self.server.token()).write(&store.dir)?;
        self.stores
            .write()
            .expect("the stores lock")
            .push(Arc::clone(&store));

        // Same shape as the startup watchers: the lock moves into the thread so
        // its life is the thread's life, which is what makes "the daemon is the
        // writer" true rather than intended.
        let watching = Arc::clone(&store);
        let debounce = self.debounce;
        let report = Arc::clone(&self.report);
        let admission = Arc::clone(&self.admission);
        std::thread::spawn(move || {
            let _lock = lock;
            if let Err(e) = tend(&watching, debounce, &watching.stop, &*report, &admission) {
                report(&format!("{}: watcher stopped: {e}", watching.name));
                watching.note(format!("watcher stopped: {e}"));
            }
            watching.watching.store(false, Ordering::Relaxed);
        });

        (self.report)(&format!(
            "opened {name} at {} — now serving it",
            dir.display()
        ));
        self.reopen_readers();
        changes::bump(changes::Domain::Stores);
        Ok(store)
    }

    /// Throw away the readers so the next request builds one that knows about
    /// every store, including any opened since.
    ///
    /// Dropped rather than rebuilt here: rebuilding loads the embedding model,
    /// and doing that while holding the lock would stall whichever request
    /// happened to be next. The reader is opened on demand anyway.
    fn reopen_readers(&self) {
        *self.fleet.lock().unwrap_or_else(|e| e.into_inner()) = None;
        *self.mcp_fleet.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// The reader every read route answers from, opened on first use and
    /// reopened after a store joins.
    pub fn open_fleet(&self) -> Result<()> {
        let mut fleet = self.fleet.lock().unwrap_or_else(|e| e.into_inner());
        if fleet.is_some() {
            return Ok(());
        }
        let open = self.stores();
        // A store whose directory has gone since the daemon opened it — it was
        // deleted by another process, or the volume it sat on was unmounted —
        // is left out rather than taken as a reason to answer nothing. It used
        // to fail the whole call, so one missing directory made `/api/stores`
        // answer 500 for every store on the machine and the portal went blank.
        let open: Vec<Arc<Store>> = open
            .into_iter()
            .filter(|s| s.dir.join("store.db").exists())
            .collect();
        let dirs: Vec<PathBuf> = open.iter().map(|s| s.dir.clone()).collect();
        if dirs.is_empty() {
            return Ok(());
        }
        let mut opened = Fleet::open(&dirs)?;
        // The daemon already knows what each store is called — it is what
        // `/api/stores` reports and what the portal's chips are made of. Handing
        // those names to the fleet is what stops the two from disagreeing, and
        // a chip from naming a store the search route says is not open.
        opened.name_from(&named(&open));
        opened.quiet = true;
        *fleet = Some(opened);
        Ok(())
    }

    pub fn refuse(&self, class: Refusal) {
        *self
            .refusals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(class.as_str())
            .or_insert(0) += 1;
    }
}

fn discovery(port: u16, token: &str) -> Discovery {
    Discovery {
        pid: std::process::id(),
        port,
        token: token.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

/// The stores `semlith start` should open.
///
/// With no arguments: every registered store. With paths: whatever each path
/// resolves to, so `semlith start ~/work/api` opens the api store whether it
/// lives in the home or beside the corpus.
pub fn stores_to_open(flags: &[PathBuf], paths: &[PathBuf], cwd: &Path) -> Result<Vec<PathBuf>> {
    if !flags.is_empty() {
        return Ok(flags.to_vec());
    }
    if paths.is_empty() {
        return home::all_dirs(&[], cwd);
    }
    let mut out = Vec::new();
    for path in paths {
        let dir = home::resolve(&[], path, None)?.one()?;
        if !dir.join("store.db").exists() {
            bail!(
                "{} has no store yet — run `semlith index {}` first",
                path.display(),
                path.display()
            );
        }
        if !out.contains(&dir) {
            out.push(dir);
        }
    }
    Ok(out)
}

/// What a store watches, and what it should be watching but cannot find.
fn roots_for(dir: &Path, registry: &Registry) -> (String, Vec<PathBuf>) {
    if let Some(name) = registry.name_of(dir) {
        let roots = registry.stores[name].roots.clone();
        return (name.to_string(), roots);
    }
    // Not in the registry: a `.semlith` beside its corpus, or a `--store`
    // path. The directory holding it is the corpus, which is the same rule
    // `adopt` uses.
    let name = dir
        .parent()
        .and_then(Path::file_name)
        .or_else(|| dir.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "store".to_string());
    let root = dir
        .parent()
        .map(crate::canonical)
        .unwrap_or_else(|| dir.to_path_buf());
    (name, vec![root])
}

/// Run until a signal stops it.
///
/// `report` is the daemon's stderr: the bound port, each store opened, each
/// re-embed, and the refusal counts on the way out. Never the token — that
/// appears in the URL and in the discovery file, and nowhere else.
#[allow(clippy::too_many_arguments)]
pub fn run(
    dirs: &[PathBuf],
    port: u16,
    debounce: Duration,
    airgap: bool,
    ledger: bool,
    mcp_http: bool,
    report: impl Fn(&str) + Send + Sync + 'static,
) -> Result<Arc<State>> {
    let registry = Registry::load()?;

    // Named, not silently passed over. A registry entry whose directory is not
    // on disk was skipped without a word, so a machine with three of them
    // started clean and offered all three everywhere a real store is offered.
    for name in registry.stores.keys() {
        let Ok(dir) = Registry::dir_of(name) else {
            continue;
        };
        if dir.join("store.db").exists() {
            continue;
        }
        report(&format!(
            "skipped {name}: registered, but there is no store at {}",
            dir.display()
        ));
    }

    // Locks first, and all of them, before anything is watched or served: a
    // daemon that took three of four locks and then failed would leave three
    // stores unusable to the `semlith index` that is about to be tried.
    let mut locks = Vec::new();
    let mut opening = Vec::new();
    for dir in dirs {
        let lock = StoreLock::acquire(dir)
            .with_context(|| format!("{} cannot be opened by the daemon", dir.display()))?;
        let (name, roots) = roots_for(dir, &registry);
        // Canonical, as `open_store` records it, because `reconcile` compares
        // what it computes from the registry against what is already open. A
        // home reached through a symlink — `/tmp` on macOS is one — gave those
        // two different spellings of one directory, so every store opened at
        // startup was reported as registered-but-not-open as well, and the
        // Stores page listed each of them twice.
        opening.push((name, crate::canonical(dir), roots));
        locks.push(lock);
    }

    // The port is the other thing worth failing on before any work: a daemon
    // that indexed for a minute and then could not listen has wasted the
    // minute.
    let server = Arc::new(Server::bind(port)?);
    report(&format!("listening on 127.0.0.1:{}", server.port()));

    // Said on every start, in both states, before anything is recorded.
    //
    // This is the whole of what makes default-on recording honest rather than
    // a surprise: the person who started the daemon is told, in the same
    // breath as the port, that it keeps a record and how to stop it. A default
    // nobody is told about is the thing 0.12.0 was right to refuse.
    report(if ledger {
        "ledger: recording (local only; --no-ledger to stop)"
    } else {
        "ledger: off for this session"
    });

    // The agent key is read, or written if this machine has none. It survives
    // restarts and upgrades on purpose: a client's configuration is written
    // once and has to keep working, which the per-run session token can never
    // do.
    let key = crate::home::agent_key()?;
    server.set_agent_key(&key);
    server.set_mcp_open(mcp_http);
    if mcp_http {
        report(&format!(
            "MCP over HTTP at http://127.0.0.1:{}{}",
            server.port(),
            crate::http::MCP_PATH
        ));
    } else {
        report("MCP over HTTP is closed (--no-mcp-http); `semlith mcp` over stdio is unaffected");
    }

    let mut stores = Vec::new();
    for (name, dir, roots) in opening {
        let watched: Vec<PathBuf> = roots.iter().filter(|r| r.exists()).cloned().collect();
        for missing in roots.iter().filter(|r| !r.exists()) {
            // Never fatal. A corpus on an unmounted disk is a Monday morning,
            // not a corruption, and the store's vectors are still searchable.
            report(&format!(
                "{name}: root {} is not there; not watching it",
                missing.display()
            ));
        }
        report(&format!(
            "opened {name} at {} — watching {} root(s)",
            dir.display(),
            watched.len()
        ));
        stores.push(Arc::new(Store {
            name,
            dir,
            roots,
            watched,
            queue: Mutex::new(VecDeque::new()),
            events: Mutex::new(VecDeque::new()),
            runs: Mutex::new(Vec::new()),
            watching: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            last_write: AtomicUsize::new(0),
            pruned: AtomicUsize::new(0),
            expecting_run_until: AtomicUsize::new(0),
        }));
    }

    let fleet = if dirs.is_empty() {
        None
    } else {
        let mut fleet = Fleet::open(dirs)?;
        // The fleet every route answers from, so this is the one that has to
        // agree with `/api/stores` about what each store is called.
        fleet.name_from(&named(&stores));
        // stderr is this process's log, and a model download progress bar in
        // the middle of it is noise; the Stores view reports readiness.
        fleet.quiet = true;
        Some(fleet)
    };

    let token = server.token();
    for store in &stores {
        discovery(server.port(), &token).write(&store.dir)?;
    }

    let report_line: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(report);

    // Read before the first run can be submitted, and printed below with where
    // each value came from: a machine that derived three when the user set one
    // should say so rather than leave them to infer it from how fast the queue
    // moves.
    let limits = Limits::in_force();
    report_line(&limits.line());

    let state = Arc::new(State {
        server: Arc::clone(&server),
        fleet: Mutex::new(fleet),
        stores: RwLock::new(stores),
        admission: Arc::new(Admission::new(limits.runs_at_once.value)),
        airgap,
        started: SystemTime::now(),
        debounce,
        report: Arc::clone(&report_line),
        refusals: Mutex::new(BTreeMap::new()),
        proxies: Mutex::new(BTreeMap::new()),
        clients: Mutex::new(BTreeMap::new()),
        mcp_fleet: Mutex::new(None),
        ledger,
    });

    // Installed before the first thread starts: the signal is how this process
    // ends, so the ordinary exit has to be the safe one.
    watch::stop_on_signal();

    let report = Arc::clone(&report_line);
    let mut watchers = Vec::new();
    for (store, lock) in state.stores().into_iter().zip(locks) {
        let report = Arc::clone(&report);
        // Set here rather than inside the thread: the flag is what the write
        // queue checks before accepting a job, and opening the store takes
        // long enough that a request arriving in that window would otherwise
        // be told the writer was gone when it was merely starting.
        store.watching.store(true, Ordering::Relaxed);
        let admission = Arc::clone(&state.admission);
        watchers.push(std::thread::spawn(move || {
            // Moved in so the lock's life is the thread's life, which is what
            // makes "the daemon is the writer" true rather than intended.
            let _lock = lock;
            if let Err(e) = tend(&store, debounce, &store.stop, &*report, &admission) {
                report(&format!("{}: watcher stopped: {e}", store.name));
                store.note(format!("watcher stopped: {e}"));
            }
            store.watching.store(false, Ordering::Relaxed);
        }));
    }

    // After the watchers, so a note lands on a store whose feed is already
    // being kept, and before the URL, so it is on the page from the first read.
    report_dropped_queue(&state.stores(), &*report_line);

    // Behind the URL, not in front of it: loading the model costs a second or
    // two, and a developer staring at a blank terminal waiting for a link is
    // paying that cost twice. The first search would otherwise pay it instead,
    // which is worse — it looks like the search is slow.
    {
        let warming = Arc::clone(&state);
        let report = report_line.clone();
        std::thread::spawn(move || {
            let mut fleet = warming.fleet.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(fleet) = fleet.as_mut()
                && let Err(e) = fleet.warm()
            {
                report(&format!("could not load the embedding model: {e}"));
            }
        });
    }

    // stdout, not stderr: the token is in this URL, and stderr is the log the
    // operational criterion says never carries it. One line, once, so a
    // terminal's scrollback holds exactly one copy.
    println!("{}", server.url());
    report("open the URL on stdout to reach the portal; Ctrl-C stops the daemon");

    Ok(state.clone()).and_then(|state| {
        let handler = crate::routes::handler(Arc::clone(&state));
        let for_refusals = Arc::clone(&state);
        let result = server.serve(handler, &watch::STOP, move |class| {
            for_refusals.refuse(class);
        });

        // Shutdown, in the order the contract states: the watcher stops, the
        // index is checkpointed by the loop leaving cleanly, the lock is
        // released when its thread joins, and the discovery file goes last so
        // nothing is pointed at a daemon that is no longer answering.
        watch::STOP.store(true, Ordering::SeqCst);
        // Before the watchers are told to stop, while a run is still marked as
        // going: what this records is which folders the next start owes the
        // user an explanation for.
        remember_dropped_queue(&state.stores(), &state.admission.waiting());
        // Each watcher waits on its own store's flag now, so that a single
        // store can be closed and deleted without stopping the daemon. A
        // shutdown is every store at once.
        for store in state.stores() {
            store.stop.store(true, Ordering::SeqCst);
        }
        for watcher in watchers {
            let _ = watcher.join();
        }
        for store in state.stores() {
            Discovery::remove(&store.dir);
        }

        let refusals = state
            .refusals
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if !refusals.is_empty() {
            let counts: Vec<String> = refusals
                .iter()
                .map(|(class, n)| format!("{class} {n}"))
                .collect();
            report(&format!("refused requests: {}", counts.join(", ")));
        }
        report("stopped");
        result.map(|()| state)
    })
}

/// One store's watcher thread: the same loop `semlith watch` runs, plus the
/// queue the HTTP routes and the MCP proxy put work on.
fn tend(
    store: &Arc<Store>,
    debounce: Duration,
    stop: &AtomicBool,
    report: &(dyn Fn(&str) + Send + Sync),
    admission: &Arc<Admission>,
) -> Result<()> {
    let mut writer = Semlith::open(&store.dir, None)?;
    writer.quiet = true;

    // Before the watcher reads a single event: a store that holds files
    // outside every root it is registered against is a store whose search
    // results carry the wrong label, and whose rows compete with the store
    // that really owns those files. The old boundary rule let the whole home
    // directory into every store, so this is the one-time cost of tightening
    // it. The files on disk are untouched.
    match writer.prune_out_of_root(&store.roots) {
        Ok(0) => {}
        Ok(dropped) => {
            store.pruned.store(dropped, Ordering::Relaxed);
            let text =
                format!("reconciled — {dropped} file(s) dropped, held outside this store's roots");
            report(&format!("{}: {text}", store.name));
            store.note(text);
        }
        Err(e) => {
            report(&format!("{}: could not reconcile: {e}", store.name));
        }
    }

    // A store with no root on disk still gets a thread, because it still has a
    // queue: the portal can index a new path into it even though there is
    // nothing to watch yet.
    let roots = store.watched.clone();

    watch::run_held(
        &mut writer,
        &roots,
        debounce,
        stop,
        // A queued job is what the catch-up steps aside for — and a run
        // waiting for admission counts, though it is on no queue of this
        // store's yet.
        //
        // Without the second half, the admission queue bounds nothing. A run
        // that has not been admitted is invisible here, so this store's
        // watcher sees an idle store and indexes the whole root itself: three
        // folders submitted with runs-at-once at 1 became one admitted run and
        // two watcher catch-ups, all three indexing at once. The work was done
        // and the store was correct, which is why it took a measurement to
        // notice — the run that was finally admitted then reported nothing
        // indexed, because its watcher had already done it.
        &|| {
            store.queue_depth() > 0
                || admission.position_of(&store.name).is_some()
                // A store the portal has just made, whose run is on its way.
                || (now() as usize) < store.expecting_run_until.load(Ordering::Relaxed)
        },
        |progress| {
            use watch::Progress;
            match progress {
                Progress::Ready {
                    catch_up,
                    files,
                    chunks,
                } => {
                    let text = format!(
                        "watching — {files} files, {chunks} chunks \
                         ({} indexed at startup, {} unchanged)",
                        catch_up.indexed, catch_up.unchanged
                    );
                    report(&format!("{}: {text}", store.name));
                    store.note(text);
                }
                Progress::Batch(batch, elapsed) => {
                    let text = format!(
                        "{} re-embedded, {} removed, {} chunks in {:.1}s",
                        batch.indexed,
                        batch.removed,
                        batch.chunks,
                        elapsed.as_secs_f32()
                    );
                    report(&format!("{}: {text}", store.name));
                    store.note(text);
                    store.last_write.store(now() as usize, Ordering::Relaxed);
                }
                Progress::Error(e) => {
                    report(&format!("{}: watch error: {e}", store.name));
                    store.note(format!("error: {e}"));
                }
                Progress::Failed { path, why } => {
                    // A note on the store's feed, not a watch error: the
                    // thread is still running and `watching` is still true.
                    // One corrupt file used to take the whole watcher with it.
                    let text = format!(
                        "could not index {}: {why}",
                        crate::plain(&path.display().to_string())
                    );
                    report(&format!("{}: {text}", store.name));
                    store.note(text);
                }
                Progress::File(_) => {}
            }
        },
        |writer| {
            // The writer is this thread, so a queued job runs here or nowhere.
            loop {
                let Some(next) = store
                    .queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front()
                else {
                    return Ok(());
                };
                perform(store, writer, next, admission);
            }
        },
    )
}

/// Run one queued job, reporting progress back to whoever asked for it.
fn perform(store: &Arc<Store>, writer: &mut Semlith, queued: Queued, admission: &Arc<Admission>) {
    let Queued { run, job, report } = queued;
    // The run the watcher was holding off for has arrived, so the hold ends
    // here rather than when its grace period runs out.
    store.expecting_run_until.store(0, Ordering::Relaxed);
    let back = report.clone();
    let say = |value: serde_json::Value| {
        // The snapshot is written by the same closure that emits the event, so
        // there is one path and a page polling the snapshot cannot be shown
        // something the stream never carried, or miss something it did. This
        // is the whole of why a run can outlive the request that started it.
        store.record(run, &value);
        // A closed receiver means the browser navigated away mid-run. The work
        // still finishes — it is the store's, not the request's.
        let _ = report.send(value);
    };

    match job {
        Job::Index(Indexing {
            work,
            already,
            scanned: scanned_before,
            total: total_before,
            mut tally,
        }) => {
            // Only the first slice announces itself; the rest are the same run
            // continuing, and a second "started" would read as a second run.
            if let Work::Roots(roots) = &work {
                let names: Vec<String> = roots.iter().map(|p| p.display().to_string()).collect();
                say(serde_json::json!({ "event": "started", "paths": names }));
            }
            // Every queued job arrived through the portal or through a
            // forwarded `semlith_index`, so both are held to the boundary: this
            // store's registered roots and the home directory, and never a
            // credential by name. The watcher's own re-embeds are inside those
            // roots by construction.
            writer.boundary = crate::Boundary::within(crate::home::index_roots(&store.dir));
            // A pause belongs to the run that was on when it was asked for.
            // A stop does not need clearing here: a job that was queued when
            // one arrived has already been dropped from the queue, so reaching
            // this line means the flag is for this run.
            store.paused.store(false, Ordering::Relaxed);
            let control = {
                let store = Arc::clone(store);
                let told = std::sync::atomic::AtomicBool::new(false);
                move || {
                    if store.cancelled.load(Ordering::Relaxed) {
                        return crate::Flow::Stop;
                    }
                    if store.paused.load(Ordering::Relaxed) {
                        // Once per pause, not once per tick.
                        if !told.swap(true, Ordering::Relaxed) {
                            say(serde_json::json!({ "event": "paused" }));
                        }
                        return crate::Flow::Pause;
                    }
                    if told.swap(false, Ordering::Relaxed) {
                        say(serde_json::json!({ "event": "resumed" }));
                    }
                    crate::Flow::Run
                }
            };
            // The run's own progress, not the slice's. A continuation's
            // counters start at one because it is indexing a shorter list; the
            // page is drawing one run, so it is told where the run is.
            // What the run had committed before this slice. A slice's own
            // report starts at zero, so these are what turn its counters back
            // into the run's — the same correction `scanned` has had since
            // 0.20.0, applied to the three counters that were left behind.
            // Without it the chunk counter climbed to a flush boundary and
            // fell back to single figures, which reads as a run losing work.
            let indexed_before = tally.indexed;
            let chunks_before = tally.chunks;
            let symbols_before = tally.symbols;
            let on_file = |path: &Path, progress: crate::IndexProgress| {
                let scanned = scanned_before + progress.scanned as u64;
                say(serde_json::json!({
                    "event": "file",
                    // Plain, like every other path semlith hands out. The
                    // store keeps the verbatim form; a `\\?\C:\` prefix in
                    // an event is a path no editor opens and no shell
                    // completes.
                    "path": crate::plain(&path.display().to_string()),
                    // What is happening to this file, so a page can say
                    // "unchanged" rather than showing nothing at all.
                    "outcome": progress.outcome.as_str(),
                    // Why, for the outcomes that owe an explanation. A
                    // page showing "skipped" against two thousand files
                    // and nothing else is a page nobody can act on.
                    "why": progress.why,
                    "scanned": scanned,
                    // The walk's total, settled on the first slice. A later
                    // slice only knows how many it was handed.
                    "total": if total_before > 0 { total_before } else { progress.total as u64 },
                    "indexed": indexed_before + progress.indexed as u64,
                    "chunks": chunks_before + progress.chunks as u64,
                    "symbols": symbols_before + progress.symbols as u64,
                    "elapsed_ms": store.run_elapsed_ms(run),
                }));
            };
            let outcome = match work {
                Work::Roots(ref roots) => {
                    writer.index_within_held_under(roots, SLICE, &control, on_file)
                }
                Work::Rest(rest) => writer.index_rest_held_under(rest, SLICE, &control, on_file),
            };
            match outcome {
                Ok(mut done) => {
                    store.last_write.store(now() as usize, Ordering::Relaxed);

                    // Everything this run has embedded, across every slice of
                    // it, so a stop undoes the run rather than the slice that
                    // happened to be going.
                    let mut written = already;
                    written.append(&mut done.written);

                    if done.stopped {
                        // The same eviction a forget performs, for exactly the
                        // files this run wrote. Done here rather than inside
                        // the index pass because only this loop knows how many
                        // slices the run has had.
                        let mut undone = 0;
                        for key in &written {
                            if writer.forget_held(Path::new(key)).is_ok() {
                                undone += 1;
                            }
                        }
                        store.note(format!(
                            "an index run was stopped; {undone} file(s) it had embedded were undone"
                        ));
                        say(serde_json::json!({
                            "event": "done",
                            "indexed": 0,
                            "unchanged": 0,
                            "skipped": 0,
                            "removed": undone,
                            "chunks": 0,
                            "images": 0,
                            "remaining": 0,
                            "stopped": true,
                        }));
                        store.paused.store(false, Ordering::Relaxed);
                        store.cancelled.store(false, Ordering::Relaxed);
                        release(run, admission);
                        return;
                    }

                    // More to do: the rest goes back on the queue with the same
                    // channel, so the watcher gets a turn between slices and
                    // the reader keeps one stream rather than being asked to
                    // press the button again.
                    tally.add(&done);
                    if done.remaining > 0 {
                        // The remainder of the walk, not the roots. Handing the
                        // roots back meant the next slice walked the tree from
                        // the top and re-opened and re-hashed every file the
                        // run had already done, to be told each time that it
                        // was unchanged — so the progress bar restarted at one
                        // every forty-five seconds, and a run of N files over S
                        // slices read N×S files instead of N.
                        let total = if total_before > 0 {
                            total_before
                        } else {
                            // Settled once, on the slice that did the walk.
                            (scanned_before + done.scanned as u64) + done.remaining as u64
                        };
                        store.requeue(Queued {
                            run,
                            job: Job::Index(Indexing {
                                work: Work::Rest(std::mem::take(&mut done.pending)),
                                already: written,
                                scanned: scanned_before + done.scanned as u64,
                                total,
                                tally,
                            }),
                            report: back,
                        });
                        say(serde_json::json!({
                            "event": "slice",
                            "remaining": done.remaining,
                            "indexed": tally.indexed,
                            "chunks": tally.chunks,
                        }));
                        return;
                    }

                    store.note(format!("{} indexed from the portal", tally.indexed));
                    say(serde_json::json!({
                        "event": "done",
                        // The run's, not this slice's. A run of 35 files over
                        // three slices used to end by announcing the four the
                        // last slice reached.
                        "indexed": tally.indexed,
                        "unchanged": tally.unchanged,
                        "skipped": tally.skipped,
                        "removed": tally.removed,
                        "chunks": tally.chunks,
                        "images": tally.images,
                        // Above zero means the slice ran out of time, not that
                        // anything failed: asking again continues where it
                        // stopped and redoes nothing.
                        "remaining": done.remaining,
                        // A stopped run undid itself: nothing it embedded is
                        // in the store, so the next attempt starts from zero.
                        "stopped": done.stopped,
                        // Named, with the rule that refused each. A count would
                        // be a number somebody has to go and investigate.
                        "refused": done.refused.iter().map(|(path, why)| {
                            serde_json::json!({ "path": crate::plain(path), "why": why })
                        }).collect::<Vec<_>>(),
                        // Named for the same reason as the refused, and the
                        // reason this release exists: a run that ends with
                        // "one file failed" and no name is a run whose one
                        // failure nobody can find.
                        "failed": done.failed.iter().map(|(path, why)| {
                            serde_json::json!({ "path": crate::plain(path), "why": why })
                        }).collect::<Vec<_>>(),
                        // How the skipped divide up. The total alone is the
                        // number that made an Angular tree look like a run
                        // that had lost two thousand files.
                        "skipped_reasons": done.skipped_reasons,
                        // The daemon's own elapsed, so the page's clock is
                        // corrected to the run rather than to the tab.
                        "elapsed_ms": store.run_elapsed_ms(run),
                    }));
                }
                Err(e) => say(serde_json::json!({ "event": "error", "error": e.to_string() })),
            }
            // The flags belong to a run, and this one is over.
            store.paused.store(false, Ordering::Relaxed);
            store.cancelled.store(false, Ordering::Relaxed);
            release(run, admission);
        }
        Job::Forget(path) => match writer.forget_held(&path) {
            // Counted apart, because an image has no chunks: a single number
            // would report forgetting a picture as having done nothing.
            Ok((chunks, images)) => {
                store.last_write.store(now() as usize, Ordering::Relaxed);
                store.note(format!("forgot {}", path.display()));
                say(serde_json::json!({
                    "event": "done",
                    "forgot": chunks,
                    "images": images,
                    "path": path.display().to_string(),
                }));
            }
            Err(e) => say(serde_json::json!({ "event": "error", "error": e.to_string() })),
        },
    }
}

/// Where a queue that never started is left for the next daemon to report.
fn dropped_queue_path() -> Option<PathBuf> {
    home::home_or_error().ok().map(|h| h.join("queued.json"))
}

/// Remember the folders that were still waiting when this daemon stopped.
///
/// A queued run is not resumed on the next start: the run's state was this
/// daemon's, and the process ending is one of the two ways a run ends. What is
/// owed to the user is the list, because a folder that never started left no
/// trace anywhere — not in a store, not in a log — and without this they would
/// find out by noticing the search results are thin.
/// A run that was on when the process ended is remembered too, with what it
/// had committed. The checkpoint invariant means those files are in the store
/// and a re-run reports them as `unchanged`, but nothing on the page would
/// have said where it got to.
fn remember_dropped_queue(stores: &[Arc<Store>], waiting: &[(u64, String, Vec<PathBuf>)]) {
    let Some(path) = dropped_queue_path() else {
        return;
    };
    let mut rows: Vec<serde_json::Value> = waiting
        .iter()
        .map(|(_, store, paths)| {
            serde_json::json!({
                "store": store,
                "paths": paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "started": false,
            })
        })
        .collect();
    for store in stores {
        if let Some((paths, kept)) = store.interrupted() {
            rows.push(serde_json::json!({
                "store": store.name,
                "paths": paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "started": true,
                "kept": kept,
            }));
        }
    }
    if rows.is_empty() {
        let _ = std::fs::remove_file(&path);
        return;
    }
    if let Ok(body) = serde_json::to_string_pretty(&rows) {
        let _ = home::write_private(&path, (body + "\n").as_bytes());
    }
}

/// Say on each store's feed what the previous daemon never got to.
///
/// Read once and the file removed, so the line appears on the next start and
/// not on every start after it.
fn report_dropped_queue(stores: &[Arc<Store>], report: &(dyn Fn(&str) + Send + Sync)) {
    let Some(path) = dropped_queue_path() else {
        return;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let _ = std::fs::remove_file(&path);
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&text) else {
        return;
    };
    for row in rows {
        let Some(name) = row.get("store").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let paths: Vec<String> = row
            .get("paths")
            .and_then(serde_json::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(crate::plain)
                    .collect()
            })
            .unwrap_or_default();
        let text = if row.get("started").and_then(serde_json::Value::as_bool) == Some(true) {
            let kept = row
                .get("kept")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            format!(
                "stopped during a run of {}; {kept} file(s) were kept, \
                 and indexing it again reports those as unchanged",
                paths.join(", ")
            )
        } else {
            format!(
                "queued when the daemon stopped and never started: {}",
                paths.join(", ")
            )
        };
        report(&format!("{name}: {text}"));
        if let Some(store) = stores.iter().find(|s| s.name == name) {
            store.note(text);
        }
    }
}

/// Give a finished run's place back, so whatever is waiting starts.
///
/// Called at the two points a run ends and nowhere else. A slice that hands
/// the writer back to the watcher is not one of them: the run is still the
/// store's, and releasing it there would admit a second run onto a machine
/// that is already carrying this one.
///
/// The run is named rather than looked up. It used to ask the store for "the
/// run", which worked while a store had exactly one; asking for its unfinished
/// run at the moment a run finishes returns nothing, and the place is never
/// given back — after `runs at once` runs the daemon admits nothing at all.
fn release(run: u64, admission: &Arc<Admission>) {
    admission.finish(run);
}

/// The daemon standing in as the writer for a forwarded `semlith_index` or
/// `semlith_forget`.
///
/// The agent's call blocks here until the watcher thread — the only thread
/// allowed to write — has run it, which is exactly the guarantee the agent
/// wanted and could not have in 0.8.0: the call either happens or reports why,
/// and never fails because somebody else holds the lock.
pub struct Writer(pub Arc<State>);

impl crate::mcp::Writer for Writer {
    fn index(&self, store: Option<&str>, paths: &[PathBuf]) -> Result<String, String> {
        let store = self.0.writable(store).map_err(|e| e.to_string())?.clone();
        // The receiver, not the run id: an agent's `semlith_index` blocks until
        // the writer has run it and answers with the summary, which is the
        // guarantee it has always had. The run is in the store's snapshot too,
        // so the portal shows an agent's index beside the page's own.
        let (_run, progress) = self
            .0
            .index(&store, paths.to_vec())
            .map_err(|e| e.to_string())?;
        let mut last = None;
        for event in progress {
            match event["event"].as_str() {
                Some("error") => {
                    return Err(event["error"]
                        .as_str()
                        .unwrap_or("indexing failed")
                        .to_string());
                }
                Some("done") => last = Some(event),
                // "file" events are the portal's progress bar; an agent gets
                // the summary it has always got.
                _ => {}
            }
        }
        let done = last.ok_or_else(|| "the writer stopped before answering".to_string())?;
        let count = |key: &str| done[key].as_u64().unwrap_or(0);
        let mut text = format!(
            "{} indexed, {} unchanged, {} skipped, {} removed ({} chunks)",
            count("indexed"),
            count("unchanged"),
            count("skipped"),
            count("removed"),
            count("chunks"),
        );
        if count("remaining") > 0 {
            text.push_str(&format!(
                "\nStopped at the time limit with {} paths remaining. \
                 Call semlith_index again with the same arguments to continue; \
                 nothing already indexed is redone.",
                count("remaining")
            ));
        }
        Ok(text)
    }

    fn add(&self, store: Option<&str>, url: &str) -> Result<String, String> {
        let store = self.0.writable(store).map_err(|e| e.to_string())?.clone();

        // The fetch happens here rather than in the writer thread: it is
        // network work, and holding the store's write queue open for the
        // length of a download would stall every other write behind it. Only
        // the indexing of what landed goes through the queue, which is the
        // part the one-writer rule is actually about.
        let fetched = crate::add::fetch(url, &store.dir).map_err(|e| format!("{e:#}"))?;

        let indexed = self.index(Some(&store.name), std::slice::from_ref(&fetched.path))?;
        Ok(format!(
            "Fetched {} into {}. {indexed}",
            fetched.url,
            fetched.path.display()
        ))
    }

    fn forget(&self, store: Option<&str>, path: &str) -> Result<String, String> {
        let store = self.0.writable(store).map_err(|e| e.to_string())?.clone();
        let progress = self
            .0
            .forget(&store, PathBuf::from(path))
            .map_err(|e| e.to_string())?;
        let done = progress
            .recv()
            .map_err(|_| "the writer stopped before answering".to_string())?;
        if let Some(error) = done["error"].as_str() {
            return Err(error.to_string());
        }
        let chunks = done["forgot"].as_u64().unwrap_or(0);
        let images = done["images"].as_u64().unwrap_or(0);
        Ok(match (chunks, images) {
            (0, 0) => format!("{path} was not indexed; nothing removed."),
            (0, images) => format!("Removed {images} image vector(s) for {path}."),
            (chunks, 0) => format!("Removed {chunks} chunks for {path}."),
            (chunks, images) => {
                format!("Removed {chunks} chunks and {images} image vector(s) for {path}.")
            }
        })
    }
}

/// The URL the daemon prints, and the only place the token is shown.
pub fn url(state: &State) -> String {
    state.server.url()
}

/// Where the portal's About view gets its numbers.
pub fn uptime(state: &State) -> u64 {
    state
        .started
        .elapsed()
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The port `semlith start` should use: the flag, then the environment, then
/// the default.
pub fn port_of(flag: Option<u16>) -> u16 {
    flag.or_else(|| {
        std::env::var(http::PORT_ENV)
            .ok()
            .and_then(|v| v.parse().ok())
    })
    .unwrap_or(http::DEFAULT_PORT)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Six false privacy warnings on every `semlith mcp` start, one per store,
    /// naming six paths that do not exist.
    #[test]
    fn a_discovery_file_that_is_not_there_is_not_a_privacy_rejection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join(DISCOVERY_FILE);
        assert!(
            !missing.exists(),
            "the fixture wrote a file it should not have"
        );

        // The old predicate still answers `false` — that is correct and is
        // what made it the wrong thing to warn on.
        assert!(!owner_only(&missing));
        assert!(
            !worth_warning_about(&missing),
            "a missing {DISCOVERY_FILE} was reported as a privacy rejection",
        );

        // A file that is there and fails the check still gets its line.
        let present = dir.path().join("present.json");
        std::fs::write(&present, "{}").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&present, std::fs::Permissions::from_mode(0o644))
                .expect("chmod");
            assert!(
                worth_warning_about(&present),
                "a world-readable discovery file stopped being worth a line",
            );
        }
    }

    /// The change counters are process-global — deliberately, because two of
    /// the six are bumped by code that has no `State` in hand. That makes them
    /// shared between every test in this binary running in parallel threads.
    ///
    /// **Every test here that causes a bump takes this, not only the one that
    /// reads the counters.** Guarding the reader alone passed on macOS and
    /// failed on Windows, where the scheduling differs: a clock test's
    /// `record` landed inside the isolation test's window and moved `runs`
    /// while it was asserting that writing `stores` had not.
    fn counters() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A store with nothing behind it, for the parts of a run that are
    /// bookkeeping rather than embedding.
    fn bare_store(name: &str) -> Arc<Store> {
        Arc::new(Store {
            name: name.to_string(),
            dir: PathBuf::from("/nowhere").join(name),
            roots: Vec::new(),
            watched: Vec::new(),
            queue: Mutex::new(VecDeque::new()),
            events: Mutex::new(VecDeque::new()),
            runs: Mutex::new(Vec::new()),
            watching: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            last_write: AtomicUsize::new(0),
            pruned: AtomicUsize::new(0),
            expecting_run_until: AtomicUsize::new(0),
        })
    }

    /// The clock restarted from zero every forty-five seconds, because it was
    /// measured inside `perform` and a slice is a fresh `Job::Index`. A page
    /// faithfully redrew that as a run that had just begun.
    #[test]
    fn a_runs_clock_spans_its_slices_and_never_goes_backwards() {
        // Recording bumps the runs counter, which another test in this
        // module asserts the isolation of. Same guard, or they race.
        let _held = counters();
        let store = bare_store("api");
        store.begin_run(1, vec![PathBuf::from("/work/api")]);
        store.record(
            1,
            &serde_json::json!({ "event": "started", "paths": ["/work/api"] }),
        );
        std::thread::sleep(Duration::from_millis(30));
        let first = store.run_elapsed_ms(1);
        // A slice hands the writer back and the run returns as a new job. The
        // clock is the run's, so it carries on.
        store.record(1, &serde_json::json!({ "event": "slice", "remaining": 12 }));
        std::thread::sleep(Duration::from_millis(30));
        let second = store.run_elapsed_ms(1);
        assert!(second >= first, "{second} went backwards from {first}");
        assert!(second >= 50, "the clock did not span the slice: {second}ms");
    }

    /// Held time is not time anything is happening, and a run that is over
    /// keeps its total rather than counting on.
    #[test]
    fn a_held_run_stops_counting_and_a_finished_one_freezes() {
        // Recording bumps the runs counter, which another test in this
        // module asserts the isolation of. Same guard, or they race.
        let _held = counters();
        let store = bare_store("api");
        store.begin_run(1, Vec::new());
        store.record(1, &serde_json::json!({ "event": "started", "paths": [] }));
        store.record(1, &serde_json::json!({ "event": "paused" }));
        let held = store.run_elapsed_ms(1);
        std::thread::sleep(Duration::from_millis(40));
        assert_eq!(
            store.run_elapsed_ms(1),
            held,
            "the clock counted while the run was held"
        );

        store.record(1, &serde_json::json!({ "event": "resumed" }));
        std::thread::sleep(Duration::from_millis(20));
        store.record(
            1,
            &serde_json::json!({ "event": "done", "indexed": 3, "stopped": false }),
        );
        let total = store.run_elapsed_ms(1);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(
            store.run_elapsed_ms(1),
            total,
            "a finished run's clock kept going"
        );
    }

    /// Two clients read one run through their own cursors, and neither is
    /// affected by what the other has read.
    #[test]
    fn two_cursors_over_one_log_each_see_every_line_once() {
        // Recording bumps the runs counter, which another test in this
        // module asserts the isolation of. Same guard, or they race.
        let _held = counters();
        let store = bare_store("api");
        store.begin_run(1, Vec::new());
        for scanned in 0..5 {
            store.record(
                1,
                &serde_json::json!({ "event": "file", "scanned": scanned }),
            );
        }

        let seqs = |lines: &[serde_json::Value]| {
            lines
                .iter()
                .map(|l| l["seq"].as_u64().unwrap())
                .collect::<Vec<_>>()
        };

        let first = store.log_after(None, None);
        assert_eq!(seqs(&first), vec![0, 1, 2, 3, 4]);

        // One client reads on from where it was; the other has read nothing
        // and still sees the whole run.
        store.record(1, &serde_json::json!({ "event": "file", "scanned": 5 }));
        assert_eq!(seqs(&store.log_after(None, Some(4))), vec![5]);
        assert_eq!(seqs(&store.log_after(None, None)), vec![0, 1, 2, 3, 4, 5]);
        // A cursor past the end is caught up, not an error.
        assert!(store.log_after(None, Some(5)).is_empty());
    }

    /// The queue is first in, first out by submission, and nothing reorders it.
    #[test]
    fn the_queue_admits_in_submission_order_and_only_up_to_the_limit() {
        // Recording bumps the runs counter, which another test in this
        // module asserts the isolation of. Same guard, or they race.
        let _held = counters();
        let admission = Admission::new(2);
        let stores: Vec<Arc<Store>> = ["a", "b", "c", "d"].into_iter().map(bare_store).collect();
        for store in &stores {
            admission.submit(store, vec![PathBuf::from("/work").join(&store.name)]);
        }

        assert_eq!(admission.running(), 2, "the limit admitted more than two");
        let waiting: Vec<String> = admission.waiting().into_iter().map(|(_, s, _)| s).collect();
        assert_eq!(waiting, vec!["c", "d"]);
        assert_eq!(admission.position_of("c"), Some(1));
        assert_eq!(admission.position_of("d"), Some(2));
        assert_eq!(
            admission.position_of("a"),
            None,
            "a running run is not waiting"
        );

        // A run finishing admits exactly the head, and the one behind it moves
        // up by having been behind it.
        let first = stores[0].current_run().expect("a is running");
        admission.finish(first);
        assert_eq!(admission.position_of("d"), Some(1));
        assert_eq!(admission.running(), 2);

        // Raising the limit admits immediately; nothing waits for a timer.
        admission.set_limit(4);
        assert!(admission.waiting().is_empty());
        assert_eq!(admission.running(), 3);
    }

    /// A run that ends gives its place back, however it ended.
    ///
    /// The place is released by run id. It used to be released by asking the
    /// store for "the run", which held while a store had exactly one — and
    /// once a store held several, asking for its *unfinished* run at the
    /// moment a run finished returned nothing, so the place was never given
    /// back. After `runs at once` runs the daemon admitted nothing ever again,
    /// and every later run sat at `queued` forever with nothing running.
    #[test]
    fn every_run_that_ends_gives_its_place_back() {
        let _held = counters();
        let admission = Arc::new(Admission::new(2));
        let stores: Vec<Arc<Store>> = ["a", "b", "c"].into_iter().map(bare_store).collect();
        let mut ids = Vec::new();
        for store in &stores {
            let (id, _) = admission.submit(store, Vec::new());
            ids.push(id);
        }
        assert_eq!(admission.running(), 2);

        // Each run ends the way `perform` ends one: its record is marked done
        // and then its place is released by id.
        for (store, id) in stores.iter().zip(&ids) {
            store.record(
                *id,
                &serde_json::json!({ "event": "done", "indexed": 0, "stopped": false }),
            );
            // The reason `release` takes the id: by now the store has no
            // unfinished run to be asked for, so a lookup would find nothing
            // and release nothing.
            assert_eq!(store.current_run(), None);
            release(*id, &admission);
        }

        assert_eq!(
            admission.running(),
            0,
            "a finished run kept its place, so the daemon can admit nothing more"
        );
        assert!(admission.waiting().is_empty(), "the queue never drained");
    }

    /// Taking a folder out of the queue costs nothing, because nothing of it
    /// was embedded — which is the whole difference from stopping a run.    /// Taking a folder out of the queue costs nothing, because nothing of it
    /// was embedded — which is the whole difference from stopping a run.
    #[test]
    fn a_dequeued_run_is_answered_at_once_and_undoes_nothing() {
        // Recording bumps the runs counter, which another test in this
        // module asserts the isolation of. Same guard, or they race.
        let _held = counters();
        let admission = Admission::new(1);
        let (a, b) = (bare_store("a"), bare_store("b"));
        admission.submit(&a, Vec::new());
        let (_, waiting) = admission.submit(&b, Vec::new());

        assert_eq!(admission.position_of("b"), Some(1));
        assert_eq!(admission.dequeue("b"), 1);
        assert_eq!(admission.position_of("b"), None);
        assert_eq!(admission.running(), 1, "dequeuing b disturbed a");

        let answer = waiting.recv().expect("the dequeued run was answered");
        assert_eq!(answer["event"], "done");
        assert_eq!(answer["stopped"], true);
        assert_eq!(answer["dequeued"], true);
        assert_eq!(answer["removed"], 0, "a dequeued run undid something");
        // And nothing is left to dequeue.
        assert_eq!(admission.dequeue("b"), 0);
    }

    /// Each counter moves when, and only when, its own domain is written.
    ///
    /// The bump sites themselves are held to "once, and nowhere else" by
    /// `one_bump_site_per_domain` below; this is the other half — that the six
    /// are genuinely separate and one write does not move two.
    #[test]
    fn a_counter_moves_for_its_own_domain_and_no_other() {
        let _held = counters();
        for domain in changes::DOMAINS {
            let before: Vec<u64> = changes::DOMAINS.iter().map(|d| changes::read(*d)).collect();
            changes::bump(domain);
            for (other, was) in changes::DOMAINS.iter().zip(&before) {
                let now = changes::read(*other);
                if *other == domain {
                    assert_eq!(now, was + 1, "{} did not move", domain.as_str());
                } else {
                    assert_eq!(
                        now,
                        *was,
                        "writing {} moved {}",
                        domain.as_str(),
                        other.as_str()
                    );
                }
            }
        }
    }

    /// How many places may write each domain.
    ///
    /// One, except `stores`: a store joining and a store being deleted are two
    /// writes of one domain and there is no single function both go through —
    /// `open_store` takes a lock and spawns a watcher, `delete_store` stops one
    /// and removes the directory. Every other domain has exactly one writer and
    /// a second site would be a second source of truth.
    const BUMP_SITES: &[(&str, usize)] = &[
        ("stores", 2),
        ("runs", 1),
        ("clients", 1),
        ("ledger", 1),
        ("events", 1),
        ("privacy", 1),
    ];

    /// A bump site that is not in the one function that writes its domain is
    /// how one page goes stale while the others are live — the worst outcome
    /// available, because a portal that is wrong in one place is trusted in all
    /// of them.
    #[test]
    fn one_bump_site_per_domain() {
        // Every source file, read as text. `include_str!` rather than walking
        // the directory, so a file added without a line here is a compile
        // error rather than a silently unchecked file.
        const SOURCES: &[(&str, &str)] = &[
            ("daemon.rs", include_str!("daemon.rs")),
            ("routes.rs", include_str!("routes.rs")),
            ("store.rs", include_str!("store.rs")),
            ("doctor.rs", include_str!("doctor.rs")),
            ("ledger.rs", include_str!("ledger.rs")),
            ("lib.rs", include_str!("lib.rs")),
            ("watch.rs", include_str!("watch.rs")),
            ("mcp.rs", include_str!("mcp.rs")),
            ("http.rs", include_str!("http.rs")),
            ("home.rs", include_str!("home.rs")),
        ];
        for domain in changes::DOMAINS {
            let needle = format!("Domain::{}", title_case(domain.as_str()));
            let sites: usize = SOURCES
                .iter()
                .map(|(_, text)| {
                    text.lines()
                        .filter(|line| {
                            line.contains("changes::bump") && line.contains(needle.as_str())
                        })
                        .count()
                })
                .sum();
            let allowed = BUMP_SITES
                .iter()
                .find(|(name, _)| *name == domain.as_str())
                .map(|(_, n)| *n)
                .expect("every domain declares how many places may write it");
            assert_eq!(
                sites,
                allowed,
                "{} is bumped at {sites} site(s) and {allowed} is what it may have; a counter \
                 is bumped where its domain is written, and a site beyond that list is a \
                 second source of truth",
                domain.as_str()
            );
        }
    }

    fn title_case(word: &str) -> String {
        let mut chars = word.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    }

    /// A `.semlith` beside its corpus is not in the registry, and the daemon
    /// still has to know what to call it and what to watch. The rule is the
    /// one `adopt` uses: the directory holding the store is the corpus.
    #[test]
    fn an_unregistered_store_is_named_after_the_directory_holding_it() {
        let registry = Registry::default();
        let (name, roots) = roots_for(Path::new("/work/api/.semlith"), &registry);
        assert_eq!(name, "api");
        assert_eq!(roots.len(), 1);
        assert!(roots[0].ends_with("api") || roots[0] == Path::new("/work/api"));
    }

    /// A write with several stores open has no "the" store, and guessing one
    /// writes into somebody's other repository.
    #[test]
    fn an_unnamed_write_among_several_stores_is_refused() {
        let state = State {
            server: Arc::new(Server::bind(0).expect("an ephemeral port")),
            fleet: Mutex::new(None),
            stores: RwLock::new(
                ["api", "cli"]
                    .into_iter()
                    .map(|name| {
                        Arc::new(Store {
                            name: name.to_string(),
                            dir: PathBuf::from("/nowhere").join(name),
                            roots: Vec::new(),
                            watched: Vec::new(),
                            queue: Mutex::new(VecDeque::new()),
                            events: Mutex::new(VecDeque::new()),
                            runs: Mutex::new(Vec::new()),
                            watching: AtomicBool::new(false),
                            stop: AtomicBool::new(false),
                            paused: AtomicBool::new(false),
                            cancelled: AtomicBool::new(false),
                            last_write: AtomicUsize::new(0),
                            pruned: AtomicUsize::new(0),
                            expecting_run_until: AtomicUsize::new(0),
                        })
                    })
                    .collect(),
            ),
            admission: Arc::new(Admission::new(1)),
            airgap: false,
            started: SystemTime::now(),
            debounce: Duration::from_millis(500),
            report: Arc::new(|_| {}),
            refusals: Mutex::new(BTreeMap::new()),
            proxies: Mutex::new(BTreeMap::new()),
            clients: Mutex::new(BTreeMap::new()),
            mcp_fleet: Mutex::new(None),
            ledger: false,
        };

        // An `Arc` because the reconciliation a miss triggers opens stores,
        // and opening one hands `Arc<State>` to the watcher thread it spawns.
        let state = Arc::new(state);
        let err = match state.writable(None) {
            Err(e) => e.to_string(),
            Ok(store) => panic!("an unnamed write picked {}", store.name),
        };
        assert!(err.contains("api, cli"), "{err}");
        assert_eq!(
            state.writable(Some("cli")).map(|s| s.name.clone()).ok(),
            Some("cli".to_string())
        );
        assert!(state.writable(Some("nope")).is_err());
    }
}
