//! An index pass as three stages running at once.
//!
//! Before 0.32.0 one thread did everything: it read a file, hashed it,
//! extracted and scanned its text, parsed it, chunked it, wrote its rows, and
//! then stopped to run the model on the CPU while every accelerator lane waited
//! for its next batch. Measured on the M1, the writer embedding inline was 64 %
//! of the run and the GPU lane spent 28 % waiting (0.30.1). So:
//!
//! - **prepare** — a pool of threads sized from the performance cores does the
//!   per-file work that needs no database: read, hash, extract, the secret
//!   scan, the parse, the chunking, and the tokenising, once per chunk. Files
//!   come back in walk order, so a run is as deterministic as it was.
//! - **write** — the thread that owns the store. It only makes decisions that
//!   need the database and commits rows and vectors; it never runs a model.
//! - **embed** — one scheduler handing token-budget batches to every lane,
//!   keeping [`LANE_DEPTH`] batches in flight on each, the CPU included, and
//!   handing whole windows back in the order they were sent.
//!
//! Vectors reach the index in id order, window by window, because `exact.f32`
//! is read back by binary search and a shard is named by its first id.

use crate::session::Ids;
use crate::{SkipReason, chunk, graph, image, keyscan};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

/// Batches a lane holds queued at once. Two, so a device never waits for the
/// scheduler to hand it the next batch while it finishes the one it has.
pub const LANE_DEPTH: usize = 2;

/// How long a batch waits on a lane that is no longer ready before it is
/// handed to another target.
const LANE_GONE: Duration = Duration::from_secs(2);

/// Windows the embed stage works on at once. A lane that finishes the oldest
/// window's work moves on to the next one rather than idling at the boundary;
/// more than this is memory with nothing to gain.
const ACTIVE_WINDOWS: usize = 3;

/// Windows the writer may hand over beyond those the stage is working on.
/// Rows are readable once written, and a keyword or graph question answers
/// from them, so a store filling from cold answers about thousands of chunks
/// in its first seconds rather than the few hundred its lanes have embedded.
/// Each window holds token ids, not vectors: at most about 400 KB.
const WRITE_AHEAD: usize = 16;

/// How far the prepare stage may run ahead of the writer, per thread. Bounds
/// the memory the stage holds: at most this many files per thread are read
/// and waiting.
const LOOKAHEAD_PER_THREAD: usize = 4;

/// The seconds of work one batch should be, from the lane's own rate. Long
/// enough that the frame and the call cost nothing against it, short enough
/// that a pause or a stop lands inside a second.
const BATCH_SECONDS: f64 = 0.2;

// ------------------------------------------------------------------- timings

/// Where a run's wall time went, stage by stage.
///
/// The writer's wall time is the run's, and it is split by construction: time
/// in the walk, time the writer spent waiting for the prepare stage (shared
/// out between that stage's parts by the CPU time each took), time waiting for
/// the embed stage (shared out between the lanes by the chunks each embedded),
/// and everything else the writer did, which is `write`. So the parts sum to
/// the wall time. `prepare_cpu_ms` is the prepare stage's own CPU time per
/// part, summed over its threads, which is what says which part to speed up.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Stages {
    pub wall_ms: u64,
    pub walk_ms: u64,
    pub read_ms: u64,
    pub extract_ms: u64,
    pub parse_ms: u64,
    pub tokenize_ms: u64,
    pub write_ms: u64,
    pub embed_wait_ms: BTreeMap<String, u64>,
    pub prepare_cpu_ms: BTreeMap<String, u64>,
}

impl Stages {
    /// Fold another slice's figures into this run's.
    pub fn add(&mut self, other: &Stages) {
        self.wall_ms += other.wall_ms;
        self.walk_ms += other.walk_ms;
        self.read_ms += other.read_ms;
        self.extract_ms += other.extract_ms;
        self.parse_ms += other.parse_ms;
        self.tokenize_ms += other.tokenize_ms;
        self.write_ms += other.write_ms;
        for (lane, ms) in &other.embed_wait_ms {
            *self.embed_wait_ms.entry(lane.clone()).or_default() += ms;
        }
        for (part, ms) in &other.prepare_cpu_ms {
            *self.prepare_cpu_ms.entry(part.clone()).or_default() += ms;
        }
    }

    /// The parts, which sum to `wall_ms`.
    pub fn parts_ms(&self) -> u64 {
        self.walk_ms
            + self.read_ms
            + self.extract_ms
            + self.parse_ms
            + self.tokenize_ms
            + self.write_ms
            + self.embed_wait_ms.values().sum::<u64>()
    }

    /// One line for a log or `index --verbose`.
    pub fn line(&self) -> String {
        let waits: Vec<String> = self
            .embed_wait_ms
            .iter()
            .map(|(lane, ms)| format!("embed wait ({lane}) {}", secs(*ms)))
            .collect();
        let mut parts = vec![
            format!("walk {}", secs(self.walk_ms)),
            format!("read+hash {}", secs(self.read_ms)),
            format!("extract+scan {}", secs(self.extract_ms)),
            format!("parse+chunk {}", secs(self.parse_ms)),
            format!("tokenize {}", secs(self.tokenize_ms)),
        ];
        parts.extend(waits);
        parts.push(format!("write {}", secs(self.write_ms)));
        format!("stages over {}: {}", secs(self.wall_ms), parts.join(", "))
    }
}

fn secs(ms: u64) -> String {
    format!("{:.1} s", ms as f64 / 1000.0)
}

/// The prepare stage's CPU time per part, summed over its threads.
#[derive(Default)]
pub struct Clocks {
    read: AtomicU64,
    extract: AtomicU64,
    parse: AtomicU64,
    tokenize: AtomicU64,
}

impl Clocks {
    fn tick(slot: &AtomicU64, since: Instant) -> Instant {
        let now = Instant::now();
        slot.fetch_add(
            now.duration_since(since).as_micros() as u64,
            Ordering::Relaxed,
        );
        now
    }

    /// `[read, extract, parse, tokenize]` in microseconds.
    fn read_all(&self) -> [u64; 4] {
        [
            self.read.load(Ordering::Relaxed),
            self.extract.load(Ordering::Relaxed),
            self.parse.load(Ordering::Relaxed),
            self.tokenize.load(Ordering::Relaxed),
        ]
    }
}

/// The writer's own clock: what it waited on, and for how long.
pub struct WriterClock {
    started: Instant,
    pub walk_ms: u64,
    prepare_wait: Duration,
    embed_wait: Duration,
}

impl WriterClock {
    pub fn start(walk_ms: u64) -> Self {
        Self {
            started: Instant::now(),
            walk_ms,
            prepare_wait: Duration::ZERO,
            embed_wait: Duration::ZERO,
        }
    }

    pub fn waited_on_prepare(&mut self, since: Instant) {
        self.prepare_wait += since.elapsed();
    }

    pub fn waited_on_embed(&mut self, since: Instant) {
        self.embed_wait += since.elapsed();
    }

    /// The writer waited for a walk that ran beside its first files: that
    /// wait is the walk's share of the run.
    pub fn waited_on_walk(&mut self, since: Instant) {
        self.walk_ms += since.elapsed().as_millis() as u64;
    }

    /// Settle the split: `lanes` is the chunks each lane embedded this call.
    pub fn stages(&self, clocks: &Clocks, lanes: &BTreeMap<String, usize>) -> Stages {
        let wall = self.started.elapsed().as_millis() as u64 + self.walk_ms;
        let prepare = self.prepare_wait.as_millis() as u64;
        let embed = self.embed_wait.as_millis() as u64;
        let cpu = clocks.read_all();
        let cpu_total: u64 = cpu.iter().sum();
        let share = |part: u64| {
            if cpu_total == 0 {
                0
            } else {
                (prepare as u128 * part as u128 / cpu_total as u128) as u64
            }
        };
        let mut stages = Stages {
            wall_ms: wall,
            walk_ms: self.walk_ms,
            read_ms: share(cpu[0]),
            extract_ms: share(cpu[1]),
            parse_ms: share(cpu[2]),
            tokenize_ms: share(cpu[3]),
            ..Stages::default()
        };
        // A prepare wait with no CPU recorded (every file unchanged, say) is
        // still time spent reading: it goes to read+hash rather than nowhere.
        if cpu_total == 0 {
            stages.read_ms = prepare;
        }
        let chunks: usize = lanes.values().sum();
        if embed > 0 {
            if chunks == 0 {
                stages.embed_wait_ms.insert("cpu".to_string(), embed);
            } else {
                let mut given = 0;
                let last = lanes.len().saturating_sub(1);
                for (i, (lane, n)) in lanes.iter().enumerate() {
                    let ms = if i == last {
                        embed - given
                    } else {
                        (embed as u128 * *n as u128 / chunks as u128) as u64
                    };
                    given += ms;
                    stages.embed_wait_ms.insert(lane.clone(), ms);
                }
            }
        }
        let accounted = stages.parts_ms();
        stages.write_ms = wall.saturating_sub(accounted);
        for (name, us) in ["read+hash", "extract+scan", "parse+chunk", "tokenize"]
            .iter()
            .zip(cpu)
        {
            stages.prepare_cpu_ms.insert(name.to_string(), us / 1000);
        }
        stages
    }
}

// ---------------------------------------------------------------- the pieces

/// What the embed stage is handed for one chunk.
#[derive(Debug, Clone)]
pub enum Piece {
    /// granite: token ids, tokenised once by the prepare stage.
    Ids(Ids),
    /// A store built with one of fastembed's own models, which tokenises for
    /// itself and runs on the CPU only.
    Text(String),
    /// Already embedded: the machine-wide cache held it.
    Cached {
        vector: Vec<f32>,
        variant: &'static str,
    },
}

impl Piece {
    /// Its length in tokens, or an estimate of it for text. A cached piece
    /// is never scheduled, and costs nothing.
    pub fn len(&self) -> usize {
        match self {
            Piece::Ids(ids) => ids.len(),
            Piece::Text(text) => text.len() / 4 + 1,
            Piece::Cached { .. } => 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ------------------------------------------------------------------- prepare

/// What the prepare stage needs to know, all of it read before the run starts
/// and none of it changed by the run: the writer is the only thing that moves
/// a file's hash, and it moves each one at most once per run.
pub struct Context {
    pub rechunk: bool,
    pub rescan: bool,
    pub regraph: bool,
    pub allow_secrets: bool,
    /// Hash recorded per stored path.
    pub hashes: HashMap<String, String>,
    /// Stored paths of a counted format with no count yet: read again, even
    /// when unchanged, so their count can be written (never re-embedded).
    pub uncounted: HashSet<String>,
    /// Paths a person accepted over the size cap.
    pub over_cap: HashSet<String>,
    /// Size, mtime and hash the scan phase read.
    pub prehashed: HashMap<PathBuf, (u64, i64, String)>,
    /// The boundary to hold each file to, for a slice that did not check its
    /// files up front.
    pub each: Option<crate::ResolvedBoundary>,
    /// granite's tokenizer; `None` for a store built with another model.
    pub tokenizer: Option<tokenizers::Tokenizer>,
    pub clocks: Clocks,
    /// What the vector cache keys on, when the cache is on for this run.
    pub cache: Option<crate::cache::Scope>,
    /// The cache this run reads and writes: the store's own, or the machine's.
    pub cache_at: Option<crate::cache::Location>,
    /// The variants a cached vector may be, in the order to try them: the
    /// lanes this run would use, best first, then the CPU's.
    pub variants: Vec<&'static str>,
    /// Cache lookups and hits the prepare stage made.
    pub lookups: AtomicU64,
    pub hits: AtomicU64,
}

/// One file, as far as it can be taken without the database.
pub enum Prepared {
    /// Outside this caller's boundary.
    Refused(crate::Refusal),
    /// Unchanged by the scan phase's reading: nothing was read again.
    Unchanged {
        size: u64,
        mtime: i64,
        file_bytes: u64,
    },
    /// Opened (or not) and found unusable before or while reading.
    Unusable {
        why: SkipReason,
        gone: bool,
        file_bytes: u64,
    },
    /// Read, but the same bytes the store holds.
    Same {
        len: u64,
        mtime: Option<i64>,
        file_bytes: u64,
        /// The text and what the scan found, when the scan rules moved.
        rescan: Option<(String, Vec<keyscan::Match>)>,
        /// A fresh extraction, when the graph rules moved.
        regraph: Option<graph::Extraction>,
        /// The file's units, when the store has none for it yet.
        units: Option<i64>,
    },
    /// An image, for the writer to embed with CLIP.
    Image {
        bytes: Vec<u8>,
        hash: String,
        file_bytes: u64,
    },
    /// A reader or parser failed on this file's own content.
    Failed {
        error: anyhow::Error,
        file_bytes: u64,
    },
    /// A reader ran and gave nothing usable.
    Skipped { why: SkipReason, file_bytes: u64 },
    /// Text, scanned, and — unless the scan found something live, which the
    /// writer must decide with the database — parsed, chunked and tokenised.
    Text {
        len: u64,
        hash: String,
        file_bytes: u64,
        text: String,
        /// Pages, slides or cells, for a counted format.
        units: Option<i64>,
        found: Vec<keyscan::Match>,
        /// `None` when the writer must decide the text first; an error when
        /// the parse or the chunking failed on this file's own content, which
        /// the writer reports after the scan's bookkeeping, as it always did.
        ready: Option<anyhow::Result<Ready>>,
    },
}

/// A file taken all the way: its extraction, its chunks, and a piece per
/// chunk.
pub struct Ready {
    pub extraction: Option<graph::Extraction>,
    pub chunks: Vec<chunk::Chunk>,
    pub pieces: Vec<Piece>,
    /// Each chunk's embedded-text hash, for the cache; `None` with it off.
    pub hashes: Vec<Option<[u8; 32]>>,
}

fn mtime_of(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs() as i64)
}

/// Take one file as far as it goes without the database, in the order the
/// writer used to: every early exit here is one the writer's loop had.
pub fn prepare(path: &Path, ctx: &Context, cache: Option<&crate::cache::Cache>) -> Prepared {
    if let Some(boundary) = &ctx.each
        && let Some(refusal) = boundary.refuses(path, true)
    {
        return Prepared::Refused(refusal);
    }
    let started = Instant::now();
    let key = path.to_string_lossy();
    let opened = std::fs::File::open(path);
    let measured = opened.as_ref().ok().and_then(|f| f.metadata().ok());
    let file_bytes = crate::embeddable_bytes(measured.clone());
    if !ctx.rechunk
        && !ctx.rescan
        && !ctx.regraph
        && let (Some((size, mtime, hash)), Some(meta)) = (ctx.prehashed.get(path), &measured)
        && meta.len() == *size
        && mtime_of(meta) == *mtime
        && ctx.hashes.get(key.as_ref()) == Some(hash)
        && !ctx.uncounted.contains(key.as_ref())
    {
        Clocks::tick(&ctx.clocks.read, started);
        return Prepared::Unchanged {
            size: *size,
            mtime: *mtime,
            file_bytes,
        };
    }
    let cap = if ctx.over_cap.contains(key.as_ref()) {
        u64::MAX - 1
    } else {
        chunk::MAX_FILE_BYTES
    };
    let unusable = match (&opened, &measured) {
        (Err(e), _) => Some(SkipReason::Unreadable(e.to_string())),
        (_, None) => Some(SkipReason::Unreadable(
            "its metadata could not be read".to_string(),
        )),
        (_, Some(m)) if !m.is_file() => Some(SkipReason::NotRegular),
        (_, Some(m)) if m.len() == 0 => Some(SkipReason::Empty),
        (_, Some(m)) if m.len() > cap => Some(SkipReason::TooLarge),
        _ => None,
    };
    if let Some(why) = unusable {
        Clocks::tick(&ctx.clocks.read, started);
        return Prepared::Unusable {
            why,
            gone: !path.exists(),
            file_bytes,
        };
    }
    let read = opened.and_then(|file| {
        use std::io::Read;
        let mut bytes = Vec::new();
        file.take(cap + 1).read_to_end(&mut bytes).map(|_| bytes)
    });
    let bytes = match read {
        Ok(bytes) if bytes.len() as u64 <= cap => bytes,
        Ok(_) => {
            Clocks::tick(&ctx.clocks.read, started);
            return Prepared::Unusable {
                why: SkipReason::TooLarge,
                gone: false,
                file_bytes,
            };
        }
        Err(e) => {
            Clocks::tick(&ctx.clocks.read, started);
            return Prepared::Unusable {
                why: SkipReason::Unreadable(e.to_string()),
                gone: false,
                file_bytes,
            };
        }
    };
    let hash = blake3::hash(&bytes).to_hex().to_string();
    let now = Clocks::tick(&ctx.clocks.read, started);
    let len = bytes.len() as u64;

    let same = !ctx.rechunk && ctx.hashes.get(key.as_ref()) == Some(&hash);
    if same {
        let is_image = image::is_image(path);
        let counting = ctx.uncounted.contains(key.as_ref());
        let extracted = ((ctx.rescan && !ctx.allow_secrets && !is_image) || counting)
            .then(|| {
                crate::contained(|| chunk::extract_counted(path, &bytes))
                    .ok()?
                    .ok()
            })
            .flatten();
        // A file that will not give a count still gets one, zero, so it is not
        // read again on every pass.
        let units = counting.then(|| extracted.as_ref().and_then(|(_, n)| *n).unwrap_or(0));
        let rescan = extracted
            .filter(|_| ctx.rescan && !ctx.allow_secrets && !is_image)
            .map(|(text, _)| {
                let found = keyscan::scan(&key, &text);
                (text, found)
            });
        let now = Clocks::tick(&ctx.clocks.extract, now);
        let regraph = (ctx.regraph && !is_image && graph::language_of(path).is_some())
            .then(|| {
                let text = match &rescan {
                    Some((text, _)) => Some(text.clone()),
                    None => chunk::extract(path, &bytes).ok(),
                }?;
                crate::contained(|| graph::extract(path, &text))
                    .ok()
                    .and_then(|r| r.ok())
                    .flatten()
            })
            .flatten();
        Clocks::tick(&ctx.clocks.parse, now);
        return Prepared::Same {
            len,
            mtime: measured.as_ref().map(mtime_of),
            file_bytes,
            rescan,
            regraph,
            units,
        };
    }

    if image::is_image(path) {
        return Prepared::Image {
            bytes,
            hash,
            file_bytes,
        };
    }

    let text = match crate::contained(|| {
        crate::fault_panic(path);
        chunk::extract_counted(path, &bytes)
    }) {
        Err(error) => {
            Clocks::tick(&ctx.clocks.extract, now);
            return Prepared::Failed { error, file_bytes };
        }
        Ok(Err(why)) => {
            Clocks::tick(&ctx.clocks.extract, now);
            return Prepared::Skipped { why, file_bytes };
        }
        Ok(Ok(extracted)) => extracted,
    };
    let (text, units) = text;
    drop(bytes);
    let found = keyscan::scan(&key, &text);
    let now = Clocks::tick(&ctx.clocks.extract, now);
    // A live match needs the database to decide — an acceptance may let it
    // in, redacted — so the writer takes the file from here.
    let live = !ctx.allow_secrets && keyscan::refuses(&found);
    let ready = (!live).then(|| take_ready(path, &text, ctx, cache, now));
    Prepared::Text {
        len,
        hash,
        file_bytes,
        text,
        units,
        found,
        ready,
    }
}

/// Parse, chunk and tokenise one file's decided text. Also the writer's own
/// path for a file whose text only the database could decide.
pub fn take_ready(
    path: &Path,
    text: &str,
    ctx: &Context,
    cache: Option<&crate::cache::Cache>,
    since: Instant,
) -> anyhow::Result<Ready> {
    let extraction = crate::contained(|| graph::extract(path, text)).and_then(|r| r)?;
    let chunks = crate::contained(|| {
        chunk::chunk_file(
            path,
            text,
            extraction.as_ref().map_or(&[][..], |e| &e.symbols),
        )
    })?;
    let now = Clocks::tick(&ctx.clocks.parse, since);
    let mut pieces = Vec::with_capacity(chunks.len());
    let mut hashes = Vec::with_capacity(chunks.len());
    for c in &chunks {
        let text = c.embedded();
        // Consulted before tokenising: a hit costs a lookup and nothing else.
        let hash = ctx.cache.as_ref().map(|_| crate::cache::Scope::text(&text));
        if let (Some(scope), Some(cache), Some(hash)) = (&ctx.cache, cache, &hash) {
            ctx.lookups.fetch_add(1, Ordering::Relaxed);
            let found = ctx.variants.iter().find_map(|variant| {
                cache
                    .get(&scope.key(hash, variant))
                    .filter(|v| v.len() == crate::session::DIM)
                    .map(|vector| Piece::Cached { vector, variant })
            });
            if let Some(piece) = found {
                ctx.hits.fetch_add(1, Ordering::Relaxed);
                pieces.push(piece);
                hashes.push(Some(*hash));
                continue;
            }
        }
        pieces.push(match &ctx.tokenizer {
            Some(tokenizer) => Piece::Ids(crate::session::encode(tokenizer, &text)?),
            None => Piece::Text(text),
        });
        hashes.push(hash);
    }
    Clocks::tick(&ctx.clocks.tokenize, now);
    Ok(Ready {
        extraction,
        chunks,
        pieces,
        hashes,
    })
}

// ------------------------------------------------------------------ prefetch

/// The prepare stage: a pool that works ahead of the writer, and hands each
/// file back in the order the writer asks for them.
///
/// It owns the list of paths, which can grow: a first slice starts on the
/// files changed most recently while the walk of the whole tree is still
/// going, and the rest is added when the walk is done. `seal` says nothing
/// more is coming.
pub struct Prefetch {
    shared: Arc<Shared>,
}

struct Shared {
    next: AtomicUsize,
    /// The paths so far, and whether that is all of them.
    list: Mutex<(Vec<PathBuf>, bool)>,
    grown: Condvar,
    /// Results not yet taken, by position, and the position the writer is at.
    done: Mutex<(BTreeMap<usize, Prepared>, usize)>,
    ready: Condvar,
    room: Condvar,
    stop: AtomicBool,
    lookahead: usize,
}

impl Prefetch {
    /// Start `threads` workers inside `scope` on `paths`, which is the whole
    /// list when `sealed`.
    pub fn start<'scope, 'env>(
        scope: &'scope std::thread::Scope<'scope, 'env>,
        paths: Vec<PathBuf>,
        sealed: bool,
        ctx: &'env Context,
        threads: usize,
    ) -> Self {
        let threads = threads.clamp(1, 32);
        let shared = Arc::new(Shared {
            next: AtomicUsize::new(0),
            list: Mutex::new((paths, sealed)),
            grown: Condvar::new(),
            done: Mutex::new((BTreeMap::new(), 0)),
            ready: Condvar::new(),
            room: Condvar::new(),
            stop: AtomicBool::new(false),
            lookahead: threads * LOOKAHEAD_PER_THREAD,
        });
        for n in 0..threads {
            let shared = Arc::clone(&shared);
            let _ = std::thread::Builder::new()
                .name(format!("semlith-prepare-{n}"))
                .spawn_scoped(scope, move || {
                    let _class = crate::priority::indexing_thread();
                    // A connection per thread: SQLite's are not shared.
                    let cache = ctx.cache_at.as_ref().and_then(crate::cache::Cache::open_in);
                    loop {
                        let i = shared.next.fetch_add(1, Ordering::Relaxed);
                        // The path at `i`, waiting for the list to grow to it,
                        // or gone when it never will.
                        let path = {
                            let mut list = shared.list.lock().unwrap_or_else(|e| e.into_inner());
                            loop {
                                if shared.stop.load(Ordering::Relaxed) {
                                    return;
                                }
                                if let Some(path) = list.0.get(i) {
                                    break path.clone();
                                }
                                if list.1 {
                                    return;
                                }
                                list = shared
                                    .grown
                                    .wait_timeout(list, Duration::from_millis(100))
                                    .unwrap_or_else(|e| e.into_inner())
                                    .0;
                            }
                        };
                        // Held back until the writer is close enough.
                        {
                            let mut done = shared.done.lock().unwrap_or_else(|e| e.into_inner());
                            while i >= done.1 + shared.lookahead
                                && !shared.stop.load(Ordering::Relaxed)
                            {
                                done = shared.room.wait(done).unwrap_or_else(|e| e.into_inner());
                            }
                        }
                        if shared.stop.load(Ordering::Relaxed) {
                            return;
                        }
                        let prepared =
                            match crate::contained(|| prepare(&path, ctx, cache.as_ref())) {
                                Ok(prepared) => prepared,
                                Err(error) => Prepared::Failed {
                                    error,
                                    file_bytes: 0,
                                },
                            };
                        let mut done = shared.done.lock().unwrap_or_else(|e| e.into_inner());
                        done.0.insert(i, prepared);
                        shared.ready.notify_all();
                    }
                });
        }
        Self { shared }
    }

    /// Add paths to the end of the list, and say it is complete.
    pub fn extend_and_seal(&self, more: Vec<PathBuf>) {
        let mut list = self.shared.list.lock().unwrap_or_else(|e| e.into_inner());
        list.0.extend(more);
        list.1 = true;
        self.shared.grown.notify_all();
    }

    /// Paths in the list now, and whether that is all of them.
    pub fn len(&self) -> (usize, bool) {
        let list = self.shared.list.lock().unwrap_or_else(|e| e.into_inner());
        (list.0.len(), list.1)
    }

    pub fn path(&self, i: usize) -> Option<PathBuf> {
        let list = self.shared.list.lock().unwrap_or_else(|e| e.into_inner());
        list.0.get(i).cloned()
    }

    /// Every path from `i` on, for the slice that continues the run.
    pub fn from(&self, i: usize) -> Vec<PathBuf> {
        let list = self.shared.list.lock().unwrap_or_else(|e| e.into_inner());
        list.0.get(i..).map(<[PathBuf]>::to_vec).unwrap_or_default()
    }

    /// The file at `i`, waiting for it if the pool has not got there yet.
    pub fn take(&self, i: usize) -> Prepared {
        let mut done = self.shared.done.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(prepared) = done.0.remove(&i) {
                done.1 = done.1.max(i + 1);
                self.shared.room.notify_all();
                return prepared;
            }
            // Every worker has gone and none left it: a worker that died
            // outside `contained` lost it. Say so rather than wait for ever.
            if Arc::strong_count(&self.shared) == 1 {
                return Prepared::Failed {
                    error: anyhow::anyhow!("the prepare stage lost this file"),
                    file_bytes: 0,
                };
            }
            done = self
                .shared
                .ready
                .wait_timeout(done, Duration::from_millis(50))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
}

impl Drop for Prefetch {
    /// The writer is done early — a stop, a yield, a deadline: the workers
    /// finish the file in hand and go.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        let _list = self.shared.list.lock().unwrap_or_else(|e| e.into_inner());
        self.shared.grown.notify_all();
        drop(_list);
        let _guard = self.shared.done.lock().unwrap_or_else(|e| e.into_inner());
        self.shared.room.notify_all();
        self.shared.ready.notify_all();
    }
}

// --------------------------------------------------------------------- embed

/// A window of chunks the writer has committed rows for, in id order.
pub struct Window {
    pub ids: Vec<u64>,
    pub pieces: Vec<Piece>,
    /// Each piece's embedded-text hash, for the cache; `None` with it off.
    pub hashes: Vec<Option<[u8; 32]>>,
}

/// A window embedded: vectors in the window's own order, and who made them.
pub struct Embedded {
    pub ids: Vec<u64>,
    pub vectors: Vec<Vec<f32>>,
    /// Which variant made each vector, and whether the cache supplied it.
    pub rows: Vec<(&'static str, bool)>,
    pub hashes: Vec<Option<[u8; 32]>>,
    /// Chunks per vector variant, for the store's `variants` row.
    pub variants: Vec<(&'static str, usize)>,
    /// Chunks per lane, for the run card.
    pub lanes: Vec<(&'static str, usize)>,
}

/// The CPU session and what it is asked to run, as a lane of its own on a
/// thread of its own, so the writer never runs the model.
pub enum CpuWork {
    /// granite on ONNX Runtime directly, fed ids. `alt` is the harness's
    /// `mix`, alternating variants batch by batch.
    Ids {
        main: crate::session::CpuSession,
        alt: Option<crate::session::CpuSession>,
    },
    /// fastembed's own session, for a store built with one of its models.
    Text(Box<fastembed::TextEmbedding>),
}

/// A batch's vectors and the variant that made them, or why not.
type Vectors = Result<(Vec<Vec<f32>>, &'static str), String>;

type CpuAnswer = mpsc::Receiver<Vectors>;

/// Where one batch's vectors will arrive: the CPU lane says which variant it
/// ran, a worker lane has one variant only.
enum Answer {
    Cpu(CpuAnswer),
    Lane(mpsc::Receiver<Result<Vec<Vec<f32>>, String>>, &'static str),
}

impl Answer {
    /// The answer if it is in; `Err(true)` while it is still coming, and
    /// `Err(false)` when it never will.
    fn wait(&self, timeout: Option<Duration>) -> Result<Vectors, bool> {
        let lift = |e: mpsc::RecvTimeoutError| matches!(e, mpsc::RecvTimeoutError::Timeout);
        let lift_try = |e: mpsc::TryRecvError| matches!(e, mpsc::TryRecvError::Empty);
        match (self, timeout) {
            (Answer::Cpu(rx), Some(t)) => rx.recv_timeout(t).map_err(lift),
            (Answer::Cpu(rx), None) => rx.try_recv().map_err(lift_try),
            (Answer::Lane(rx, variant), Some(t)) => rx
                .recv_timeout(t)
                .map(|r| r.map(|v| (v, *variant)))
                .map_err(lift),
            (Answer::Lane(rx, variant), None) => rx
                .try_recv()
                .map(|r| r.map(|v| (v, *variant)))
                .map_err(lift_try),
        }
    }
}

struct CpuJob {
    batch: Vec<Piece>,
    reply: mpsc::Sender<Vectors>,
}

/// Run the CPU lane until its channel closes, then hand the session back.
/// The intra-op threads the CPU lane of the run going now embeds with, for
/// the run card: a count saved mid-run changes it at the next batch.
pub static CPU_THREADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn cpu_lane(mut work: CpuWork, jobs: mpsc::Receiver<CpuJob>) -> CpuWork {
    let mut batches = 0u64;
    while let Ok(job) = jobs.recv() {
        batches += 1;
        let answer = match &mut work {
            CpuWork::Ids { main, alt } => {
                main.follow_threads();
                if let Some(alt) = alt.as_mut() {
                    alt.follow_threads();
                }
                CPU_THREADS.store(main.threads(), Ordering::Relaxed);
                let rows: Vec<&[u32]> = job
                    .batch
                    .iter()
                    .map(|piece| match piece {
                        Piece::Ids(ids) => ids.as_slice(),
                        _ => &[],
                    })
                    .collect();
                let session = match alt {
                    Some(alt) if batches.is_multiple_of(2) => alt,
                    _ => main,
                };
                let variant = session.variant().name();
                session
                    .embed(&rows)
                    .map(|v| (v, variant))
                    .map_err(|e| format!("{e:#}"))
            }
            CpuWork::Text(model) => {
                let texts: Vec<&str> = job
                    .batch
                    .iter()
                    .map(|piece| match piece {
                        Piece::Text(text) => text.as_str(),
                        _ => "",
                    })
                    .collect();
                model
                    .embed(&texts, Some(texts.len().max(1)))
                    .map(|mut out| {
                        for v in &mut out {
                            crate::normalize(v);
                        }
                        (out, crate::embed::Variant::Int8.name())
                    })
                    .map_err(|e| format!("{e}"))
            }
        };
        crate::accel::count_cpu(job.batch.len());
        let _ = job.reply.send(answer);
    }
    work
}

/// One place a batch can go.
#[derive(Clone)]
enum Target {
    Cpu,
    Lane(Arc<crate::accel::Lane>),
}

impl Target {
    fn id(&self) -> &'static str {
        match self {
            Target::Cpu => "cpu",
            Target::Lane(lane) => lane.id,
        }
    }
}

/// Which queue a position was taken from.
enum Taken {
    CpuOnly,
    Retry,
    Order,
}

/// Whether a vector may go into a store: every value finite, and not all
/// zero. A Neural Engine LayerNorm that overflowed fp16 returns zeros, and
/// nothing downstream would notice.
pub fn sound(vector: &[f32]) -> bool {
    vector.iter().all(|v| v.is_finite()) && vector.iter().any(|v| *v != 0.0)
}

struct Active {
    ids: Vec<u64>,
    pieces: Vec<Piece>,
    /// Positions sorted shortest first; the CPU takes from the front and an
    /// accelerator from the back, so each gets what it is best at.
    order: Vec<usize>,
    front: usize,
    back: usize,
    /// Positions a failed batch handed back, taken before anything else.
    retry: VecDeque<usize>,
    /// Positions whose vector came back unsound — not finite, or all zero —
    /// which only the CPU may take again.
    cpu_only: VecDeque<usize>,
    vectors: Vec<Option<Vec<f32>>>,
    rows: Vec<(&'static str, bool)>,
    hashes: Vec<Option<[u8; 32]>>,
    left: usize,
    variants: BTreeMap<&'static str, usize>,
    lanes: BTreeMap<&'static str, usize>,
}

impl Active {
    fn new(mut window: Window) -> Self {
        let n = window.pieces.len();
        let mut vectors = vec![None; n];
        let mut rows = vec![("", false); n];
        let mut variants: BTreeMap<&'static str, usize> = BTreeMap::new();
        let mut lanes: BTreeMap<&'static str, usize> = BTreeMap::new();
        // What the cache supplied is done before the window starts.
        for (at, piece) in window.pieces.iter_mut().enumerate() {
            if let Piece::Cached { vector, variant } = piece {
                vectors[at] = Some(std::mem::take(vector));
                rows[at] = (*variant, true);
                *variants.entry(*variant).or_default() += 1;
                *lanes.entry("cache").or_default() += 1;
            }
        }
        let mut order: Vec<usize> = (0..n).filter(|i| vectors[*i].is_none()).collect();
        // Stable, so equal lengths keep their order and a run's batches are
        // the same batches every time (#88).
        if std::env::var(crate::UNSORTED_ENV).is_err() {
            order.sort_by_key(|&i| window.pieces[i].len());
        }
        let left = order.len();
        if window.hashes.len() != n {
            window.hashes = vec![None; n];
        }
        Self {
            ids: window.ids,
            pieces: window.pieces,
            back: order.len(),
            order,
            front: 0,
            retry: VecDeque::new(),
            cpu_only: VecDeque::new(),
            vectors,
            rows,
            hashes: window.hashes,
            left,
            variants,
            lanes,
        }
    }

    fn untaken(&self, cpu: bool) -> bool {
        (cpu && !self.cpu_only.is_empty()) || !self.retry.is_empty() || self.front < self.back
    }

    /// Up to `budget` padded tokens: rows times the longest row, which is what
    /// the device computes, in a whole number of `per_call` rows where there
    /// are that many: a model converted at a fixed batch computes the rest as
    /// padding. The CPU (`from_back` false) takes what only it may first.
    fn take(&mut self, from_back: bool, budget: usize, per_call: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut longest = 0;
        loop {
            let next = if !from_back && let Some(&at) = self.cpu_only.front() {
                Some((at, Taken::CpuOnly))
            } else if let Some(&at) = self.retry.front() {
                Some((at, Taken::Retry))
            } else if self.front < self.back {
                Some((
                    if from_back {
                        self.order[self.back - 1]
                    } else {
                        self.order[self.front]
                    },
                    Taken::Order,
                ))
            } else {
                None
            };
            let Some((at, taken)) = next else { break };
            let len = self.pieces[at].len().max(1);
            let cost = (out.len() + 1) * longest.max(len);
            if !out.is_empty() && cost > budget && out.len() % per_call.max(1) == 0 {
                break;
            }
            longest = longest.max(len);
            out.push(at);
            match taken {
                Taken::CpuOnly => {
                    self.cpu_only.pop_front();
                }
                Taken::Retry => {
                    self.retry.pop_front();
                }
                Taken::Order if from_back => self.back -= 1,
                Taken::Order => self.front += 1,
            }
        }
        out
    }
}

/// What the scheduler reads about a lane's pace, per lane.
struct Pace {
    tokens: u64,
    busy: Duration,
}

/// The padded tokens a batch on this lane should carry: [`BATCH_SECONDS`] of
/// its own measured pace, inside the lane's floor and ceiling.
fn budget(target: &Target, pace: Option<&Pace>) -> usize {
    let (floor, start, ceiling) = match target {
        Target::Cpu => (512, 2_048, 8_192),
        Target::Lane(lane) => lane.token_budget(),
    };
    match pace {
        Some(p) if p.busy.as_secs_f64() > 0.5 => {
            let per_second = p.tokens as f64 / p.busy.as_secs_f64();
            ((per_second * BATCH_SECONDS) as usize).clamp(floor, ceiling)
        }
        _ => start,
    }
}

/// The run's side of the embed stage.
pub struct Embedder {
    windows: Option<mpsc::SyncSender<Window>>,
    done: mpsc::Receiver<Result<Embedded, String>>,
    pub abort: Arc<AtomicBool>,
    /// Chunks whose batch has come back from a device, counted as each batch
    /// does rather than as its window lands: what a run's progress moves on
    /// while the writer waits.
    pub batched: Arc<AtomicUsize>,
    /// Windows sent and not yet received back.
    pub outstanding: usize,
}

impl Embedder {
    /// Start the CPU lane and the scheduler inside `scope`. `lanes` is whether
    /// this store may use the accelerator lanes at all: only granite stores
    /// may, because every lane runs granite.
    pub fn start<'scope>(
        scope: &'scope std::thread::Scope<'scope, '_>,
        cpu: CpuWork,
        lanes: bool,
        cpu_back: mpsc::Sender<CpuWork>,
        paused: Arc<AtomicBool>,
    ) -> Self {
        let (windows_tx, windows_rx) = mpsc::sync_channel::<Window>(WRITE_AHEAD);
        let (done_tx, done_rx) = mpsc::channel();
        let (cpu_tx, cpu_rx) = mpsc::channel::<CpuJob>();
        let abort = Arc::new(AtomicBool::new(false));
        std::thread::Builder::new()
            .name("semlith-cpu-lane".to_string())
            .spawn_scoped(scope, move || {
                let _class = crate::priority::indexing_thread();
                let _ = cpu_back.send(cpu_lane(cpu, cpu_rx));
            })
            .expect("spawning the CPU lane");
        let batched = Arc::new(AtomicUsize::new(0));
        let (a, p, b) = (
            Arc::clone(&abort),
            Arc::clone(&paused),
            Arc::clone(&batched),
        );
        std::thread::Builder::new()
            .name("semlith-embed".to_string())
            .spawn_scoped(scope, move || {
                let _class = crate::priority::indexing_thread();
                schedule(windows_rx, done_tx, cpu_tx, lanes, &a, &p, &b);
            })
            .expect("spawning the embed stage");
        Self {
            windows: Some(windows_tx),
            done: done_rx,
            abort,
            batched,
            outstanding: 0,
        }
    }

    /// Hand a window over, waiting while the stage is full. `false` if the
    /// stage has gone, which only an abort does.
    pub fn send(&mut self, window: Window, mut wait: impl FnMut() -> bool) -> bool {
        let Some(tx) = &self.windows else {
            return false;
        };
        let mut window = window;
        loop {
            match tx.try_send(window) {
                Ok(()) => {
                    self.outstanding += 1;
                    return true;
                }
                Err(mpsc::TrySendError::Full(back)) => {
                    window = back;
                    if !wait() {
                        return false;
                    }
                }
                Err(mpsc::TrySendError::Disconnected(_)) => return false,
            }
        }
    }

    /// The next finished window, waiting up to `timeout`.
    pub fn next(&mut self, timeout: Duration) -> Option<Result<Embedded, String>> {
        match self.done.recv_timeout(timeout) {
            Ok(done) => {
                self.outstanding = self.outstanding.saturating_sub(1);
                Some(done)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                self.outstanding = 0;
                Some(Err("the embed stage ended".to_string()))
            }
        }
    }

    /// No more windows: the stage finishes what it holds and ends.
    pub fn close(&mut self) {
        self.windows = None;
    }

    /// Give up on everything in flight.
    pub fn cancel(&mut self) {
        self.abort.store(true, Ordering::Relaxed);
        self.windows = None;
    }
}

/// The scheduler: windows in, embedded windows out, in order.
fn schedule(
    windows: mpsc::Receiver<Window>,
    done: mpsc::Sender<Result<Embedded, String>>,
    cpu: mpsc::Sender<CpuJob>,
    may_use_lanes: bool,
    abort: &AtomicBool,
    paused: &AtomicBool,
    batched: &AtomicUsize,
) {
    let mut active: VecDeque<Active> = VecDeque::new();
    let mut closed = false;
    let mut flying: Vec<(Target, usize, Vec<usize>, Answer, Instant, usize)> = Vec::new();
    // Windows retire from the front; `base` is the sequence of `active[0]`.
    let mut base = 0usize;
    let mut pace: HashMap<&'static str, Pace> = HashMap::new();
    // When each target last gave a batch back: a batch queued behind another
    // is busy from then, not from when it was sent.
    let mut last_back: HashMap<&'static str, Instant> = HashMap::new();
    loop {
        if abort.load(Ordering::Relaxed) {
            return;
        }
        // Take windows while there is room; wait for one only when idle.
        while !closed && active.len() < ACTIVE_WINDOWS {
            let got = if active.is_empty() && flying.is_empty() {
                windows.recv_timeout(Duration::from_millis(20))
            } else {
                windows.try_recv().map_err(|e| match e {
                    mpsc::TryRecvError::Empty => mpsc::RecvTimeoutError::Timeout,
                    mpsc::TryRecvError::Disconnected => mpsc::RecvTimeoutError::Disconnected,
                })
            };
            match got {
                Ok(window) => active.push_back(Active::new(window)),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => closed = true,
            }
        }
        if closed && active.is_empty() && flying.is_empty() {
            return;
        }

        // Hand out batches, unless the run is paused: what is in flight
        // finishes, nothing new starts.
        if !paused.load(Ordering::Relaxed) {
            let (lanes, cpu_on) = if may_use_lanes {
                crate::accel::for_run()
            } else {
                (Vec::new(), true)
            };
            // A lane that has not started is woken and left to start in the
            // background; the run uses it once its worker says hello.
            let mut targets: Vec<Target> = Vec::new();
            for lane in lanes {
                if lane.ready() {
                    targets.push(Target::Lane(lane));
                } else {
                    lane.wake();
                }
            }
            // The CPU runs when its switch says so, and as the fallback when
            // no lane is ready or on its way (`accel::for_run` decides both).
            // A lane still starting, downloading or compiling is waited for.
            // The CPU also takes every vector a lane gave back unsound,
            // whatever its switch says.
            let guard = active.iter().any(|w| !w.cpu_only.is_empty());
            if cpu_on || guard {
                targets.push(Target::Cpu);
            }
            // Nothing can take a batch yet: wait for a lane rather than spin.
            if targets.is_empty() && flying.is_empty() {
                std::thread::sleep(Duration::from_millis(50));
            }
            for target in &targets {
                let from_back = !matches!(target, Target::Cpu);
                // A CPU brought in only for the guard takes only that.
                let guard_only =
                    !from_back && !cpu_on && !targets.iter().all(|t| matches!(t, Target::Cpu));
                while flying
                    .iter()
                    .filter(|(t, ..)| t.id() == target.id())
                    .count()
                    < LANE_DEPTH
                {
                    let Some(slot) = active.iter().position(|w| {
                        if guard_only {
                            !w.cpu_only.is_empty()
                        } else {
                            w.untaken(!from_back)
                        }
                    }) else {
                        break;
                    };
                    let want = budget(target, pace.get(target.id()));
                    let window = &mut active[slot];
                    let group = if guard_only {
                        let n = window.cpu_only.len();
                        window.cpu_only.drain(..n).collect()
                    } else {
                        let per_call = match target {
                            Target::Lane(lane) => lane.rows_per_call(),
                            Target::Cpu => 1,
                        };
                        window.take(from_back, want, per_call)
                    };
                    if group.is_empty() {
                        break;
                    }
                    let tokens: usize = group
                        .iter()
                        .map(|&i| window.pieces[i].len())
                        .max()
                        .unwrap_or(0)
                        * group.len();
                    let batch: Vec<Piece> =
                        group.iter().map(|&i| window.pieces[i].clone()).collect();
                    let answer: Answer = match target {
                        Target::Cpu => {
                            let (reply, answer) = mpsc::channel();
                            if cpu.send(CpuJob { batch, reply }).is_err() {
                                let _ = done.send(Err("the CPU lane has gone".to_string()));
                                return;
                            }
                            Answer::Cpu(answer)
                        }
                        Target::Lane(lane) => {
                            let ids: Vec<Ids> = batch
                                .into_iter()
                                .map(|piece| match piece {
                                    Piece::Ids(ids) => ids,
                                    _ => Vec::new(),
                                })
                                .collect();
                            Answer::Lane(lane.submit(ids), lane.variant())
                        }
                    };
                    flying.push((
                        target.clone(),
                        base + slot,
                        group,
                        answer,
                        Instant::now(),
                        tokens,
                    ));
                }
            }
        }

        // Whatever has come back; wait a moment on the oldest if nothing has.
        let mut moved = false;
        let mut still = Vec::with_capacity(flying.len());
        let oldest = flying.first().map(|f| f.4);
        for (target, seq, group, answer, sent, tokens) in flying.drain(..) {
            let got =
                answer.wait((Some(sent) == oldest && !moved).then_some(Duration::from_millis(5)));
            let window = &mut active[seq - base];
            match got {
                Ok(Ok((vectors, variant))) if vectors.len() == group.len() => {
                    moved = true;
                    let entry = pace.entry(target.id()).or_insert(Pace {
                        tokens: 0,
                        busy: Duration::ZERO,
                    });
                    entry.tokens += tokens as u64;
                    // Its own time on the device. Counted from when it was
                    // sent, a second batch in flight was charged the first's
                    // time too, the pace read half what it was, the next batch
                    // was cut to match, and the GPU lane settled at two rows a
                    // batch on a model that computes eight.
                    let now = Instant::now();
                    let from = last_back
                        .get(target.id())
                        .map_or(sent, |&back| back.max(sent));
                    entry.busy += now.saturating_duration_since(from);
                    last_back.insert(target.id(), now);
                    let mut kept = 0;
                    for (at, vector) in group.iter().zip(vectors) {
                        // Checked on every lane, the CPU's included: a vector
                        // that is not finite or is all zero never reaches the
                        // store, and goes to the CPU instead. The CPU's own
                        // unsound vector has nowhere better to go, and is kept
                        // rather than retried for ever.
                        if !sound(&vector) && !matches!(target, Target::Cpu) {
                            window.cpu_only.push_back(*at);
                            continue;
                        }
                        window.vectors[*at] = Some(vector);
                        window.rows[*at] = (variant, false);
                        kept += 1;
                    }
                    window.left -= kept;
                    batched.fetch_add(kept, Ordering::Relaxed);
                    *window.variants.entry(variant).or_default() += kept;
                    *window.lanes.entry(target.id()).or_default() += kept;
                }
                // A lane that stopped being ready with a batch still queued on
                // it — switched off, failed, or restarted and compiling again,
                // which is minutes — gives the batch back to the window for
                // whichever target can take it now. A run waited on it for as
                // long as the lane took to start, and a daemon told to stop
                // waited with it.
                Err(true)
                    if matches!(&target, Target::Lane(lane) if !lane.ready())
                        && sent.elapsed() > LANE_GONE =>
                {
                    moved = true;
                    window.retry.extend(group);
                }
                Err(true) => still.push((target, seq, group, answer, sent, tokens)),
                // Failed, lost, or the wrong shape: back on the window, first
                // in line, for whichever lane is free.
                _ => {
                    moved = true;
                    window.retry.extend(group);
                }
            }
        }
        flying = still;

        // Retire finished windows from the front, in order.
        while active.front().is_some_and(|w| w.left == 0) {
            let window = active.pop_front().expect("checked above");
            base += 1;
            let vectors: Vec<Vec<f32>> = window
                .vectors
                .into_iter()
                .map(|v| v.unwrap_or_default())
                .collect();
            let finished = Embedded {
                ids: window.ids,
                vectors,
                rows: window.rows,
                hashes: window.hashes,
                variants: window.variants.into_iter().collect(),
                lanes: window.lanes.into_iter().collect(),
            };
            if done.send(Ok(finished)).is_err() {
                return;
            }
        }
        if paused.load(Ordering::Relaxed) && flying.is_empty() {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Performance cores, or every core where they are all the same: the
/// prepare stage's size.
pub fn prepare_threads() -> usize {
    crate::embed::embed_threads().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(lens: &[usize]) -> Active {
        Active::new(Window {
            ids: (0..lens.len() as u64).collect(),
            pieces: lens.iter().map(|n| Piece::Ids(vec![1; *n])).collect(),
            hashes: Vec::new(),
        })
    }

    #[test]
    fn a_cached_piece_is_done_before_the_window_starts() {
        let mut w = Active::new(Window {
            ids: vec![7, 8],
            pieces: vec![
                Piece::Cached {
                    vector: vec![1.0; 3],
                    variant: "fp16-ane",
                },
                Piece::Ids(vec![1; 5]),
            ],
            hashes: vec![Some([1; 32]), Some([2; 32])],
        });
        assert_eq!(w.left, 1);
        assert_eq!(w.take(true, 100, 1), vec![1]);
        assert!(!w.untaken(true));
        assert_eq!(w.rows[0], ("fp16-ane", true));
        assert_eq!(w.lanes["cache"], 1);
    }

    #[test]
    fn a_fixed_batch_lane_takes_whole_calls_of_rows() {
        let mut w = window(&[100; 20]);
        // A budget of two rows still takes eight, the model's batch.
        assert_eq!(w.take(true, 200, 8).len(), 8);
        // Past the budget only as far as the end of that call.
        assert_eq!(w.take(true, 300, 8).len(), 8);
        // Fewer left than a call: whatever is left.
        assert_eq!(w.take(true, 200, 8).len(), 4);
    }

    #[test]
    fn the_cpu_takes_the_shortest_and_a_lane_the_longest_within_a_budget() {
        let mut w = window(&[10, 300, 20, 200, 30]);
        // Front: 10, 20, 30 cost 3 x 30 = 90 padded tokens.
        assert_eq!(w.take(false, 90, 1), vec![0, 2, 4]);
        // Back: 300 alone, since 300 + 200 would be 2 x 300.
        assert_eq!(w.take(true, 500, 1), vec![1]);
        assert_eq!(w.take(true, 500, 1), vec![3]);
        assert!(!w.untaken(false));
    }

    #[test]
    fn a_batch_always_takes_one_even_over_budget_and_retries_come_first() {
        let mut w = window(&[1000, 5]);
        assert_eq!(w.take(true, 10, 1), vec![0]);
        w.retry.push_back(0);
        assert_eq!(w.take(false, 10, 1), vec![0]);
        assert_eq!(w.take(false, 10, 1), vec![1]);
    }

    #[test]
    fn a_zero_or_non_finite_vector_is_unsound() {
        assert!(sound(&[0.1, 0.0]));
        assert!(!sound(&[0.0, 0.0]));
        assert!(!sound(&[f32::NAN, 1.0]));
        assert!(!sound(&[f32::INFINITY, 1.0]));
    }

    #[test]
    fn only_the_cpu_takes_what_the_guard_handed_back() {
        let mut w = window(&[10, 20, 30]);
        assert_eq!(w.take(true, 30, 1), vec![2]);
        w.cpu_only.push_back(2);
        assert!(!w.untaken(false) || w.front < w.back);
        // An accelerator never sees position 2 again; the CPU takes it first.
        assert_eq!(w.take(true, 1000, 1), vec![1, 0]);
        assert!(w.untaken(true));
        assert_eq!(w.take(false, 1000, 1), vec![2]);
    }

    #[test]
    fn the_stages_sum_to_the_wall_time() {
        let clock = WriterClock {
            started: Instant::now() - Duration::from_millis(1_000),
            walk_ms: 100,
            prepare_wait: Duration::from_millis(300),
            embed_wait: Duration::from_millis(400),
        };
        let clocks = Clocks::default();
        clocks.read.store(100_000, Ordering::Relaxed);
        clocks.parse.store(300_000, Ordering::Relaxed);
        let lanes: BTreeMap<String, usize> =
            [("ane".to_string(), 300), ("cpu".to_string(), 100)].into();
        let stages = clock.stages(&clocks, &lanes);
        assert_eq!(stages.parts_ms(), stages.wall_ms);
        assert_eq!(stages.read_ms, 75);
        assert_eq!(stages.parse_ms, 225);
        assert_eq!(stages.embed_wait_ms["ane"], 300);
        assert_eq!(stages.embed_wait_ms["cpu"], 100);
    }
}
