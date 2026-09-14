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
        if !owner_only(&path) {
            eprintln!(
                "semlith: ignoring {} — it is not a file this user wrote privately",
                path.display()
            );
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
enum Job {
    /// The paths to walk, and every file the earlier slices of this same run
    /// already embedded. A slice yields the writer back to the watcher when
    /// its budget runs out and the rest is re-queued behind whatever the
    /// watcher had waiting, so one logical run is several `Index` jobs on one
    /// report channel.
    Index(Vec<PathBuf>, Vec<String>),
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
    /// Set to stop this store's watcher and release its lock, so the store can
    /// be deleted while the daemon keeps running. The daemon's own shutdown
    /// sets it on every open store alongside the global stop.
    pub stop: AtomicBool,
    /// A queued index run holds here between files while this is set.
    pub paused: AtomicBool,
    /// A queued index run gives up and undoes itself when this is set.
    pub cancelled: AtomicBool,
    /// Unix seconds of the last write this daemon made to the store.
    pub last_write: AtomicUsize,
}

impl Store {
    fn note(&self, text: String) {
        let mut events = self.events.lock().unwrap_or_else(|e| e.into_inner());
        if events.len() == EVENT_HISTORY {
            events.pop_front();
        }
        events.push_back(Event { at: now(), text });
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
        queue.push_back(Queued { job, report });
        progress
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
    pub fn cancel_queued(&self) -> usize {
        let mut queue = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        let mut dropped = 0;
        queue.retain(|queued| {
            if !matches!(queued.job, Job::Index(..)) {
                return true;
            }
            let _ = queued.report.send(serde_json::json!({
                "event": "done",
                "indexed": 0,
                "unchanged": 0,
                "skipped": 0,
                "removed": 0,
                "chunks": 0,
                "images": 0,
                "remaining": 0,
                "stopped": true,
            }));
            dropped += 1;
            false
        });
        dropped
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

impl State {
    /// Every open store, as a snapshot.
    ///
    /// Cloned out rather than handed back under the guard: an `Arc` clone is a
    /// counter bump, and holding a read lock across a search would stop a new
    /// store being opened for as long as the search took.
    pub fn stores(&self) -> Vec<Arc<Store>> {
        self.stores.read().expect("the stores lock").clone()
    }

    pub fn store(&self, name: &str) -> Option<Arc<Store>> {
        self.stores().into_iter().find(|s| s.name == name)
    }

    /// The one store a write means, or an error naming the alternatives.
    pub fn writable(&self, name: Option<&str>) -> Result<Arc<Store>> {
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

    /// Queue an index run and stream its progress.
    pub fn index(
        &self,
        store: &Arc<Store>,
        paths: Vec<PathBuf>,
    ) -> Result<mpsc::Receiver<serde_json::Value>> {
        Self::writer_alive(store)?;
        // The writer is one thread and it may be mid-catch-up. A page that
        // shows nothing for a minute looks like a page that lost the request
        // rather than one waiting its turn.
        Ok(store.submit(
            Job::Index(paths, Vec::new()),
            Some(serde_json::json!({ "event": "queued" })),
        ))
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
    pub fn delete_store(&self, name: &str) -> Result<PathBuf> {
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
        let dirs: Vec<PathBuf> = self.stores().iter().map(|s| s.dir.clone()).collect();
        if dirs.is_empty() {
            bail!("this daemon has no store open");
        }
        let mut opened = Fleet::open(&dirs)?;
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
    pub fn open_store(self: &Arc<Self>, dir: &Path) -> Result<Arc<Store>> {
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
            watching: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            last_write: AtomicUsize::new(0),
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
        std::thread::spawn(move || {
            let _lock = lock;
            if let Err(e) = tend(&watching, debounce, &watching.stop, &*report) {
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
        let dirs: Vec<PathBuf> = self.stores().iter().map(|s| s.dir.clone()).collect();
        if dirs.is_empty() {
            return Ok(());
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
            watching: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
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

    let report_line: Arc<dyn Fn(&str) + Send + Sync> = Arc::new(report);

    let state = Arc::new(State {
        server: Arc::clone(&server),
        fleet: Mutex::new(fleet),
        stores: RwLock::new(stores),
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
        watchers.push(std::thread::spawn(move || {
            // Moved in so the lock's life is the thread's life, which is what
            // makes "the daemon is the writer" true rather than intended.
            let _lock = lock;
            if let Err(e) = tend(&store, debounce, &store.stop, &*report) {
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
        // A queued job is what the catch-up steps aside for.
        &|| store.queue_depth() > 0,
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
                let Some(next) = store
                    .queue
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pop_front()
                else {
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
    let back = report.clone();
    let say = |value: serde_json::Value| {
        // A closed receiver means the browser navigated away mid-run. The work
        // still finishes — it is the store's, not the request's.
        let _ = report.send(value);
    };

    match job {
        Job::Index(paths, already) => {
            // Only the first slice announces itself; the rest are the same run
            // continuing, and a second "started" would read as a second run.
            if already.is_empty() {
                let names: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
                say(serde_json::json!({ "event": "started", "paths": names }));
            }
            let started_at = std::time::Instant::now();
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
            let outcome =
                writer.index_within_held_under(&paths, SLICE, &control, |path, progress| {
                    say(serde_json::json!({
                        "event": "file",
                        "path": path.display().to_string(),
                        // What is happening to this file, so a page can say
                        // "unchanged" rather than showing nothing at all.
                        "outcome": progress.outcome.as_str(),
                        "scanned": progress.scanned,
                        "total": progress.total,
                        "indexed": progress.indexed,
                        "chunks": progress.chunks,
                        "symbols": progress.symbols,
                        "elapsed_ms": started_at.elapsed().as_millis() as u64,
                    }));
                });
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
                        return;
                    }

                    // More to do: the rest goes back on the queue with the same
                    // channel, so the watcher gets a turn between slices and
                    // the reader keeps one stream rather than being asked to
                    // press the button again.
                    if done.remaining > 0 {
                        store.requeue(Queued {
                            job: Job::Index(paths, written),
                            report: back,
                        });
                        say(serde_json::json!({
                            "event": "slice",
                            "remaining": done.remaining,
                            "indexed": done.indexed,
                            "chunks": done.chunks,
                        }));
                        return;
                    }

                    store.note(format!("{} indexed from the portal", done.indexed));
                    say(serde_json::json!({
                        "event": "done",
                        "indexed": done.indexed,
                        "unchanged": done.unchanged,
                        "skipped": done.skipped,
                        "removed": done.removed,
                        "chunks": done.chunks,
                        "images": done.images,
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
                            serde_json::json!({ "path": path, "why": why })
                        }).collect::<Vec<_>>(),
                    }));
                }
                Err(e) => say(serde_json::json!({ "event": "error", "error": e.to_string() })),
            }
            // The flags belong to a run, and this one is over.
            store.paused.store(false, Ordering::Relaxed);
            store.cancelled.store(false, Ordering::Relaxed);
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
                            watching: AtomicBool::new(false),
                            stop: AtomicBool::new(false),
                            paused: AtomicBool::new(false),
                            cancelled: AtomicBool::new(false),
                            last_write: AtomicUsize::new(0),
                        })
                    })
                    .collect(),
            ),
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
