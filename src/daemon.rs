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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
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
    pub fn read(store_dir: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(store_dir.join(DISCOVERY_FILE)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn write(&self, store_dir: &Path) -> Result<()> {
        let path = store_dir.join(DISCOVERY_FILE);
        std::fs::write(&path, serde_json::to_string_pretty(self)? + "\n")
            .with_context(|| format!("writing {}", path.display()))?;
        // The token is in this file, so nobody else on a shared machine reads
        // it. Best effort: a filesystem without modes is not a reason to fail
        // to start.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    fn remove(store_dir: &Path) {
        let _ = std::fs::remove_file(store_dir.join(DISCOVERY_FILE));
    }
}

/// Something the watcher thread should do next, on behalf of a request.
enum Job {
    Index(Vec<PathBuf>),
    Forget(PathBuf),
}

/// A job and the channel its progress goes back on.
struct Queued {
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
    /// False once the watcher thread has returned, so the portal can say a
    /// store stopped being kept current rather than showing a stale count.
    pub watching: AtomicBool,
    /// Unix seconds of the last write this daemon made to the store.
    pub last_write: AtomicUsize,
}

impl Store {
    fn note(&self, text: String) {
        let mut events = self.events.lock().expect("the event lock");
        if events.len() == EVENT_HISTORY {
            events.pop_front();
        }
        events.push_back(Event { at: now(), text });
    }

    /// The watcher's recent events, oldest first.
    pub fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .expect("the event lock")
            .iter()
            .cloned()
            .collect()
    }

    /// Hand a job to the writer and read its progress back as it happens.
    ///
    /// The receiver is what a streaming route writes chunks from, so the
    /// browser sees an index run while it is running rather than when it ends.
    fn submit(&self, job: Job) -> mpsc::Receiver<serde_json::Value> {
        let (report, progress) = mpsc::channel();
        self.queue
            .lock()
            .expect("the queue lock")
            .push_back(Queued { job, report });
        progress
    }

    /// Whether the writer has anything waiting — the Index view's queue depth.
    pub fn queue_depth(&self) -> usize {
        self.queue.lock().expect("the queue lock").len()
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
    pub stores: Vec<Arc<Store>>,
    pub airgap: bool,
    pub started: SystemTime,
    /// Refusals by class, for the line the daemon logs on shutdown.
    pub refusals: Mutex<BTreeMap<&'static str, u64>>,
    /// `semlith mcp` processes forwarding here: pid to the unix second it was
    /// last heard from. A proxy has no disconnect to observe — its client may
    /// simply stop asking — so recency is the only honest answer to "how many
    /// are connected".
    pub proxies: Mutex<BTreeMap<u32, u64>>,
    /// A second reader, for forwarded MCP calls, opened on first use.
    ///
    /// Separate from `fleet` on purpose: a forwarded `semlith_index` blocks its
    /// caller until the writer has run it, and sharing one lock would mean an
    /// agent's index freezing the portal for as long as the slice lasts. A
    /// reader that is never used costs a SQLite handle and no vectors.
    pub mcp_fleet: Mutex<Option<Fleet>>,
}

/// How recently a proxy must have called to count as connected.
const PROXY_FRESH: u64 = 120;

impl State {
    pub fn store(&self, name: &str) -> Option<&Arc<Store>> {
        self.stores.iter().find(|s| s.name == name)
    }

    /// The one store a write means, or an error naming the alternatives.
    pub fn writable(&self, name: Option<&str>) -> Result<&Arc<Store>> {
        match (name, self.stores.as_slice()) {
            (Some(name), _) => self
                .store(name)
                .with_context(|| format!("no store called {name} is open")),
            (None, [one]) => Ok(one),
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

    /// Queue an index run and stream its progress.
    pub fn index(
        &self,
        store: &Arc<Store>,
        paths: Vec<PathBuf>,
    ) -> Result<mpsc::Receiver<serde_json::Value>> {
        Self::writer_alive(store)?;
        Ok(store.submit(Job::Index(paths)))
    }

    pub fn forget(
        &self,
        store: &Arc<Store>,
        path: PathBuf,
    ) -> Result<mpsc::Receiver<serde_json::Value>> {
        Self::writer_alive(store)?;
        Ok(store.submit(Job::Forget(path)))
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
        for store in &self.stores {
            let _ = discovery(self.server.port(), &fresh).write(&store.dir);
        }
        fresh
    }

    /// Note that a forwarding `semlith mcp` is alive.
    pub fn saw_proxy(&self, pid: u32) {
        let mut proxies = self.proxies.lock().expect("the proxy lock");
        let now = now();
        proxies.insert(pid, now);
        proxies.retain(|_, seen| now.saturating_sub(*seen) <= PROXY_FRESH);
    }

    /// How many `semlith mcp` processes are currently forwarding here.
    pub fn proxy_count(&self) -> usize {
        let now = now();
        self.proxies
            .lock()
            .expect("the proxy lock")
            .values()
            .filter(|seen| now.saturating_sub(**seen) <= PROXY_FRESH)
            .count()
    }

    /// The reader forwarded MCP calls answer from, opened on first use.
    pub fn open_mcp_fleet(&self) -> Result<()> {
        let mut fleet = self.mcp_fleet.lock().expect("the mcp fleet lock");
        if fleet.is_some() {
            return Ok(());
        }
        let dirs: Vec<PathBuf> = self.stores.iter().map(|s| s.dir.clone()).collect();
        if dirs.is_empty() {
            bail!("this daemon has no store open");
        }
        let mut opened = Fleet::open(&dirs)?;
        opened.quiet = true;
        *fleet = Some(opened);
        Ok(())
    }

    pub fn refuse(&self, class: Refusal) {
        *self
            .refusals
            .lock()
            .expect("the refusal lock")
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
pub fn run(
    dirs: &[PathBuf],
    port: u16,
    debounce: Duration,
    airgap: bool,
    report: impl Fn(&str) + Send + Sync + 'static,
) -> Result<Arc<State>> {
    let registry = Registry::load()?;

    // Locks first, and all of them, before anything is watched or served: a
    // daemon that took three of four locks and then failed would leave three
    // stores unusable to the `semlith index` that is about to be tried.
    let mut locks = Vec::new();
    let mut opening = Vec::new();
    for dir in dirs {
        let lock = StoreLock::acquire(dir)
            .with_context(|| format!("{} cannot be opened by the daemon", dir.display()))?;
        let (name, roots) = roots_for(dir, &registry);
        opening.push((name, dir.clone(), roots));
        locks.push(lock);
    }

    // The port is the other thing worth failing on before any work: a daemon
    // that indexed for a minute and then could not listen has wasted the
    // minute.
    let server = Arc::new(Server::bind(port)?);
    report(&format!("listening on 127.0.0.1:{}", server.port()));

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
            watching: AtomicBool::new(false),
            last_write: AtomicUsize::new(0),
        }));
    }

    let fleet = if dirs.is_empty() {
        None
    } else {
        let mut fleet = Fleet::open(dirs)?;
        // stderr is this process's log, and a model download progress bar in
        // the middle of it is noise; the Stores view reports readiness.
        fleet.quiet = true;
        Some(fleet)
    };

    let token = server.token();
    for store in &stores {
        discovery(server.port(), &token).write(&store.dir)?;
    }

    let state = Arc::new(State {
        server: Arc::clone(&server),
        fleet: Mutex::new(fleet),
        stores,
        airgap,
        started: SystemTime::now(),
        refusals: Mutex::new(BTreeMap::new()),
        proxies: Mutex::new(BTreeMap::new()),
        mcp_fleet: Mutex::new(None),
    });

    // Installed before the first thread starts: the signal is how this process
    // ends, so the ordinary exit has to be the safe one.
    watch::stop_on_signal();

    let report_line = Arc::new(report);
    let report = Arc::clone(&report_line);
    let mut watchers = Vec::new();
    for (store, lock) in state.stores.iter().cloned().zip(locks) {
        let report = Arc::clone(&report);
        // Set here rather than inside the thread: the flag is what the write
        // queue checks before accepting a job, and opening the store takes
        // long enough that a request arriving in that window would otherwise
        // be told the writer was gone when it was merely starting.
        store.watching.store(true, Ordering::Relaxed);
        watchers.push(std::thread::spawn(move || {
            // Moved in so the lock's life is the thread's life, which is what
            // makes "the daemon is the writer" true rather than intended.
            let _lock = lock;
            if let Err(e) = tend(&store, debounce, &watch::STOP, &*report) {
                report(&format!("{}: watcher stopped: {e}", store.name));
                store.note(format!("watcher stopped: {e}"));
            }
            store.watching.store(false, Ordering::Relaxed);
        }));
    }

    // Behind the URL, not in front of it: loading the model costs a second or
    // two, and a developer staring at a blank terminal waiting for a link is
    // paying that cost twice. The first search would otherwise pay it instead,
    // which is worse — it looks like the search is slow.
    {
        let warming = Arc::clone(&state);
        let report = report_line.clone();
        std::thread::spawn(move || {
            let mut fleet = warming.fleet.lock().expect("the fleet lock");
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
        for watcher in watchers {
            let _ = watcher.join();
        }
        for store in &state.stores {
            Discovery::remove(&store.dir);
        }

        let refusals = state.refusals.lock().expect("the refusal lock").clone();
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
) -> Result<()> {
    let mut writer = Semlith::open(&store.dir, None)?;
    writer.quiet = true;

    // A store with no root on disk still gets a thread, because it still has a
    // queue: the portal can index a new path into it even though there is
    // nothing to watch yet.
    let roots = store.watched.clone();

    watch::run_held(
        &mut writer,
        &roots,
        debounce,
        stop,
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
                Progress::File(_) => {}
            }
        },
        |writer| {
            // The writer is this thread, so a queued job runs here or nowhere.
            loop {
                let Some(next) = store.queue.lock().expect("the queue lock").pop_front() else {
                    return Ok(());
                };
                perform(store, writer, next);
            }
        },
    )
}

/// Run one queued job, reporting progress back to whoever asked for it.
fn perform(store: &Arc<Store>, writer: &mut Semlith, queued: Queued) {
    let Queued { job, report } = queued;
    let say = |value: serde_json::Value| {
        // A closed receiver means the browser navigated away mid-run. The work
        // still finishes — it is the store's, not the request's.
        let _ = report.send(value);
    };

    match job {
        Job::Index(paths) => {
            let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
            say(serde_json::json!({ "event": "started", "paths": names }));
            let outcome = writer.index_within_held(&paths, SLICE, |path, progress| {
                say(serde_json::json!({
                    "event": "file",
                    "path": path.display().to_string(),
                    "scanned": progress.scanned,
                    "total": progress.total,
                    "chunks": progress.chunks,
                }));
            });
            match outcome {
                Ok(done) => {
                    store.last_write.store(now() as usize, Ordering::Relaxed);
                    store.note(format!("{} indexed from the portal", done.indexed));
                    say(serde_json::json!({
                        "event": "done",
                        "indexed": done.indexed,
                        "unchanged": done.unchanged,
                        "skipped": done.skipped,
                        "removed": done.removed,
                        "chunks": done.chunks,
                        // Above zero means the slice ran out of time, not that
                        // anything failed: asking again continues where it
                        // stopped and redoes nothing.
                        "remaining": done.remaining,
                    }));
                }
                Err(e) => say(serde_json::json!({ "event": "error", "error": e.to_string() })),
            }
        }
        Job::Forget(path) => match writer.forget_held(&path) {
            Ok(n) => {
                store.last_write.store(now() as usize, Ordering::Relaxed);
                store.note(format!("forgot {}", path.display()));
                say(serde_json::json!({
                    "event": "done",
                    "forgot": n,
                    "path": path.display().to_string(),
                }));
            }
            Err(e) => say(serde_json::json!({ "event": "error", "error": e.to_string() })),
        },
    }
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
        let progress = self
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
        Ok(match done["forgot"].as_u64().unwrap_or(0) {
            0 => format!("{path} was not indexed; nothing removed."),
            n => format!("Removed {n} chunks for {path}."),
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
            stores: ["api", "cli"]
                .into_iter()
                .map(|name| {
                    Arc::new(Store {
                        name: name.to_string(),
                        dir: PathBuf::from("/nowhere").join(name),
                        roots: Vec::new(),
                        watched: Vec::new(),
                        queue: Mutex::new(VecDeque::new()),
                        events: Mutex::new(VecDeque::new()),
                        watching: AtomicBool::new(false),
                        last_write: AtomicUsize::new(0),
                    })
                })
                .collect(),
            airgap: false,
            started: SystemTime::now(),
            refusals: Mutex::new(BTreeMap::new()),
            proxies: Mutex::new(BTreeMap::new()),
            mcp_fleet: Mutex::new(None),
        };

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
