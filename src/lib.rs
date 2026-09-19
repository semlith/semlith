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
pub mod agentfiles;
pub mod brief;
pub mod chunk;
pub mod clientfile;
pub mod clients;
pub mod clock;
pub mod daemon;
pub mod doctor;
pub mod embed;
pub mod filter;
pub mod fleet;
/// Readers for the formats that are not plain text. Private: what semlith
/// extracts from a given document is documented behaviour, not an API.
mod formats;
pub mod graph;
pub mod home;
pub mod hook;
pub mod http;
pub mod image;
pub mod index;
pub mod ledger;
pub mod lock;
pub mod mcp;
pub mod pattern;
pub mod portal;
pub mod proxy;
pub mod rerank;
pub mod routes;
/// The daemon as a login service, so a client never finds nothing.
pub mod service;
pub mod setup;
pub mod store;
pub mod system;
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

/// How many definitions of a typed name are lifted above the fused order.
///
/// See the lift in [`Semlith::search_preferring`]. Three, so an ambiguous name
/// says it is ambiguous without spending the whole answer on it.
const DEFINITION_LIFT: usize = 3;

/// What a query looks like, which decides which half of the fusion is trusted.
///
/// Read off the query's own text, deterministically, with no model: an agent
/// that pastes `record_retrieval` and an agent that asks "where does a
/// retrieval get written down" want the same code, and until 0.16.0 both were
/// scored as though the two halves were equally likely to know. They are not —
/// FTS5 is exact about an identifier and vague about a sentence, and the
/// embedding is the other way round.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Shape {
    /// One token that is shaped like something a programmer typed.
    Identifier,
    /// Anything else, which in practice is a sentence.
    Question,
}

impl Shape {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identifier => "identifier-shaped",
            Self::Question => "question-shaped",
        }
    }

    /// What the shape did to the fusion, in the words the portal prints.
    pub fn weighting(self) -> &'static str {
        match self {
            Self::Identifier => "keyword weighted 2×",
            Self::Question => "vector and keyword equal",
        }
    }

    /// The multiplier this shape gives the keyword list.
    fn keyword_weight(self) -> f32 {
        match self {
            Self::Identifier => 2.0,
            Self::Question => 1.0,
        }
    }

    /// The reciprocal-rank constant each list is fused with, under this shape.
    ///
    /// One constant for every list flattens the curve equally for all of them,
    /// which is the same as saying no list is ever more certain about its own
    /// first place than any other. That is false, and measurably: FTS5 is exact
    /// about an identifier and vague about a sentence, and the embedding is the
    /// other way round. A chunk found at ranks four and five by the two vague
    /// lists, plus the graph list derived from them, outscored the chunk the one
    /// authoritative list put first.
    ///
    /// A smaller constant steepens a list's curve, so its own first place is
    /// worth much more than its fourth. `RRF_K` stays the value for a list this
    /// shape has no reason to trust.
    fn list_constant(self, list: &str) -> f32 {
        const STEEP: f32 = 12.0;
        match (self, list) {
            (Self::Identifier, "keyword") => STEEP,
            (Self::Question, "vector") => STEEP,
            _ => RRF_K,
        }
    }
}

/// One list going into the fusion: its name, whether its ids are image ids,
/// and each candidate with the weight that list gives it.
type CandidateList = (&'static str, bool, Vec<(u64, f32)>);

/// One candidate out of the fusion: which id space it belongs to, its id, and
/// its fused score.
type Scored = ((bool, u64), f32);

/// Reciprocal-rank fusion over the candidate lists, with the derived list
/// adding candidates and never amplifying them.
///
/// Lists in, one fused score and one provenance badge set per candidate out,
/// in the order the lists were given. Pulled out of `search_preferring` so the
/// one rule that is easy to state and easy to lose — the graph list is derived
/// from the other two, so a chunk they already found takes nothing from it —
/// can be asserted on three lists with a known overlap rather than inferred
/// from a store.
///
/// A candidate's score is the sum of `weight / (constant + rank + 1)` over the
/// lists that found it first-hand. `constant_of` is the shape's own curve for
/// that list; `rank` is the position within the list.
fn fuse(
    lists: &[CandidateList],
    constant_of: impl Fn(&str) -> f32,
) -> (Vec<Scored>, Vec<Vec<&'static str>>) {
    let mut fused: Vec<((bool, u64), f32)> = Vec::new();
    let mut seen: std::collections::HashMap<(bool, u64), usize> = std::collections::HashMap::new();
    let mut badges: Vec<Vec<&'static str>> = Vec::new();

    for (name, is_image, ranking) in lists {
        let constant = constant_of(name);
        // The graph list brings candidates and never votes on them.
        //
        // It is derived from the other two: `graph_expansion` walks out from
        // what the dense and keyword lists already found. So when it returns a
        // chunk those lists also returned, its contribution is not a second
        // opinion — it is the first opinion counted twice, and a chunk two
        // lists ranked mediocrely beats the one an authoritative list ranked
        // first partly on that double count.
        //
        // It earns its place by reaching chunks neither list found, and that
        // half is kept: a candidate only this list holds enters with its own
        // score, and the rescoring stage is what sorts those.
        let derived = *name == "graph";
        for (rank, (id, weight)) in ranking.iter().enumerate() {
            let key = (*is_image, *id);
            let contribution = weight / (constant + rank as f32 + 1.0);
            match seen.get(&key) {
                Some(&slot) => {
                    if !derived {
                        fused[slot].1 += contribution;
                    }
                    badges[slot].push(name);
                }
                None => {
                    seen.insert(key, fused.len());
                    fused.push((key, contribution));
                    badges.push(vec![name]);
                }
            }
        }
    }
    (fused, badges)
}

/// Whether a query is shaped like an identifier or like a question.
///
/// The rule is deliberately crude and entirely inspectable: one token, made of
/// the characters an identifier is made of, is an identifier. Everything else
/// is a question. A crude rule that a user can predict beats an accurate one
/// they cannot, because the shape is reported in the answer and `prefer` is
/// there to overrule it.
pub fn shape_of(query: &str) -> Shape {
    let trimmed = query.trim();
    if trimmed.is_empty() || trimmed.split_whitespace().count() > 1 {
        return Shape::Question;
    }
    let identifier = trimmed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '.' || c == '!');
    if identifier && trimmed.chars().any(|c| c.is_alphabetic()) {
        Shape::Identifier
    } else {
        Shape::Question
    }
}

/// Which side of the corpus a caller would rather be given.
///
/// The automatic weighting above is about *how* the query was written;
/// this is about what the caller is looking for, which the query text cannot
/// say. "How does indexing work" matches the architecture document and the
/// function that does it equally well, and an agent about to edit code wants
/// the second one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Prefer {
    /// No bias. The default, and what every release before 0.16.0 did.
    #[default]
    Any,
    /// The implementation rather than the prose about it.
    Code,
    /// The prose rather than the implementation.
    Docs,
}

impl Prefer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::Code => "code",
            Self::Docs => "docs",
        }
    }

    /// Parse the argument every surface takes, rejecting anything else by
    /// name: a caller that passed `prefer: source` has to be told, or it reads
    /// the unbiased answer as a biased one.
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "any" | "" => Ok(Self::Any),
            "code" => Ok(Self::Code),
            "docs" | "doc" => Ok(Self::Docs),
            other => anyhow::bail!("prefer is code, docs or any, not {other:?}"),
        }
    }

    /// What a hit's score is multiplied by under this preference.
    ///
    /// A bias, not a filter. A preferred hit is lifted and the rest keep their
    /// place, so `prefer: code` over a corpus with no code still answers with
    /// the prose rather than with nothing.
    fn multiplier(self, is_code: bool) -> f32 {
        const LIFT: f32 = 1.5;
        match (self, is_code) {
            (Self::Any, _) => 1.0,
            (Self::Code, true) | (Self::Docs, false) => LIFT,
            _ => 1.0,
        }
    }
}

/// How much a chunk the graph walk ranked first is lifted over one it barely
/// reached.
///
/// Small on purpose. The fusion already knows what the query matched; this is
/// the code's opinion about what else is relevant, and it is a tiebreak rather
/// than a second ranking. There is no model behind either of these and there is
/// not going to be one — every input is something the store already holds.
///
/// Measured: removing this factor costs two hits at k@1 and one at k@3 on the
/// harness's question set, so it stays. A third factor, a lift for a chunk
/// inside a named definition, was measured out of the release — in a code
/// repository it is a second and blunter `prefer: code` applied to every query,
/// and it fights the real one (US-SEMLITH-0.16.0-I02).
const GRAPH_PROXIMITY: f32 = 0.15;

/// How much a chunk from a file edited since it was indexed is pushed down.
///
/// Not removed: the excerpt may still be the best answer there is, and 0.15.0
/// already flags it as stale. This only means a current answer of equal
/// quality is preferred to it.
const STALE_PENALTY: f32 = 0.10;

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
///
/// Raised from 4 in 0.23.0, measured. At k=8 the four lists were each searched
/// 32 deep, and the questions that still missed were not missing by a rank --
/// twelve of seventy-seven were not in the fused list at any depth, and seven of
/// those were not found by any list at all. Rescoring cannot help there: it
/// reorders a candidate pool and never adds to it. The pool itself is the only
/// thing that can, so this is the mean that was tried for recall, with the pair
/// on either side of it recorded in the release.
const RANK_DEPTH: usize = 4;

/// How many files an index run may get through without making its work
/// durable.
///
/// This is what an interruption costs: the vectors embedded since the last
/// checkpoint, and no more.
///
/// It counts files rather than seconds, and that is issue #88's last cause. A
/// checkpoint flushes the pending batch, and the embedder pads a batch to the
/// longest sequence in it — so a batch split at a wall-clock boundary is a
/// batch with different padding, and the vectors it produces differ in their
/// last bits. int8 quantization turns a last-bit difference into a rank flip,
/// and the retrieval harness reported a different hit@k on every run because
/// of it. Three runs over one corpus produced three different indexes; with
/// the checkpoint interval pushed past the length of the run, two runs produced
/// byte-identical ones. Counting files makes the split a function of the corpus
/// rather than of how busy the machine was, so a store is reproducible from its
/// corpus.
///
/// Two hundred files is the same order of durability as the thirty seconds it
/// replaces on the corpora this is built for, and unlike thirty seconds it is
/// the same on a slow machine.
///
/// Only a sharded store checkpoints. A store written before 0.7.0 would have to
/// rewrite its entire index to do it, which is the cost sharding exists to
/// remove; those stores behave exactly as they did.
const CHECKPOINT_FILES: usize = 200;

/// Override for [`CHECKPOINT_FILES`], in files. For the tests, which cannot
/// index two hundred files to prove a checkpoint happened. Not part of the
/// documented environment.
const CHECKPOINT_FILES_ENV: &str = "SEMLITH_CHECKPOINT_FILES";

fn checkpoint_files() -> usize {
    match std::env::var(CHECKPOINT_FILES_ENV)
        .ok()
        .and_then(|v| v.parse().ok())
    {
        Some(files) if files > 0 => files,
        _ => CHECKPOINT_FILES,
    }
}

/// What a chunk reached through a bare-name edge is worth against one reached
/// through an edge the source resolved.
const INFERRED_EXPANSION: f32 = 0.5;

/// A chunk the third list reached, with how it got there.
struct Reached {
    id: u64,
    weight: f32,
    tier: String,
    /// Where the walk put this chunk, as a share of the best one it found.
    ///
    /// 1.0 for the chunk the PageRank ranked first, falling away from there.
    /// This is the "distance from the seeds" the rerank reads: a chunk the
    /// walk barely reached should not be lifted as though the code insisted
    /// on it.
    proximity: f32,
}

/// The tier of the edge that reached `id`, if the graph list reached it at all.
fn provenance_of(graph: &[Reached], id: u64) -> Option<String> {
    graph.iter().find(|r| r.id == id).map(|r| r.tier.clone())
}

/// What `read` was asked for: a span of a file, or a symbol by name.
///
/// Parsed rather than guessed at, so `src/store.rs:1041-1080` and
/// `record_retrieval` are told apart by shape and a caller is never handed the
/// wrong kind of answer for a typo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Span { path: String, start: u32, end: u32 },
    Symbol(String),
}

impl Target {
    /// `path:start-end`, `path:line`, or a symbol name.
    ///
    /// A colon with a number after it is a span; anything else is a name. A
    /// Windows drive letter is not a false positive, because what follows the
    /// colon there is a separator rather than a digit.
    pub fn parse(raw: &str) -> Self {
        let raw = raw.trim();
        if let Some((path, lines)) = raw.rsplit_once(':')
            && !path.is_empty()
        {
            let (start, end) = match lines.split_once('-') {
                Some((a, b)) => (a.parse::<u32>().ok(), b.parse::<u32>().ok()),
                None => (lines.parse::<u32>().ok(), lines.parse::<u32>().ok()),
            };
            if let (Some(start), Some(end)) = (start, end) {
                return Self::Span {
                    path: path.to_string(),
                    start: start.min(end),
                    end: start.max(end),
                };
            }
        }
        Self::Symbol(raw.to_string())
    }
}

/// One span of one file, and nothing around it.
///
/// The second stage of a retrieval. A locate answer costs about 150 bytes a
/// hit and says where to look; this is what turns one of those into the text,
/// without the whole file riding along with it.
#[derive(Debug, Clone, Serialize)]
pub struct Span {
    #[serde(serialize_with = "serialize_plain")]
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    /// The definition the span sits inside, when it sits inside one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<String>,
    /// Whether the file still looks the way it did when it was indexed.
    pub fresh: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// What a `read` produced: the span, or the definitions to choose between.
///
/// A name with several definitions returns the list rather than one of them.
/// Guessing would be the same defect the graph's `ambiguous` value exists to
/// refuse: a confident answer that is right a fraction of the time reads
/// exactly like one that is right.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Read {
    One(Span),
    Choose(Vec<store::SymbolRow>),
}

#[derive(Debug, Clone, Serialize)]
pub struct Hit {
    pub score: f32,
    #[serde(serialize_with = "serialize_plain")]
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
    /// Whether the file still looks the way it did when it was indexed.
    ///
    /// A store being watched is a moving target, and a store that is not being
    /// watched goes out of date the moment someone saves. Either way the
    /// failure is the same and it is silent: an agent quotes an excerpt, the
    /// line numbers are wrong, and nothing in the answer said so.
    ///
    /// Decided by one `stat` per distinct path: the size and the modification
    /// time against what the store recorded when it read the file. That is
    /// deliberately conservative — a `touch` with no edit reads as stale — and
    /// conservative is the right direction, because the cost of a false "check
    /// this" is a reread and the cost of a false "this is current" is a wrong
    /// quotation.
    pub fresh: bool,
    /// The definition this chunk sits inside, and what kind of definition it
    /// is.
    ///
    /// `None` for prose, for a file in a language that carries no symbols, and
    /// for a chunk that falls between definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<String>,
    /// For a hit the graph list reached: how well supported the edge that
    /// reached it was.
    ///
    /// A chunk that arrives through a `resolved` edge is a neighbour the
    /// source vouches for. One that arrives through a bare-name match is a
    /// guess about a neighbour, and an agent should weigh it as one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<String>,
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
    /// Not a file at all: the run is writing what it has embedded to disk.
    ///
    /// Every [`CHECKPOINT_FILES`] files a run flushes its batch and rewrites
    /// the shards, which on a large corpus is twenty seconds in which nothing
    /// is read and nothing was said. A progress bar that sits at 400/600 for
    /// twenty seconds and then jumps to 462 is indistinguishable from a hang;
    /// this is the run saying which of the two it is.
    Writing,
    /// semlith tried to index it and the attempt failed on this file's own
    /// content — a decoder that rejected the bytes, a parser that could not
    /// read them, a path the operating system would not open.
    ///
    /// Told apart from `Skipped` because a skipped file is a decision and a
    /// failed one is an accident, and from an error because the run continues:
    /// one corrupt PNG in a tree of ten thousand files used to end the whole
    /// run, which is the defect this release exists to fix.
    Failed,
}

impl FileOutcome {
    /// The word the portal and the CLI both label a line with.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Indexing => "indexing",
            Self::Writing => "writing",
            Self::Unchanged => "unchanged",
            Self::Skipped => "skipped",
            Self::Removed => "removed",
            Self::Refused => "refused",
            Self::Failed => "failed",
        }
    }
}

/// Why a file was skipped, as a closed set.
///
/// Closed on purpose. "Skipped" with no reason is the line that sent this
/// release's Windows logs in: a run that says nothing about two thousand files
/// is indistinguishable from a run that lost them. Every branch that produces
/// [`FileOutcome::Skipped`] names one of these, and `tests/index_failure.rs`
/// fails if a `file` event ever carries `skipped` without a `why`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// Zero bytes. A generated `.component.scss` or a package's `__init__.py`.
    Empty,
    /// Over [`chunk::MAX_FILE_BYTES`].
    TooLarge,
    /// A socket, a fifo, a directory entry that is not a file.
    NotRegular,
    /// The operating system refused to open or read it, with its own words.
    Unreadable(String),
    /// Bytes no text reader will take.
    Binary,
    /// A reader ran and produced nothing — an empty `.docx`, a notebook with
    /// no cells. Not the same as binary, and a user chasing a missing file
    /// needs to know which of the two happened.
    NoText,
    /// An image whose header no decoder recognised.
    NotDecodableImage,
}

impl SkipReason {
    /// The words that go in `why`. Short, lower-case, and the same string in
    /// the CLI, the portal and the MCP summary.
    pub fn as_str(&self) -> String {
        match self {
            Self::Empty => "empty".to_string(),
            Self::TooLarge => format!("over {} MiB", chunk::MAX_FILE_BYTES / (1024 * 1024)),
            Self::NotRegular => "not a regular file".to_string(),
            Self::Unreadable(e) => format!("unreadable: {e}"),
            Self::Binary => "binary".to_string(),
            Self::NoText => "no text in this document".to_string(),
            Self::NotDecodableImage => "not a decodable image".to_string(),
        }
    }

    /// The reason with its detail stripped, so a summary can count by kind
    /// without every operating-system message becoming its own bucket.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::TooLarge => "too large",
            Self::NotRegular => "not a regular file",
            Self::Unreadable(_) => "unreadable",
            Self::Binary => "binary",
            Self::NoText => "no text in this document",
            Self::NotDecodableImage => "not a decodable image",
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
    ///
    /// `walked` is whether the walk yielded this path or the caller named it,
    /// and it decides one rule only: the hidden-file rule. A dotfile the walk
    /// yielded is there because the user's own `.gitignore` whitelisted it —
    /// `dist/*` then `!dist/.gitkeep` — and refusing it as hidden is the walk
    /// contradicting itself, which is what ended a run on the reporter's
    /// Angular tree. A dotfile the caller named is still refused: `semlith
    /// index ~/.npmrc` is a mistake worth catching. Every other rule — the
    /// credential directories, the credential names — applies to both.
    pub fn refuses(&self, path: &Path, walked: bool, home: Option<&Path>) -> Option<Refusal> {
        if let Some(roots) = &self.roots
            && !filter::within_boundary(path, roots)
        {
            // The roots by name. A refusal that says "outside this store's
            // roots" without saying what they are leaves the caller to guess
            // which store it is holding and what that store is about — and the
            // Privacy page promises this rule by name, so the refusal that
            // enforces it should be legible on its own.
            let named = if roots.is_empty() {
                "this store has no registered roots, so only the home directory is inside \
                 its boundary"
                    .to_string()
            } else {
                format!(
                    "this store's roots are {}",
                    roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            return Some(Refusal {
                why: format!(
                    "is outside this store's roots, so semlith will not index it into this \
                     store — {named}. Index it into the store that covers it, add it as a \
                     root first, or index it from the command line."
                ),
                // Emphatically not. This says nothing about the file's
                // contents — only that this caller may not reach it — and a
                // caller who may not read a path must not be able to delete
                // what a caller who could has already indexed.
                credential: false,
            });
        }
        if !self.allow_secrets
            && let Some(why) = filter::denied_against(path, home)
        {
            if walked && why == filter::Denied::Hidden {
                return None;
            }
            return Some(Refusal {
                why: why.reason(),
                credential: matches!(why, filter::Denied::Name(_) | filter::Denied::Directory(_)),
            });
        }
        None
    }
}

/// Why a path was refused, and whether the refusal is about what it holds.
///
/// The second half decides whether an earlier run's copy is evicted. A file
/// refused because it names a credential should not go on sitting in a store
/// that has decided it will not hold it. A file refused because *this caller*
/// may not reach it is a different statement entirely: an agent confined to one
/// root that asks to index a path outside it would otherwise be able to delete
/// what the command line put there, which is a caller who cannot read a file
/// deleting it.
#[derive(Debug, Clone)]
pub struct Refusal {
    pub why: String,
    pub credential: bool,
}

/// Where an index run has got to, handed to the callback once per file.
///
/// `total` is what the walk found, so `scanned` against it is a real fraction
/// rather than a spinner — which is the difference between a long wait and a
/// wait a person is willing to sit through.
#[derive(Debug, Default, Clone)]
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
    /// Why, for the three outcomes that owe an explanation: `skipped`,
    /// `refused` and `failed`. `None` for the rest, where the outcome is the
    /// whole of what there is to say.
    ///
    /// A `String` rather than a code, because the operating system's own
    /// message and the decoder's own message are the useful part and neither
    /// is drawn from a set semlith controls.
    pub why: Option<String>,
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
    /// Exactly the paths this slice did not reach, in the order it would have
    /// taken them.
    ///
    /// The next slice indexes these rather than walking the roots again. A
    /// re-walk is not merely wasted directory traversal: every file the run had
    /// already done is re-opened and re-hashed to discover it is unchanged, so
    /// a run of N files over S slices read N×S files instead of N, and the
    /// progress a page drew restarted from one on every slice. The content
    /// hashes made it *correct* to re-walk, which is why it went unnoticed.
    #[serde(skip)]
    pub pending: Vec<PathBuf>,
    /// Generated or vendored directories the walk stepped over — `node_modules`,
    /// a `target` beside a `Cargo.toml`, and the rest of the default-ignore
    /// table.
    ///
    /// Named rather than counted in files, because pruning the subtree is the
    /// point and counting what is inside it would undo the saving to report it.
    /// A person looking for a file that is not in their store needs the name of
    /// the directory that was stepped over, which is what this gives them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub generated: Vec<String>,
    /// Whether this pass re-embedded files nothing on disk had changed,
    /// because the store was written by a release that showed the embedding
    /// model less than this one does.
    ///
    /// A user whose unchanged repository suddenly indexes from scratch is owed
    /// the reason, and the reason is a property of the run rather than of any
    /// one file — so it is reported here and said once, not per file.
    pub rechunked: bool,
    /// Paths refused, each with the rule that refused it.
    ///
    /// Listed rather than counted: "three paths were refused" is not something
    /// an agent or a person can act on, and a refusal that is not named reads
    /// as a file that quietly failed to index.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refused: Vec<(String, String)>,
    /// Paths that failed on their own content, each with what failed.
    ///
    /// Listed like `refused`, and for the same reason: a count of failures is
    /// a number somebody has to go and investigate, and this release exists
    /// because those investigations had nothing to go on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed: Vec<(String, String)>,
    /// How many files were skipped for each reason, by kind.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub skipped_reasons: std::collections::BTreeMap<String, usize>,
    /// Files the content scan would have refused and `--include-secrets`
    /// indexed anyway. Reported so that the flag is never silent about what it
    /// did: a store that holds credentials should say how many.
    pub secrets_indexed: usize,
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

/// Hand one file's verdict to the caller's callback.
///
/// A free function rather than a closure because the loop it serves holds
/// `report` mutably between calls, and a closure capturing both would be
/// borrowing the report for the whole run. It exists to make the nine
/// emission sites in `index_set_writing` one line each: they used to be a
/// nine-line struct literal apiece, and the branch that forgot to call it at
/// all — `chunk::extract` returning nothing — is why the portal's counter
/// never reached its total.
fn say_file(
    on_file: &mut impl FnMut(&Path, IndexProgress),
    report: &IndexReport,
    total: usize,
    path: &Path,
    outcome: FileOutcome,
    why: Option<String>,
) {
    debug_assert!(
        why.is_some()
            || !matches!(
                outcome,
                FileOutcome::Skipped | FileOutcome::Refused | FileOutcome::Failed
            ),
        "{} must say why",
        outcome.as_str()
    );
    on_file(
        path,
        IndexProgress {
            outcome,
            scanned: report.scanned,
            indexed: report.indexed,
            chunks: report.chunks,
            total,
            symbols: report.symbols,
            why,
        },
    );
}

/// Count one skip, by kind as well as in total.
///
/// The per-reason counts are what turn "1 847 skipped" into a line somebody
/// can act on, and keeping them here means no branch can raise `skipped`
/// without also saying what kind of skip it was.
fn skip(report: &mut IndexReport, why: &SkipReason) {
    report.skipped += 1;
    *report
        .skipped_reasons
        .entry(why.kind().to_string())
        .or_insert(0) += 1;
}

/// Record one file that failed on its own content.
///
/// `{e:#}` rather than `{e}` so the decoder's own message survives the
/// `anyhow` context above it. "failed to embed image" alone is not a reason,
/// and the reason is the point of the line.
fn failed(report: &mut IndexReport, path: &Path, e: &anyhow::Error) {
    report
        .failed
        .push((path.display().to_string(), format!("{e:#}")));
}

/// One file a store holds that today's rules would refuse.
///
/// The path as a person reads it and the rule, never the credential: `why` is
/// a kind and a line number, and no character of what was matched appears in
/// it. See [`filter::Found`].
#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    /// The store's own key, serialised plain for whoever reads it.
    #[serde(serialize_with = "serialize_plain")]
    pub path: String,
    pub why: String,
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
    /// The full-precision copy of what the codes approximate, for reordering
    /// the handful of candidates a query is about to return.
    exact: index::Exact,
    model: Model,
    dim: usize,
    embedder: Option<TextEmbedding>,
    /// The cross-encoder, once a query has needed it. `None` until then, and
    /// `None` for ever on a machine whose cache does not hold it — the stage
    /// is skipped rather than fetched from inside a query.
    reranker: Option<fastembed::TextRerank>,
    /// The embedder's own tokenizer, for counting rather than estimating.
    tokenizer: Option<tokenizers::Tokenizer>,
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
            exact: index::Exact::open(&dir, dim),
            dir,
            db,
            index,
            images,
            model,
            dim,
            embedder: None,
            tokenizer: None,
            clip: image::Clip::default(),
            generation,
            quiet: false,
            reranker: None,
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

    /// The tokenizer this store's model embeds with, once it has been loaded.
    ///
    /// `None` before the first embedding, which is why the ledger labels every
    /// row with what counted it: a graph-only session never loads a model, and
    /// a row counted at four characters per token must never be added to one
    /// counted properly.
    pub fn tokenizer(&self) -> Option<&tokenizers::Tokenizer> {
        self.tokenizer.as_ref()
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
            let cache = model_cache_dir()?;
            // Read from the same cache, in the same breath. The ledger counts
            // tokens with it, so it is loaded exactly when the model is and
            // never fetched on its own.
            self.tokenizer = self.model.tokenizer(&cache);
            self.embedder = Some(self.model.load(cache, chunk::MAX_CHARS / 2, self.quiet)?);
        }
        Ok(self.embedder.as_mut().unwrap())
    }

    /// The cross-encoder, loaded on the first query that can use it.
    ///
    /// `None` rather than an error when the model is not in the cache: the
    /// stage is an improvement on an answer semlith can already give, and a
    /// search that failed because a reranker was missing would be a worse
    /// product than one that ranks by fusion alone. `semlith setup` fetches
    /// it and `semlith doctor` says whether it is there, so the silence has
    /// somewhere to be reported.
    fn reranker(&mut self) -> Option<&mut fastembed::TextRerank> {
        if self.reranker.is_none() {
            let cache = model_cache_dir().ok()?;
            if !rerank::cached(&cache) {
                return None;
            }
            match rerank::load(&cache, self.quiet) {
                Ok(model) => self.reranker = Some(model),
                Err(e) => {
                    if !self.quiet {
                        eprintln!("the rescoring model did not load, ranking by fusion alone: {e}");
                    }
                    return None;
                }
            }
        }
        self.reranker.as_mut()
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

    /// Carry on a run from exactly where its last slice stopped.
    ///
    /// `files` is the previous slice's [`IndexReport::pending`] — the paths it
    /// did not reach, in the order it would have taken them. No walk: the tree
    /// was walked when the run started, and walking it again on every slice is
    /// what made a long run re-open and re-hash everything it had already done,
    /// forty-five seconds at a time.
    ///
    /// The orphan sweep still belongs to this call, because `index_set` only
    /// performs it on the slice that reaches the end of the list.
    pub(crate) fn index_rest_held_under(
        &mut self,
        files: Vec<PathBuf>,
        budget: std::time::Duration,
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        let deadline = std::time::Instant::now() + budget;
        self.index_set(
            // Every one of these came out of the walk this run started with,
            // so they are walked paths and are held to the same boundary rule
            // they were held to then.
            Walked {
                files,
                named: Vec::new(),
                unreadable: Vec::new(),
                generated: Vec::new(),
            },
            true,
            Some(deadline),
            Some(control),
            on_file,
        )
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
        self.index_set(
            // Every one of these came out of a filesystem event on a watched
            // tree and was filtered through the same walk, so they are walked
            // paths and not paths a caller named.
            Walked {
                files: paths,
                named: Vec::new(),
                unreadable: Vec::new(),
                generated: Vec::new(),
            },
            false,
            None,
            None,
            on_file,
        )
    }

    /// The body both entry points share. `sweep` drops every recorded file
    /// that is no longer on disk — right for a full walk, wrong for a batch of
    /// events, which only knows about the paths in it.
    fn index_set(
        &mut self,
        walked: Walked,
        sweep: bool,
        deadline: Option<std::time::Instant>,
        control: Option<&dyn Fn() -> Flow>,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        // Every path that writes to this store funnels through here, so this is
        // where the connection stops refusing writes — and, when this returns,
        // starts refusing them again. See `store::Writing` and `writing` below.
        self.writing(move |me| me.index_set_writing(walked, sweep, deadline, control, on_file))
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
        walked: Walked,
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
        let every = checkpoint_files();
        let mut since_checkpoint = 0usize;

        // A store written before 0.25.0 holds code chunks embedded without the
        // definition they sit inside, and before 0.22.0 fixed-window chunks as
        // well. The hash check below would leave it holding them for ever: its
        // files have not changed, the rule for what a chunk is and what the
        // model is shown has. So the first pass under this release re-chunks
        // and re-embeds everything it walks, and the format row moves at the
        // end of a pass that swept the whole store — a pass over one directory
        // leaves the rest of the store on the old rule, and saying otherwise in
        // the meta row would be a claim about files this run never looked at.
        let rechunk = store::format(&self.db)? < store::CODE_CONTEXT;
        report.rechunked = rechunk;

        // Refused before anything is read. A path that names a credential or
        // sits outside this caller's boundary is reported by name with the rule
        // that refused it, rather than dropped from the walk — an agent that
        // asked for a file and got silence cannot tell that from a file that
        // was not there.
        let Walked {
            files: walked_paths,
            named,
            unreadable: unwalkable,
            generated,
        } = walked;
        let (paths, refused): (Vec<PathBuf>, Vec<(PathBuf, Refusal)>) = {
            let mut allowed = Vec::with_capacity(walked_paths.len() + named.len());
            let mut refused = Vec::new();
            // Once for the run, not once for the file. The home directory
            // cannot move while a run is going, and resolving it per file was
            // an opened handle per file on Windows.
            let home = crate::home::user_home().ok().map(|h| canonical(&h));
            let all = named
                .into_iter()
                .map(|p| (p, false))
                .chain(walked_paths.into_iter().map(|p| (p, true)));
            for (path, walked) in all {
                match self.boundary.refuses(&path, walked, home.as_deref()) {
                    Some(why) => refused.push((path, why)),
                    None => allowed.push(path),
                }
            }
            allowed.sort();
            (allowed, refused)
        };
        let total = paths.len() + refused.len() + unwalkable.len();
        for (path, refusal) in &refused {
            // A file semlith has decided it will not hold is a file it does
            // not keep holding. A rule that widens — this release widened two
            // of them — otherwise leaves every store that was indexed under
            // the old rule still carrying what the new one refuses. Only for a
            // refusal about the file's own contents: see `Refusal`.
            let evicted = if refusal.credential {
                let key = path.to_string_lossy().into_owned();
                let (chunks, images) = self.evict(&key)?;
                chunks + images
            } else {
                0
            };
            report.removed += usize::from(evicted > 0);
            let why = if evicted > 0 {
                format!(
                    "{} Its earlier contents have been removed from this store.",
                    refusal.why
                )
            } else {
                refusal.why.clone()
            };
            report
                .refused
                .push((path.display().to_string(), why.clone()));
            report.scanned += 1;
            say_file(
                &mut on_file,
                &report,
                total,
                path,
                FileOutcome::Refused,
                Some(why),
            );
        }
        report.generated = generated.iter().map(|p| p.display().to_string()).collect();
        // Entries the walk could not read. They used to be a line on stderr,
        // which the daemon and the portal never see, so an unreadable
        // directory looked like a tree that simply had nothing in it.
        for (path, why) in &unwalkable {
            report
                .failed
                .push((path.display().to_string(), why.clone()));
            report.scanned += 1;
            say_file(
                &mut on_file,
                &report,
                total,
                path,
                FileOutcome::Failed,
                Some(why.clone()),
            );
        }

        for (seen, path) in paths.iter().enumerate() {
            // Cloned per file so `paths` outlives the loop and the remainder
            // can be handed to the next slice. One `PathBuf` clone against
            // opening and hashing the file it names.
            let path = path.clone();
            // Only ever after something was embedded: a budget too small for
            // any work at all must still make progress, or calling again is
            // the same call forever.
            if let Some(deadline) = deadline
                && report.indexed > 0
                && std::time::Instant::now() >= deadline
            {
                report.remaining = total - seen;
                report.pending = paths[seen..].to_vec();
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
                    report.pending = paths[seen..].to_vec();
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
            // Named, not just detected. Every one of these used to be the same
            // silent `skipped`, and a person looking at two thousand of them
            // could not tell an empty `__init__.py` from a file the operating
            // system would not open.
            let unusable = match (&opened, &measured) {
                (Err(e), _) => Some(SkipReason::Unreadable(e.to_string())),
                (_, None) => Some(SkipReason::Unreadable(
                    "its metadata could not be read".to_string(),
                )),
                (_, Some(m)) if !m.is_file() => Some(SkipReason::NotRegular),
                (_, Some(m)) if m.len() == 0 => Some(SkipReason::Empty),
                (_, Some(m)) if m.len() > chunk::MAX_FILE_BYTES => Some(SkipReason::TooLarge),
                _ => None,
            };
            if let Some(why) = unusable {
                // A batch of events can name a file that has just been
                // deleted or renamed away. Evicting it here is what makes
                // a deletion visible without a full sweep.
                if !path.exists() {
                    let ids = store::delete_file(&self.db, &key, now())?;
                    if !ids.is_empty() {
                        for id in ids {
                            self.index.remove(id)?;
                        }
                        report.removed += 1;
                        say_file(
                            &mut on_file,
                            &report,
                            total,
                            &path,
                            FileOutcome::Removed,
                            None,
                        );
                        continue;
                    }
                }
                skip(&mut report, &why);
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Skipped,
                    Some(why.as_str()),
                );
                continue;
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
                // Grew past the cap between the measure and the read, or the
                // read itself failed. Two different answers, and the person
                // chasing the file needs to know which.
                Ok(_) => {
                    let why = SkipReason::TooLarge;
                    skip(&mut report, &why);
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Skipped,
                        Some(why.as_str()),
                    );
                    continue;
                }
                Err(e) => {
                    let why = SkipReason::Unreadable(e.to_string());
                    skip(&mut report, &why);
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Skipped,
                        Some(why.as_str()),
                    );
                    continue;
                }
            };

            let hash = blake3::hash(&bytes).to_hex().to_string();
            if !rechunk && store::file_hash(&self.db, &key)?.as_deref() == Some(hash.as_str()) {
                report.unchanged += 1;
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Unchanged,
                    None,
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
                    report
                        .refused
                        .push((path.display().to_string(), why.clone()));
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Refused,
                        Some(why),
                    );
                    continue;
                }
                let Some((width, height)) = image::dimensions(&bytes) else {
                    let why = SkipReason::NotDecodableImage;
                    skip(&mut report, &why);
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Skipped,
                        Some(why.as_str()),
                    );
                    continue;
                };
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Indexing,
                    None,
                );
                // The one call in the image path that can fail on this file's
                // own bytes — a header the dimension reader accepted and the
                // decoder did not. Caught here, before a single row is
                // written, so the file leaves the store exactly as it found
                // it and the next file is embedded. Everything after this line
                // is the store's, and a failure there is the run's.
                let vector = match self.clip.embed_image(&path, self.quiet) {
                    Ok(vector) => vector,
                    Err(e) => {
                        failed(&mut report, &path, &e);
                        say_file(
                            &mut on_file,
                            &report,
                            total,
                            &path,
                            FileOutcome::Failed,
                            Some(format!("{e:#}")),
                        );
                        continue;
                    }
                };
                // Replacing an image: its old vector goes before the new one
                // arrives, and the row goes with the file's cascade.
                for id in store::image_ids_of(&self.db, &key)? {
                    self.images.remove(id as u64)?;
                }
                for id in store::delete_file(&self.db, &key, now())? {
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

            // This branch reported nothing at all before 0.19.0 — no event,
            // no reason, no counter movement — so a tree of binaries left the
            // portal's progress bar short of its own total with no line
            // saying why.
            let text = match chunk::extract(&path, &bytes) {
                Ok(text) => text,
                Err(why) => {
                    skip(&mut report, &why);
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Skipped,
                        Some(why.as_str()),
                    );
                    continue;
                }
            };
            // Before a single chunk, a single row or a single vector. The
            // scan is the one rule that cannot be decided from a file's name,
            // so it is decided from the text a reader produced — which is also
            // what catches an AWS key sitting in the body of a `.docx`. Images
            // never reach this line; they were never text.
            match filter::scan_text(&text) {
                Some(found) if self.boundary.allow_secrets => {
                    // Counted, not hidden. `--include-secrets` is the user
                    // saying they meant it, not semlith agreeing it is fine.
                    let _ = found;
                    report.secrets_indexed += 1;
                }
                Some(found) => {
                    let why = found.reason();
                    // A file that held no credential when it was indexed and
                    // holds one now leaves the store on this run.
                    let (gone, images) = self.evict(&key)?;
                    let evicted = gone + images;
                    report.removed += usize::from(evicted > 0);
                    let why = if evicted > 0 {
                        format!("{why}. Its earlier contents have been removed from this store.")
                    } else {
                        why
                    };
                    report
                        .refused
                        .push((path.display().to_string(), why.clone()));
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Refused,
                        Some(why),
                    );
                    continue;
                }
                None => {}
            }

            // Parsed before a single row is written, which is the whole
            // reason this call sits above the inserts: tree-sitter failing on
            // this file's own bytes is the text path's one file-shaped
            // failure, and catching it before the rows exist means there is
            // nothing to undo and the shared pending batch is never left
            // holding an id whose row was rolled back.
            //
            let extraction = match graph::extract(&path, &text) {
                Ok(extraction) => extraction,
                Err(e) => {
                    failed(&mut report, &path, &e);
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Failed,
                        Some(format!("{e:#}")),
                    );
                    continue;
                }
            };

            let chunks = chunk::chunk_file(
                &path,
                &text,
                extraction.as_ref().map_or(&[][..], |e| &e.symbols),
            );
            if chunks.is_empty() {
                let why = SkipReason::NoText;
                skip(&mut report, &why);
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Skipped,
                    Some(why.as_str()),
                );
                continue;
            }

            say_file(
                &mut on_file,
                &report,
                total,
                &path,
                FileOutcome::Indexing,
                None,
            );

            // Replacing a file: evict its old vectors before adding new ones.
            for id in store::delete_file(&self.db, &key, now())? {
                self.index.remove(id)?;
            }

            let file_id = store::insert_file(&self.db, &key, PENDING, bytes.len() as u64, now())?;
            // What the parser made of this file, recorded now because it
            // cannot be told afterwards: a file whose parse expired and a file
            // whose language has no grammar both leave no symbols behind, and
            // only one of them is a gap in the graph's coverage.
            let parsed = match (&extraction, graph::language_of(&path)) {
                (Some(_), _) => "parsed",
                (None, Some(lang)) if graph::has_graph(lang) => "timeout",
                (None, _) => "none",
            };
            store::set_file_graph(&self.db, file_id, parsed)?;
            let mut spans: Vec<(u32, u32, i64)> = Vec::with_capacity(chunks.len());
            for (ord, c) in chunks.iter().enumerate() {
                let id =
                    store::insert_chunk(&self.db, file_id, ord, c.start_line, c.end_line, &c.text)?;
                spans.push((c.start_line, c.end_line, id));
                pending.ids.push(id as u64);
                pending.texts.push(c.embedded());

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
            let (symbols, edges) = self.write_graph(extraction, file_id, &spans)?;
            report.symbols += symbols;
            report.edges += edges;

            completed.push((file_id, hash));
            written.push(key.clone());
            report.indexed += 1;
            report.chunks += chunks.len();

            // Between files, never inside one: a file half-written into the
            // index is a file whose hash must not be committed, and this is the
            // one point in the loop where that cannot be true.
            //
            // Counted, not timed. See `CHECKPOINT_FILES`: a checkpoint flushes
            // the pending batch, so a checkpoint that lands somewhere different
            // on every run splits the batches differently and produces
            // different vectors from the same corpus.
            since_checkpoint += 1;
            if checkpointing && since_checkpoint >= every {
                // Said before and after, because the work between these two
                // lines is the longest thing a run does without reading a
                // file: the batch is flushed and every shard is rewritten.
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Writing,
                    Some("writing the index to disk".to_string()),
                );
                self.flush(&mut pending)?;
                self.checkpoint(&mut completed)?;
                since_checkpoint = 0;
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Indexing,
                    None,
                );
            }
        }

        self.flush(&mut pending)?;

        // A stopped slice still commits what it embedded. Undoing is the
        // caller's, because one logical run is several slices and a stop has
        // to undo all of them — see `Job::Index` in the daemon.
        report.written = written;

        // Anything recorded but no longer on disk is dead weight — but only
        // once the run has actually reached the end of what it walked. A slice
        // that yielded has not seen the rest of the corpus yet, and sweeping on
        // every slice re-read every recorded path once per slice for an answer
        // that could not change until the walk was done.
        if sweep && report.pending.is_empty() && !report.stopped {
            for key in store::all_paths(&self.db)? {
                if !Path::new(&key).exists() {
                    for id in store::delete_file(&self.db, &key, now())? {
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

        // The chunking rule this store is now on, recorded only when a pass
        // that swept the whole store ran to the end. A slice that yielded, a
        // run that was stopped, or an index of one directory leaves files
        // behind on the old rule, and a meta row saying otherwise would be a
        // claim about files this run never opened.
        if rechunk && sweep && report.pending.is_empty() && !report.stopped {
            store::set_meta(
                &self.db,
                store::FORMAT_KEY,
                &store::FORMAT_VERSION.to_string(),
            )?;
        }

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
    /// resolved back to the chunks those neighbours live in — ranked, from
    /// 0.16.0, by a personalised PageRank seeded with each hit's own fusion
    /// contribution rather than by one flat hop. 0.15.0 left this as "a real
    /// idea and a change to be justified by a recall measurement"; the
    /// measurement is `tests/retrieval.rs`, and the marginal contribution of
    /// this list is one of the numbers it prints.
    ///
    /// Everything here is gated by the same `Filter` the other two halves use,
    /// through the same `symbols_by_names` predicate — so the one-id-set
    /// invariant `filter.rs` documents holds across all three lists rather than
    /// two. A chunk outside the filter cannot arrive through the graph.
    /// What a chunk reached through one edge is worth in the fusion.
    ///
    /// The third list is evidence about the code, not about the query, and how
    /// good the evidence is varies: an edge the syntax tree resolved says this
    /// chunk really is related, and an edge matched by bare name says it might
    /// be. Ranking both at 1.0, as 0.14.0 did, spends the same confidence on
    /// both claims.
    fn expansion_weight(confidence: &str) -> f32 {
        match confidence {
            graph::EXTRACTED | graph::RESOLVED => 1.0,
            _ => INFERRED_EXPANSION,
        }
    }

    /// Reorder the vector list by the vectors themselves, where the store kept
    /// them.
    ///
    /// The index ranks by 4-bit codes, which is what makes it fast and small
    /// and is also the only reason its ordering is ever wrong: two chunks whose
    /// codes are indistinguishable are separated by the vectors they were
    /// quantized from. This reads back only the candidates the index just
    /// returned -- at `depth`, a few dozen of them -- and reorders those.
    ///
    /// It never adds a candidate and never removes one, so recall is untouched
    /// and only the order inside the list can change. What reaches fusion is a
    /// rank, so a candidate the codes placed eighth and the vectors place first
    /// arrives with the weight of a first place.
    ///
    /// A store written before 0.23.0 has no sidecar and is returned unchanged,
    /// as is any candidate the sidecar does not hold. Rescoring some of a list
    /// and not the rest would order two candidates by two different scales, so
    /// it is all of them or none.
    fn rescored(&self, query: &[f32], scores: Vec<f32>, ids: Vec<u64>) -> (Vec<f32>, Vec<u64>) {
        if ids.len() < 2 || !self.exact.exists() {
            return (scores, ids);
        }
        let mut exact: Vec<(u64, f32)> = Vec::with_capacity(ids.len());
        for id in &ids {
            match self.exact.get(*id) {
                Some(vector) => exact.push((*id, index::cosine(query, &vector))),
                None => return (scores, ids),
            }
        }
        exact.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (rescored_ids, rescored_scores): (Vec<u64>, Vec<f32>) = exact.into_iter().unzip();
        (rescored_scores, rescored_ids)
    }

    fn graph_expansion(
        &self,
        dense: &[u64],
        keyword: &[u64],
        depth: usize,
        filter: &Filter,
    ) -> Result<Vec<Reached>> {
        // Seeded from the best of each list rather than all of it. Expanding
        // from a chunk ranked fortieth is expansion from noise.
        const SEEDS: usize = 8;

        // The seed mass is the fusion contribution each chunk is about to
        // carry, so the walk starts out already knowing which hits the query
        // answered best. A chunk both lists found seeds twice as hard as one
        // only a single list found, which is the same judgement the fusion
        // makes a few lines later.
        // Ordered, not hashed. The walk this seeds sums `f32` masses, and a
        // random iteration order made those sums differ between runs of one
        // binary over one store — issue #88. `graph::expand` says the rest.
        let mut mass: std::collections::BTreeMap<u64, f32> = std::collections::BTreeMap::new();
        for list in [dense, keyword] {
            for (rank, id) in list.iter().take(SEEDS).enumerate() {
                *mass.entry(*id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
            }
        }
        if mass.is_empty() {
            return Ok(Vec::new());
        }

        // Per chunk rather than in one query, because which symbol carries
        // which chunk's mass is the whole point of a personalised walk. A
        // chunk holding three symbols splits its mass between them rather than
        // seeding each of them as though it were a hit of its own.
        let mut personal: std::collections::BTreeMap<String, f32> =
            std::collections::BTreeMap::new();
        for (id, mass) in &mass {
            let names = store::symbols_in_chunks(&self.db, &[*id])?;
            if names.is_empty() {
                continue;
            }
            let share = mass / names.len() as f32;
            for name in names {
                *personal.entry(name).or_default() += share;
            }
        }
        if personal.is_empty() {
            return Ok(Vec::new());
        }

        // Only the kinds that mean "depends on". `defines` and `contains` say
        // a symbol sits inside a file or another symbol, which is true and
        // useless here: following them pulls in every symbol that shares a
        // file with a hit, so the third list fills with neighbours-by-accident
        // and the two lists that answered the question get diluted.
        let kinds = graph::dependency_kinds();
        let ranked = graph::expand(&personal, |name| {
            let mut out = Vec::new();
            for end in store::edges_out(&self.db, name, &kinds)?
                .into_iter()
                .chain(store::edges_in(&self.db, name, &kinds)?)
            {
                // An ambiguous name is several unrelated definitions wearing
                // one label. Expanding through it returns whichever chunk the
                // join reached first, as a hit that claims the code says it is
                // related — which is the same wrong answer the path finder
                // used to give, in the search results instead.
                if end.confidence == graph::AMBIGUOUS {
                    continue;
                }
                // Expansion answers "what else does the code say is related to
                // this". A prose or configuration file reached through the
                // graph is not that: 0.17.0 gave every language symbols, and
                // the third list filled with headings, keys and selectors that
                // the harness scored at zero satisfied spans out of nineteen
                // reached. They stay symbols, and `neighbors`, `path` and the
                // blast radius still walk them — a Dockerfile stage or a table
                // a view reads is exactly the edge those answer. It is the
                // search's third list they do not belong in.
                if !crate::filter::is_code(&end.symbol.path) {
                    continue;
                }
                let weight = Self::expansion_weight(&end.confidence);
                out.push((end.symbol.name, weight, end.confidence));
            }
            Ok(out)
        })?;

        let names: Vec<String> = ranked.iter().map(|(name, _, _)| name.clone()).collect();
        let place: std::collections::HashMap<&str, usize> = names
            .iter()
            .enumerate()
            .map(|(i, name)| (name.as_str(), i))
            .collect();

        // The store answers in its own order; the walk's order is the answer,
        // so the rows are put back into it before the budget is applied.
        let best = ranked.first().map(|(_, mass, _)| *mass);
        let mut symbols = store::symbols_by_names(&self.db, &names, filter.groups())?;
        symbols.sort_by_key(|s| place.get(s.name.as_str()).copied().unwrap_or(usize::MAX));

        let mut ids: Vec<Reached> = Vec::new();
        for symbol in symbols {
            let Some(chunk_id) = symbol.chunk_id else {
                continue;
            };
            let id = chunk_id as u64;
            let found = ranked.iter().find(|(name, _, _)| *name == symbol.name);
            let tier = found
                .map(|(_, _, tier)| tier.clone())
                .unwrap_or_else(|| graph::INFERRED.to_string());
            let weight = Self::expansion_weight(&tier);
            // As a share of the best score the walk produced, so the number
            // means the same thing whatever the absolute masses came out at.
            let proximity = match (found, best) {
                (Some((_, mass, _)), Some(best)) if best > 0.0 => mass / best,
                _ => 0.0,
            };
            // A chunk the other two lists already ranked gains nothing from
            // being re-ranked here; the fusion adds the contribution anyway.
            if !ids.iter().any(|r| r.id == id) {
                ids.push(Reached {
                    id,
                    weight,
                    tier,
                    proximity,
                });
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
    /// Write an already-parsed extraction into the store.
    ///
    /// Split from the parse in 0.19.0. The parse is the half that can fail on
    /// a file's own bytes and it now runs before any row is inserted; what is
    /// left here touches only the database, so a failure in it is the run's
    /// and not the file's.
    fn write_graph(
        &self,
        extraction: Option<graph::Extraction>,
        file_id: i64,
        spans: &[(u32, u32, i64)],
    ) -> Result<(usize, usize)> {
        let Some(extraction) = extraction else {
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
            store::insert_edge(
                &self.db,
                *src,
                &edge.to,
                &edge.kind,
                &edge.confidence,
                edge.hint.as_deref(),
                edge.line,
            )?;
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
        // The sidecar is written in the same breath as the codes, from the same
        // values, so the two cannot describe different vectors. A failure to
        // write it fails the index pass rather than leaving a store whose
        // rescoring silently reorders by a stale vector.
        self.exact.append(&flat, &ids)?;
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
        let (chunks, images) = self.evict(&key)?;
        if chunks > 0 || images > 0 {
            self.save()?;
        }
        Ok((chunks, images))
    }

    /// Take one file's rows and vectors out of the store, without saving.
    ///
    /// The save is the caller's, because an index run evicts many files and
    /// writing the whole index out after each one would make a re-index of a
    /// tree that gained a `.env` quadratic. `forget` saves straight away
    /// because it is the whole of what it was asked to do.
    fn evict(&mut self, key: &str) -> Result<(usize, usize)> {
        // Read before the delete: the cascade that removes the rows is what
        // makes their ids unreadable, and the vectors they address still have
        // to leave the image index.
        let images = store::image_ids_of(&self.db, key)?;
        let ids = store::delete_file(&self.db, key, now())?;
        for id in &ids {
            self.index.remove(*id)?;
        }
        for id in &images {
            self.images.remove(*id as u64)?;
        }
        Ok((ids.len(), images.len()))
    }

    /// Every file this store holds that sits under none of `roots`.
    ///
    /// A store is about its roots. Rows for anything else got in through a
    /// mis-scoped run under the old boundary rule, which treated the whole home
    /// directory as fair game for every store, and they are wrong twice over:
    /// the search results carry the wrong store label, and a file that two
    /// stores hold competes with itself for the same `k` slots.
    ///
    /// Returns the paths in the store's own spelling, which is what `evict`
    /// takes.
    pub fn out_of_root(&self, roots: &[PathBuf]) -> Result<Vec<String>> {
        // With nothing to be outside of, nothing is. A store whose roots the
        // registry does not record — a bare `--store` directory — is not one
        // this can reason about, and dropping its whole contents on the
        // strength of an empty list is the one outcome worth ruling out.
        if roots.is_empty() {
            return Ok(Vec::new());
        }
        // Through the boundary rule rather than a second copy of it. A path
        // and a root have to be compared in one shape — see
        // `filter::within_boundary` — and a store that dropped rows by a rule
        // slightly different from the one that refuses writes would hold
        // exactly the files the daemon then declined to re-index.
        Ok(store::all_paths(&self.db)?
            .into_iter()
            .filter(|key| !filter::within_boundary(Path::new(key), roots))
            .collect())
    }

    /// Drop every row this store holds for a file outside its roots.
    ///
    /// The count is what the daemon logs and what the Stores page shows, so a
    /// store that was contaminated says so rather than quietly correcting
    /// itself. The files on disk are untouched; the store that owns them still
    /// holds them.
    pub fn prune_out_of_root(&mut self, roots: &[PathBuf]) -> Result<usize> {
        let strays = self.out_of_root(roots)?;
        if strays.is_empty() {
            return Ok(0);
        }
        self.writing(|me| {
            for key in &strays {
                me.evict(key)?;
            }
            me.index.save()?;
            Ok(())
        })?;
        Ok(strays.len())
    }

    /// Every file this store holds that today's rules would refuse.
    ///
    /// The path for a store indexed before a rule widened. Both halves of the
    /// decision run here — the deny-list against the name, and the content
    /// table against the text the store is actually holding — so a finding is
    /// exactly what an index run today would refuse. The CLI's `semlith scan`
    /// and the portal's Privacy page both call this one function; two
    /// implementations of "what should not be here" would eventually disagree,
    /// and the one that mattered would be whichever the user did not run.
    pub fn scan(&self) -> Result<Vec<Finding>> {
        // Refused outright rather than answered. The directory rules fail
        // closed, so with no home every file in the store comes back as
        // `NoHome` — and this is the one function with a `--forget` behind it,
        // which would then evict the whole store on the strength of a missing
        // environment variable.
        let home = match crate::home::user_home() {
            Ok(home) => canonical(&home),
            Err(e) => bail!(
                "semlith cannot tell where your home directory is, so it cannot say which \
                 of this store's files sit under a credential directory: {e}. Set \
                 SEMLITH_HOME, or HOME, and scan again."
            ),
        };
        let home = Some(home);
        let mut out = Vec::new();
        for key in store::all_paths(&self.db)? {
            let path = Path::new(&key);
            // Walked, because everything in a store got there through a walk
            // or through a caller who named it and was checked then. Applying
            // the hidden rule here would report every whitelisted dotfile this
            // release deliberately indexes.
            if let Some(why) = filter::denied_against(path, home.as_deref())
                && why != filter::Denied::Hidden
            {
                out.push(Finding {
                    path: key,
                    why: why.reason(),
                });
                continue;
            }
            if let Some(found) = filter::scan_text(&store::text_of(&self.db, &key)?) {
                out.push(Finding {
                    path: key,
                    why: found.reason(),
                });
            }
        }
        Ok(out)
    }

    /// Evict everything [`Semlith::scan`] found, and say how many files went.
    pub fn forget_findings(&mut self, findings: &[Finding]) -> Result<usize> {
        self.writing(|me| {
            let mut gone = 0;
            for finding in findings {
                let (chunks, images) = me.evict(&finding.path)?;
                if chunks > 0 || images > 0 {
                    gone += 1;
                }
            }
            if gone > 0 {
                me.save()?;
            }
            Ok(gone)
        })
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
        self.search_preferring(query, vector, k, filter, Prefer::default())
    }

    /// [`Semlith::search_ranked`] with the caller's preference applied.
    ///
    /// The query's shape is read here rather than passed in, because it is a
    /// function of the query text and nothing else — every surface that wants
    /// to print it calls [`shape_of`] on the same string and gets the same
    /// answer, with no second source of truth to drift.
    pub fn search_preferring(
        &mut self,
        query: &str,
        vector: &[f32],
        k: usize,
        filter: &Filter,
        prefer: Prefer,
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

        // Which half of the fusion this query's own text says to trust.
        let shape = shape_of(query);

        let allowlist = self.allowlist(filter)?;
        if matches!(allowlist, Allowlist::Empty) {
            return Ok(Vec::new());
        }

        // The chunks that *define* the thing the query named, when the query
        // named a thing.
        //
        // An agent that already knows a term is the one case where retrieval
        // has no excuse, and it was the largest measured miss class on the
        // pinned corpus: `DEPENDENCY_KINDS` and `SEMLITH_IMAGE_FLOOR` ranked
        // first in FTS5 alone and still missed at k=8. Fusion is why. A
        // contribution of `weight / (60 + rank)` is nearly flat across the
        // first ranks, so one authoritative list placing a chunk first adds
        // 2/61 while two vague lists placing a chunk fourth and fifth add
        // 1/65 + 1/66 — and a third list derived from those two adds again.
        // Rank is the thing that carries the information and the formula
        // spends it.
        //
        // So this is precedence rather than a weight. The `symbols` table
        // already knows which chunk holds which definition, and
        // `symbols_by_names` already applies the same filter the other lists
        // see, so there is no new query shape and no way for this list to
        // return a chunk the filter excluded. It is read only for an
        // identifier-shaped query: a sentence naming a symbol in passing is
        // not a request for that symbol's definition, which is what `prefer`
        // and the fusion are for.
        let definitions: Vec<u64> = if shape == Shape::Identifier {
            let name = query.trim().to_string();
            let mut ids = Vec::new();
            for row in
                store::symbols_by_names(&self.db, std::slice::from_ref(&name), filter.groups())?
            {
                if let Some(chunk) = row.chunk_id {
                    let id = chunk as u64;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
                // Capped, because the lift is a promise about the first few
                // results and not a licence to fill them. A name like `record`
                // or `resolve` has several definitions in this corpus alone,
                // and a name like `new` has dozens; lifting all of them would
                // answer "where is this defined" and bury every call site,
                // which is the other half of what an identifier query is
                // usually for. Three is enough to say "here it is, and it is
                // ambiguous"; past that the answer is `semlith symbol`, which
                // is the tool for exactly that question.
                if ids.len() >= DEFINITION_LIFT {
                    break;
                }
            }
            ids
        } else {
            Vec::new()
        };

        let (dense_scores, dense_ids) = self.index.search(vector, depth, &allowlist)?;
        // The graph list seeds from what the *index* found, in the order the
        // index found it, which is not the order rescoring leaves behind.
        //
        // Rescoring reorders the vector list by the vectors themselves, and
        // `graph_expansion` below takes its seeds from that same list. So
        // feeding it the rescored order silently changed which symbols the
        // third list expanded from, and that cost two questions at hit@8 on the
        // sealed thirty -- 25 of 30 against 27 -- while gaining nothing at any
        // depth. Measured by turning the pass off and on with everything else
        // held still.
        //
        // The ordering a caller sees is the rescored one; the seeds stay the
        // index's. One list's improvement has no business changing another
        // list's input.
        let seeds = dense_ids.clone();
        let (dense_scores, dense_ids) = self.rescored(vector, dense_scores, dense_ids);
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
        let graph_ids = self.graph_expansion(&seeds, &keyword_ids, depth, filter)?;

        // Two id spaces — a chunk id and an image id both count from one — so
        // the fusion is keyed by which space an id belongs to as well as by the
        // id. Everything else about reciprocal-rank fusion is unchanged: an
        // image is a fourth list, ranked against the other three rather than
        // appended after them.
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

        let candidate_lists = [
            // Weightless on purpose. A definition does not out-*score* the
            // other lists, it is lifted past them below, and a score here
            // would be a second mechanism doing the same job badly: tuned
            // high it would drag a definition's neighbours up with it through
            // the graph list, tuned low it would do nothing. What it is here
            // for is to make sure the chunk exists in the fused set at all,
            // and to carry its badge, so a caller can see which hits arrived
            // this way.
            (
                "definition",
                false,
                definitions.iter().map(|id| (*id, 0.0)).collect(),
            ),
            ("image", true, image_list),
            (
                "vector",
                false,
                dense_ids.iter().map(|id| (*id, 1.0)).collect(),
            ),
            (
                "keyword",
                false,
                keyword_ids
                    .iter()
                    .map(|id| (*id, shape.keyword_weight()))
                    .collect(),
            ),
            (
                "graph",
                false,
                graph_ids.iter().map(|r| (r.id, r.weight)).collect(),
            ),
        ];

        let (mut fused, lists) = fuse(&candidate_lists, |name| shape.list_constant(name));

        // The lift, written as a score rather than as a second sort.
        //
        // A stable sort that moved the definitions to the front would order
        // this store's answer correctly and lose that order the moment the
        // answer left it: `fleet::merge` takes the best `k` across stores by
        // fused score, so a definition lifted to rank 1 here on a score of 0.0
        // would merge behind every scored hit of every other store. Putting the
        // lift in the score is the same order in one store and the right order
        // in several, and `merge` needs to know nothing about it.
        //
        // Above the maximum rather than at a fixed constant, because the
        // maximum is a sum of rank reciprocals and no constant is reliably
        // above it. Descending within the definitions, so several definitions
        // of one name arrive in the order `symbols_by_names` gave them — by
        // path, then by line, which is the same on every run.
        if !definitions.is_empty() {
            let top = fused.iter().map(|(_, score)| *score).fold(0.0f32, f32::max);
            for (at, id) in definitions.iter().enumerate() {
                if let Some(slot) = fused.iter().position(|(key, _)| *key == (false, *id)) {
                    fused[slot].1 = top + (definitions.len() - at) as f32;
                }
            }
        }

        // Sorted together with their provenance, so a hit never carries the
        // badges of whichever chunk happened to land in its slot.
        let mut ranked: Vec<Candidate> = fused.into_iter().zip(lists).collect();
        // Total, and this is the last of issue #88. Fusion scores are
        // `1 / (60 + rank)`, so exact ties are common by construction — two
        // chunks that placed at the same rank in the same list carry the same
        // f32 to the bit. A sort on score alone leaves those in whatever order
        // the vector index handed them over, and a store built twice from one
        // corpus hands near-equal vectors over in a different order, so the
        // same query returned the same eight chunks in two orders. Breaking on
        // the id makes the answer a function of the corpus rather than of the
        // shard scan.
        ranked.sort_by(|a, b| b.0.1.total_cmp(&a.0.1).then_with(|| a.0.0.cmp(&b.0.0)));
        // Cut to the deeper list, not to `k`. Everything that reorders the
        // answer below — the preference, and the rerank — needs each
        // candidate's path and enclosing symbol, which only exist once the
        // rows are fetched, and a candidate cut here can never be lifted.
        ranked.truncate(depth.max(k));

        let mut hits = Vec::with_capacity(ranked.len());
        // Kept beside the hits rather than on them: how near the graph walk
        // put a chunk is an input to the ranking, not something a caller of
        // `search` has any use for. Index-aligned with `hits`, which nothing
        // between here and the rerank reorders.
        let mut proximity: Vec<f32> = Vec::with_capacity(ranked.len());
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
                            // Filled in below, once every path in the answer
                            // is known and each can be stat'd once.
                            fresh: true,
                            symbol: None,
                            symbol_kind: None,
                            provenance: None,
                        },
                        0.0,
                    ));
                    proximity.push(0.0);
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
                        fresh: true,
                        symbol: None,
                        symbol_kind: None,
                        provenance: provenance_of(&graph_ids, id),
                    },
                    similarity,
                ));
                proximity.push(
                    graph_ids
                        .iter()
                        .find(|r| r.id == id)
                        .map(|r| r.proximity)
                        .unwrap_or(0.0),
                );
            }
        }
        self.mark_freshness(&mut hits)?;
        self.name_enclosing_symbols(&mut hits)?;

        // The rescoring stage: a cross-encoder reads the query and the
        // candidate together, which nothing before this point has done.
        //
        // Only for a question. An identifier query is answered by its
        // definitions, lifted above the fused maximum a few lines above, and a
        // model asked to judge `RRF_K` against a paragraph about it has no
        // more to go on than the keyword list already had. This is the routed
        // cascade: each shape gets the stage that suits it.
        //
        // The scores are permuted rather than replaced. Each candidate in the
        // head keeps one of the head's own fused scores and the cross-encoder
        // decides which, so the tiebreaks below — proximity, staleness, the
        // caller's preference — still apply to a fused-scale number, and a
        // hit outside the head is never reordered against one inside it.
        if shape != Shape::Identifier && rerank::enabled() && hits.len() > 1 {
            let head = hits.len().min(rerank::RERANK_DEPTH);
            let texts: Vec<String> = hits[..head]
                .iter()
                .map(|(hit, _)| format!("{}\n{}", hit.path, hit.text))
                .collect();
            let quiet = self.quiet;
            if let Some(model) = self.reranker() {
                match rerank::order(model, query, &texts) {
                    Ok(order) if order.len() == head => {
                        let mut scores: Vec<f32> =
                            hits[..head].iter().map(|(hit, _)| hit.score).collect();
                        scores.sort_by(|a, b| b.total_cmp(a));
                        for (rank, at) in order.into_iter().enumerate() {
                            hits[at].0.score = scores[rank];
                            hits[at].0.lists.push("rerank");
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        if !quiet {
                            eprintln!("the rescoring pass failed, keeping the fused order: {e}");
                        }
                    }
                }
            }
        }

        // The rerank, and the preference with it. Both are applied here rather
        // than inside the fusion because both are about things the fusion has
        // no way to know — the file a chunk is in, the definition it sits
        // inside, whether that file has been edited since — and all three are
        // only known once the rows have been fetched.
        //
        // The fused score stays the dominant term. These are tiebreaks: a
        // chunk the query matched badly does not climb over one it matched
        // well because it happens to sit in a function.
        for ((hit, _), proximity) in hits.iter_mut().zip(&proximity) {
            hit.score *= 1.0 + GRAPH_PROXIMITY * proximity;
            if !hit.fresh {
                hit.score *= 1.0 - STALE_PENALTY;
            }
            hit.score *= prefer.multiplier(filter::is_code(&hit.path));
        }
        // Total for the same reason as the fusion sort above: the rerank's
        // factors are multipliers on a fused score, so two candidates that
        // entered tied and took the same multipliers leave tied.
        hits.sort_by(|a, b| {
            b.0.score
                .total_cmp(&a.0.score)
                .then_with(|| a.0.path.cmp(&b.0.path))
                .then_with(|| a.0.start_line.cmp(&b.0.start_line))
        });
        hits.truncate(k);
        Ok(hits)
    }

    /// One span of one file, or the definitions to choose between.
    ///
    /// Answers only from the store's own chunks. Reading the file off disk
    /// would answer for content semlith never indexed and was never allowed to
    /// look at — the boundary that governs indexing has to govern reading, or
    /// an agent holding the agent key could read `~/.ssh/id_rsa` by naming it
    /// as a span.
    pub fn read(&self, target: &Target, filter: &Filter) -> Result<Option<Read>> {
        let (path, start, end) = match target {
            Target::Span { path, start, end } => {
                // A locate answer prints a store-relative path, so that is
                // what comes back in. Two indexed files can end with the same
                // suffix, and choosing one of them would be a guess.
                let candidates = store::files_ending_with(&self.db, path, 8)?;
                match candidates.len() {
                    0 => return Ok(None),
                    1 => (candidates[0].clone(), *start, *end),
                    _ => anyhow::bail!(
                        "{path:?} matches {} indexed files: {}. Name more of the path.",
                        candidates.len(),
                        candidates.join(", ")
                    ),
                }
            }
            Target::Symbol(name) => {
                let found = store::symbols_named(&self.db, name, 200)?;
                match found.len() {
                    0 => return Ok(None),
                    1 => {
                        let only = &found[0];
                        (only.path.clone(), only.start_line, only.end_line)
                    }
                    // Several definitions and nothing to choose between them.
                    // The list is the answer.
                    _ => return Ok(Some(Read::Choose(found))),
                }
            }
        };

        // The same eligibility the other surfaces use, through the same
        // predicate, so a chunk a filter excludes cannot be read either.
        let allowed = if filter.is_empty() {
            None
        } else {
            Some(store::filtered_chunk_ids(&self.db, filter.groups())?)
        };
        let chunks: Vec<store::ChunkRow> = store::chunks_overlapping(&self.db, &path, start, end)?
            .into_iter()
            .filter(|c| match &allowed {
                Some(ids) => ids.contains(&(c.id as u64)),
                None => true,
            })
            .collect();
        if chunks.is_empty() {
            return Ok(None);
        }

        // Chunks overlap by two lines, so they are stitched by line number
        // rather than concatenated — concatenation would repeat the seam and
        // the caller would read two copies of the same line as two lines.
        let mut lines: Vec<(u32, String)> = Vec::new();
        for chunk in &chunks {
            for (offset, line) in chunk.text.lines().enumerate() {
                let number = chunk.start_line + offset as u32;
                if lines.iter().any(|(n, _)| *n == number) {
                    continue;
                }
                lines.push((number, line.to_string()));
            }
        }
        lines.sort_by_key(|(number, _)| *number);
        lines.retain(|(number, _)| *number >= start && *number <= end);
        if lines.is_empty() {
            return Ok(None);
        }

        let first = lines.first().map(|(n, _)| *n).unwrap_or(start);
        let last = lines.last().map(|(n, _)| *n).unwrap_or(end);
        let text = lines
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n");

        let mut span = Span {
            path,
            start_line: first,
            end_line: last,
            text,
            symbol: None,
            symbol_kind: None,
            fresh: true,
            store: None,
        };
        self.name_enclosing_symbol(&mut span)?;
        self.mark_span_freshness(&mut span)?;
        Ok(Some(Read::One(span)))
    }

    /// The innermost definition a span sits inside, if any.
    fn name_enclosing_symbol(&self, span: &mut Span) -> Result<()> {
        let by_file = store::symbols_in_files(&self.db, std::slice::from_ref(&span.path))?;
        let Some(symbols) = by_file.get(&span.path) else {
            return Ok(());
        };
        // The same containment-then-overlap rule search uses, for the same
        // reason: a span that begins in a function's doc comment is that
        // function's.
        let contains = symbols
            .iter()
            .filter(|(start, end, _, _)| *start <= span.start_line && *end >= span.start_line)
            .min_by_key(|(start, end, _, _)| end.saturating_sub(*start));
        let best = contains.or_else(|| {
            symbols
                .iter()
                .filter(|(start, end, _, _)| *start <= span.end_line && *end >= span.start_line)
                .min_by_key(|(start, end, _, _)| end.saturating_sub(*start))
        });
        if let Some((_, _, name, kind)) = best {
            span.symbol = Some(name.clone());
            span.symbol_kind = Some(kind.clone());
        }
        Ok(())
    }

    /// Whether the file a span came from still looks the way it did when it
    /// was indexed. The same conservative rule search uses.
    fn mark_span_freshness(&self, span: &mut Span) -> Result<()> {
        let stamps = store::file_stamps(&self.db, std::slice::from_ref(&span.path))?;
        let Some((bytes, indexed_at)) = stamps.get(&span.path) else {
            return Ok(());
        };
        span.fresh = match std::fs::metadata(&span.path) {
            Ok(meta) => {
                meta.len() as i64 == *bytes
                    && meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .is_none_or(|d| d.as_secs() as i64 <= *indexed_at)
            }
            Err(_) => false,
        };
        Ok(())
    }

    /// Give every hit the name of the definition it sits inside.
    ///
    /// One query for the whole answer, and the innermost definition wins: a
    /// method inside a class is more use to a reader than the class.
    fn name_enclosing_symbols(&self, hits: &mut [(Hit, f32)]) -> Result<()> {
        let mut paths: Vec<String> = Vec::new();
        for (hit, _) in hits.iter() {
            if hit.image.is_none() && !paths.contains(&hit.path) {
                paths.push(hit.path.clone());
            }
        }
        let by_file = store::symbols_in_files(&self.db, &paths)?;
        for (hit, _) in hits.iter_mut() {
            let Some(symbols) = by_file.get(&hit.path) else {
                continue;
            };
            // Containment first, overlap second, innermost within each.
            let contains = symbols
                .iter()
                .filter(|(start, end, _, _)| *start <= hit.start_line && *end >= hit.start_line)
                .min_by_key(|(start, end, _, _)| end.saturating_sub(*start));
            let best = contains.or_else(|| {
                symbols
                    .iter()
                    .filter(|(start, end, _, _)| *start <= hit.end_line && *end >= hit.start_line)
                    .min_by_key(|(start, end, _, _)| end.saturating_sub(*start))
            });
            if let Some((_, _, name, kind)) = best {
                hit.symbol = Some(name.clone());
                hit.symbol_kind = Some(kind.clone());
            }
        }
        Ok(())
    }

    /// Say, for every hit, whether its file still looks the way it did when it
    /// was indexed.
    ///
    /// One `stat` per distinct path in the answer, never one per hit: a search
    /// that returns eight chunks of one file asks the filesystem about that
    /// file once.
    fn mark_freshness(&self, hits: &mut [(Hit, f32)]) -> Result<()> {
        if hits.is_empty() {
            return Ok(());
        }
        let paths: Vec<String> = {
            let mut seen: Vec<String> = Vec::new();
            for (hit, _) in hits.iter() {
                if !seen.contains(&hit.path) {
                    seen.push(hit.path.clone());
                }
            }
            seen
        };
        let stamps = store::file_stamps(&self.db, &paths)?;
        let mut fresh: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
        for path in &paths {
            let Some((bytes, indexed_at)) = stamps.get(path) else {
                // The store has no row for this path. It cannot be said to
                // have changed, and claiming it had would be a warning about
                // nothing.
                fresh.insert(path.as_str(), true);
                continue;
            };
            let state = match std::fs::metadata(path) {
                Ok(meta) => {
                    let size_matches = meta.len() as i64 == *bytes;
                    let unmodified = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .is_none_or(|d| d.as_secs() as i64 <= *indexed_at);
                    size_matches && unmodified
                }
                // Gone or unreadable. The excerpt in hand is the only copy
                // there is, and it is certainly not current.
                Err(_) => false,
            };
            fresh.insert(path.as_str(), state);
        }
        for (hit, _) in hits.iter_mut() {
            hit.fresh = fresh.get(hit.path.as_str()).copied().unwrap_or(true);
        }
        Ok(())
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

    /// The f32 sidecar's size, or `None` where the store has none.
    ///
    /// A store written before 0.23.0 has no sidecar and is never rescored until
    /// its next full index pass, which is invisible from the outside -- the
    /// answers are simply the ones the codes gave. Said here rather than left
    /// to be inferred from a ranking that looks slightly worse.
    pub fn exact_bytes(&self) -> Option<u64> {
        self.exact.exists().then(|| self.exact.bytes())
    }

    /// How many definitions this store has retired, over every name.
    ///
    /// Zero on a store that has never re-indexed since it began keeping
    /// history, which from the outside is the same as a store where nothing
    /// ever changed -- so it is printed rather than inferred.
    pub fn retired_symbols(&self) -> Result<i64> {
        store::symbols_past_count(&self.db)
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

/// Serialize a stored path in the form a person reads, leaving the value in
/// memory as the store's own key. One attribute per field beats a call at every
/// place a struct reaches JSON, and the CLI's `--json`, the portal and the MCP
/// results all serialise these same structs.
pub fn serialize_plain<S: serde::Serializer>(path: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&plain(path))
}

/// How many times this process has canonicalised a path.
///
/// Counted because the cost is invisible until it is not: a `canonicalize` is
/// an opened handle on Windows, and 0.18.0 did three per file — one in the
/// walk, one on the home directory and one on the path, the last two on every
/// file for a home that cannot change mid-run. `tests/filter_canonical.rs`
/// asserts the count over a walk of N files stays near N rather than near 3N.
static CANONICAL_CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The reading for [`CANONICAL_CALLS`], for the test that pins the cost.
pub fn canonical_calls() -> u64 {
    CANONICAL_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn canonical(path: &Path) -> PathBuf {
    CANONICAL_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// A path as a person or an agent should read it.
///
/// `std::fs::canonicalize` on Windows returns the verbatim form —
/// `\\?\C:\work\api\src\lock.rs`, or `\\?\UNC\server\share\...` for a network
/// path — which is what the store holds and what the long-path APIs need. It is
/// not what an editor opens, what a shell completes, or what anybody pastes
/// back: every locator semlith printed on Windows was unusable (#74).
///
/// The store keeps the verbatim form, because that is what makes a path longer
/// than 260 characters work. Only the text on its way out is plain, and
/// `semlith read` takes either.
///
/// A dozen lines rather than a crate: this runs on every hit and every row, and
/// the rule is two prefixes.
///
/// Applied where a path is rendered — [`serialize_plain`] for everything that
/// goes out as JSON, `display` for the CLI's text — and never where one is
/// stored or looked up. A stripped path is not the key the store holds, and a
/// row's own path is what the next query uses to fetch its text.
pub fn plain(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{rest}");
    }
    match text.strip_prefix(r"\\?\") {
        // Only a drive-letter path. `\\?\Volume{...}` names a volume with no
        // mount point, and shortening that would make it name nothing.
        Some(rest)
            if rest.len() >= 2
                && rest.as_bytes()[0].is_ascii_alphabetic()
                && rest.as_bytes()[1] == b':' =>
        {
            rest.to_string()
        }
        _ => text.to_string(),
    }
}

/// Overrides [`model_cache_dir`].
pub const MODEL_CACHE_ENV: &str = "SEMLITH_MODEL_CACHE";

/// Where ONNX model weights are cached. Shared across stores — the weights are
/// large and identical for a given model.
///
/// An error rather than `.` when there is no home: 52 MB of weights written
/// into whatever directory the process started in is not a cache, it is litter
/// that the next run does not find either (#73).
pub fn model_cache_dir() -> Result<PathBuf> {
    if let Ok(dir) = std::env::var(MODEL_CACHE_ENV) {
        return Ok(PathBuf::from(dir));
    }
    Ok(home::user_home()?
        .join(".cache")
        .join("semlith")
        .join("models"))
}

/// Which of `roots` can be read, and why the others cannot.
///
/// Asked before a store is opened, because opening one creates it: a typo in a
/// path used to leave an empty store registered under the typo's own name and
/// exit 0, so a script that indexed a moved directory was told it had worked
/// (#76). A directory is probed with `read_dir` rather than `metadata` alone,
/// because a directory whose contents cannot be listed is one that would index
/// as empty.
pub fn check_roots(roots: &[PathBuf]) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let mut readable = Vec::new();
    let mut refused = Vec::new();
    for root in roots {
        let listed = match std::fs::metadata(root) {
            Ok(meta) if meta.is_dir() => std::fs::read_dir(root).map(|_| ()),
            Ok(_) => std::fs::File::open(root).map(|_| ()),
            Err(e) => Err(e),
        };
        match listed {
            Ok(()) => readable.push(root.clone()),
            Err(e) => refused.push((root.clone(), e.to_string())),
        }
    }
    (readable, refused)
}

/// Walk `roots`, honouring `.gitignore` and skipping hidden files. Returns
/// canonical paths so the same file reached two ways is one entry.
/// Which path a walk error is about.
///
/// `ignore` wraps its errors — a path around a depth around an io error — and
/// exposes no accessor for the path, so unwrapping it is the caller's. Without
/// it every unreadable entry would be reported against the root rather than
/// against the directory that could not be read.
fn walk_error_path(e: &ignore::Error) -> Option<PathBuf> {
    match e {
        ignore::Error::WithPath { path, .. } => Some(path.clone()),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            walk_error_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child.clone()),
        _ => None,
    }
}

/// What a walk found, and what it could not read.
///
/// A pair rather than a bare `Vec` since 0.19.0: the entries the walk gave up
/// on are part of what the run has to report, and a function that returns only
/// the successes gives its caller nothing to report them with.
pub(crate) struct Walked {
    pub files: Vec<PathBuf>,
    /// Roots that were files rather than directories, so the caller named them
    /// one by one. They are held to the hidden-file rule; the walked ones are
    /// not. See [`Boundary::refuses`].
    pub named: Vec<PathBuf>,
    /// Paths the walk could not read, each with what the walk said.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Generated or vendored directories the walk stepped over, in the order it
    /// met them.
    ///
    /// Named rather than counted in files: pruning the subtree is the point, so
    /// counting what is inside it would undo the saving to report it. A reader
    /// looking for a file that is not in their store needs the directory's name,
    /// not an integer.
    pub generated: Vec<PathBuf>,
}

/// Directories that are generated or vendored rather than written.
///
/// `.gitignore` is not enough on its own, and the reason is worth stating: it
/// is a list of what is not *committed*, it is absent from a folder somebody
/// downloaded rather than cloned, and where a user keeps `node_modules` in
/// their global gitignore the walk cannot see it at all. A corpus that swallows
/// a dependency tree is the wrong corpus twice over — every one of its files
/// becomes chunks nobody asked about, and every one of its symbols becomes a
/// graph node with no edge into the project, which is what a user sees when the
/// graph of their own repository is a field of unconnected dots.
///
/// # Two kinds of name
///
/// A name in [`ALWAYS`] is never a directory a person wrote. A name in
/// [`GENERATED`] very often is — plenty of projects have a hand-written
/// `build/` or their own `vendor/` — so it is skipped only when the manifest
/// that generates it is sitting beside it. That keeps the table from quietly
/// deleting somebody's source tree because it shares a name with cargo's output.
///
/// `SEMLITH_DEFAULT_IGNORES=0` turns the whole table off, for the person whose
/// corpus really is a vendored tree.
const ALWAYS: &[&str] = &[
    // JavaScript and TypeScript
    "node_modules",
    "bower_components",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".turbo",
    ".parcel-cache",
    ".yarn",
    // Python
    "__pycache__",
    ".venv",
    "site-packages",
    ".tox",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".eggs",
    // JVM, Android
    ".gradle",
    // Swift, Xcode
    "DerivedData",
    ".swiftpm",
    // Dart and Flutter
    ".dart_tool",
    // Elixir
    "_build",
    // Everything
    ".terraform",
    ".serverless",
    ".gradle-cache",
];

/// `(directory, the manifests that make it generated)`.
///
/// Skipped only when one of the manifests is a sibling of the directory, so a
/// `build/` in a project with no build file is somebody's own code and stays.
const GENERATED: &[(&str, &[&str])] = &[
    ("target", &["Cargo.toml"]),
    ("vendor", &["go.mod", "composer.json", "Gemfile"]),
    ("deps", &["mix.exs"]),
    (
        "build",
        &[
            "build.gradle",
            "build.gradle.kts",
            "pom.xml",
            "CMakeLists.txt",
            "meson.build",
            "package.json",
        ],
    ),
    (
        "dist",
        &[
            "package.json",
            "pyproject.toml",
            "setup.py",
            "rollup.config.js",
        ],
    ),
    ("out", &["package.json", "tsconfig.json"]),
    ("bin", &["*.csproj", "*.sln", "*.fsproj"]),
    ("obj", &["*.csproj", "*.sln", "*.fsproj"]),
    ("coverage", &["package.json", "pyproject.toml"]),
    ("Pods", &["Podfile"]),
];

/// Whether this directory is generated output rather than somebody's code.
///
/// Public so `semlith stats` and the index run can say how many files went
/// unindexed for this reason: a file missing from a store should be missing for
/// a reason a user can read, not for one they have to guess.
pub fn is_generated_dir(path: &Path) -> bool {
    if !default_ignores_on(std::env::var("SEMLITH_DEFAULT_IGNORES").ok().as_deref()) {
        return false;
    }
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if ALWAYS.contains(&name) {
        return true;
    }
    let Some(parent) = path.parent() else {
        return false;
    };
    GENERATED
        .iter()
        .filter(|(dir, _)| *dir == name)
        .any(|(_, manifests)| manifests.iter().any(|m| sibling_exists(parent, m)))
}

/// Whether the table applies, given whatever `SEMLITH_DEFAULT_IGNORES` says.
///
/// Split from the environment read so it can be tested without setting a
/// process-wide variable: a test that mutates the environment races every other
/// test in the same binary, which is how a passing suite turns into a flaky one.
pub fn default_ignores_on(value: Option<&str>) -> bool {
    !matches!(value, Some("0") | Some("off") | Some("false"))
}

/// Whether `parent` holds `manifest`, which may be a `*.ext` pattern for the
/// ecosystems that name their project file after the project.
fn sibling_exists(parent: &Path, manifest: &str) -> bool {
    let Some(extension) = manifest.strip_prefix("*.") else {
        return parent.join(manifest).exists();
    };
    std::fs::read_dir(parent)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(extension))
            })
        })
        .unwrap_or(false)
}

fn walk(roots: &[PathBuf]) -> Walked {
    let mut out = Vec::new();
    let mut named = Vec::new();
    let mut unreadable = Vec::new();
    let mut seen = std::collections::HashSet::new();
    // Written from inside `filter_entry`, which the walker may call from
    // several threads even on the single-threaded builder it is handed here,
    // and which must outlive the borrow the builder takes.
    let generated: std::sync::Arc<std::sync::Mutex<Vec<PathBuf>>> = Default::default();

    for root in roots {
        // A root that is a file is a path the caller named, not one a walk
        // found — `semlith index ~/notes/.env` is one argument, and the rules
        // for a named path are the stricter ones.
        if root.is_file() {
            let path = canonical(root);
            if seen.insert(path.clone()) {
                named.push(path);
            }
            continue;
        }
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
            // `.semlith` is the store itself; the rest is generated output
            // that `.gitignore` covers only where somebody wrote one.
            .filter_entry({
                let generated = generated.clone();
                move |e| {
                    if e.file_name() == ".semlith" {
                        return false;
                    }
                    if e.file_type().is_some_and(|t| t.is_dir()) && is_generated_dir(e.path()) {
                        if let Ok(mut seen) = generated.lock() {
                            seen.push(e.path().to_path_buf());
                        }
                        return false;
                    }
                    true
                }
            });

        for result in builder.build() {
            // Unreadable directories should not abort the run, but silently
            // indexing nothing is worse than a noisy line on stderr.
            let entry = match result {
                Ok(e) => e,
                Err(e) => {
                    // Carried out rather than printed. `eprintln!` reaches a
                    // terminal and nothing else: the daemon, the portal and
                    // every agent were told nothing, so an unreadable
                    // directory read as a tree that was simply empty.
                    let at = walk_error_path(&e).unwrap_or_else(|| root.clone());
                    unreadable.push((at, format!("the walk could not read it: {e}")));
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
    // Sorted, and this is issue #88's index-time half. `ignore::Walk` yields
    // entries in whatever order the filesystem hands the directory over, which
    // is not stable between two walks of two byte-identical trees. Indexing in
    // that order assigns `chunks.id` in that order, and the ids are what every
    // exact tie in the fusion, the FTS predicate and the graph walk falls back
    // to — so the same corpus indexed twice produced two rankings, and the
    // retrieval harness reported a different hit@k each time it ran.
    //
    // Sorting makes a store a function of its corpus rather than of the order
    // the filesystem happened to be in, which is worth having on its own: two
    // people indexing the same checkout get the same store.
    out.sort();
    named.sort();
    unreadable.sort();
    let mut generated = generated.lock().map(|g| g.clone()).unwrap_or_default();
    generated.sort();
    generated.dedup();
    Walked {
        files: out,
        named,
        unreadable,
        generated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The graph list adds candidates and never amplifies them.
    ///
    /// The structural defect 0.25.0 exists to hold still: reciprocal-rank
    /// fusion is only justified over independent evidence, and the graph list
    /// is walked out of the other two. A chunk both text lists found must
    /// score exactly what those two lists gave it, whatever the graph says
    /// about it afterwards.
    #[test]
    fn the_graph_list_adds_candidates_and_does_not_amplify_them() {
        let both = 10u64;
        let only_graph = 99u64;
        let lists: [CandidateList; 3] = [
            ("vector", false, vec![(both, 1.0), (11, 1.0)]),
            ("keyword", false, vec![(both, 1.0)]),
            ("graph", false, vec![(both, 1.0), (only_graph, 1.0)]),
        ];
        let text_only: [CandidateList; 2] = [lists[0].clone(), lists[1].clone()];

        let (with_graph, badges) = fuse(&lists, |_| RRF_K);
        let (without_graph, _) = fuse(&text_only, |_| RRF_K);

        let score = |fused: &[Scored], id: u64| {
            fused
                .iter()
                .find(|(key, _)| *key == (false, id))
                .map(|(_, score)| *score)
                .expect("the candidate is in the fused set")
        };
        assert_eq!(
            score(&with_graph, both),
            score(&without_graph, both),
            "a chunk both text lists found was scored a third time by the graph"
        );
        // It still says where it came from, and it still reaches what the
        // other two missed — that half is the graph's whole job.
        let slot = with_graph
            .iter()
            .position(|(key, _)| *key == (false, both))
            .unwrap();
        assert_eq!(badges[slot], vec!["vector", "keyword", "graph"]);
        assert!(
            with_graph
                .iter()
                .any(|(key, _)| *key == (false, only_graph)),
            "the graph list stopped adding candidates of its own"
        );
    }

    /// The rule is crude on purpose, so these are the whole of it.
    #[test]
    fn a_query_is_identifier_shaped_only_when_it_is_one_token_of_identifier() {
        for identifier in [
            "record_retrieval",
            "store::edges_out",
            "Semlith",
            "self.index.search",
            "format!",
            "k8s",
        ] {
            assert_eq!(
                shape_of(identifier),
                Shape::Identifier,
                "{identifier:?} is an identifier"
            );
        }
        for question in [
            "where does a retrieval get written down",
            "record retrieval",
            "how does indexing work?",
            "",
            "   ",
            "42",
        ] {
            assert_eq!(shape_of(question), Shape::Question, "{question:?}");
        }
    }

    /// An identifier leans on FTS5, which is exact about a name; a question
    /// leaves the two halves level, because the embedding is the one that
    /// reads a sentence.
    #[test]
    fn the_shape_decides_what_the_keyword_list_is_worth() {
        assert_eq!(Shape::Identifier.keyword_weight(), 2.0);
        assert_eq!(Shape::Question.keyword_weight(), 1.0);
        assert_eq!(Shape::Identifier.weighting(), "keyword weighted 2×");
        assert_eq!(Shape::Question.weighting(), "vector and keyword equal");
    }

    /// A bias, not a filter: the unpreferred side keeps its score rather than
    /// being cut, so `prefer: code` over a corpus of prose still answers.
    #[test]
    fn prefer_lifts_one_side_and_never_removes_the_other() {
        assert_eq!(Prefer::Any.multiplier(true), 1.0);
        assert_eq!(Prefer::Any.multiplier(false), 1.0);
        assert!(Prefer::Code.multiplier(true) > Prefer::Code.multiplier(false));
        assert!(Prefer::Docs.multiplier(false) > Prefer::Docs.multiplier(true));
        assert!(Prefer::Code.multiplier(false) > 0.0);
        assert!(Prefer::Docs.multiplier(true) > 0.0);
    }

    /// A misspelled preference is told, not silently ignored: an agent that
    /// passed `prefer: source` would otherwise read an unbiased answer as a
    /// biased one.
    #[test]
    fn an_unknown_preference_is_an_error() {
        assert_eq!(Prefer::parse("code").unwrap(), Prefer::Code);
        assert_eq!(Prefer::parse("DOCS").unwrap(), Prefer::Docs);
        assert_eq!(Prefer::parse("").unwrap(), Prefer::Any);
        assert!(Prefer::parse("source").is_err());
    }

    /// The rerank is a tiebreak, not a second ranking. Every factor has to be
    /// small enough that a chunk the query matched badly cannot climb over one
    /// it matched well, and large enough to separate two that matched equally.
    #[test]
    fn every_rerank_factor_is_a_tiebreak_rather_than_a_ranking() {
        let strongest = 1.0 + GRAPH_PROXIMITY;
        let weakest = 1.0 - STALE_PENALTY;
        assert!(
            strongest / weakest < 1.5,
            "the whole rerank spans {strongest}/{weakest}, which is a ranking rather than a \
             tiebreak"
        );
        const { assert!(GRAPH_PROXIMITY > 0.0 && STALE_PENALTY > 0.0) };
        // A stale hit is pushed down, never removed: the excerpt in hand may
        // still be the best answer there is.
        const { assert!(STALE_PENALTY < 1.0) };
    }

    /// A span and a name are told apart by shape, not by trying one and
    /// falling back — a caller mistyping a path must not silently get a
    /// symbol lookup for it.
    #[test]
    fn a_read_target_is_a_span_or_a_name_by_its_shape() {
        assert_eq!(
            Target::parse("src/store.rs:1041-1080"),
            Target::Span {
                path: "src/store.rs".to_string(),
                start: 1041,
                end: 1080
            }
        );
        // One line is a span of one.
        assert_eq!(
            Target::parse("src/store.rs:12"),
            Target::Span {
                path: "src/store.rs".to_string(),
                start: 12,
                end: 12
            }
        );
        // Backwards is still a span, in the order the file has.
        assert_eq!(
            Target::parse("a.rs:80-40"),
            Target::Span {
                path: "a.rs".to_string(),
                start: 40,
                end: 80
            }
        );
        for name in [
            "record_retrieval",
            "store::edges_out",
            "src/store.rs",
            "a.rs:notanumber",
        ] {
            assert!(
                matches!(Target::parse(name), Target::Symbol(_)),
                "{name:?} is a name"
            );
        }
    }

    /// Chunking does not respect syntax: the chunk holding a function's
    /// opening almost always starts a few lines above it, in the doc comment.
    /// That is the most useful chunk in the function to label, and matching on
    /// the hit's first line alone left exactly those unlabelled.
    #[test]
    fn a_hit_that_straddles_a_definitions_start_is_labelled_with_it() {
        // (start, end, name, kind), as `symbols_in_files` returns them.
        let symbols = [
            (100u32, 180u32, "outer".to_string(), "function".to_string()),
            (120u32, 140u32, "inner".to_string(), "function".to_string()),
        ];
        // A chunk from the doc comment above `inner` into its body.
        let pick = |start: u32, end: u32| {
            let contains = symbols
                .iter()
                .filter(|(s, e, _, _)| *s <= start && *e >= start)
                .min_by_key(|(s, e, _, _)| e.saturating_sub(*s));
            contains
                .or_else(|| {
                    symbols
                        .iter()
                        .filter(|(s, e, _, _)| *s <= end && *e >= start)
                        .min_by_key(|(s, e, _, _)| e.saturating_sub(*s))
                })
                .map(|(_, _, name, _)| name.as_str())
        };
        // Straddling: line 115 is inside `outer` only, but the chunk reaches
        // into `inner`. Containment wins, so it is `outer` — the rule prefers
        // the definition the hit actually begins in.
        assert_eq!(pick(115, 130), Some("outer"));
        // Entirely above both definitions but overlapping `outer`: labelled.
        assert_eq!(pick(90, 105), Some("outer"));
        // Innermost wins when both contain the start.
        assert_eq!(pick(125, 135), Some("inner"));
        // Nothing near it at all.
        assert_eq!(pick(200, 210), None);
    }

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

#[cfg(test)]
mod plain_path_tests {
    use super::plain;

    /// Runs on every platform against literal strings, because the bug is about
    /// text and a macOS machine can still prove the rule (#74).
    #[test]
    fn a_verbatim_prefix_is_not_what_anybody_pastes_back() {
        assert_eq!(
            plain(r"\\?\C:\work\api\src\lock.rs"),
            r"C:\work\api\src\lock.rs"
        );
        assert_eq!(plain(r"\\?\c:\work"), r"c:\work");
        // A UNC path keeps both leading slashes, which is the form that opens.
        assert_eq!(
            plain(r"\\?\UNC\server\share\notes.md"),
            r"\\server\share\notes.md"
        );
        // A volume with no mount point is named by that GUID and by nothing
        // else, so shortening it would leave a path that names nothing.
        let volume = r"\\?\Volume{9f8a7b6c-0000-0000-0000-000000000000}\data";
        assert_eq!(plain(volume), volume);
        // Everything else is left exactly as it is.
        assert_eq!(
            plain("/home/someone/api/src/lock.rs"),
            "/home/someone/api/src/lock.rs"
        );
        assert_eq!(plain(r"C:\work\api"), r"C:\work\api");
        assert_eq!(
            plain(r"\\server\share\notes.md"),
            r"\\server\share\notes.md"
        );
        assert_eq!(plain(""), "");
    }
}
