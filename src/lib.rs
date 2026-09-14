//! semlith — a local semantic cache for AI agents.
//!
//! Point it at files, get back a store that answers "what part of my corpus is
//! relevant to this question?" in milliseconds, without shipping anything off
//! the machine and without an agent burning tokens reading whole files.
//!
//! Two pieces of state live side by side in the store directory:
//!
//! - the vectors — a [`turbovec`] TurboQuant index holding only quantized
//!   vectors keyed by chunk id. A store created by 0.7.0 keeps them as a
//!   directory of shards under `index/`; every store written before it keeps
//!   the single `index.tv` it already has. See [`index`].
//! - `store.db` — SQLite holding the chunk text, its file, and its line span.
//!
//! A search quantizes the query, gets ids back from the index, then resolves
//! them to text with one SQLite lookup each.

pub mod add;
pub mod chunk;
pub mod clients;
pub mod daemon;
pub mod embed;
pub mod filter;
pub mod fleet;
/// Readers for the formats that are not plain text. Private: what semlith
/// extracts from a given document is documented behaviour, not an API.
mod formats;
pub mod graph;
pub mod home;
pub mod http;
pub mod image;
pub mod index;
pub mod lock;
pub mod mcp;
pub mod portal;
pub mod proxy;
pub mod routes;
pub mod setup;
pub mod store;
pub mod upgrade;
pub mod watch;

use anyhow::{Result, bail};
use embed::Model;
use fastembed::TextEmbedding;
use filter::Filter;
use index::{Allowlist, VectorIndex};
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bits per coordinate kept by TurboQuant. 4 is the top of the supported range
/// — the memory saving from going lower is not worth the recall on a store
/// that is meant to be the source of truth for an agent's context.
const BIT_WIDTH: usize = 4;

/// Texts handed to ONNX Runtime in one go.
///
/// Keep this small. The transformer pads every text in a batch to the longest
/// one, and attention memory grows with `batch * seq_len^2`. Measured over the
/// same 6527-chunk corpus, a batch of 8 peaked at 615 MB and a batch of 32 at
/// 1799 MB — 2.9x the memory for no throughput at all (23.2 against 23.3
/// chunks/sec), because a smaller batch also wastes less of itself on padding.
const EMBED_BATCH: usize = 8;

/// Default model: 384-dim, ~52 MB on disk. Measured against the previous
/// default (BGE-small) on a 6260-chunk corpus it scored 16.00 code MRR@10
/// against 14.84, at a third of the download.
pub fn default_model() -> Model {
    Model::Granite
}

/// Sentinel hash meaning "chunks are in SQLite but their vectors are not
/// durable yet". Any file left in this state is re-indexed on the next run.
const PENDING: &str = "";

/// Meta key counting index.tv rewrites.
///
/// It is what tells a long-running reader — an MCP server an agent is holding
/// open — that `semlith watch` has replaced the index underneath it. A
/// timestamp cannot do this job: re-embedding one file can leave both the size
/// and the second-granularity mtime unchanged, and the reader would keep
/// answering from vectors that no longer exist.
const GENERATION: &str = "index_generation";

/// Reciprocal-rank-fusion constant. Rank position matters more than the raw
/// scores, which are not comparable: cosine similarity and BM25 are different
/// units on different scales. 60 is the value from the original TREC work and
/// flattens the curve enough that a result ranked third is not dismissed.
const RRF_K: f32 = 60.0;

/// One candidate in the fusion: which id space it is in, its id, the score it
/// has accumulated, and which lists put it there.
///
/// A named type because the tuple had grown four deep and unreadable — two id
/// spaces is what added the fourth.
type Candidate = (((bool, u64), f32), Vec<&'static str>);

/// How many ranked lists a text chunk can be found by: vector, keyword and
/// graph. An image can be found by one, which is what
/// [`Semlith::search_ranked`] weighs a confident image match against.
const TEXT_LISTS: f32 = 3.0;

/// The CLIP cosine above which an image match is treated as a confident one.
///
/// Measured rather than picked: against a fixture of four shapes, a query
/// describing one of them scores 0.30 to 0.34 on the right image and 0.24 to
/// 0.26 on the others, and a query about something else entirely tops out at
/// 0.26. The floor sits between those, and a corpus whose images are all of one
/// subject will want it moved — which is what [`IMAGE_FLOOR_ENV`] is for.
///
/// ponytail: one global floor for every corpus. A per-store calibration from
/// the store's own score distribution would be better and needs a store's worth
/// of queries to compute; this is the knob until then.
const IMAGE_FLOOR: f32 = 0.28;

/// Overrides [`IMAGE_FLOOR`].
pub const IMAGE_FLOOR_ENV: &str = "SEMLITH_IMAGE_FLOOR";

fn image_floor() -> f32 {
    std::env::var(IMAGE_FLOOR_ENV)
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(IMAGE_FLOOR)
}

/// How much deeper than `k` to look in each ranking before fusing.
const RANK_DEPTH: usize = 4;

/// How long an index run may go without making its work durable.
///
/// This is what an interruption costs: the vectors embedded since the last
/// checkpoint, and no more. Thirty seconds is short enough that losing it is an
/// annoyance rather than an evening, and long enough that a checkpoint's cost —
/// rewriting the shards touched since the last one — stays a rounding error
/// against the embedding it protects.
///
/// Only a sharded store checkpoints. A store written before 0.7.0 would have to
/// rewrite its entire index to do it, which is the cost sharding exists to
/// remove; those stores behave exactly as they did.
const CHECKPOINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// Override for [`CHECKPOINT_INTERVAL`], in seconds. For the tests, which
/// cannot spend thirty seconds proving a checkpoint happened. Not part of the
/// documented environment.
const CHECKPOINT_SECS_ENV: &str = "SEMLITH_CHECKPOINT_SECS";

fn checkpoint_interval() -> std::time::Duration {
    match std::env::var(CHECKPOINT_SECS_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
    {
        Some(secs) => std::time::Duration::from_secs(secs),
        None => CHECKPOINT_INTERVAL,
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub score: f32,
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    /// Which store this came from, set only when more than one was searched.
    ///
    /// Absent for a single-store search, so its output — including `--json` —
    /// is byte for byte what it was before stores could be combined.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    /// Which ranked lists found this chunk: any of `vector`, `keyword`,
    /// `graph`. A hit reached only through the graph is a different kind of
    /// answer from one the embedding matched, and saying so is what keeps it
    /// from being mistaken for a semantic match.
    ///
    /// Empty is impossible — a hit is in the result because some list ranked
    /// it — but it is skipped when empty so a caller that never looks at it
    /// sees the JSON it saw before.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<&'static str>,
    /// An image hit's pixel size, in place of a line range.
    ///
    /// `None` for a chunk, and skipped when it is, so every existing consumer
    /// of `--json` and of the MCP output sees exactly the shape it saw before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image: Option<Pixels>,
}

/// An image's pixel size.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
}

/// What the run decided about one file.
///
/// Reported for every file rather than only for the ones being embedded: a
/// re-index of an unchanged corpus used to say nothing at all between "started"
/// and "done", which looks identical to a run that has hung.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum FileOutcome {
    /// About to be read, chunked and embedded.
    #[default]
    Indexing,
    /// The hash on disk matches the hash in the store.
    Unchanged,
    /// Empty, too large, unreadable, or a format with no reader.
    Skipped,
    /// Gone from disk, so its chunks were evicted.
    Removed,
    /// Named a credential, or sat outside the boundary this caller may index.
    ///
    /// Told apart from `Skipped` because the two mean different things to
    /// whoever reads the line: a skipped file is one semlith has no reader for,
    /// and a refused one is a file semlith will not read. An agent that asked
    /// for it needs to know which.
    Refused,
}

impl FileOutcome {
    /// The word the portal and the CLI both label a line with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Indexing => "indexing",
            Self::Unchanged => "unchanged",
            Self::Skipped => "skipped",
            Self::Removed => "removed",
            Self::Refused => "refused",
        }
    }
}

/// What this caller is allowed to index.
///
/// Two different callers, two different answers. The person typing `semlith
/// index` owns the machine: they are held to the deny-list, because indexing a
/// private key by accident is a mistake rather than a decision, and
/// `--include-secrets` is how they say they meant it. An agent holding the
/// agent key is held to both the deny-list and a boundary, because the whole of
/// what that key can reach is what a leaked key can reach — and "every file this
/// user can read" is too much for a credential that lives in a config file.
///
/// The default is the CLI's: the deny-list, and no confinement.
#[derive(Debug, Default, Clone)]
pub struct Boundary {
    /// Directories a path must be under. `None` means anywhere, which is what
    /// the command line gets.
    pub roots: Option<Vec<PathBuf>>,
    /// Whether the deny-list is off for this run, which only the command line
    /// can ask for.
    pub allow_secrets: bool,
}

impl Boundary {
    /// Confine to these roots, and to the home directory.
    pub fn within(roots: Vec<PathBuf>) -> Self {
        Self {
            roots: Some(roots),
            allow_secrets: false,
        }
    }

    /// Why this path may not be indexed, in a line naming the rule.
    pub fn refuses(&self, path: &Path) -> Option<String> {
        if let Some(roots) = &self.roots
            && !filter::within_boundary(path, roots)
        {
            return Some(
                "is outside this store's roots and outside the home directory, so \
                 semlith will not index it. Add it as a root first, or index it \
                 from the command line."
                    .to_string(),
            );
        }
        if !self.allow_secrets
            && let Some(why) = filter::denied(path)
        {
            return Some(why.reason());
        }
        None
    }
}

/// Where an index run has got to, handed to the callback once per file.
///
/// `total` is what the walk found, so `scanned` against it is a real fraction
/// rather than a spinner — which is the difference between a long wait and a
/// wait a person is willing to sit through.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexProgress {
    /// What this file is: being embedded, unchanged, skipped or removed.
    pub outcome: FileOutcome,
    /// Files considered so far, including the unchanged and the skipped.
    pub scanned: usize,
    /// Files embedded so far in this run.
    pub indexed: usize,
    /// Chunks embedded so far in this run — the unit the rate is in, because
    /// files vary in size by orders of magnitude and chunks do not.
    pub chunks: usize,
    /// Files the walk found.
    pub total: usize,
    /// Symbols extracted so far in this run. Zero for a corpus of languages
    /// that carry no edges, which is not an error — see [`crate::graph`].
    pub symbols: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct IndexReport {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub removed: usize,
    pub chunks: usize,
    /// Paths a time-bounded run never reached. Zero unless a budget cut the
    /// run short — an unbounded `index_paths` always finishes what it walked.
    pub remaining: usize,
    /// Paths refused, each with the rule that refused it.
    ///
    /// Listed rather than counted: "three paths were refused" is not something
    /// an agent or a person can act on, and a refusal that is not named reads
    /// as a file that quietly failed to index.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<(String, String)>,
    /// Symbols extracted in this run, and the edges between them.
    pub symbols: usize,
    pub edges: usize,
    /// Images embedded in this run.
    pub images: usize,
    /// Whether the run was stopped rather than finished.
    pub stopped: bool,
    /// Every file this call embedded, in order. The caller keeps these across
    /// the slices of one logical run, so stopping can undo the whole run
    /// rather than only the slice that happened to be going.
    pub written: Vec<String>,
}

/// What a controlled run should do at the next file boundary.
///
/// Asked between files, never inside one: a file half-written into the index
/// is a file whose hash must not be committed, and the boundary is the one
/// point in the loop where that cannot be true.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// Keep going.
    Run,
    /// Hold here, and ask again shortly.
    Pause,
    /// Stop here and keep what is done. The run reports what it did not reach,
    /// and asking again continues from there — the same answer a time budget
    /// gives. This is how the watcher's catch-up steps aside for a request
    /// that arrived while it was working.
    Yield,
    /// Give up, and undo what this run embedded.
    Stop,
}

/// How long a paused run waits before asking again. Short enough that
/// resuming feels immediate, long enough that a paused run costs nothing.
const PAUSE_TICK: std::time::Duration = std::time::Duration::from_millis(120);

pub struct Semlith {
    dir: PathBuf,
    db: Connection,
    index: VectorIndex,
    /// The image vectors, in their own space at CLIP's 512 dimensions.
    ///
    /// A second index rather than a second kind of row in the first: vectors
    /// from two models are not comparable, and one index holding both would
    /// rank a picture against a paragraph by arithmetic that means nothing.
    images: VectorIndex,
    model: Model,
    dim: usize,
    embedder: Option<TextEmbedding>,
    /// CLIP's two encoders, loaded on the first image indexed or searched for.
    clip: image::Clip,
    /// The index generation this process has loaded. Compared against the
    /// store's on every search to notice another process's writes.
    generation: u64,
    /// Print model-download progress to stderr. Off for the MCP server, where
    /// stdout/stderr are a protocol channel.
    pub quiet: bool,
    /// What this caller may index. The default is the command line's: the
    /// deny-list, and no confinement.
    pub boundary: Boundary,
}

impl Semlith {
    /// Open (or create) a store in `dir`.
    ///
    /// `model` is only honoured when the store is new; an existing store keeps
    /// the model it was built with, since vectors from two models are not
    /// comparable.
    pub fn open(dir: impl AsRef<Path>, model: Option<Model>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        // Owner-only, and narrowed on every open rather than only at creation:
        // a store holds the text of every file it indexed, and one made by an
        // older semlith is already readable by everyone on the machine.
        home::secure_dir(&dir)?;

        let db_path = dir.join("store.db");
        let db = store::open(&db_path)?;
        home::tighten_file(&db_path);
        // The write-ahead log and its index carry the same rows as the database
        // and are created by SQLite rather than by semlith.
        for beside in ["store.db-wal", "store.db-shm"] {
            home::tighten_file(&dir.join(beside));
        }

        let model = match store::get_meta(&db, "model")? {
            Some(existing) => {
                let existing: Model = existing.parse().map_err(anyhow::Error::msg)?;
                if let Some(want) = model
                    && want != existing
                {
                    bail!(
                        "store was built with {existing}, not {want}; \
                         delete {} to rebuild with a different model",
                        dir.display()
                    );
                }
                existing
            }
            None => {
                let m = model.unwrap_or_else(default_model);
                // Creating a store is a write, and it is the one write that
                // happens before there is a `Semlith` to ask for permission
                // through. See `store::read_only`.
                let _writing = store::Writing::begin(&db)?;
                store::set_meta(&db, "model", &m.to_string())?;
                // Only here, where a store is being created. Stamping it on
                // open would rewrite every store this binary ever reads, and
                // a store written before the key existed is format 1 whether
                // or not it says so.
                store::set_meta(&db, store::FORMAT_KEY, &store::FORMAT_VERSION.to_string())?;
                m
            }
        };

        let dim = model.dim()?;
        // Named, not read. The vectors are the largest thing a store owns and
        // most commands never look at them. Which layout they are in is the
        // store's business, decided when it was created and never migrated.
        let sharded = store::format(&db)? >= store::SHARDED_FORMAT;
        let index = VectorIndex::open(&dir, dim, BIT_WIDTH, sharded)?;
        // Beside the text index rather than inside it. Created lazily by
        // `VectorIndex::open`, so a store that never holds an image never grows
        // the directory.
        let images = VectorIndex::open(&dir.join(image::INDEX_DIR), image::DIM, BIT_WIDTH, true)?;

        let generation = generation(&db)?;

        Ok(Self {
            dir,
            db,
            index,
            images,
            model,
            dim,
            embedder: None,
            clip: image::Clip::default(),
            generation,
            quiet: false,
            boundary: Boundary::default(),
        })
    }

    /// Open a store that must already be one.
    ///
    /// [`Semlith::open`] creates what it is given, which is what `index` wants
    /// and the opposite of what every read command wants: a mistyped store
    /// directory becomes an empty store that answers every question with
    /// nothing, and when several stores are searched at once the others hide it
    /// completely.
    pub fn open_existing(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref();
        if !dir.join("store.db").exists() {
            bail!(
                "{} is not a semlith store — no store.db in it; index it first",
                dir.display()
            );
        }
        Self::open(dir, None)
    }

    /// Reload the vector index if another process has replaced it since this
    /// one read it — `semlith watch` re-embedding while an agent holds an MCP
    /// server open.
    ///
    /// Costs one SQLite read when nothing has changed, which is the case
    /// almost every time it is called.
    pub fn refresh(&mut self) -> Result<()> {
        let current = generation(&self.db)?;
        if current == self.generation {
            return Ok(());
        }
        if !self.index.exists() {
            return Ok(());
        }

        // Dropped rather than reloaded here: whatever asked for this refresh is
        // about to search, and the load it triggers is the same load this used
        // to do eagerly. A reader that only ever calls `stats` pays nothing.
        let was_resident = self.index.is_resident();
        self.index.evict();
        if was_resident {
            // Already warm before the writer moved underneath it, so warm again
            // rather than handing the repack cost to the next question.
            self.index.prepare()?;
        }
        // Only now, and to the generation read *before* the reload: a failed
        // load leaves the reader due for another attempt rather than stuck on
        // a stale index forever, and a write that landed during the load has a
        // higher number, so it is still noticed next time.
        self.generation = current;
        Ok(())
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// How many vectors the store holds.
    ///
    /// Answered from SQLite unless the vectors happen to be resident already,
    /// because the caller asking this is usually `stats` or a fleet totalling
    /// its members, and neither is worth loading an index for. The two agree: a
    /// file's hash is committed only once its vectors are durable, so the chunks
    /// of hashed files are exactly the vectors on disk. Mid-run, in the process
    /// doing the writing, the resident index is ahead of that and is the truth.
    pub fn len(&self) -> usize {
        if let Some(resident) = self.index.resident_len() {
            return resident;
        }
        // A failure here is a store whose SQLite side is unreadable, and every
        // other call on it is about to say so far more usefully than a count.
        store::durable_chunks(&self.db).unwrap_or(0) as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn db(&self) -> &Connection {
        &self.db
    }

    /// The store directory, for a caller that needs to take the store lock
    /// itself rather than for the length of one call.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// How many shards the store's vectors are split across, and how many of
    /// them may be resident at once. `None` for a store written before 0.7.0,
    /// whose single index is all or nothing.
    pub fn shards(&self) -> Option<(usize, usize)> {
        self.index
            .max_resident()
            .map(|max| (self.index.shards(), max))
    }

    /// Shards this store has put down to stay inside its memory budget. Above
    /// zero means the corpus has outgrown the budget and queries are paying to
    /// read shards back.
    pub fn evictions(&self) -> u64 {
        self.index.evictions()
    }

    /// Loading the ONNX model costs a second or so, so it is deferred until a
    /// command actually needs to embed something.
    fn embedder(&mut self) -> Result<&mut TextEmbedding> {
        if self.embedder.is_none() {
            // Cap the sequence length to what a chunk can actually produce.
            // The default 512-token window would let a pathologically dense
            // chunk (base64, minified JS) blow up attention memory for no
            // retrieval benefit; two characters per token is a safe floor for
            // real text and code.
            self.embedder = Some(self.model.load(
                model_cache_dir(),
                chunk::MAX_CHARS / 2,
                self.quiet,
            )?);
        }
        Ok(self.embedder.as_mut().unwrap())
    }

    /// Pay the model-load and index-warmup cost up front, so the first query
    /// is not slower than the rest.
    pub fn warm(&mut self) -> Result<()> {
        self.embedder()?;
        self.index.prepare()?;
        Ok(())
    }

    /// Warm the vector index without loading a model, for a caller that shares
    /// one loaded model across several stores.
    pub fn warm_index(&mut self) -> Result<()> {
        self.index.prepare()
    }

    fn embed(&mut self, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let mut out = self
            .embedder()?
            .embed(texts, Some(EMBED_BATCH))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for v in &mut out {
            normalize(v);
        }
        Ok(out)
    }

    /// Walk `roots`, embed anything new or changed, and drop anything that has
    /// disappeared from disk.
    pub fn index_paths(
        &mut self,
        roots: &[PathBuf],
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        // Held for the whole run, including the index.tv write at the end.
        // Two concurrent runs would otherwise interleave their SQLite writes
        // and their index rewrites until the two disagree.
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.index_walk(roots, on_file)
    }

    /// [`Semlith::index_paths`] that gives up its remaining work when `budget`
    /// runs out, reporting what it did not reach in [`IndexReport::remaining`].
    ///
    /// For a caller whose own deadline is shorter than a corpus — an MCP tool
    /// call, where a client that waited too long declares the server hung. One
    /// file is always indexed however small the budget, so calling repeatedly
    /// always finishes: indexing is keyed on content hashes, so the next call
    /// walks past what this one completed rather than redoing it.
    pub fn index_paths_within(
        &mut self,
        roots: &[PathBuf],
        budget: std::time::Duration,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.index_within_held(roots, budget, on_file)
    }

    /// [`Semlith::index_paths_within`] without taking the lock, for a caller
    /// that already holds it — `semlith start` holds it for the daemon's life,
    /// and its queue runs on the thread that holds it.
    pub(crate) fn index_within_held(
        &mut self,
        roots: &[PathBuf],
        budget: std::time::Duration,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        let deadline = std::time::Instant::now() + budget;
        self.index_set(walk(roots), true, Some(deadline), None, on_file)
    }

    /// [`Semlith::index_walk`] under a control, so the catch-up a watcher runs
    /// at startup can step aside the moment a request arrives.
    pub(crate) fn index_walk_under(
        &mut self,
        roots: &[PathBuf],
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        self.index_set(walk(roots), true, None, Some(control), on_file)
    }

    /// [`Semlith::index_within_held`] that can be paused and stopped from
    /// another thread.
    ///
    /// `control` is asked once per file. A stop undoes everything the run
    /// embedded, so the corpus is exactly as it was before it started — which
    /// is what makes stopping a real answer rather than a half-indexed store.
    pub(crate) fn index_within_held_under(
        &mut self,
        roots: &[PathBuf],
        budget: std::time::Duration,
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        let deadline = std::time::Instant::now() + budget;
        self.index_set(walk(roots), true, Some(deadline), Some(control), on_file)
    }

    /// `index_paths` without taking the lock, for a caller that already holds
    /// it — `semlith watch` holds it for its whole life.
    pub(crate) fn index_walk(
        &mut self,
        roots: &[PathBuf],
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        self.index_set(walk(roots), true, None, None, on_file)
    }

    /// Re-index exactly `paths`, evicting any that have gone from disk.
    ///
    /// No walk and no orphan sweep: the caller already knows which files
    /// changed, which is the whole point of watching. The lock is the caller's
    /// too.
    pub(crate) fn index_changed(
        &mut self,
        paths: Vec<PathBuf>,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        self.index_set(paths, false, None, None, on_file)
    }

    /// The body both entry points share. `sweep` drops every recorded file
    /// that is no longer on disk — right for a full walk, wrong for a batch of
    /// events, which only knows about the paths in it.
    fn index_set(
        &mut self,
        paths: Vec<PathBuf>,
        sweep: bool,
        deadline: Option<std::time::Instant>,
        control: Option<&dyn Fn() -> Flow>,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        // Every path that writes to this store funnels through here, so this is
        // where the connection stops refusing writes — and, when this returns,
        // starts refusing them again. See `store::Writing` and `writing` below.
        self.writing(move |me| me.index_set_writing(paths, sweep, deadline, control, on_file))
    }

    /// Do something that writes, with the connection's refusal lifted for
    /// exactly as long as it takes.
    ///
    /// A closure rather than a guard because the body needs `&mut self` and a
    /// guard would be holding `&self.db` for its whole life. The restore runs
    /// whether the body succeeded or not: a store left writable after a failed
    /// run is the state this is here to prevent.
    fn writing<T>(&mut self, body: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        store::read_only(&self.db, false)?;
        let out = body(self);
        let _ = store::read_only(&self.db, true);
        out
    }

    fn index_set_writing(
        &mut self,
        paths: Vec<PathBuf>,
        sweep: bool,
        deadline: Option<std::time::Instant>,
        control: Option<&dyn Fn() -> Flow>,
        mut on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        // A run killed mid-save leaves a temp index behind. Removing it here
        // and not on open is deliberate: the caller holds the store lock, so
        // there is no live writer whose half-written index this could be.
        self.index.clean();

        let mut report = IndexReport::default();
        let mut pending = Batch::default();
        // Every file this run embedded, so a stop can put the store back the
        // way it found it rather than leaving half a corpus indexed.
        let mut written: Vec<String> = Vec::new();
        // Files whose vectors are embedded but not yet durable. Their hash is
        // written only after the index lands, so a crash re-indexes them.
        let mut completed: Vec<(i64, String)> = Vec::new();

        let checkpointing = matches!(self.index, index::VectorIndex::Sharded(_));
        let interval = checkpoint_interval();
        let mut last_checkpoint = std::time::Instant::now();

        // Refused before anything is read. A path that names a credential or
        // sits outside this caller's boundary is reported by name with the rule
        // that refused it, rather than dropped from the walk — an agent that
        // asked for a file and got silence cannot tell that from a file that
        // was not there.
        let (paths, refused): (Vec<PathBuf>, Vec<(PathBuf, String)>) = {
            let mut allowed = Vec::with_capacity(paths.len());
            let mut refused = Vec::new();
            for path in paths {
                match self.boundary.refuses(&path) {
                    Some(why) => refused.push((path, why)),
                    None => allowed.push(path),
                }
            }
            (allowed, refused)
        };
        let total = paths.len() + refused.len();
        for (path, why) in &refused {
            report
                .refused
                .push((path.display().to_string(), why.clone()));
            report.scanned += 1;
            on_file(
                path,
                IndexProgress {
                    outcome: FileOutcome::Refused,
                    scanned: report.scanned,
                    indexed: report.indexed,
                    chunks: report.chunks,
                    total,
                    symbols: report.symbols,
                },
            );
        }

        for (seen, path) in paths.into_iter().enumerate() {
            // Only ever after something was embedded: a budget too small for
            // any work at all must still make progress, or calling again is
            // the same call forever.
            if let Some(deadline) = deadline
                && report.indexed > 0
                && std::time::Instant::now() >= deadline
            {
                report.remaining = total - seen;
                break;
            }
            if let Some(ask) = control {
                let mut stop = false;
                let mut yielded = false;
                loop {
                    match ask() {
                        Flow::Run => break,
                        // Held here rather than returning: the run keeps the
                        // store lock, so resuming is this loop waking up and
                        // not a second walk of the tree.
                        Flow::Pause => std::thread::sleep(PAUSE_TICK),
                        Flow::Yield => {
                            yielded = true;
                            break;
                        }
                        Flow::Stop => {
                            stop = true;
                            break;
                        }
                    }
                }
                if yielded {
                    report.remaining = total - seen;
                    break;
                }
                if stop {
                    report.remaining = total - seen;
                    report.stopped = true;
                    break;
                }
            }
            report.scanned += 1;
            let key = path.to_string_lossy().into_owned();

            // Opened once, and the size read off the open handle rather than
            // off the name. Two `stat`s and a `read` of the same path are three
            // answers about three moments: a file that grew between the check
            // and the read was read in full anyway, so the cap was advisory.
            let opened = std::fs::File::open(&path);
            let measured = opened.as_ref().ok().and_then(|f| f.metadata().ok());
            match measured {
                Some(m) if m.is_file() && m.len() > 0 && m.len() <= chunk::MAX_FILE_BYTES => {}
                _ => {
                    // A batch of events can name a file that has just been
                    // deleted or renamed away. Evicting it here is what makes
                    // a deletion visible without a full sweep.
                    if !path.exists() {
                        let ids = store::delete_file(&self.db, &key)?;
                        if !ids.is_empty() {
                            for id in ids {
                                self.index.remove(id)?;
                            }
                            report.removed += 1;
                            on_file(
                                &path,
                                IndexProgress {
                                    outcome: FileOutcome::Removed,
                                    scanned: report.scanned,
                                    indexed: report.indexed,
                                    chunks: report.chunks,
                                    total,
                                    symbols: report.symbols,
                                },
                            );
                            continue;
                        }
                    }
                    report.skipped += 1;
                    on_file(
                        &path,
                        IndexProgress {
                            outcome: FileOutcome::Skipped,
                            scanned: report.scanned,
                            indexed: report.indexed,
                            chunks: report.chunks,
                            total,
                            symbols: report.symbols,
                        },
                    );
                    continue;
                }
            }
            // From the handle that was measured, through a reader that stops
            // one byte past the cap: a file that grew between the two is
            // refused by the `take` rather than read whole.
            let read = opened.and_then(|file| {
                use std::io::Read;
                let mut bytes = Vec::new();
                file.take(chunk::MAX_FILE_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes)
            });
            let bytes = match read {
                Ok(bytes) if bytes.len() as u64 <= chunk::MAX_FILE_BYTES => bytes,
                _ => {
                    report.skipped += 1;
                    on_file(
                        &path,
                        IndexProgress {
                            outcome: FileOutcome::Skipped,
                            scanned: report.scanned,
                            indexed: report.indexed,
                            chunks: report.chunks,
                            total,
                            symbols: report.symbols,
                        },
                    );
                    continue;
                }
            };

            let hash = blake3::hash(&bytes).to_hex().to_string();
            if store::file_hash(&self.db, &key)?.as_deref() == Some(hash.as_str()) {
                report.unchanged += 1;
                on_file(
                    &path,
                    IndexProgress {
                        outcome: FileOutcome::Unchanged,
                        scanned: report.scanned,
                        indexed: report.indexed,
                        chunks: report.chunks,
                        total,
                        symbols: report.symbols,
                    },
                );
                continue;
            }

            // An image is a different kind of content in the same pass: read
            // for its pixels rather than for its text, embedded with CLIP's
            // vision encoder, and recorded in the store's second vector space.
            // Handled before `chunk::extract`, which reads a PNG as binary and
            // rejects it.
            if image::is_image(&path) {
                // Before the decoder sees it. A header claiming 60,000 by
                // 60,000 pixels is a few hundred bytes on disk and fourteen
                // gigabytes in memory, and the refusal says which file and how
                // big it claimed to be.
                if let Some(why) = image::too_large(&bytes) {
                    report.refused.push((path.display().to_string(), why));
                    on_file(
                        &path,
                        IndexProgress {
                            outcome: FileOutcome::Refused,
                            scanned: report.scanned,
                            indexed: report.indexed,
                            chunks: report.chunks,
                            total,
                            symbols: report.symbols,
                        },
                    );
                    continue;
                }
                let Some((width, height)) = image::dimensions(&bytes) else {
                    report.skipped += 1;
                    on_file(
                        &path,
                        IndexProgress {
                            outcome: FileOutcome::Skipped,
                            scanned: report.scanned,
                            indexed: report.indexed,
                            chunks: report.chunks,
                            total,
                            symbols: report.symbols,
                        },
                    );
                    continue;
                };
                on_file(
                    &path,
                    IndexProgress {
                        outcome: FileOutcome::Indexing,
                        scanned: report.scanned,
                        indexed: report.indexed,
                        chunks: report.chunks,
                        total,
                        symbols: report.symbols,
                    },
                );
                let vector = self.clip.embed_image(&path, self.quiet)?;
                // Replacing an image: its old vector goes before the new one
                // arrives, and the row goes with the file's cascade.
                for id in store::image_ids_of(&self.db, &key)? {
                    self.images.remove(id as u64)?;
                }
                for id in store::delete_file(&self.db, &key)? {
                    self.index.remove(id)?;
                }
                let file_id =
                    store::insert_file(&self.db, &key, PENDING, bytes.len() as u64, now())?;
                let image_id = store::insert_image(&self.db, file_id, width, height)?;
                self.images.add(&vector, &[image_id as u64])?;
                completed.push((file_id, hash));
                written.push(key.clone());
                report.indexed += 1;
                report.images += 1;
                continue;
            }

            let Some(text) = chunk::extract(&path, &bytes) else {
                report.skipped += 1;
                continue;
            };
            let chunks = chunk::chunk_text(&text);
            if chunks.is_empty() {
                report.skipped += 1;
                on_file(
                    &path,
                    IndexProgress {
                        outcome: FileOutcome::Skipped,
                        scanned: report.scanned,
                        indexed: report.indexed,
                        chunks: report.chunks,
                        total,
                        symbols: report.symbols,
                    },
                );
                continue;
            }

            on_file(
                &path,
                IndexProgress {
                    outcome: FileOutcome::Indexing,
                    scanned: report.scanned,
                    indexed: report.indexed,
                    chunks: report.chunks,
                    total,
                    symbols: report.symbols,
                },
            );

            // Replacing a file: evict its old vectors before adding new ones.
            for id in store::delete_file(&self.db, &key)? {
                self.index.remove(id)?;
            }

            let file_id = store::insert_file(&self.db, &key, PENDING, bytes.len() as u64, now())?;
            let mut spans: Vec<(u32, u32, i64)> = Vec::with_capacity(chunks.len());
            for (ord, c) in chunks.iter().enumerate() {
                let id =
                    store::insert_chunk(&self.db, file_id, ord, c.start_line, c.end_line, &c.text)?;
                spans.push((c.start_line, c.end_line, id));
                pending.ids.push(id as u64);
                pending.texts.push(c.text.clone());

                // Flush per chunk, not per file. One 8 MB file chunks into
                // thousands of pieces, and holding them all to embed in a
                // single call makes peak memory a function of the largest file
                // in the corpus rather than of the batch size.
                if pending.ids.len() >= EMBED_BATCH {
                    self.flush(&mut pending)?;
                }
            }

            // The structure half, on the same changed-file path and inside the
            // same lock. A file whose symbols were extracted by an earlier run
            // had them deleted by `delete_file` above, along with its chunks
            // and the edges leaving them, so this writes a whole fresh set
            // rather than reconciling one.
            let (symbols, edges) = self.extract_graph(&path, &text, file_id, &spans)?;
            report.symbols += symbols;
            report.edges += edges;

            completed.push((file_id, hash));
            written.push(key.clone());
            report.indexed += 1;
            report.chunks += chunks.len();

            // Between files, never inside one: a file half-written into the
            // index is a file whose hash must not be committed, and this is the
            // one point in the loop where that cannot be true.
            if checkpointing && last_checkpoint.elapsed() >= interval {
                self.flush(&mut pending)?;
                self.checkpoint(&mut completed)?;
                last_checkpoint = std::time::Instant::now();
            }
        }

        self.flush(&mut pending)?;

        // A stopped slice still commits what it embedded. Undoing is the
        // caller's, because one logical run is several slices and a stop has
        // to undo all of them — see `Job::Index` in the daemon.
        report.written = written;

        // Anything recorded but no longer on disk is dead weight.
        if sweep {
            for key in store::all_paths(&self.db)? {
                if !Path::new(&key).exists() {
                    for id in store::delete_file(&self.db, &key)? {
                        self.index.remove(id)?;
                    }
                    report.removed += 1;
                }
            }
        }

        // A batch that changed nothing must not rewrite index.tv. A watcher
        // sees plenty of events on files whose bytes are identical, and each
        // rewrite is the whole index.
        if report.indexed > 0 || report.removed > 0 || !self.index.exists() {
            self.save()?;
        }

        self.commit_hashes(&mut completed)?;

        Ok(report)
    }

    /// Make everything embedded so far durable, then record the files it
    /// covers as indexed.
    ///
    /// Strictly in that order. A hash written before its vectors are on disk is
    /// a file the next run believes is indexed and cannot answer for — which is
    /// what the [`PENDING`] sentinel exists to prevent, and what a run that
    /// checkpoints has many more chances to get wrong than one that does it
    /// once at the end.
    fn checkpoint(&mut self, completed: &mut Vec<(i64, String)>) -> Result<()> {
        if completed.is_empty() {
            return Ok(());
        }
        self.save()?;
        self.commit_hashes(completed)
    }

    /// Chunks reached from the top hits by one hop through the graph.
    ///
    /// Seeded from the chunks the other two lists already found, mapped to the
    /// symbols defined in them, expanded one hop in both directions, and
    /// resolved back to the chunks those neighbours live in. One hop, not a
    /// ranked walk: a personalized PageRank over the graph is a real idea and
    /// a change to be justified by a recall measurement, not shipped untested
    /// inside a release that is already large.
    ///
    /// Everything here is gated by the same `Filter` the other two halves use,
    /// through the same `symbols_by_names` predicate — so the one-id-set
    /// invariant `filter.rs` documents holds across all three lists rather than
    /// two. A chunk outside the filter cannot arrive through the graph.
    fn graph_expansion(
        &self,
        dense: &[u64],
        keyword: &[u64],
        depth: usize,
        filter: &Filter,
    ) -> Result<Vec<u64>> {
        // Seeded from the best of each list rather than all of it. Expanding
        // from a chunk ranked fortieth is expansion from noise.
        const SEEDS: usize = 8;
        let seeds: Vec<u64> = dense
            .iter()
            .take(SEEDS)
            .chain(keyword.iter().take(SEEDS))
            .copied()
            .collect();
        if seeds.is_empty() {
            return Ok(Vec::new());
        }

        let names = store::symbols_in_chunks(&self.db, &seeds)?;
        if names.is_empty() {
            return Ok(Vec::new());
        }

        let mut neighbours: Vec<String> = Vec::new();
        for name in &names {
            for end in store::edges_out(&self.db, name, &[])?
                .into_iter()
                .chain(store::edges_in(&self.db, name, &[])?)
            {
                if !neighbours.contains(&end.symbol.name) {
                    neighbours.push(end.symbol.name);
                }
            }
            if neighbours.len() >= depth * 4 {
                break;
            }
        }

        let mut ids = Vec::new();
        for symbol in store::symbols_by_names(&self.db, &neighbours, filter.groups())? {
            let Some(chunk_id) = symbol.chunk_id else {
                continue;
            };
            let id = chunk_id as u64;
            // A chunk the other two lists already ranked gains nothing from
            // being re-ranked here; the fusion adds the contribution anyway.
            if !ids.contains(&id) {
                ids.push(id);
            }
            if ids.len() >= depth {
                break;
            }
        }
        Ok(ids)
    }

    /// Extract one file's symbols and outgoing edges, and write them.
    ///
    /// Returns `(symbols, edges)` written. A file in a language that carries no
    /// edges returns `(0, 0)` and is not an error — most corpora are mostly
    /// prose, and the graph simply has nothing to say about them.
    ///
    /// Edges are written by name, so an edge to something not indexed yet — or
    /// never — is still recorded and resolves when and if its target appears.
    /// That is what lets a repository be indexed in any order.
    fn extract_graph(
        &self,
        path: &Path,
        text: &str,
        file_id: i64,
        spans: &[(u32, u32, i64)],
    ) -> Result<(usize, usize)> {
        let Some(extraction) = graph::extract(path, text)? else {
            return Ok((0, 0));
        };

        // Name to id, for the edges below. A file may define the same name
        // twice — two `new` methods on two types — and the first wins, because
        // an edge out of this file names its source by name and nothing here
        // can tell them apart either.
        let mut ids: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
        for symbol in &extraction.symbols {
            let chunk_id = spans
                .iter()
                .find(|(start, end, _)| symbol.start_line >= *start && symbol.start_line <= *end)
                .map(|(_, _, id)| *id);
            let id = store::insert_symbol(&self.db, file_id, chunk_id, symbol)?;
            ids.entry(symbol.name.as_str()).or_insert(id);
        }

        let mut written = 0;
        for edge in &extraction.edges {
            let Some(src) = ids.get(edge.from.as_str()) else {
                continue;
            };
            store::insert_edge(&self.db, *src, &edge.to, &edge.kind, &edge.confidence)?;
            written += 1;
        }

        Ok((extraction.symbols.len(), written))
    }

    /// Record files as indexed, in one transaction. Their vectors must already
    /// be durable.
    fn commit_hashes(&mut self, completed: &mut Vec<(i64, String)>) -> Result<()> {
        if completed.is_empty() {
            return Ok(());
        }
        let tx = self.db.unchecked_transaction()?;
        for (file_id, hash) in completed.iter() {
            tx.execute(
                "UPDATE files SET hash = ?1 WHERE id = ?2",
                rusqlite::params![hash, file_id],
            )?;
        }
        tx.commit()?;
        completed.clear();
        Ok(())
    }

    fn flush(&mut self, batch: &mut Batch) -> Result<()> {
        if batch.ids.is_empty() {
            return Ok(());
        }
        let ids = std::mem::take(&mut batch.ids);
        let texts = std::mem::take(&mut batch.texts);

        let vectors = self.embed(texts)?;
        let flat: Vec<f32> = vectors.into_iter().flatten().collect();
        self.index.add(&flat, &ids)?;
        Ok(())
    }

    /// Remove a single file from the store. Returns `(chunks, images)`.
    pub fn forget(&mut self, path: &Path) -> Result<(usize, usize)> {
        // Dropping a file rewrites `index.tv` exactly as indexing does, so it
        // is a writer and takes the writer's lock. Without it a `forget` that
        // lands while `semlith watch` is saving leaves the index and the
        // database disagreeing about which chunks exist.
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.forget_held(path)
    }

    /// [`Semlith::forget`] without taking the lock, for a caller that already
    /// holds it.
    pub(crate) fn forget_held(&mut self, path: &Path) -> Result<(usize, usize)> {
        self.writing(|me| me.forget_writing(path))
    }

    fn forget_writing(&mut self, path: &Path) -> Result<(usize, usize)> {
        let key = canonical(path).to_string_lossy().into_owned();
        // Read before the delete: the cascade that removes the rows is what
        // makes their ids unreadable, and the vectors they address still have
        // to leave the image index.
        let images = store::image_ids_of(&self.db, &key)?;
        let ids = store::delete_file(&self.db, &key)?;
        for id in &ids {
            self.index.remove(*id)?;
        }
        for id in &images {
            self.images.remove(*id as u64)?;
        }
        if !ids.is_empty() || !images.is_empty() {
            self.save()?;
        }
        Ok((ids.len(), images.len()))
    }

    /// Top-`k` chunks for `query`, best first, over the whole store.
    pub fn search(&mut self, query: &str, k: usize) -> Result<Vec<Hit>> {
        self.search_filtered(query, k, &Filter::default())
    }

    /// Resolve a filter to the ids the vector index may consider.
    fn allowlist(&mut self, filter: &Filter) -> Result<Allowlist> {
        if filter.is_empty() {
            return Ok(Allowlist::All);
        }
        let candidates = store::filtered_chunk_ids(&self.db, filter.groups())?;
        let mut ids = Vec::with_capacity(candidates.len());
        for id in candidates {
            // turbovec panics on an id the index does not hold, and SQLite can
            // hold a chunk the index does not if a run was interrupted between
            // the two. A stale row must not take a search down with it.
            if self.index.contains(id)? {
                ids.push(id);
            }
        }

        Ok(if ids.is_empty() {
            Allowlist::Empty
        } else if ids.len() == self.len() {
            // The filter excludes nothing, so skip building a mask the size of
            // the whole index for no benefit.
            Allowlist::All
        } else {
            Allowlist::Subset(ids)
        })
    }

    /// How many indexed files `filter` selects.
    ///
    /// Zero is worth reporting on its own: a glob that matches nothing is a
    /// different problem from a corpus that does not discuss the query.
    pub fn matching_files(&self, filter: &Filter) -> Result<i64> {
        store::matching_files(&self.db, filter.groups())
    }

    /// The indexed paths `filter` selects, in path order.
    pub fn matching_paths(&self, filter: &Filter) -> Result<Vec<String>> {
        store::filtered_paths(&self.db, filter.groups())
    }

    /// Top-`k` chunks for `query` within the part of the corpus `filter`
    /// selects, best first.
    ///
    /// Both halves of the store are consulted: the vector index for meaning,
    /// FTS5 for the literal terms. Dense search alone reliably misses exact
    /// identifiers — an embedding of `EMBED_BATCH` is a point in the same
    /// neighbourhood as every other constant — which is precisely what someone
    /// grepping a codebase is looking for.
    ///
    /// The filter is applied *before* each half picks its top-`k`, not after
    /// fusion. Post-filtering a global ranking returns almost nothing whenever
    /// the subset is a minority of the corpus, which is the case the filter
    /// exists for.
    pub fn search_filtered(&mut self, query: &str, k: usize, filter: &Filter) -> Result<Vec<Hit>> {
        let vector = self.embed_query(query)?;
        self.search_with_vector(query, &vector, k, filter)
    }

    /// The query, embedded with this store's model: the vector
    /// [`Semlith::search_with_vector`] takes.
    ///
    /// Public because a caller comparing two stores has to be able to hold the
    /// query vector still. Embedding the same text twice does not give the same
    /// floats — ONNX Runtime reduces across its threads in whatever order they
    /// finish — and under int8 quantisation that moves a ranking, which is
    /// noise in a measurement that is about something else.
    pub fn embed_query(&mut self, query: &str) -> Result<Vec<f32>> {
        Ok(self.embed(vec![self.model.query_text(query)])?.remove(0))
    }

    /// [`Semlith::search_filtered`] with the query already embedded.
    ///
    /// This is what lets several stores share one loaded model: the caller
    /// embeds the query once per distinct model rather than once per store, so
    /// searching four stores built with the same model costs one embed and one
    /// resident copy of the weights.
    ///
    /// `vector` must come from *this* store's model. Vectors from two models
    /// are not comparable, and nothing downstream can tell.
    pub fn search_with_vector(
        &mut self,
        query: &str,
        vector: &[f32],
        k: usize,
        filter: &Filter,
    ) -> Result<Vec<Hit>> {
        Ok(self
            .search_ranked(query, vector, k, filter)?
            .into_iter()
            .map(|(hit, _)| hit)
            .collect())
    }

    /// [`Semlith::search_with_vector`], also returning each hit's similarity to
    /// the query vector — `0.0` for a hit only the keyword half found.
    ///
    /// The fused score is a sum of rank reciprocals, so two chunks that hold
    /// the same position in their own store's ranking score identically. Within
    /// one store that is a rare tie between two chunks; across stores it is the
    /// normal case, because every store has a best hit whether or not it has an
    /// answer. The similarity is what tells those apart, so it leaves the store
    /// alongside the score rather than being thrown away here.
    pub fn search_ranked(
        &mut self,
        query: &str,
        vector: &[f32],
        k: usize,
        filter: &Filter,
    ) -> Result<Vec<(Hit, f32)>> {
        // A store being watched changes under a long-lived reader. Answering
        // from the index this process happened to load at startup is how an
        // agent ends up quoting a function that no longer exists.
        self.refresh()?;

        // `is_empty` is about the text index. A store holding only images has
        // no chunks and is still searchable, so it is asked about separately
        // rather than dismissed with the same test.
        if k == 0 || (self.is_empty() && store::image_count(&self.db)? == 0) {
            return Ok(Vec::new());
        }

        // Look deeper than `k` in each half. Fusion can only rank what it is
        // given, and a chunk that is second on one side and absent from the
        // other still deserves to be considered.
        let depth = (k * RANK_DEPTH).max(k);

        let allowlist = self.allowlist(filter)?;
        if matches!(allowlist, Allowlist::Empty) {
            return Ok(Vec::new());
        }

        let (dense_scores, dense_ids) = self.index.search(vector, depth, &allowlist)?;
        let keyword_ids = store::keyword_search(&self.db, query, depth, filter.groups())?;

        // The image list. Only when the store actually holds an image: the
        // query has to be embedded a second time, with CLIP's text encoder
        // rather than the store's own model, and a store of source code should
        // not pay for a model it has nothing to compare against.
        let images = self.image_search(query, depth, filter)?;

        // The third list. The two lists above are what the query said; this is
        // what the code says about what they found — the symbols inside the top
        // hits, one hop out, and the chunks those neighbours live in. It costs
        // no embedding and no model call, and it is what pulls together a
        // concept spread across files that share no vocabulary.
        let graph_ids = self.graph_expansion(&dense_ids, &keyword_ids, depth, filter)?;

        // Two id spaces — a chunk id and an image id both count from one — so
        // the fusion is keyed by which space an id belongs to as well as by the
        // id. Everything else about reciprocal-rank fusion is unchanged: an
        // image is a fourth list, ranked against the other three rather than
        // appended after them.
        let mut fused: Vec<((bool, u64), f32)> = Vec::new();
        let mut seen: std::collections::HashMap<(bool, u64), usize> =
            std::collections::HashMap::new();
        let mut lists: Vec<Vec<&'static str>> = Vec::new();
        // The image list goes in first so that a tie resolves to the image.
        // A tie means the two are equally ranked, and only one of them was
        // found by a model that looked at the thing being asked about.
        let image_list: Vec<(u64, f32)> = images
            .iter()
            .map(|(id, similarity)| {
                // A confident image match stands in for the three lists a
                // chunk can appear in: an image can only ever be found by this
                // one, and the graph list is derived from the other two, so a
                // chunk collects three contributions for what is really one
                // match. Without this weight a picture could never place above
                // a passing text match however well CLIP matched it.
                //
                // Below the floor the image is a weak candidate rather than a
                // wrong one, so it keeps a single list's weight and sits where
                // that puts it instead of being dropped.
                let weight = if *similarity >= image_floor() {
                    TEXT_LISTS
                } else {
                    1.0
                };
                (*id, weight)
            })
            .collect();

        for (name, is_image, ranking) in [
            ("image", true, image_list),
            (
                "vector",
                false,
                dense_ids.iter().map(|id| (*id, 1.0)).collect(),
            ),
            (
                "keyword",
                false,
                keyword_ids.iter().map(|id| (*id, 1.0)).collect(),
            ),
            (
                "graph",
                false,
                graph_ids.iter().map(|id| (*id, 1.0)).collect(),
            ),
        ] {
            for (rank, (id, weight)) in ranking.iter().enumerate() {
                let key = (is_image, *id);
                let contribution = weight / (RRF_K + rank as f32 + 1.0);
                match seen.get(&key) {
                    Some(&slot) => {
                        fused[slot].1 += contribution;
                        lists[slot].push(name);
                    }
                    None => {
                        seen.insert(key, fused.len());
                        fused.push((key, contribution));
                        lists.push(vec![name]);
                    }
                }
            }
        }
        // Sorted together with their provenance, so a hit never carries the
        // badges of whichever chunk happened to land in its slot.
        let mut ranked: Vec<Candidate> = fused.into_iter().zip(lists).collect();
        ranked.sort_by(|a, b| b.0.1.total_cmp(&a.0.1));
        ranked.truncate(k);

        let mut hits = Vec::with_capacity(ranked.len());
        for (((is_image, id), score), found_by) in ranked {
            if is_image {
                // An image hit carries its path and pixel size where a chunk
                // carries a line range and its text.
                if let Some(row) = store::image(&self.db, id as i64)? {
                    hits.push((
                        Hit {
                            score,
                            path: row.path,
                            start_line: 0,
                            end_line: 0,
                            text: String::new(),
                            store: None,
                            lists: found_by,
                            image: Some(Pixels {
                                width: row.width,
                                height: row.height,
                            }),
                        },
                        0.0,
                    ));
                }
                continue;
            }
            // A dangling id means SQLite and the index drifted apart; skip it
            // rather than fail the whole query.
            if let Some(row) = store::chunk(&self.db, id)? {
                let similarity = dense_ids
                    .iter()
                    .position(|d| *d == id)
                    .and_then(|i| dense_scores.get(i).copied())
                    .unwrap_or(0.0);
                hits.push((
                    Hit {
                        score,
                        path: row.path,
                        start_line: row.start_line,
                        end_line: row.end_line,
                        text: row.text,
                        // Set by the caller when it knows there is more than one
                        // store to tell apart; a store cannot label itself.
                        store: None,
                        lists: found_by,
                        image: None,
                    },
                    similarity,
                ));
            }
        }
        Ok(hits)
    }

    /// The image half of a search: the query in CLIP's text space, against the
    /// store's image vectors.
    ///
    /// Empty, and free, for a store that holds no image — which is every store
    /// that has only ever been pointed at source code.
    fn image_search(
        &mut self,
        query: &str,
        depth: usize,
        filter: &Filter,
    ) -> Result<Vec<(u64, f32)>> {
        if store::image_count(&self.db)? == 0 {
            return Ok(Vec::new());
        }
        let allowlist = if filter.is_empty() {
            index::Allowlist::All
        } else {
            let mut ids = Vec::new();
            for id in store::filtered_image_ids(&self.db, filter.groups())? {
                // Same guard the text half uses: turbovec panics on an id its
                // index does not hold, and a run interrupted between the row
                // and the vector leaves exactly that.
                if self.images.contains(id as u64)? {
                    ids.push(id as u64);
                }
            }
            if ids.is_empty() {
                return Ok(Vec::new());
            }
            index::Allowlist::Subset(ids)
        };

        let mut vector = self.clip.embed_query(query, self.quiet)?;
        normalize(&mut vector);
        let (scores, ids) = self.images.search(&vector, depth, &allowlist)?;
        Ok(ids
            .into_iter()
            .zip(scores.into_iter().chain(std::iter::repeat(0.0)))
            .collect())
    }

    /// How many images this store holds.
    pub fn image_count(&self) -> Result<i64> {
        store::image_count(&self.db)
    }

    /// `(files, chunks, indexed bytes)`
    pub fn stats(&self) -> Result<(i64, i64, i64)> {
        store::stats(&self.db)
    }

    /// Write the index out durably, so an interrupted save cannot leave a
    /// half-written index behind.
    pub fn save(&mut self) -> Result<()> {
        self.index.save()?;
        // The second space is saved with the first: a run that embedded an
        // image and did not write its vector would leave a row addressing
        // nothing, which is the one inconsistency this store's whole design is
        // arranged to prevent.
        self.images.save()?;

        // Bumped after the rename, never before: a reader that sees the new
        // generation is guaranteed to find the new index behind it. Read back
        // from the store rather than incremented from this process's copy,
        // which may predate another writer's run.
        self.generation = generation(&self.db)? + 1;
        store::set_meta(&self.db, GENERATION, &self.generation.to_string())?;
        Ok(())
    }
}

#[derive(Default)]
struct Batch {
    ids: Vec<u64>,
    texts: Vec<String>,
}

pub(crate) fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v {
            *x /= norm;
        }
    }
}

/// How many times this store's index has been rewritten. Absent on a store
/// written before 0.4.0, which reads as zero and is bumped on its first write.
fn generation(db: &Connection) -> Result<u64> {
    Ok(store::get_meta(db, GENERATION)?
        .and_then(|v| v.parse().ok())
        .unwrap_or(0))
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Byte counts sized to the unit that actually shows a digit — a small corpus
/// reported as "0.0 MB" reads like a bug rather than a small corpus.
pub fn human_bytes(bytes: i64) -> String {
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

pub fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Overrides [`model_cache_dir`].
pub const MODEL_CACHE_ENV: &str = "SEMLITH_MODEL_CACHE";

/// Where ONNX model weights are cached. Shared across stores — the weights are
/// large and identical for a given model.
pub fn model_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var(MODEL_CACHE_ENV) {
        return PathBuf::from(dir);
    }
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join(".cache").join("semlith").join("models")
}

/// Walk `roots`, honouring `.gitignore` and skipping hidden files. Returns
/// canonical paths so the same file reached two ways is one entry.
fn walk(roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for root in roots {
        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            // Honour `.gitignore` even outside a git repo. A notes or docs
            // folder is a perfectly normal thing to index, and a `.gitignore`
            // sitting in it still means "not this".
            .require_git(false)
            .filter_entry(|e| e.file_name() != ".semlith");

        for result in builder.build() {
            // Unreadable directories should not abort the run, but silently
            // indexing nothing is worse than a noisy line on stderr.
            let entry = match result {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("semlith: skipping unreadable path: {e}");
                    continue;
                }
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let path = canonical(entry.path());
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_gives_unit_length() {
        let mut v = vec![3.0, 4.0];
        normalize(&mut v);
        assert!((v.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-6);

        // A zero vector must survive rather than become NaN.
        let mut z = vec![0.0, 0.0];
        normalize(&mut z);
        assert_eq!(z, vec![0.0, 0.0]);
    }

    /// A single file must never queue more than one batch of work. Before
    /// this was enforced per chunk rather than per file, one 8 MB file could
    /// hold thousands of chunks in memory and embed them in a single call.
    #[test]
    fn one_large_file_does_not_queue_more_than_a_batch() {
        let text = "a line of perfectly ordinary text\n".repeat(4000);
        let chunks = chunk::chunk_text(&text);
        assert!(
            chunks.len() > EMBED_BATCH * 4,
            "test file is too small to prove anything: {} chunks",
            chunks.len()
        );

        // Mirror the accumulate-and-flush rule from index_paths.
        let mut queued = 0usize;
        let mut high_water = 0usize;
        for _ in &chunks {
            queued += 1;
            high_water = high_water.max(queued);
            if queued >= EMBED_BATCH {
                queued = 0;
            }
        }
        assert_eq!(
            high_water, EMBED_BATCH,
            "queue grew past one batch within a single file"
        );
    }

    #[test]
    fn a_new_store_records_the_default_model_and_reopens_on_it() {
        let dir = tempdir();
        let created = Semlith::open(&dir, None).unwrap();
        assert_eq!(*created.model(), default_model());
        assert_eq!(created.dim(), 384);
        drop(created);

        // Reopening must read the model back out of the store, not re-derive
        // it from the default — otherwise changing the default would silently
        // orphan every existing store.
        let reopened = Semlith::open(&dir, None).unwrap();
        assert_eq!(*reopened.model(), default_model());
    }

    #[test]
    fn an_existing_store_keeps_its_own_model() {
        use fastembed::EmbeddingModel;
        let dir = tempdir2();
        let old = Model::Builtin(EmbeddingModel::BGESmallENV15);
        let created = Semlith::open(&dir, Some(old.clone())).unwrap();
        assert_eq!(*created.model(), old);
        drop(created);

        // Opening with no preference must not migrate it to the new default.
        let reopened = Semlith::open(&dir, None).unwrap();
        assert_eq!(*reopened.model(), old);

        // Asking for a different model than the store holds is an error, not a
        // silent rebuild.
        assert!(Semlith::open(&dir, Some(Model::Granite)).is_err());
    }

    fn tempdir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("semlith-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn tempdir2() -> PathBuf {
        let d = std::env::temp_dir().join(format!("semlith-test-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
}
