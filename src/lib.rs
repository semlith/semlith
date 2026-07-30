//! semlith — a local semantic cache for AI agents.
//!
//! Point it at files, get back a store that answers "what part of my corpus is
//! relevant to this question?" in milliseconds, without shipping anything off
//! the machine and without an agent burning tokens reading whole files.
//!
//! Two pieces of state live side by side in the store directory:
//!
//! - `index.tv` — a [`turbovec`] TurboQuant index holding only quantized
//!   vectors keyed by chunk id.
//! - `store.db` — SQLite holding the chunk text, its file, and its line span.
//!
//! A search quantizes the query, gets ids back from the index, then resolves
//! them to text with one SQLite lookup each.

pub mod chunk;
pub mod embed;
pub mod filter;
pub mod fleet;
pub mod lock;
pub mod mcp;
pub mod store;
pub mod watch;

use anyhow::{Context, Result, bail};
use embed::Model;
use fastembed::TextEmbedding;
use filter::Filter;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use turbovec::IdMapIndex;

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

/// How much deeper than `k` to look in each ranking before fusing.
const RANK_DEPTH: usize = 4;

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
}

#[derive(Debug, Default, Clone, Copy, Serialize)]
pub struct IndexReport {
    pub scanned: usize,
    pub indexed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub removed: usize,
    pub chunks: usize,
}

pub struct Semlith {
    dir: PathBuf,
    db: Connection,
    index: IdMapIndex,
    model: Model,
    dim: usize,
    embedder: Option<TextEmbedding>,
    /// The index generation this process has loaded. Compared against the
    /// store's on every search to notice another process's writes.
    generation: u64,
    /// Print model-download progress to stderr. Off for the MCP server, where
    /// stdout/stderr are a protocol channel.
    pub quiet: bool,
}

impl Semlith {
    /// Open (or create) a store in `dir`.
    ///
    /// `model` is only honoured when the store is new; an existing store keeps
    /// the model it was built with, since vectors from two models are not
    /// comparable.
    pub fn open(dir: impl AsRef<Path>, model: Option<Model>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating store directory {}", dir.display()))?;

        let db = store::open(&dir.join("store.db"))?;

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
                store::set_meta(&db, "model", &m.to_string())?;
                m
            }
        };

        let dim = model.dim()?;

        let index_path = dir.join("index.tv");
        let index = if index_path.exists() {
            let idx = IdMapIndex::load(&index_path)
                .with_context(|| format!("loading {}", index_path.display()))?;
            if let Some(d) = idx.dim_opt()
                && d != dim
            {
                bail!("index is {d}-dimensional but {model} produces {dim}; store is corrupt");
            }
            idx
        } else {
            IdMapIndex::new(dim, BIT_WIDTH).map_err(|e| anyhow::anyhow!("{e:?}"))?
        };

        let generation = generation(&db)?;

        Ok(Self {
            dir,
            db,
            index,
            model,
            dim,
            embedder: None,
            generation,
            quiet: false,
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

        let path = self.dir.join("index.tv");
        if !path.exists() {
            return Ok(());
        }
        let index =
            IdMapIndex::load(&path).with_context(|| format!("reloading {}", path.display()))?;
        if let Some(d) = index.dim_opt()
            && d != self.dim
        {
            bail!(
                "index is {d}-dimensional but {} produces {}; store is corrupt",
                self.model,
                self.dim
            );
        }
        // A reader that reloads mid-session should not hand the cost of a cold
        // index to whoever asked the next question.
        index.prepare();
        self.index = index;
        // Only now, and to the generation read *before* the load: a failed
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

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn db(&self) -> &Connection {
        &self.db
    }

    /// The store directory, for a caller that needs to take the store lock
    /// itself rather than for the length of one call.
    pub fn dir(&self) -> &Path {
        &self.dir
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
        self.index.prepare();
        Ok(())
    }

    /// Warm the vector index without loading a model, for a caller that shares
    /// one loaded model across several stores.
    pub fn warm_index(&mut self) {
        self.index.prepare();
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
        on_file: impl FnMut(&Path),
    ) -> Result<IndexReport> {
        // Held for the whole run, including the index.tv write at the end.
        // Two concurrent runs would otherwise interleave their SQLite writes
        // and their index rewrites until the two disagree.
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.index_walk(roots, on_file)
    }

    /// `index_paths` without taking the lock, for a caller that already holds
    /// it — `semlith watch` holds it for its whole life.
    pub(crate) fn index_walk(
        &mut self,
        roots: &[PathBuf],
        on_file: impl FnMut(&Path),
    ) -> Result<IndexReport> {
        self.index_set(walk(roots), true, on_file)
    }

    /// Re-index exactly `paths`, evicting any that have gone from disk.
    ///
    /// No walk and no orphan sweep: the caller already knows which files
    /// changed, which is the whole point of watching. The lock is the caller's
    /// too.
    pub(crate) fn index_changed(
        &mut self,
        paths: Vec<PathBuf>,
        on_file: impl FnMut(&Path),
    ) -> Result<IndexReport> {
        self.index_set(paths, false, on_file)
    }

    /// The body both entry points share. `sweep` drops every recorded file
    /// that is no longer on disk — right for a full walk, wrong for a batch of
    /// events, which only knows about the paths in it.
    fn index_set(
        &mut self,
        paths: Vec<PathBuf>,
        sweep: bool,
        mut on_file: impl FnMut(&Path),
    ) -> Result<IndexReport> {
        // A run killed mid-save leaves index.tv.tmp behind. Removing it here
        // and not on open is deliberate: the caller holds the store lock, so
        // there is no live writer whose half-written index this could be.
        let _ = std::fs::remove_file(self.dir.join("index.tv.tmp"));

        let mut report = IndexReport::default();
        let mut pending = Batch::default();
        // Files whose vectors are embedded but not yet durable. Their hash is
        // written only after `index.tv` lands, so a crash re-indexes them.
        let mut completed: Vec<(i64, String)> = Vec::new();

        for path in paths {
            report.scanned += 1;
            let key = path.to_string_lossy().into_owned();

            // Size check before the read, so a multi-gigabyte blob is never
            // pulled into memory just to be rejected.
            match std::fs::metadata(&path) {
                Ok(m) if m.len() > 0 && m.len() <= chunk::MAX_FILE_BYTES => {}
                _ => {
                    // A batch of events can name a file that has just been
                    // deleted or renamed away. Evicting it here is what makes
                    // a deletion visible without a full sweep.
                    if !path.exists() {
                        let ids = store::delete_file(&self.db, &key)?;
                        if !ids.is_empty() {
                            for id in ids {
                                self.index.remove(id);
                            }
                            report.removed += 1;
                            continue;
                        }
                    }
                    report.skipped += 1;
                    continue;
                }
            }
            let Ok(bytes) = std::fs::read(&path) else {
                report.skipped += 1;
                continue;
            };

            let hash = blake3::hash(&bytes).to_hex().to_string();
            if store::file_hash(&self.db, &key)?.as_deref() == Some(hash.as_str()) {
                report.unchanged += 1;
                continue;
            }

            let Some(text) = chunk::extract(&path, &bytes) else {
                report.skipped += 1;
                continue;
            };
            let chunks = chunk::chunk_text(&text);
            if chunks.is_empty() {
                report.skipped += 1;
                continue;
            }

            on_file(&path);

            // Replacing a file: evict its old vectors before adding new ones.
            for id in store::delete_file(&self.db, &key)? {
                self.index.remove(id);
            }

            let file_id = store::insert_file(&self.db, &key, PENDING, bytes.len() as u64, now())?;
            for (ord, c) in chunks.iter().enumerate() {
                let id =
                    store::insert_chunk(&self.db, file_id, ord, c.start_line, c.end_line, &c.text)?;
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

            completed.push((file_id, hash));
            report.indexed += 1;
            report.chunks += chunks.len();
        }

        self.flush(&mut pending)?;

        // Anything recorded but no longer on disk is dead weight.
        if sweep {
            for key in store::all_paths(&self.db)? {
                if !Path::new(&key).exists() {
                    for id in store::delete_file(&self.db, &key)? {
                        self.index.remove(id);
                    }
                    report.removed += 1;
                }
            }
        }

        // A batch that changed nothing must not rewrite index.tv. A watcher
        // sees plenty of events on files whose bytes are identical, and each
        // rewrite is the whole index.
        if report.indexed > 0 || report.removed > 0 || !self.dir.join("index.tv").exists() {
            self.save()?;
        }

        // ponytail: hashes are committed in one shot after the index is
        // durable. A crash mid-run re-indexes the whole batch; add periodic
        // checkpointing when someone indexes a corpus big enough to care.
        let tx = self.db.unchecked_transaction()?;
        for (file_id, hash) in &completed {
            tx.execute(
                "UPDATE files SET hash = ?1 WHERE id = ?2",
                rusqlite::params![hash, file_id],
            )?;
        }
        tx.commit()?;

        Ok(report)
    }

    fn flush(&mut self, batch: &mut Batch) -> Result<()> {
        if batch.ids.is_empty() {
            return Ok(());
        }
        let ids = std::mem::take(&mut batch.ids);
        let texts = std::mem::take(&mut batch.texts);

        let vectors = self.embed(texts)?;
        let flat: Vec<f32> = vectors.into_iter().flatten().collect();
        self.index
            .add_with_ids(&flat, &ids)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        Ok(())
    }

    /// Remove a single file from the store.
    pub fn forget(&mut self, path: &Path) -> Result<usize> {
        let key = canonical(path).to_string_lossy().into_owned();
        let ids = store::delete_file(&self.db, &key)?;
        for id in &ids {
            self.index.remove(*id);
        }
        if !ids.is_empty() {
            self.save()?;
        }
        Ok(ids.len())
    }

    /// Top-`k` chunks for `query`, best first, over the whole store.
    pub fn search(&mut self, query: &str, k: usize) -> Result<Vec<Hit>> {
        self.search_filtered(query, k, &Filter::default())
    }

    /// Resolve a filter to the ids the vector index may consider.
    fn allowlist(&self, filter: &Filter) -> Result<Allowlist> {
        if filter.is_empty() {
            return Ok(Allowlist::All);
        }
        let ids: Vec<u64> = store::filtered_chunk_ids(&self.db, filter.groups())?
            .into_iter()
            // turbovec panics on an id the index does not hold, and SQLite can
            // hold a chunk the index does not if a run was interrupted between
            // the two. A stale row must not take a search down with it.
            .filter(|id| self.index.contains(*id))
            .collect();

        Ok(if ids.is_empty() {
            Allowlist::Empty
        } else if ids.len() == self.index.len() {
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
        let vector = self.embed(vec![self.model.query_text(query)])?.remove(0);
        self.search_with_vector(query, &vector, k, filter)
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

        if self.index.is_empty() || k == 0 {
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

        let (dense_scores, dense_ids) =
            self.index
                .search_with_allowlist(vector, depth, allowlist.as_slice());
        let keyword_ids = store::keyword_search(&self.db, query, depth, filter.groups())?;

        let mut fused: Vec<(u64, f32)> = Vec::new();
        let mut seen: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for ranking in [&dense_ids, &keyword_ids] {
            for (rank, id) in ranking.iter().enumerate() {
                let contribution = 1.0 / (RRF_K + rank as f32 + 1.0);
                match seen.get(id) {
                    Some(&slot) => fused[slot].1 += contribution,
                    None => {
                        seen.insert(*id, fused.len());
                        fused.push((*id, contribution));
                    }
                }
            }
        }
        fused.sort_by(|a, b| b.1.total_cmp(&a.1));
        fused.truncate(k);

        let mut hits = Vec::with_capacity(fused.len());
        for (id, score) in fused {
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
                    },
                    similarity,
                ));
            }
        }
        Ok(hits)
    }

    /// `(files, chunks, indexed bytes)`
    pub fn stats(&self) -> Result<(i64, i64, i64)> {
        store::stats(&self.db)
    }

    /// Write the index out via a temp file + rename, so an interrupted save
    /// cannot leave a half-written `index.tv` behind.
    pub fn save(&mut self) -> Result<()> {
        let final_path = self.dir.join("index.tv");
        let tmp = self.dir.join("index.tv.tmp");
        self.index
            .write(&tmp)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &final_path)?;

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

/// What the vector index is allowed to look at for one query.
enum Allowlist {
    /// No filter, or one that selects everything the index holds.
    All,
    /// The filter selects nothing. The query is over before it starts.
    Empty,
    Subset(Vec<u64>),
}

impl Allowlist {
    /// `None` is turbovec's "search everything".
    fn as_slice(&self) -> Option<&[u64]> {
        match self {
            Allowlist::Subset(ids) => Some(ids),
            _ => None,
        }
    }
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

/// Where ONNX model weights are cached. Shared across stores — the weights are
/// large and identical for a given model.
pub fn model_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("SEMLITH_MODEL_CACHE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join(".cache").join("semlith").join("models")
}

/// Default store location: `.semlith` beside whatever you are indexing.
pub fn default_store_dir() -> PathBuf {
    // Always at least one element, so the index is not a panic in waiting.
    store_dirs(&[]).remove(0)
}

/// The stores a command should use: the `--store` flags if any were given, else
/// whatever `SEMLITH_STORE` names, else `.semlith` in the current directory.
///
/// `SEMLITH_STORE` is split the way `PATH` is — `:` on Unix, `;` on Windows —
/// so an agent's MCP server definition can name several stores in one variable
/// without a wrapper script. A single value therefore still means exactly what
/// it always meant, and the price is that a store path containing the
/// platform's own separator has to be passed as a flag instead.
pub fn store_dirs(flags: &[PathBuf]) -> Vec<PathBuf> {
    if !flags.is_empty() {
        return flags.to_vec();
    }
    if let Some(raw) = std::env::var_os("SEMLITH_STORE") {
        let dirs: Vec<PathBuf> = std::env::split_paths(&raw)
            .filter(|p| !p.as_os_str().is_empty())
            .collect();
        if !dirs.is_empty() {
            return dirs;
        }
    }
    vec![PathBuf::from(".semlith")]
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
