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

pub mod accel;
pub mod add;
pub mod agentfiles;
pub mod attest;
pub mod brief;
pub mod cache;
pub mod chunk;
pub mod clientfile;
pub mod clientid;
pub mod clients;
pub mod clock;
pub mod cloud;
pub mod compact;
pub mod coreml;
pub mod cpucap;
pub mod cuda;
pub mod daemon;
pub mod doctor;
pub mod drop;
pub mod embed;
pub mod filter;
pub mod fleet;
/// Readers for the formats that are not plain text. Private: what semlith
/// extracts from a given document is documented behaviour, not an API.
mod formats;
pub mod gpu;
pub mod graph;
pub mod helpers;
pub mod home;
pub mod hook;
pub mod http;
pub mod image;
pub mod index;
pub mod keyscan;
pub mod ledger;
pub mod leftovers;
pub mod llama;
pub mod lock;
pub mod mcp;
pub mod openvino;
pub mod packs;
pub mod pattern;
/// The daemon as a login service, so a client never finds nothing.
pub mod pipeline;
pub mod portal;
pub mod prices;
pub mod priority;
pub mod progress;
pub mod proxy;
pub mod remote;
pub mod replay;
pub mod report;
pub mod rerank;
pub mod routes;
pub mod schedule;
pub mod service;
pub mod session;
pub mod setup;
pub mod store;
pub mod system;
pub mod tree;
pub mod trt;
pub mod upgrade;
pub mod usage;
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

/// Embed each window in walk order rather than by length: the unsorted path
/// the length-sorted window is measured against.
pub(crate) const UNSORTED_ENV: &str = "SEMLITH_UNSORTED";

/// The meta row counting a store's chunks per vector variant.
const VARIANTS_KEY: &str = "variants";

/// Embedding sessions writers hold in this process, for `/api/about`.
static SESSIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// How many writer embedding sessions are loaded in this process.
pub fn writer_sessions() -> usize {
    SESSIONS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Chunks an index pass hands the embed stage at once, sorted by length there.
///
/// A batch pads every row to its longest, and the pass used to flush eight
/// chunks in file order — so one long chunk made seven short ones pay for its
/// length. Holding sixty-four and embedding them shortest first measured 19.5
/// to 29.1 chunks/s on the reference M1, CPU alone, median of three. From
/// 0.32.0 a window is 256: the stage works on several at once, and a longer
/// window sorts tighter. Formed in walk order and sorted stably, so a corpus
/// always produces the same batches.
const SORT_WINDOW: usize = 256;

/// The longest an index pass holds written rows uncommitted.
const ROW_COMMIT: std::time::Duration = std::time::Duration::from_millis(250);

/// How often a pass says how far it has got while the writer waits on the
/// devices. A window lands every 256 chunks, which on a slow CPU is a quarter
/// of a minute of a run card standing still; batches come back far oftener.
const PROGRESS_TICK: std::time::Duration = std::time::Duration::from_millis(250);

/// How often a writer waiting with nothing moving says so anyway.
const PROGRESS_HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(2);

/// The last batch count a waiting writer reported, and when.
struct Progress {
    seen: usize,
    at: std::time::Instant,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            seen: 0,
            at: std::time::Instant::now(),
        }
    }
}

impl Progress {
    /// The count, when it has moved and a tick has passed since the last, or
    /// every [`PROGRESS_HEARTBEAT`] when it has not: a run waiting for a lane
    /// to compile still says so, with the lane's time left, as it goes.
    fn due(&mut self, batched: &std::sync::atomic::AtomicUsize) -> Option<usize> {
        let n = batched.load(std::sync::atomic::Ordering::Relaxed);
        let since = self.at.elapsed();
        if since < PROGRESS_TICK || (n == self.seen && since < PROGRESS_HEARTBEAT) {
            return None;
        }
        self.seen = n;
        self.at = std::time::Instant::now();
        Some(n)
    }
}

/// Keyword candidates with no vector yet that a search mid-run embeds on the
/// query path. Sixteen chunks on the CPU is about half a second on the M1,
/// which keeps a mid-run search inside two.
const PENDING_EMBED: usize = 16;

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
            // The image list is steep for the same reason, and its absence
            // here was the whole of #122. CLIP is exact about a query that
            // describes a picture and vague about anything else, which is the
            // shape this curve is for — and the floor above is what decides
            // which of the two a given match is. Measured on the four-shape
            // fixture before the change: "a red circle" scored parse.rs at
            // 0.093317 (vector at the steep 12 plus keyword at 60) and
            // red-circle.png at 0.049180 (3 / 61), so the image could not
            // reach a passing text chunk however confident CLIP was. The
            // weight below carries the confidence; this carries the curve.
            (_, "image") => STEEP,
            (_, "named") => STEEP,
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

/// Reciprocal-rank fusion over the candidate lists.
///
/// Lists in, one fused score and one provenance badge set per candidate out,
/// in the order the lists were given. Until 0.36.0 a fourth list, the graph
/// walk, was derived from the vector and keyword lists and only ever added
/// candidates; the 727-question benchmark of 2026-10-01 scored the same with
/// and without it (398 against 397 of 507) while it took a third of a search's
/// time on a large store, so search no longer builds it. `neighbors`,
/// `impact`, `path` and `trace` still walk the graph.
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
        for (rank, (id, weight)) in ranking.iter().enumerate() {
            let key = (*is_image, *id);
            let contribution = weight / (constant + rank as f32 + 1.0);
            match seen.get(&key) {
                Some(&slot) => {
                    fused[slot].1 += contribution;
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

/// Whether a question is about code, which is what `brief` spends its one
/// text span and its graph lines on.
///
/// A second reading of the same text, kept beside [`shape_of`] so there is
/// still one place the query is read: an identifier-shaped query is code, and
/// so is a sentence that names an identifier (`semlith_search`, `Fleet::open`,
/// `run()`, `searchPreferring`) or asks about a function, a caller or a type.
pub fn code_shaped(query: &str) -> bool {
    const CODE_WORDS: [&str; 22] = [
        "function",
        "functions",
        "method",
        "methods",
        "call",
        "calls",
        "called",
        "caller",
        "callers",
        "callee",
        "struct",
        "class",
        "impl",
        "trait",
        "enum",
        "type",
        "field",
        "variable",
        "signature",
        "parameter",
        "return",
        "returns",
    ];
    if shape_of(query) == Shape::Identifier {
        return true;
    }
    query.split_whitespace().any(|word| {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != ':');
        let inner_capital =
            word.chars().skip(1).any(char::is_uppercase) && word.chars().any(char::is_lowercase);
        word.contains('_')
            || word.contains("::")
            || inner_capital
            || CODE_WORDS.contains(&word.to_lowercase().as_str())
    }) || query.contains("()")
}

/// The identifiers a sentence names: backticked, `snake_case`, `camelCase`,
/// `a::b` and `call()` words, at most three.
fn named_identifiers(query: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in query.split_whitespace() {
        let call = raw.contains("()");
        let word = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != ':');
        let word = word.rsplit("::").next().unwrap_or(word);
        if word.len() < 3
            || !word
                .chars()
                .next()
                .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            continue;
        }
        let inner_capital =
            word.chars().skip(1).any(char::is_uppercase) && word.chars().any(char::is_lowercase);
        let ticked = raw.starts_with('`');
        if (word.contains('_') || inner_capital || ticked || call || raw.contains("::"))
            && !out.iter().any(|w| w == word)
        {
            out.push(word.to_string());
        }
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// Whether a query asks about releases, which keeps release notes in place.
fn names_releases(query: &str) -> bool {
    query.split(|c: char| !c.is_alphanumeric()).any(|w| {
        let w = w.to_lowercase();
        w.starts_with("changelog") || w.starts_with("release") || w == "version" || w == "news"
    })
}

/// Whether a path is release notes: a CHANGELOG, CHANGES, HISTORY, RELEASES or
/// NEWS file, or anything under a `changelogs` directory.
fn is_release_notes(path: &str) -> bool {
    let p = Path::new(path);
    let stem = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // Prose only: `history.rs` is code that happens to share the name.
    (!filter::is_code(path)
        && matches!(
            stem.as_str(),
            "changelog"
                | "changes"
                | "history"
                | "releases"
                | "release-notes"
                | "release_notes"
                | "news"
        ))
        || p.components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case("changelogs"))
}

/// Whether a query asks about tests, which is what exempts it from
/// [`TEST_PENALTY`].
fn names_tests(query: &str) -> bool {
    query.split(|c: char| !c.is_alphanumeric()).any(|w| {
        let w = w.to_lowercase();
        w.starts_with("test") || w.starts_with("spec") || w.starts_with("fixture")
    })
}

/// Whether a path sits under a test or fixture directory.
pub fn is_test_path(path: &str) -> bool {
    Path::new(path).components().any(|c| {
        matches!(
            c.as_os_str().to_str(),
            Some("tests" | "test" | "fixtures" | "spec" | "__tests__")
        )
    })
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

/// How far the project the head of the answer agrees on is lifted, on a store
/// of several projects: a hit is multiplied by `1 + PROJECT_LIFT * share`,
/// where `share` is its project's part of the head's fused score.
///
/// Not a tiebreak, on purpose: it has to move a chunk of the right repository
/// past one of another that shares the question's words. Measured on the
/// 727-question benchmark (development split, unscoped, 4k tokens) together
/// with [`NAMED_PROJECT_LIFT`], release notes last and the named-identifier
/// list: 319 to 340 of 507, 25 wins against 4 losses; scoped 398 to 407 with
/// no loss.
const PROJECT_LIFT: f32 = 1.0;

/// How far a hit is lifted when the query names its project: a word of four
/// letters or more of the project directory's name is a word of the query.
/// A bug report that says "ripgrep" is about ripgrep. Adding it took the
/// unscoped development split from 333 to 340.
const NAMED_PROJECT_LIFT: f32 = 3.0;

/// How many of the fused head vote on which project a query is about.
const PROJECT_HEAD: usize = 20;

/// Each hit's project's share of the head's fused score, or 0.0 for every hit
/// when the candidates sit in fewer than two projects.
///
/// A store of seventy repositories answered an unscoped question 15 points
/// worse than one scoped to the right repository (63 % against 78 % on the
/// 727-question benchmark of 2026-10-01): a chunk of another repository that
/// shares the question's vocabulary outranks the one that answers it. The
/// lists that found the head mostly agree on the repository even where they
/// disagree on the chunk, so the agreement is the prior. A project is the
/// nearest directory holding `.git`; one project, or none, is no change.
fn project_prior(hits: &[(Hit, f32)], query: &str) -> (Vec<f32>, Vec<bool>) {
    let mut seen: std::collections::HashMap<PathBuf, Option<PathBuf>> =
        std::collections::HashMap::new();
    let mut project_of = |path: &str| -> Option<PathBuf> {
        let dir = Path::new(path).parent()?.to_path_buf();
        if let Some(found) = seen.get(&dir) {
            return found.clone();
        }
        let found = dir
            .ancestors()
            .find(|d| d.join(".git").exists())
            .map(Path::to_path_buf);
        seen.insert(dir, found.clone());
        found
    };
    let projects: Vec<Option<PathBuf>> = hits.iter().map(|(h, _)| project_of(&h.path)).collect();
    // A project the query names: a word of four letters or more of the
    // project directory's name is a word of the query.
    let words: std::collections::HashSet<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_lowercase)
        .collect();
    let named: Vec<bool> = projects
        .iter()
        .map(|p| {
            p.as_ref()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|n| {
                    n.to_lowercase()
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|t| t.len() >= 4 && words.contains(t))
                })
        })
        .collect();
    let named = if named.iter().all(|n| *n) {
        vec![false; named.len()]
    } else {
        named
    };
    let mut mass: std::collections::HashMap<&PathBuf, f32> = std::collections::HashMap::new();
    for ((hit, _), project) in hits.iter().zip(&projects).take(PROJECT_HEAD) {
        if let Some(project) = project {
            *mass.entry(project).or_default() += hit.score.max(0.0);
        }
    }
    let total: f32 = mass.values().sum();
    if mass.len() < 2 || total <= 0.0 {
        return (vec![0.0; hits.len()], named);
    }
    let shares = projects
        .iter()
        .map(|p| {
            p.as_ref()
                .and_then(|p| mass.get(p))
                .map_or(0.0, |m| m / total)
        })
        .collect();
    (shares, named)
}

/// How much a chunk from a file edited since it was indexed is pushed down.
///
/// Not removed: the excerpt may still be the best answer there is, and 0.15.0
/// already flags it as stale. This only means a current answer of equal
/// quality is preferred to it.
const STALE_PENALTY: f32 = 0.10;

/// How much a chunk under `tests/`, `fixtures/`, `spec/` or `__tests__/` is
/// pushed down for a question that does not name tests.
///
/// The same size and the same reasoning as [`STALE_PENALTY`]: a tiebreak, not
/// a filter. On 2026-09-25 `brief` put `tests/watch.rs` first for a question
/// about the MCP request path, because a test that drives a path names every
/// step of it. A question about tests is exempt and ranks them as before.
///
/// A weight alone was not enough — on the 0.30.0 tree that test outranked
/// `src/` by 16 % through the graph list, and a demotion large enough to undo
/// that would be a ranking rather than a tiebreak — so [`product_first`]
/// adds a precedence beside it.
const TEST_PENALTY: f32 = 0.10;

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

/// What an image below [`IMAGE_FLOOR`] is worth in the fusion.
///
/// Small enough that a weak image scores about what it did before the list
/// was given the steep curve, so #122's fix moved the confident case and
/// nothing else.
const WEAK_IMAGE: f32 = 0.25;

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

/// One resolved filter: what it was, the store state it was resolved
/// against, and what it resolved to.
/// How many resolved filters a store keeps (#183).
const FILTER_MEMOS: usize = 8;

struct FilterMemo {
    groups: Vec<Vec<String>>,
    generation: u64,
    newest_chunk: i64,
    allowlist: Allowlist,
    selected: std::sync::Arc<std::collections::HashSet<u64>>,
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
    /// The text is the file's current lines, read because the store's copy
    /// was out of date. See [`Semlith::read_within`].
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub from_disk: bool,
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
    /// The line `symbol` is defined on, which is what an answer should cite:
    /// the chunk's range can start at a doc comment or sit in the middle of a
    /// long function, and "line 3290" of a 40-line method is not where it is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_line: Option<u32>,
    /// Other files holding exactly this chunk's text — a vendored or fixture
    /// copy — shown once here instead of spending a result slot each.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    #[serde(serialize_with = "serialize_plain_list")]
    pub copies: Vec<String>,
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
    /// Not a verdict: a batch of the file in hand has been embedded. Said per
    /// batch so a file of thousands of chunks moves the counters, the rate
    /// and the thread count as it goes rather than all at once at its end.
    Progress,
    /// An image, read for its pixels and embedded by the image model.
    Image,
    /// Not a file: the run has entered a phase ([`IndexProgress::phase`]).
    Phase,
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
            Self::Progress => "progress",
            Self::Image => "image",
            Self::Phase => "phase",
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

    /// Which not-indexed class this is (2.3): over the cap is a policy a
    /// person may override, everything else is a fact accepting cannot change.
    pub fn class(&self) -> &'static str {
        match self {
            Self::TooLarge => store::class::POLICY,
            _ => store::class::UNINDEXABLE,
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
        self.resolved(home).refuses(path, walked)
    }

    /// This boundary with its roots resolved once, for a pass that asks about
    /// many paths. `home` is the canonical home directory, already resolved.
    pub fn resolved(&self, home: Option<&Path>) -> ResolvedBoundary {
        ResolvedBoundary {
            within: self.roots.as_deref().map(filter::resolve_boundary),
            boundary: self.clone(),
            home: home.map(Path::to_path_buf),
        }
    }
}

/// A [`Boundary`] with its roots and the home directory resolved, so asking
/// about a path costs that path's resolution and nothing more.
///
/// Owned rather than borrowing the boundary: a run holds one for its whole
/// loop, and the loop evicts through `&mut self`.
#[derive(Debug, Clone)]
pub struct ResolvedBoundary {
    boundary: Boundary,
    within: Option<Vec<String>>,
    home: Option<PathBuf>,
}

impl ResolvedBoundary {
    /// [`Boundary::refuses`], against the roots resolved when this was made.
    pub fn refuses(&self, path: &Path, walked: bool) -> Option<Refusal> {
        if let (Some(roots), Some(within)) = (&self.boundary.roots, &self.within)
            && !filter::within_resolved(path, within)
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
        if !self.boundary.allow_secrets
            && let Some(why) = filter::denied_against(path, self.home.as_deref())
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
    /// The intra-op thread count the run's session was built with, once it
    /// has one. What the run card shows, because a saved setting is a request
    /// and this is what the engine is doing.
    pub threads: usize,
    /// Chunks each lane has embedded for this store: `cpu`, `gpu`, `cuda`.
    pub lanes: std::collections::BTreeMap<String, usize>,
    /// Bytes of the walk's files this run has got through, a file being
    /// embedded counted in proportion to its chunks, and the bytes the walk
    /// found. What the daemon's remaining-time estimate is taken from: files
    /// vary in size by orders of magnitude, so a count of them says little
    /// about how much work is left.
    pub bytes: u64,
    pub bytes_total: u64,
    /// Chunk rows written so far, embedded or not yet.
    pub rows: usize,
    /// What the run is doing now, and in a sentence. See [`progress::Phase`].
    pub phase: progress::Phase,
    pub phase_detail: Option<String>,
    /// The chunks this run is expected to embed in all: rows written so far
    /// and the estimate for every file not reached yet, which each file
    /// replaces with its real count as it is chunked.
    pub expected_chunks: u64,
    /// Images embedded so far, and images this run expects to embed.
    pub images: usize,
    pub images_total: usize,
    /// The run's share done, in work units ([`progress::Work`]), never moving
    /// backwards and below 1 until the run is over.
    pub progress: f64,
    /// Time left, and a low and high bound, from [`progress::Eta`].
    pub eta_ms: Option<u64>,
    pub eta_range_ms: Option<(u64, u64)>,
    /// What an image weighs in chunks in this run: fixed for the run, from
    /// this machine's measured image-model and lane rates. Re-measured as a
    /// run went, it moved the units already done and the time left with them.
    pub image_weight: f64,
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
    /// Files indexed whose every secret-shaped match was a declared test dummy.
    pub dummies_indexed: usize,
    /// Files indexed because a person accepted them, in their accepted mode.
    pub accepted_indexed: usize,
    /// Symbols extracted in this run, and the edges between them.
    pub symbols: usize,
    pub edges: usize,
    /// Images embedded in this run.
    pub images: usize,
    /// Whether the run was stopped rather than finished.
    pub stopped: bool,
    /// The intra-op thread count the session ran with at the end of this
    /// call, or 0 when nothing was embedded.
    pub threads: usize,
    /// Chunks embedded so far in this call, counted per batch. `chunks` counts
    /// finished files, so it stands still through a large one; this does not.
    pub embedded: usize,
    /// Chunks whose batch has come back from a device, ahead of their window
    /// landing: what progress moves on while the writer waits.
    #[serde(skip)]
    pub batched: usize,
    /// Chunks each lane has embedded for this store, since it was opened.
    #[serde(skip)]
    pub lanes: std::collections::BTreeMap<String, usize>,
    /// See [`IndexProgress::bytes`].
    #[serde(skip)]
    pub bytes: u64,
    #[serde(skip)]
    pub bytes_total: u64,
    /// Every file this call embedded, in order. The caller keeps these across
    /// the slices of one logical run, so stopping can undo the whole run
    /// rather than only the slice that happened to be going.
    pub written: Vec<String>,
    /// Where this call's wall time went, stage by stage.
    pub stages: pipeline::Stages,
    /// Chunks the machine-wide vector cache was asked for, and held.
    pub cache_lookups: usize,
    pub cache_hits: usize,
    /// Chunk rows this call wrote, embedded or not yet: what `embedded` is
    /// catching up with, and what a card's pending share is taken from.
    pub rows: usize,
    /// The chunks and images expected from the files this call has not
    /// reached yet: the estimate a continuation slice starts from.
    pub expected_left: u64,
    pub images_left: usize,
    /// Images with the image lane: read, not embedded yet. Counted in the
    /// total, or the total dropped by each one sent and rose as it landed.
    pub images_flight: usize,
    /// The file in hand's estimate and the rows it has written so far: the
    /// rest of its estimate stays in the expected total until it is written,
    /// so a large file does not take its whole estimate out at its first row.
    #[serde(skip)]
    pub(crate) current_est: u64,
    #[serde(skip)]
    pub(crate) current_rows: u64,

    /// What the pass is doing, its share-done high water and its time-left
    /// estimate. Behind a cell because every progress line reads it through
    /// a shared reference.
    #[serde(skip)]
    pub(crate) live: std::cell::RefCell<Live>,
}

/// The parts of a pass's account that move with every line it says.
#[derive(Debug, Clone)]
pub(crate) struct Live {
    pub phase: progress::Phase,
    pub detail: Option<String>,
    pub high: f64,
    pub eta: progress::Eta,
    pub started: std::time::Instant,
    pub image_weight: f64,
    pub images_parallel: bool,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            phase: progress::Phase::Read,
            detail: None,
            high: 0.0,
            eta: progress::Eta::default(),
            started: std::time::Instant::now(),
            image_weight: progress::IMAGE_UNITS,
            images_parallel: false,
        }
    }
}

/// What an index call puts into the vector cache and takes out of it, by the
/// hash of each chunk's embedded text and the variant of its vector.
/// One image for the image lane: its bytes and what the writer needs to
/// record it once its vector comes back.
struct ImageJob {
    path: PathBuf,
    key: String,
    bytes: Vec<u8>,
    hash: String,
    width: u32,
    height: u32,
}

/// An image the image lane has finished, or failed on its bytes.
struct ImageDone {
    job: ImageJob,
    vector: std::result::Result<Vec<f32>, String>,
}

/// Image bytes the writer may have waiting for the image lane before it waits
/// itself. The image model is slower than reading, and a folder of
/// screenshots would otherwise sit in memory whole.
const IMAGE_BYTES_IN_FLIGHT: u64 = 256 * 1024 * 1024;

#[derive(Default)]
struct CacheWrites {
    fresh: Vec<([u8; 32], &'static str, Vec<f32>)>,
    hits: Vec<([u8; 32], &'static str)>,
}

/// The version of the scan rules a store was last swept under. A store below
/// it has every unchanged file scanned again on its next full pass: 0.30.0's
/// dummy rules (2.1) let some files in, and its no-prefix rule (2.2) keeps
/// some out, and neither shows up in a content hash.
const SCAN_RULES: u32 = 2;

/// The version of the graph extraction a store was last swept under. A store
/// below it has every unchanged file's symbols and edges extracted again on its
/// next full pass, without re-embedding: 0.30.0 records a Rust method's owner
/// and a receiver's type, and an unchanged file would otherwise keep the edges
/// an older release wrote until somebody edited it.
const GRAPH_RULES: u32 = 3;

/// How many paths of each not-indexed class a plan lists.
const PLAN_PATHS: usize = 50;

/// One file or folder the scan phase says a person could accept.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    #[serde(serialize_with = "serialize_plain")]
    pub path: String,
    pub class: String,
    pub rule: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matches: Vec<keyscan::Match>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    /// `keyscan::assess`'s fields — risk, band, likely, tone, kind, why,
    /// evidence, suggest — beside the row, as `/api/refused` carries them.
    #[serde(flatten)]
    pub assessment: serde_json::Map<String, serde_json::Value>,
    /// Chunks the file would add if a person lets it in, for the estimate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks: Option<u64>,
}

impl Review {
    fn new(
        path: String,
        class: &str,
        rule: String,
        matches: Vec<keyscan::Match>,
        confidence: Option<u8>,
    ) -> Self {
        let stored: Vec<serde_json::Value> = matches
            .iter()
            .filter_map(|m| serde_json::to_value(m).ok())
            .collect();
        let assessment = match keyscan::assess(class, &path, &stored, confidence) {
            serde_json::Value::Object(map) => map,
            _ => Default::default(),
        };
        Self {
            path,
            class: class.to_string(),
            rule,
            matches,
            confidence,
            assessment,
            chunks: None,
        }
    }
}

/// How far a scan has got: `walk`, then `read` (each file read, hashed and
/// checked for credentials), then `rules` once every file is decided.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanProgress {
    pub phase: &'static str,
    pub scanned: usize,
    pub total: usize,
    pub bytes: u64,
    pub bytes_total: u64,
    /// Files held for a person's decision or refused as credentials so far.
    pub flagged: usize,
}

/// What a run would do, before it embeds anything (2.7).
#[derive(Debug, Clone, Default, Serialize)]
pub struct Plan {
    /// Files to embed, their bytes, and how many of each language.
    pub embed: usize,
    pub embed_bytes: u64,
    pub languages: std::collections::BTreeMap<String, usize>,
    pub unchanged: usize,
    /// Not indexed, by class.
    pub not_indexed: std::collections::BTreeMap<String, usize>,
    /// The first [`PLAN_PATHS`] paths of each not-indexed class, so a page
    /// can list what was left out rather than only count it.
    #[serde(serialize_with = "serialize_plain_lists")]
    pub not_indexed_paths: std::collections::BTreeMap<String, Vec<String>>,
    /// What a person could accept, one entry each.
    pub review: Vec<Review>,
    /// Credential files: listed, never offered.
    #[serde(serialize_with = "serialize_plain_list")]
    pub credential: Vec<String>,
    /// Files whose every match is a declared test dummy.
    pub dummies: usize,
    /// Milliseconds to embed, from the store's last measured byte rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eta_ms: Option<u64>,
    /// How long the scan itself took.
    pub seconds: f64,
    /// Chunks the files to embed hold, counted with the run's own chunker,
    /// and the images among them with their bytes: what an estimate before
    /// the run is taken from.
    pub chunks: u64,
    pub images: usize,
    pub image_bytes: u64,
    /// The same count per file, by stored key, for the run that follows.
    #[serde(skip)]
    pub counts: std::collections::HashMap<String, u64>,
}

/// The size a file counts for in a run's byte totals: its length, or nothing
/// when it cannot be read or is over the cap and will be skipped unread.
fn embeddable_bytes(meta: Option<std::fs::Metadata>) -> u64 {
    meta.filter(|m| m.is_file() && m.len() <= chunk::MAX_FILE_BYTES)
        .map_or(0, |m| m.len())
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
    on_file: &mut dyn FnMut(&Path, IndexProgress),
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
    let chunks = report.embedded.max(report.batched);
    let expected_chunks = report.rows as u64
        + report.expected_left
        + report.current_est.saturating_sub(report.current_rows);
    let images_total = report.images + report.images_flight + report.images_left;
    let (phase, phase_detail, share, eta, image_weight) = {
        let mut live = report.live.borrow_mut();
        // A lane still loading is the run's to say, not a global's: the
        // writer is waiting on it whatever the run is otherwise doing.
        // In any phase in which the writer can be waiting on the lanes: a new
        // slice opens in Read and waits there for a lane that went idle.
        match accel::waiting_for() {
            Some(waiting)
                if matches!(
                    live.phase,
                    progress::Phase::Read | progress::Phase::Embed | progress::Phase::Lane
                ) =>
            {
                live.phase = progress::Phase::Lane;
                live.detail = Some(waiting);
            }
            // Embedding has begun inside a long file, between the writer's
            // turns at the top of its loop: say so then, not after the file.
            None if live.phase == progress::Phase::Read && chunks > 0 => {
                live.phase = progress::Phase::Embed;
                live.detail = None;
            }
            None if live.phase == progress::Phase::Lane => {
                live.phase = progress::Phase::Embed;
                live.detail = None;
            }
            _ => {}
        }
        let work = progress::Work {
            files: (report.scanned as u64, total as u64),
            chunks: (chunks as u64, expected_chunks),
            images: (report.images as u64, images_total as u64),
            image_weight: live.image_weight,
            parallel: live.images_parallel,
        };
        live.high = live.high.max(work.share()).min(0.999);
        let at = live.started.elapsed().as_secs_f64();
        // Held only while nothing the estimate counts can move: a lane
        // loading, the index being written, the walk. A drain is the lanes
        // finishing the embedding, which is the work itself, and counts down.
        let hold = matches!(
            live.phase,
            progress::Phase::Lane | progress::Phase::Save | progress::Phase::Walk
        );
        let (done, all) = work.eta_units();
        live.eta.observe_unless(hold, at, done);
        let eta = (all > 0.0)
            .then(|| live.eta.left(at, all - done, hold))
            .flatten();
        (
            live.phase,
            live.detail.clone(),
            live.high,
            eta,
            live.image_weight,
        )
    };
    on_file(
        path,
        IndexProgress {
            outcome,
            scanned: report.scanned,
            indexed: report.indexed,
            // Vectors landed, not rows written: the writer runs ahead of the
            // embed stage, and a rate taken from rows would count work not
            // yet done.
            chunks,
            total,
            symbols: report.symbols,
            why,
            threads: report.threads,
            lanes: report.lanes.clone(),
            bytes: report.bytes,
            bytes_total: report.bytes_total,
            rows: report.rows,
            phase,
            phase_detail,
            expected_chunks,
            images: report.images,
            images_total,
            progress: share,
            eta_ms: eta.map(|(ms, _)| ms),
            eta_range_ms: eta.map(|(_, range)| range),
            image_weight,
        },
    );
}

/// Enter a phase and say so: one line, no file.
fn say_phase(
    on_file: &mut dyn FnMut(&Path, IndexProgress),
    report: &IndexReport,
    total: usize,
    phase: progress::Phase,
    detail: Option<String>,
) {
    {
        let mut live = report.live.borrow_mut();
        if live.phase == phase && live.detail == detail {
            return;
        }
        live.phase = phase;
        live.detail = detail;
    }
    say_file(
        on_file,
        report,
        total,
        Path::new(""),
        FileOutcome::Phase,
        None,
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
/// Names a file whose reader should panic, so the containment below can be
/// proved on every runner without a real reader bug to lean on. Read only here.
pub const FAULT_PANIC_ENV: &str = "SEMLITH_FAULT_PANIC";

fn fault_panic(path: &Path) {
    if let Ok(name) = std::env::var(FAULT_PANIC_ENV)
        && path
            .file_name()
            .is_some_and(|n| n.to_string_lossy() == name)
    {
        panic!("{FAULT_PANIC_ENV} names {name}");
    }
}

impl Drop for Semlith {
    fn drop(&mut self) {
        self.release_embedder();
    }
}

/// Run one file's reading, parsing or chunking with a panic caught.
///
/// Only work that has not written a row yet goes through here, so a panic
/// leaves nothing half-done: the file is reported as failed, with the message,
/// and the pass moves to the next one.
fn contained<T>(work: impl FnOnce() -> T) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|panic| {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "no message".to_string());
        anyhow::anyhow!("the reader panicked on this file: {message}")
    })
}

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
    /// The intra-op thread count `embedder` was built with. A different value
    /// in force rebuilds it at the next batch.
    embedder_threads: usize,
    /// The index pass's own CPU session, fed token ids, kept between slices.
    /// Queries go through `embedder`; the query path is not the index pass's.
    index_session: Option<pipeline::CpuWork>,
    /// The intra-op thread count `index_session` was built with.
    index_threads: usize,
    /// The second session a `mix` harness run alternates with, batch by batch.
    embedder_alt: Option<TextEmbedding>,
    /// A query session of a variant other than the index pass's, for the
    /// harness's mixed runs.
    query_embedder: Option<TextEmbedding>,
    /// Batches this store has embedded, which is what the alternation counts.
    batches: u64,
    /// The variant the last batch was embedded with.
    last_variant: &'static str,
    /// When this store last embedded anything, so a daemon can drop an idle
    /// writer's session and the arena that comes with it.
    last_embed: Option<std::time::Instant>,
    /// The writer's time by part over the index pass in progress (#198).
    write_parts: pipeline::WriteParts,
    /// The budget generation this store's indexes were last fitted to.
    budget_generation: u64,
    /// Chunks each lane embedded for this store since it was opened, which a
    /// run card turns into a rate per lane.
    lane_chunks: std::collections::BTreeMap<String, usize>,
    /// The embedder's own tokenizer, for counting rather than estimating.
    tokenizer: Option<tokenizers::Tokenizer>,
    /// CLIP's two encoders, loaded on the first image indexed or searched for.
    clip: image::Clip,
    /// The index generation this process has loaded. Compared against the
    /// store's on every search to notice another process's writes.
    generation: u64,
    /// The filters this store resolved most recently, newest first, each kept
    /// until the store changes. An agent scoped to one repository asks with
    /// the same `path` call after call, and resolving it again was the largest
    /// cost left in a scoped search (#170); an agent moving between a few
    /// repositories missed a one-entry memo on every call (#183).
    filter_memo: std::collections::VecDeque<FilterMemo>,
    /// Print model-download progress to stderr. Off for the MCP server, where
    /// stdout/stderr are a protocol channel.
    pub quiet: bool,
    /// What this caller may index. The default is the command line's: the
    /// deny-list, and no confinement.
    pub boundary: Boundary,
    /// Whether walks honour `.gitignore` and the global gitignore. The
    /// store's `gitignore` setting (0.35.0); the deny-list, the hidden-file
    /// rule, `.semlithignore` and the secret scan apply either way.
    pub gitignore: bool,
    /// Re-embed every file this pass reaches, whatever its recorded hash, and
    /// without the vector cache: a forced re-index (0.35.0) exists to replace
    /// what is stored, and a cache hit would hand back the same vectors.
    pub(crate) force: bool,
    /// A vector cache of this store's own in place of the machine's: index
    /// runs read and write it and never the machine's. `None`, the binary's
    /// only setting, keeps the machine's cache where this process uses it
    /// ([`accel::cache_in_use`]).
    pub vector_cache: Option<cache::Location>,
    /// Chunks the scan counted for each file it planned, by stored key, so a
    /// run's expected total starts exact rather than estimated.
    pub planned: Option<std::sync::Arc<std::collections::HashMap<String, u64>>>,
    /// The plan's images, which `planned` (chunks a file) does not count.
    pub planned_images: Option<usize>,
    /// What a continuation slice has left to embed, from the slice before it:
    /// chunks and images. Taken by the next pass instead of sizing every
    /// remaining file again.
    pub expect_rest: Option<(u64, usize)>,
    /// Files being written or whose vectors are still with the lanes: chunks
    /// not landed yet, the hash to commit, and whether every row is written
    /// (sealed). A file leaves for `landed` once it is sealed and none are
    /// left. A checkpoint commits only those, so it no longer waits for every
    /// window in flight (#203). Counted per chunk as each is inserted, since a
    /// window handed over mid-file can land before its file is finished.
    awaiting: std::collections::HashMap<i64, (usize, String, bool)>,
    /// Which file each in-flight chunk belongs to.
    file_of: std::collections::HashMap<u64, i64>,
    /// Files whose last vector has landed since the last commit of hashes.
    landed: Vec<(i64, String)>,
    /// Size, mtime and content hash of each file the scan phase read, so the
    /// embed pass that follows it does not read an unchanged file twice.
    prehashed: std::collections::HashMap<PathBuf, (u64, i64, String)>,
    /// A refusal about a file's contents evicted what the store held of it
    /// during this run, so the database's dead pages and the keyword index's
    /// old terms are rewritten before the run ends (#148).
    scrub: bool,
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
            embedder_threads: 0,
            index_session: None,
            index_threads: 0,
            embedder_alt: None,
            query_embedder: None,
            batches: 0,
            last_variant: embed::Variant::Int8.name(),
            last_embed: None,
            write_parts: Default::default(),
            budget_generation: index::budget_generation(),
            lane_chunks: std::collections::BTreeMap::new(),
            tokenizer: None,
            clip: image::Clip::default(),
            generation,
            filter_memo: std::collections::VecDeque::new(),
            quiet: false,
            boundary: Boundary::default(),
            gitignore: true,
            force: false,
            vector_cache: None,
            planned: None,
            planned_images: None,
            expect_rest: None,
            awaiting: Default::default(),
            file_of: Default::default(),
            landed: Vec::new(),
            prehashed: Default::default(),
            scrub: false,
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

        // Dropped rather than reloaded here: whatever asked for this refresh is
        // about to search, and the load it triggers is the same load this used
        // to do eagerly. A reader that only ever calls `stats` pays nothing.
        //
        // Re-opened rather than only evicted: the shard list is read from the
        // directory when an index is opened, so evicting alone left a reader
        // blind to every shard a writer created after it opened, and to a
        // compaction's whole new set. The image index is the store's too.
        let was_resident = self.index.is_resident();
        self.reopen_indexes()?;
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

    /// Name the store's two vector indexes again from what is on disk.
    fn reopen_indexes(&mut self) -> Result<()> {
        let sharded = store::format(&self.db)? >= store::SHARDED_FORMAT;
        self.index = VectorIndex::open(&self.dir, self.dim, BIT_WIDTH, sharded)?;
        self.images = VectorIndex::open(
            &self.dir.join(image::INDEX_DIR),
            image::DIM,
            BIT_WIDTH,
            true,
        )?;
        Ok(())
    }

    /// The vector half of a search, read consistently against a compaction
    /// in another process.
    ///
    /// A compaction swaps the shard set with two renames, marking the store
    /// while it does and moving the generation after. A search that began or
    /// ended inside that window, or across a generation change, may have read
    /// a shard that was no longer there -- which reads as empty, not as an
    /// error -- so it is asked again against the set now on disk. Two meta
    /// reads when nothing is happening, which is every search but a handful.
    fn search_vectors(
        &mut self,
        vector: &[f32],
        depth: usize,
        allowlist: &Allowlist,
    ) -> Result<(Vec<f32>, Vec<u64>)> {
        // ponytail: about two seconds of retries, then the answer the index
        // gives. A swap is two renames; one still marked after that is a
        // compaction that died mid-swap, and the next writer's recovery clears
        // the mark.
        for _ in 0..40 {
            if !compact::swapping(&self.db)? {
                self.refresh()?;
                let seen = self.generation;
                let found = self.index.search(vector, depth, allowlist)?;
                if generation(&self.db)? == seen && !compact::swapping(&self.db)? {
                    return Ok(found);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        self.refresh()?;
        self.index.search(vector, depth, allowlist)
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
        // A session built with a thread count other than the one in force is
        // rebuilt here, at a batch boundary, which is the only point a count
        // saved on the page can reach a run already going.
        let threads = embed::threads_in_force();
        if self.embedder.is_some() && self.embedder_threads != threads {
            self.release_embedder();
        }
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
            let (variant, _) = embed::index_variant();
            self.embedder = Some(self.model.load_variant(
                cache,
                chunk::MAX_CHARS / 2,
                self.quiet,
                threads,
                variant,
            )?);
            self.embedder_threads = threads;
            SESSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Ok(self.embedder.as_mut().unwrap())
    }

    /// Drop the embedding session, its arena and its worker threads. The next
    /// embed loads it again.
    fn release_embedder(&mut self) {
        if self.embedder.take().is_some() {
            SESSIONS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
        if self.index_session.take().is_some() {
            SESSIONS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
        self.embedder_alt = None;
        self.query_embedder = None;
        self.clip = image::Clip::default();
    }

    /// Release the session if nothing has been embedded for `idle`.
    ///
    /// ONNX Runtime's arenas grow to the largest batch a session ever ran and
    /// never shrink, so a writer that indexed once held its peak for the life
    /// of the daemon: 5 392 MB across seven stores on an 8 GB machine,
    /// measured 2026-09-23. A reload costs about a second, on a batch nobody
    /// is waiting on.
    pub fn release_if_idle(&mut self, idle: std::time::Duration) -> bool {
        match self.last_embed {
            Some(at)
                if (self.embedder.is_some() || self.index_session.is_some())
                    && at.elapsed() >= idle =>
            {
                self.release_embedder();
                true
            }
            _ => false,
        }
    }

    /// The thread count this store's session was built with, or `None` while
    /// no session is loaded.
    pub fn embedder_threads(&self) -> Option<usize> {
        self.embedder.as_ref().map(|_| self.embedder_threads)
    }

    /// Fit this store's indexes to the `MiB per store` in force, if it has
    /// changed since they were last fitted. Cheap when it has not.
    pub fn follow_budget(&mut self) {
        let now = index::budget_generation();
        if now != self.budget_generation {
            self.budget_generation = now;
            self.index.apply_budget();
            self.images.apply_budget();
        }
    }

    /// Shards of this store's text vectors in memory right now.
    pub fn resident_shards(&self) -> usize {
        self.index.resident_shards()
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
        let _lifted = priority::embedding();
        self.last_embed = Some(std::time::Instant::now());
        self.embedder()?;
        self.batches += 1;
        // A `mix` run alternates variants by batch, deterministically, so a
        // store holds both and the harness can measure what that costs.
        let (main, alt) = embed::index_variant();
        let use_alt = alt.is_some() && self.batches.is_multiple_of(2);
        let session = if let (true, Some(variant)) = (use_alt, alt) {
            if self.embedder_alt.is_none() {
                self.embedder_alt = Some(self.model.load_variant(
                    model_cache_dir()?,
                    chunk::MAX_CHARS / 2,
                    self.quiet,
                    self.embedder_threads,
                    variant,
                )?);
            }
            self.last_variant = variant.name();
            self.embedder_alt.as_mut().expect("loaded above")
        } else {
            self.last_variant = main.name();
            self.embedder.as_mut().expect("loaded above")
        };
        let mut out = session
            .embed(texts, Some(EMBED_BATCH))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        for v in &mut out {
            normalize(v);
        }
        Ok(out)
    }

    /// Hold one read transaction open, so everything this connection reads
    /// until [`Semlith::release_snapshot`] sees the store as it was now, however
    /// a writer beside it moves on. What a plan read beside its own run needs.
    pub fn pin_snapshot(&self) -> Result<()> {
        self.db.execute_batch("BEGIN")?;
        // The snapshot is taken at the first read, not at BEGIN.
        let _: i64 = self
            .db
            .query_row("SELECT COUNT(*) FROM meta", [], |r| r.get(0))?;
        Ok(())
    }

    pub fn release_snapshot(&self) {
        let _ = self.db.execute_batch("COMMIT");
    }

    /// The generated folders a person accepted, which the walk goes into.
    fn accepted_folders(&self) -> Vec<PathBuf> {
        store::acceptances(&self.db)
            .unwrap_or_default()
            .into_iter()
            .filter(|a| a.class == store::class::POLICY && a.mode != "refused")
            .map(|a| PathBuf::from(a.path))
            .collect()
    }

    /// The walk an index pass takes: [`walk`], plus the generated folders a
    /// person accepted.
    fn walk(&self, roots: &[PathBuf]) -> Walked {
        let allowed = self.accepted_folders();
        let started = std::time::Instant::now();
        let mut walked = walk_allowing(roots, &allowed, self.gitignore);
        walked.walk_ms = started.elapsed().as_millis() as u64;
        walked
    }

    /// The scan phase (2.7): what a run over `roots` would do, decided without
    /// the embedding model.
    ///
    /// Walks, stats, reads and hashes each file, sorts it into the
    /// not-indexed classes, runs the secret scan with its confidence and
    /// checks the store and the acceptances. Measured on 2026-09-25 at 0.01 to
    /// 0.05 s for this repository against 8 to 13 minutes to embed it. The
    /// hashes it took are kept, so the embed pass that follows reads an
    /// unchanged file once, not twice.
    pub fn plan(&mut self, roots: &[PathBuf]) -> Result<Plan> {
        self.plan_with(roots, &mut |_| {})
    }

    /// [`Semlith::plan`], saying how far it has got: the walk, then each file
    /// read, hashed and checked for credentials, then the rules decided.
    pub fn plan_with(
        &mut self,
        roots: &[PathBuf],
        on: &mut dyn FnMut(ScanProgress),
    ) -> Result<Plan> {
        let started = std::time::Instant::now();
        on(ScanProgress {
            phase: "walk",
            ..ScanProgress::default()
        });
        let walked = self.walk(roots);
        let total = walked.files.len() + walked.named.len();
        let bytes_total: u64 = walked
            .files
            .iter()
            .chain(&walked.named)
            .map(|p| embeddable_bytes(p.metadata().ok()))
            .sum();
        let mut said = ScanProgress {
            phase: "read",
            total,
            bytes_total,
            ..ScanProgress::default()
        };
        on(said.clone());
        let mut last_said = std::time::Instant::now();
        let mut plan = Plan::default();
        let home = crate::home::user_home().ok().map(|h| canonical(&h));
        let rechunk = store::format(&self.db)? < store::CODE_CONTEXT;
        let rescan = self.rules_outdated()?;
        let all = walked
            .named
            .into_iter()
            .map(|p| (p, false))
            .chain(walked.files.into_iter().map(|p| (p, true)));
        let not = |plan: &mut Plan, class: &str, path: &Path| {
            *plan.not_indexed.entry(class.to_string()).or_insert(0) += 1;
            let listed = plan.not_indexed_paths.entry(class.to_string()).or_default();
            if listed.len() < PLAN_PATHS {
                listed.push(path.to_string_lossy().into_owned());
            }
        };
        for dir in &walked.generated {
            plan.review.push(Review::new(
                dir.to_string_lossy().into_owned(),
                store::class::POLICY,
                "a generated or vendored folder the walk steps over".to_string(),
                Vec::new(),
                None,
            ));
            not(&mut plan, store::class::POLICY, dir);
        }
        for (path, _) in &walked.unreadable {
            not(&mut plan, store::class::UNINDEXABLE, path);
        }
        for path in &walked.credentials {
            plan.credential.push(path.to_string_lossy().into_owned());
            not(&mut plan, store::class::CREDENTIAL, path);
        }
        for (path, _) in &walked.excluded {
            not(&mut plan, store::class::EXCLUDED, path);
        }
        let boundary = self.boundary.resolved(home.as_deref());
        for (path, walked) in all {
            said.scanned += 1;
            said.bytes += embeddable_bytes(path.metadata().ok());
            said.flagged = plan.review.len() + plan.credential.len();
            if last_said.elapsed() >= std::time::Duration::from_millis(100) {
                on(said.clone());
                last_said = std::time::Instant::now();
            }
            let key = path.to_string_lossy().into_owned();
            if let Some(refusal) = boundary.refuses(&path, walked) {
                if refusal.credential {
                    plan.credential.push(key);
                    not(&mut plan, store::class::CREDENTIAL, &path);
                } else {
                    not(&mut plan, store::class::EXCLUDED, &path);
                }
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                not(&mut plan, store::class::UNINDEXABLE, &path);
                continue;
            };
            if !meta.is_file() || meta.len() == 0 {
                not(&mut plan, store::class::UNINDEXABLE, &path);
                continue;
            }
            let accepted = store::acceptance(&self.db, &key)?;
            // Kept out by a person: decided, so not offered again, and not
            // indexed until the decision is reset.
            if let Some(kept) = accepted.as_ref().filter(|a| a.mode == "refused") {
                not(&mut plan, &kept.class.clone(), &path);
                continue;
            }
            if meta.len() > chunk::MAX_FILE_BYTES
                && !accepted
                    .as_ref()
                    .is_some_and(|a| a.class == store::class::POLICY)
            {
                plan.review.push(Review::new(
                    key,
                    store::class::POLICY,
                    format!(
                        "over {} MiB ({} MiB)",
                        chunk::MAX_FILE_BYTES / (1024 * 1024),
                        meta.len() / (1024 * 1024)
                    ),
                    Vec::new(),
                    None,
                ));
                not(&mut plan, store::class::POLICY, &path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                not(&mut plan, store::class::UNINDEXABLE, &path);
                continue;
            };
            let hash = blake3::hash(&bytes).to_hex().to_string();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs() as i64);
            self.prehashed
                .insert(path.clone(), (meta.len(), mtime, hash.clone()));
            let unchanged =
                !rechunk && store::file_hash(&self.db, &key)?.as_deref() == Some(hash.as_str());
            if unchanged && !rescan {
                plan.unchanged += 1;
                continue;
            }
            if image::is_image(&path) {
                if unchanged {
                    plan.unchanged += 1;
                } else {
                    plan.embed += 1;
                    plan.images += 1;
                    plan.image_bytes += meta.len();
                }
                continue;
            }
            let text = match chunk::extract(&path, &bytes) {
                Ok(text) => text,
                Err(_) => {
                    not(&mut plan, store::class::UNINDEXABLE, &path);
                    continue;
                }
            };
            // Counted as the run will chunk it, symbols and all, so the run's
            // expected total starts where it will end.
            let count = |plan: &mut Plan, key: &str| {
                let n = planned_chunks(&path, &text);
                plan.chunks += n;
                plan.counts.insert(key.to_string(), n);
            };
            if self.boundary.allow_secrets {
                plan.embed += usize::from(!unchanged);
                plan.unchanged += usize::from(unchanged);
                if !unchanged {
                    plan.embed_bytes += meta.len();
                    count(&mut plan, &key);
                }
                continue;
            }
            let found = keyscan::scan(&key, &text);
            match keyscan::decide(&self.db, &key, &text, &found)? {
                keyscan::Decision::Index { .. } => {
                    if !found.is_empty() {
                        plan.dummies += 1;
                    }
                    if unchanged {
                        plan.unchanged += 1;
                    } else {
                        plan.embed += 1;
                        plan.embed_bytes += meta.len();
                        count(&mut plan, &key);
                        if let Some(lang) = filter::language_of_path(&key) {
                            *plan.languages.entry(lang.name.to_string()).or_insert(0) += 1;
                        }
                    }
                }
                keyscan::Decision::Refuse(rule) => {
                    // Counted for the page, in case a person accepts it; not in
                    // the total, nor in the run's per-file counts, where a file
                    // kept out would hold the expected total up until the run
                    // reached it. An accepted one is estimated by its size.
                    let n = planned_chunks(&path, &text);
                    let live: Vec<keyscan::Match> =
                        found.into_iter().filter(|m| m.dummy.is_none()).collect();
                    let confidence = live.iter().map(|m| m.confidence).max();
                    let mut review =
                        Review::new(key, store::class::CONTENT, rule, live, confidence);
                    review.chunks = Some(n);
                    plan.review.push(review);
                    not(&mut plan, store::class::CONTENT, &path);
                }
            }
        }
        said.phase = "rules";
        said.flagged = plan.review.len() + plan.credential.len();
        on(said);
        // From what this machine's lanes manage, in work units; a store's own
        // byte rate only when no lane has a figure yet.
        let images = plan.images as f64 * progress::image_units();
        let units = if accel::images_beside_text() {
            (plan.chunks as f64).max(images)
        } else {
            plan.chunks as f64 + images
        };
        plan.eta_ms = match accel::expected_rate() {
            Some(rate) => Some((units / rate * 1000.0) as u64),
            None => store::get_meta(&self.db, "embed_bytes_per_sec")?
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|r| *r > 0.0)
                .map(|rate| ((plan.embed_bytes + plan.image_bytes) as f64 / rate * 1000.0) as u64),
        };
        plan.seconds = started.elapsed().as_secs_f64();
        Ok(plan)
    }

    /// Whether this store was last swept under an older set of scan rules,
    /// so its unchanged files must be scanned again (2.7's upgrade pass).
    fn rules_outdated(&self) -> Result<bool> {
        Ok(store::get_meta(&self.db, "scan_rules")?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            < SCAN_RULES)
    }

    /// Accept one refused file, as a person, and index it at once (2.5, 2.6).
    ///
    /// `mode` is `redacted`, `as-is`, or `refused` (for a dummy-only file a
    /// person wants kept out). The fingerprints of the matches the person saw
    /// are what is remembered, never the values. A credential file cannot be
    /// accepted: the deny-list is not a review.
    pub fn accept(&mut self, path: &str, mode: &str, source: &str) -> Result<store::Acceptance> {
        let key = self.stored_key(path);
        if let Some(why) = filter::denied(Path::new(&key))
            && why != filter::Denied::Hidden
        {
            anyhow::bail!(
                "{key} is a credential file ({}) and is never accepted; `semlith index \
                 --include-secrets` is the only way to index one",
                why.reason()
            );
        }
        if !matches!(mode, "redacted" | "as-is" | "refused") {
            anyhow::bail!("mode is redacted, as-is or refused, not {mode:?}");
        }
        let row = store::refusal(&self.db, &key)?;
        // No row yet is a file a held scan offered for review before any pass
        // recorded it (0.35.0's wizard decides there): a folder or a file
        // over the cap is the policy class, anything else is content.
        let unrecorded = || {
            let at = Path::new(&key);
            if at.is_dir() || std::fs::metadata(at).is_ok_and(|m| m.len() > chunk::MAX_FILE_BYTES) {
                store::class::POLICY
            } else {
                store::class::CONTENT
            }
        };
        let class = row
            .as_ref()
            .map_or_else(unrecorded, |r| r.class.as_str())
            .to_string();
        if !(store::class::reviewable(&class) || class == store::class::DUMMY) {
            anyhow::bail!(
                "{key} is not indexable ({}); accepting cannot change that",
                class
            );
        }
        // `refused` is a person keeping the file out (0.35.0's Keep out): of a
        // dummy-only file that would otherwise be indexed, and of a content or
        // policy row so it leaves the waiting list until the decision is reset.
        if mode == "redacted" && class == store::class::POLICY {
            anyhow::bail!("{key} holds nothing to redact; index it as it is or keep it out");
        }
        let accepted = self.writing(|me| {
            let salt = store::salt(&me.db)?;
            let mut prints = Vec::new();
            let mut confidence = None;
            if class == store::class::CONTENT || class == store::class::DUMMY {
                let bytes = anyhow::Context::with_context(std::fs::read(&key), || {
                    format!("reading {key}")
                })?;
                let text = chunk::extract(Path::new(&key), &bytes)
                    .map_err(|why| anyhow::anyhow!("{key}: {}", why.as_str()))?;
                let found = keyscan::scan(&key, &text);
                for m in found.iter().filter(|m| m.dummy.is_none()) {
                    prints.push(keyscan::fingerprint(&salt, &text, m));
                }
                confidence = found.iter().map(|m| m.confidence).max();
            }
            let a = store::Acceptance {
                path: key.clone(),
                class: if class == store::class::DUMMY {
                    store::class::DUMMY.to_string()
                } else {
                    class.clone()
                },
                mode: mode.to_string(),
                fingerprints: prints,
                confidence,
                at: now(),
                source: source.to_string(),
            };
            store::accept(&me.db, &a)?;
            if mode == "refused" {
                let (gone, images) = me.evict(&key)?;
                if gone + images > 0 {
                    me.save()?;
                }
            }
            crate::ledger::acceptance(&me.db, &a, "accept")?;
            Ok(a)
        })?;
        Ok(accepted)
    }

    /// Undo an acceptance: the file leaves the store and returns to the list.
    pub fn revoke(&mut self, path: &str) -> Result<bool> {
        let key = self.stored_key(path);
        self.writing(|me| {
            let Some(a) = store::acceptance(&me.db, &key)? else {
                return Ok(false);
            };
            store::revoke(&me.db, &key)?;
            let (gone, images) = me.evict(&key)?;
            if gone + images > 0 {
                me.save()?;
            }
            crate::ledger::acceptance(&me.db, &a, "revoke")?;
            Ok(true)
        })
    }

    /// The key a path is stored under: canonical when it exists, as given
    /// when a row names it already.
    fn stored_key(&self, path: &str) -> String {
        let given = Path::new(path);
        if given.exists() {
            canonical(given).to_string_lossy().into_owned()
        } else {
            path.to_string()
        }
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

    /// [`Semlith::index_paths`] under a control another thread drives: asked
    /// once per file, it may pause the run, yield it (stop and keep what is
    /// done, reporting the rest in [`IndexReport::pending`]) or stop it. A
    /// stopped slice still commits what it embedded and lists it in
    /// [`IndexReport::written`]; one logical run is several slices, so the
    /// undo is the caller's: [`Semlith::undo_run`] with every slice's
    /// `written`, which leaves the store as it was before the run.
    ///
    /// Public for an embedder that schedules runs of its own — Semlith Cloud
    /// pauses, stops and slices them — and takes the store's lock exactly as
    /// `index_paths` does.
    pub fn index_paths_under(
        &mut self,
        roots: &[PathBuf],
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.index_walk_under(roots, control, on_file)
    }

    /// Carry on a run that yielded, from the previous slice's
    /// [`IndexReport::pending`], under the same kind of control. No walk, and
    /// no time budget of its own: the control decides when it yields again.
    pub fn index_rest_under(
        &mut self,
        files: Vec<PathBuf>,
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        // A century is "no budget" without the overflow `Duration::MAX` would
        // risk when it is added to an `Instant`.
        let unbounded = std::time::Duration::from_secs(100 * 365 * 24 * 3600);
        self.index_rest_held_under(files, unbounded, None, control, on_file)
    }

    /// Take a stopped run's files back out of the store: every path its
    /// slices listed in [`IndexReport::written`]. Takes the store's lock.
    /// Returns how many files were taken out.
    pub fn undo_run(&mut self, written: &[String]) -> Result<usize> {
        let _lock = lock::StoreLock::acquire(&self.dir)?;
        self.undo_held(written)
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
        self.index_set(
            Walk::Roots(roots.to_vec()),
            true,
            Handed::Walked(Some(deadline)),
            None,
            on_file,
        )
    }

    /// [`Semlith::index_walk`] under a control, so the catch-up a watcher runs
    /// at startup can step aside the moment a request arrives.
    pub(crate) fn index_walk_under(
        &mut self,
        roots: &[PathBuf],
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        self.index_set(
            Walk::Roots(roots.to_vec()),
            true,
            Handed::Walked(None),
            Some(control),
            on_file,
        )
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
        self.index_set(
            Walk::Roots(roots.to_vec()),
            true,
            Handed::Walked(Some(deadline)),
            Some(control),
            on_file,
        )
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
    ///
    /// `bytes_total` is the run's, when the caller already knows it; `None`
    /// has this slice measure the files it was handed.
    pub(crate) fn index_rest_held_under(
        &mut self,
        files: Vec<PathBuf>,
        budget: std::time::Duration,
        bytes_total: Option<u64>,
        control: &dyn Fn() -> Flow,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        self.index_set(
            // Every one of these came out of the walk this run started with,
            // so they are walked paths and are held to the same boundary rule
            // they were held to then — each as the slice reaches it.
            Walk::Done(Walked {
                walk_ms: 0,
                files,
                named: Vec::new(),
                unreadable: Vec::new(),
                generated: Vec::new(),
                credentials: Vec::new(),
                excluded: Vec::new(),
            }),
            true,
            Handed::Rest {
                budget,
                bytes_total,
            },
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
        self.index_set(
            Walk::Roots(roots.to_vec()),
            true,
            Handed::Walked(None),
            None,
            on_file,
        )
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
            Walk::Done(Walked {
                walk_ms: 0,
                files: paths,
                named: Vec::new(),
                unreadable: Vec::new(),
                generated: Vec::new(),
                credentials: Vec::new(),
                excluded: Vec::new(),
            }),
            false,
            Handed::Walked(None),
            None,
            on_file,
        )
    }

    /// The body both entry points share. `sweep` drops every recorded file
    /// that is no longer on disk — right for a full walk, wrong for a batch of
    /// events, which only knows about the paths in it.
    fn index_set(
        &mut self,
        walk: Walk,
        sweep: bool,
        handed: Handed,
        control: Option<&dyn Fn() -> Flow>,
        on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        // The whole pass, not only its embeds: reading, hashing and chunking
        // between batches is the run's work too, and dropping to background
        // for it would put the next batch behind the efficiency cores again.
        let _lifted = priority::embedding();
        let _class = priority::indexing_thread();
        let _writer = embed::writer();
        // Every path that writes to this store funnels through here, so this is
        // where the connection stops refusing writes — and, when this returns,
        // starts refusing them again. See `store::Writing` and `writing` below.
        self.writing(move |me| me.index_set_writing(walk, sweep, handed, control, on_file))
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
        // A pass that failed part-way leaves its open transaction committed,
        // as its autocommitted statements always were: the rows carry the
        // pending hash, which is what makes the next run redo them.
        let _ = self.tx_commit();
        let _ = store::read_only(&self.db, true);
        out
    }

    /// Report one refused file, record it with the rule that refused it, and
    /// evict what an earlier run held of it when the refusal is about what it
    /// contains.
    fn refuse_file(
        &mut self,
        report: &mut IndexReport,
        total: usize,
        path: &Path,
        refusal: &Refusal,
        on_file: &mut dyn FnMut(&Path, IndexProgress),
    ) -> Result<()> {
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
        self.scrub |= evicted > 0;
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
        let class = if refusal.credential {
            store::class::CREDENTIAL
        } else {
            store::class::EXCLUDED
        };
        store::refuse(
            &self.db,
            &path.to_string_lossy(),
            class,
            &refusal.why,
            &[],
            1,
            now(),
        )?;
        say_file(
            on_file,
            report,
            total,
            path,
            FileOutcome::Refused,
            Some(why),
        );
        Ok(())
    }

    fn index_set_writing(
        &mut self,
        walk: Walk,
        sweep: bool,
        handed: Handed,
        control: Option<&dyn Fn() -> Flow>,
        mut on_file: impl FnMut(&Path, IndexProgress),
    ) -> Result<IndexReport> {
        // A run killed mid-save leaves a temp index behind. Removing it here
        // and not on open is deliberate: the caller holds the store lock, so
        // there is no live writer whose half-written index this could be.
        self.index.clean();

        let mut report = IndexReport::default();
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
        let rescan = self.rules_outdated()?;
        let regraph = store::get_meta(&self.db, "graph_rules")?
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(0)
            < GRAPH_RULES;
        let prehashed = std::mem::take(&mut self.prehashed);
        let run_started = std::time::Instant::now();

        // Once for the run, not once for the file. The home directory and the
        // roots cannot move while a run is going, and resolving them per file
        // was an opened handle per file on Windows.
        let home = crate::home::user_home().ok().map(|h| canonical(&h));
        let boundary = self.boundary.resolved(home.as_deref());
        let (deadline, budget, bytes_known) = match handed {
            Handed::Walked(deadline) => (deadline, None, None),
            Handed::Rest {
                budget,
                bytes_total,
            } => (None, Some(budget), bytes_total),
        };
        // A continuation slice is handed everything the run has not reached,
        // and checking all of it here, every 45 seconds, is what halved the
        // speed of a large run: ~66,000 paths re-proved admissible each slice
        // before anything was embedded. Its files are held to the same rule
        // one at a time, as the loop reaches them, so a slice costs the files
        // it handles rather than the files left.
        let each = budget.is_some().then_some(&boundary);

        // A pass over roots starts on the files changed most recently while
        // the walk of the whole tree goes on beside it, so a store filling
        // from cold answers about the code being worked on within a second or
        // two rather than after a walk of every file. The library alone walks
        // first and keeps path order, which is what its figures were measured on.
        let (head, pending_walk, done_walk) = match walk {
            Walk::Done(walked) => (Vec::new(), None, Some(walked)),
            Walk::Roots(roots) => {
                let accepted = self.accepted_folders();
                let head = if accel::managed() {
                    recent_head(&roots, &boundary, &accepted)
                } else {
                    Vec::new()
                };
                (head, Some((roots, accepted)), None)
            }
        };
        let mut total;
        let mut walk_ms = 0u64;
        let first = match done_walk {
            Some(walked) => {
                walk_ms = walked.walk_ms;
                let (paths, counted) =
                    self.walk_setup(walked, &boundary, each.is_some(), &mut report, &mut on_file)?;
                total = counted;
                report.bytes_total = bytes_known.unwrap_or_else(|| bytes_of(&paths));
                let (chunks, images) = match self.expect_rest.take().filter(|_| budget.is_some()) {
                    Some(known) => known,
                    None => expected_of(&paths, self.planned.as_deref()),
                };
                report.expected_left = chunks;
                report.images_left = images;
                paths
            }
            None => {
                total = head.len();
                report.bytes_total = bytes_of(&head);
                let (chunks, images) = expected_of(&head, self.planned.as_deref());
                report.expected_left = chunks;
                report.images_left = images;
                // A reviewed run's scan counted every file already: its whole
                // total from the first second, rather than the recent head's
                // 48 files until the walk catches up (walk 3 said "about 25
                // min" for an hour's run).
                if let Some(planned) = self.planned.as_deref() {
                    report.expected_left = report.expected_left.max(planned.values().sum());
                    total = total.max(planned.len());
                }
                if let Some(images) = self.planned_images {
                    report.images_left = report.images_left.max(images);
                }
                head.clone()
            }
        };
        {
            let mut live = report.live.borrow_mut();
            live.eta.set_prior(accel::expected_rate());
            live.image_weight = progress::image_units();
            live.images_parallel = accel::images_beside_text();
        }
        let head_set: std::collections::HashSet<PathBuf> = head.iter().cloned().collect();

        // Taken here, after the setup above, so a slice's budget is spent on
        // its files rather than on getting ready to read them.
        let deadline = deadline.or_else(|| budget.map(|b| std::time::Instant::now() + b));

        // What the prepare stage reads, all of it before the first file: the
        // writer is the one thing that moves a file's hash, and it moves each
        // one at most once in a run.
        let granite = self.model == Model::Granite;
        let tokenizer = if granite && (!first.is_empty() || pending_walk.is_some()) {
            let cache = model_cache_dir()?;
            // Fetched and checked before the tokenizer beside it is read; a
            // no-op after the first time.
            embed::granite_graph(&cache, self.quiet, embed::index_variant().0)?;
            if self.tokenizer.is_none() {
                self.tokenizer = self.model.tokenizer(&cache);
            }
            Some(session::tokenizer(&cache)?)
        } else {
            None
        };
        let cache_at = if self.force {
            None
        } else if let Some(own) = &self.vector_cache {
            (own.cap > 0).then(|| own.clone())
        } else if accel::cache_in_use() {
            cache::Location::machine()
        } else {
            None
        };
        let ctx = pipeline::Context {
            rechunk,
            rescan,
            regraph,
            allow_secrets: self.boundary.allow_secrets,
            hashes: if self.force {
                Default::default()
            } else {
                store::all_hashes(&self.db)?
            },
            uncounted: store::uncounted(&self.db)?,
            over_cap: store::acceptances(&self.db)?
                .into_iter()
                .filter(|a| a.class == store::class::POLICY && a.mode != "refused")
                .map(|a| a.path)
                .collect(),
            prehashed,
            each: each.cloned(),
            tokenizer,
            clocks: pipeline::Clocks::default(),
            cache_at: cache_at.clone(),
            cache: cache_at.as_ref().map(|_| cache::Scope::of(&self.model)),
            variants: accel::cache_variants(),
            lookups: Default::default(),
            hits: Default::default(),
        };
        let mut cached = CacheWrites::default();
        let mut clock = pipeline::WriterClock::start(walk_ms);
        self.write_parts.take();
        self.awaiting.clear();
        self.file_of.clear();
        self.landed.clear();
        let sealed = pending_walk.is_none();
        let mut run_lanes: std::collections::BTreeMap<String, usize> = Default::default();
        let (cpu_back, cpu_returned) = std::sync::mpsc::channel();
        let prepare_threads = pipeline::prepare_threads();
        let gitignore = self.gitignore;

        std::thread::scope(|scope| -> Result<()> {
            let prefetch = pipeline::Prefetch::start(scope, first, sealed, &ctx, prepare_threads);
            // The image lane (#203): the image model on a thread of its own,
            // started at the first image, so the text lanes keep embedding
            // while it works. It was the writer's own call, and on the owner's
            // walk a stretch of screenshots left the Neural Engine idle for
            // minutes. The writer records each image when its vector returns.
            let (image_send, image_jobs) = std::sync::mpsc::channel::<ImageJob>();
            let (image_back, image_done) = std::sync::mpsc::channel::<ImageDone>();
            let mut image_send = Some(image_send);
            let mut image_jobs = Some(image_jobs);
            let mut image_lane: Option<std::thread::ScopedJoinHandle<'_, image::Clip>> = None;
            let mut clip = Some(std::mem::take(&mut self.clip));
            let quiet = self.quiet;
            let mut images_out = 0usize;
            let mut image_bytes_out = 0u64;
            // The walk of the whole tree, when the pass started on its head.
            let mut walker = pending_walk.map(|(roots, accepted)| {
                scope.spawn(move || {
                    let started = std::time::Instant::now();
                    let mut walked = walk_allowing(&roots, &accepted, gitignore);
                    walked.walk_ms = started.elapsed().as_millis() as u64;
                    walked
                })
            });
            let mut stage: Option<pipeline::Embedder> = None;
            let mut window = pipeline::Window {
                ids: Vec::new(),
                pieces: Vec::new(),
                hashes: Vec::new(),
            };
            let paused = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            // Asked whenever the writer waits on the embed stage: a pause
            // stops new batches at once, a stop gives up. `false` for a stop.
            let ask = |paused: &std::sync::atomic::AtomicBool| -> bool {
                let Some(ask) = control else { return true };
                match ask() {
                    Flow::Pause => {
                        paused.store(true, std::sync::atomic::Ordering::Relaxed);
                        std::thread::sleep(PAUSE_TICK);
                        true
                    }
                    Flow::Stop => false,
                    Flow::Run | Flow::Yield => {
                        paused.store(false, std::sync::atomic::Ordering::Relaxed);
                        true
                    }
                }
            };

            let mut committed = std::time::Instant::now();
            // Finish the walk if it is still going, hold its files to the
            // run's rules, and put what the head did not cover on the list.
            // Asked when the head runs out, and before a slice hands its
            // remainder on.
            let finish_walk = |me: &mut Self,
                               walker: &mut Option<std::thread::ScopedJoinHandle<'_, Walked>>,
                               report: &mut IndexReport,
                               total: &mut usize,
                               clock: &mut pipeline::WriterClock,
                               on_file: &mut dyn FnMut(&Path, IndexProgress)|
             -> Result<()> {
                let Some(handle) = walker.take() else {
                    return Ok(());
                };
                let waiting = std::time::Instant::now();
                let walked = handle
                    .join()
                    .map_err(|_| anyhow::anyhow!("the walk of the tree panicked"))?;
                clock.waited_on_walk(waiting);
                let (rest, counted) = me.walk_setup(walked, &boundary, false, report, on_file)?;
                let rest: Vec<PathBuf> =
                    rest.into_iter().filter(|p| !head_set.contains(p)).collect();
                *total = head_set.len() + counted.saturating_sub(head_set.len());
                report.bytes_total += bytes_of(&rest);
                let (chunks, images) = expected_of(&rest, me.planned.as_deref());
                // With a plan the head already stood for its whole total.
                if me.planned.is_none() {
                    report.expected_left += chunks;
                }
                // Likewise its images, once the plan has counted them.
                if me.planned_images.is_none() {
                    report.images_left += images;
                }
                prefetch.extend_and_seal(rest);
                if report.live.borrow().phase == progress::Phase::Walk {
                    say_phase(
                        on_file,
                        report,
                        *total,
                        progress::Phase::Read,
                        Some(format!("{} files found", *total)),
                    );
                }
                Ok(())
            };
            let mut next = 0usize;
            if walker.is_some() {
                say_phase(
                    &mut on_file,
                    &report,
                    total,
                    progress::Phase::Walk,
                    Some(if head_set.is_empty() {
                        "finding the files under the roots".to_string()
                    } else {
                        "finding the files; reading the most recently changed first".to_string()
                    }),
                );
            }
            loop {
                // Embedding has begun once the embed stage exists.
                if stage.is_some()
                    && matches!(
                        report.live.borrow().phase,
                        progress::Phase::Read | progress::Phase::Walk
                    )
                    && walker.is_none()
                {
                    say_phase(&mut on_file, &report, total, progress::Phase::Embed, None);
                }
                let seen = next;
                let (listed, complete) = prefetch.len();
                if seen >= listed {
                    if complete {
                        break;
                    }
                    finish_walk(
                        self,
                        &mut walker,
                        &mut report,
                        &mut total,
                        &mut clock,
                        &mut on_file,
                    )?;
                    continue;
                }
                next += 1;
                let path = prefetch.path(seen).expect("inside the list");
                // Only ever after something was embedded: a budget too small for
                // any work at all must still make progress, or calling again is
                // the same call forever.
                if let Some(deadline) = deadline
                    && report.indexed > 0
                    && std::time::Instant::now() >= deadline
                {
                    finish_walk(
                        self,
                        &mut walker,
                        &mut report,
                        &mut total,
                        &mut clock,
                        &mut on_file,
                    )?;
                    report.pending = prefetch.from(seen);
                    report.remaining = report.pending.len();
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
                            Flow::Pause => {
                                paused.store(true, std::sync::atomic::Ordering::Relaxed);
                                std::thread::sleep(PAUSE_TICK)
                            }
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
                    paused.store(false, std::sync::atomic::Ordering::Relaxed);
                    if yielded {
                        finish_walk(
                            self,
                            &mut walker,
                            &mut report,
                            &mut total,
                            &mut clock,
                            &mut on_file,
                        )?;
                        report.pending = prefetch.from(seen);
                        report.remaining = report.pending.len();
                        break;
                    }
                    if stop {
                        report.remaining = prefetch.len().0 - seen;
                        report.stopped = true;
                        break;
                    }
                }
                // Whatever the image lane has finished, into the store.
                while let Ok(done) = image_done.try_recv() {
                    images_out -= 1;
                    report.images_flight = images_out;
                    image_bytes_out = image_bytes_out.saturating_sub(done.job.bytes.len() as u64);
                    if let Some((failed_path, why)) =
                        self.land_image(done, &mut report, &mut written, &mut completed)?
                    {
                        say_file(
                            &mut on_file,
                            &report,
                            total,
                            &failed_path,
                            FileOutcome::Failed,
                            Some(why),
                        );
                    }
                }
                // Whatever the embed stage has finished, into the index.
                if let Some(stage) = stage.as_mut() {
                    while let Some(done) = stage.next(std::time::Duration::ZERO) {
                        let done = done.map_err(anyhow::Error::msg)?;
                        self.land(done, &mut report, &mut run_lanes, &mut cached)?;
                        say_file(
                            &mut on_file,
                            &report,
                            total,
                            &path,
                            FileOutcome::Progress,
                            None,
                        );
                    }
                }

                let waiting = std::time::Instant::now();
                let prepared = prefetch.take_ticking(seen, &mut || {
                    // Said every two seconds the pool keeps this file: which
                    // one, and how big, rather than a card that stands still.
                    let size = embeddable_bytes(path.metadata().ok());
                    let name = path.file_name().map_or_else(
                        || path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    report.live.borrow_mut().detail = Some(format!(
                        "reading and chunking {name} ({})",
                        human_bytes(size as i64)
                    ));
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &path,
                        FileOutcome::Progress,
                        None,
                    );
                });
                clock.waited_on_prepare(waiting);
                {
                    let mut live = report.live.borrow_mut();
                    if live
                        .detail
                        .as_deref()
                        .is_some_and(|d| d.starts_with("reading and chunking"))
                    {
                        live.detail = None;
                    }
                }
                // This file's estimate leaves the expected total as its rows
                // arrive (`current_est`); an image's arrives as an image.
                let (estimated, image) = expected_one(&path, self.planned.as_deref());
                report.expected_left = report.expected_left.saturating_sub(estimated);
                report.current_est = estimated;
                report.current_rows = 0;
                if image {
                    report.images_left = report.images_left.saturating_sub(1);
                } else {
                    let mut live = report.live.borrow_mut();
                    if live
                        .detail
                        .as_deref()
                        .is_some_and(|d| d.starts_with("embedding images"))
                    {
                        live.detail = None;
                    }
                }

                let key = path.to_string_lossy().into_owned();
                if let pipeline::Prepared::Refused(refusal) = &prepared {
                    self.refuse_file(&mut report, total, &path, refusal, &mut on_file)?;
                    continue;
                }
                report.scanned += 1;
                let bytes_before = report.bytes;
                // Named, not just detected. Every one of these used to be the
                // same silent `skipped`, and a person looking at two thousand of
                // them could not tell an empty `__init__.py` from a file the
                // operating system would not open.
                let (hash, len, text, units, found, ready) = match prepared {
                    pipeline::Prepared::Refused(_) => unreachable!("handled above"),
                    // Taken by the scan phase a moment ago and unchanged since:
                    // the store already holds these bytes, so they were not read
                    // again.
                    pipeline::Prepared::Unchanged {
                        size,
                        mtime,
                        file_bytes,
                    } => {
                        report.bytes += file_bytes;
                        self.tx_begin()?;
                        store::heal_stamps(&self.db, &key, size as i64, mtime, now())?;
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
                    pipeline::Prepared::Unusable {
                        why,
                        gone,
                        file_bytes,
                    } => {
                        report.bytes += file_bytes;
                        self.tx_begin()?;
                        // A batch of events can name a file that has just been
                        // deleted or renamed away. Evicting it here is what makes
                        // a deletion visible without a full sweep. A file still
                        // there but no longer usable -- emptied, grown past the
                        // cap, unreadable -- is evicted too: its old chunks
                        // would go on answering for text no longer on disk.
                        // Through `evict`, so an image's vectors leave the
                        // image index with its rows.
                        let (chunks, images) = self.evict(&key)?;
                        if chunks > 0 || images > 0 {
                            report.removed += 1;
                            if gone {
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
                        store::refuse(&self.db, &key, why.class(), &why.as_str(), &[], 1, now())?;
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
                    pipeline::Prepared::Same {
                        len,
                        mtime,
                        file_bytes,
                        rescan: rescanned,
                        regraph: regraphed,
                        units,
                    } => {
                        report.bytes += file_bytes;
                        self.tx_begin()?;
                        // A file an older binary stored, counted now: the
                        // count only — its chunks and vectors are as current
                        // as its bytes.
                        if let Some(units) = units {
                            store::set_units_by_path(&self.db, &key, units)?;
                        }
                        // The upgrade pass (2.7): an unchanged file is scanned
                        // again under the new rules, and one they now refuse
                        // leaves the store.
                        if let Some((text, found)) = rescanned
                            && let keyscan::Decision::Refuse(why) =
                                keyscan::decide(&self.db, &key, &text, &found)?
                        {
                            let live: Vec<keyscan::Match> =
                                found.into_iter().filter(|m| m.dummy.is_none()).collect();
                            store::refuse(
                                &self.db,
                                &key,
                                store::class::CONTENT,
                                &why,
                                &live,
                                1,
                                now(),
                            )?;
                            let (gone, images) = self.evict(&key)?;
                            report.removed += usize::from(gone + images > 0);
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
                        // The graph half of the upgrade pass: an unchanged file's
                        // symbols and edges written again under this release's
                        // extractor.
                        if let Some(extraction) = regraphed
                            && let Some((file_id, spans)) = store::graph_input(&self.db, &key)?
                        {
                            store::delete_graph(&self.db, file_id)?;
                            let (symbols, edges) =
                                self.write_graph(Some(extraction), file_id, &spans)?;
                            report.symbols += symbols;
                            report.edges += edges;
                        }
                        // Same bytes, but `git checkout` gave the file a new
                        // mtime, and every search hit in it read as stale from
                        // then on. The row's stamps catch up with the file;
                        // nothing is re-embedded, and the freshness rule itself
                        // stays as conservative as it was.
                        if let Some(mtime) = mtime {
                            store::heal_stamps(&self.db, &key, len as i64, mtime, now())?;
                        }
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
                    // An image is a different kind of content in the same pass:
                    // read for its pixels rather than for its text, embedded with
                    // CLIP's vision encoder, and recorded in the store's second
                    // vector space.
                    pipeline::Prepared::Image {
                        bytes,
                        hash,
                        file_bytes,
                    } => {
                        report.bytes += file_bytes;
                        self.tx_begin()?;
                        // Before the decoder sees it. A header claiming 60,000 by
                        // 60,000 pixels is a few hundred bytes on disk and
                        // fourteen gigabytes in memory, and the refusal says which
                        // file and how big it claimed to be.
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
                            store::refuse(
                                &self.db,
                                &key,
                                why.class(),
                                &why.as_str(),
                                &[],
                                1,
                                now(),
                            )?;
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
                            FileOutcome::Image,
                            None,
                        );
                        if image_lane.is_none()
                            && let (Some(jobs), Some(mut model)) = (image_jobs.take(), clip.take())
                        {
                            let back = image_back.clone();
                            image_lane = Some(scope.spawn(move || {
                                for job in jobs {
                                    crate::cpucap::pace();
                                    let vector = model
                                        .embed_image(&job.path, &job.bytes, quiet)
                                        .map_err(|e| format!("{e:#}"));
                                    if back.send(ImageDone { job, vector }).is_err() {
                                        break;
                                    }
                                }
                                model
                            }));
                        }
                        // Held back while the lane is far behind, landing what
                        // it has finished meanwhile.
                        while image_bytes_out > IMAGE_BYTES_IN_FLIGHT && images_out > 0 {
                            let Ok(done) = image_done.recv() else { break };
                            images_out -= 1;
                            report.images_flight = images_out;
                            image_bytes_out =
                                image_bytes_out.saturating_sub(done.job.bytes.len() as u64);
                            if let Some((path, why)) =
                                self.land_image(done, &mut report, &mut written, &mut completed)?
                            {
                                say_file(
                                    &mut on_file,
                                    &report,
                                    total,
                                    &path,
                                    FileOutcome::Failed,
                                    Some(why),
                                );
                            }
                        }
                        image_bytes_out += bytes.len() as u64;
                        images_out += 1;
                        report.images_flight = images_out;
                        if let Some(send) = &image_send {
                            let _ = send.send(ImageJob {
                                path: path.clone(),
                                key: key.clone(),
                                bytes,
                                hash,
                                width,
                                height,
                            });
                        }
                        continue;
                    }
                    // A reader that panics on one file's bytes is that file's
                    // failure. Before 0.28.0 the panic unwound the store's
                    // writer thread, the store stopped being kept current, and
                    // `/api/stores` went on saying it was watched.
                    pipeline::Prepared::Failed { error, file_bytes } => {
                        report.bytes += file_bytes;
                        failed(&mut report, &path, &error);
                        say_file(
                            &mut on_file,
                            &report,
                            total,
                            &path,
                            FileOutcome::Failed,
                            Some(format!("{error:#}")),
                        );
                        continue;
                    }
                    // This branch reported nothing at all before 0.19.0 — no
                    // event, no reason, no counter movement — so a tree of
                    // binaries left the portal's progress bar short of its own
                    // total with no line saying why.
                    pipeline::Prepared::Skipped { why, file_bytes } => {
                        report.bytes += file_bytes;
                        self.tx_begin()?;
                        skip(&mut report, &why);
                        store::refuse(&self.db, &key, why.class(), &why.as_str(), &[], 1, now())?;
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
                    pipeline::Prepared::Text {
                        len,
                        hash,
                        file_bytes,
                        text,
                        units,
                        found,
                        ready,
                    } => {
                        report.bytes += file_bytes;
                        (hash, len, text, units, found, ready)
                    }
                };
                let file_bytes = report.bytes - bytes_before;
                self.tx_begin()?;

                // Before a single chunk, a single row or a single vector. The
                // scan is the one rule that cannot be decided from a file's
                // name, so it is decided from the text a reader produced —
                // which is also what catches an AWS key sitting in the body of a
                // `.docx`. Images never reach this line; they were never text.
                // The prepare stage scanned; this is where it is decided.
                let decided = if self.boundary.allow_secrets {
                    // Counted, not hidden. `--include-secrets` is the user
                    // saying they meant it, not semlith agreeing it is fine.
                    if keyscan::refuses(&found) {
                        report.secrets_indexed += 1;
                    }
                    store::unrefuse(&self.db, &key)?;
                    None
                } else {
                    match keyscan::decide(&self.db, &key, &text, &found)? {
                        keyscan::Decision::Index {
                            text: decided,
                            accepted,
                        } => {
                            if accepted {
                                report.accepted_indexed += 1;
                                store::unrefuse(&self.db, &key)?;
                            } else if !found.is_empty() {
                                // Every match a declared test dummy: indexed, and
                                // listed so a person can see what was let through.
                                report.dummies_indexed += 1;
                                store::refuse(
                                    &self.db,
                                    &key,
                                    store::class::DUMMY,
                                    "every match is a declared test dummy",
                                    &found,
                                    1,
                                    now(),
                                )?;
                            } else {
                                store::unrefuse(&self.db, &key)?;
                            }
                            (decided != text).then_some(decided)
                        }
                        keyscan::Decision::Refuse(why) => {
                            let live: Vec<keyscan::Match> = found
                                .iter()
                                .filter(|m| m.dummy.is_none())
                                .cloned()
                                .collect();
                            store::refuse(
                                &self.db,
                                &key,
                                store::class::CONTENT,
                                &why,
                                if live.is_empty() { &found } else { &live },
                                1,
                                now(),
                            )?;
                            // A file that held no credential when it was indexed
                            // and holds one now leaves the store on this run.
                            let (gone, images) = self.evict(&key)?;
                            let evicted = gone + images;
                            report.removed += usize::from(evicted > 0);
                            let why = if evicted > 0 {
                                format!(
                                    "{why}. Its earlier contents have been removed from this store."
                                )
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
                    }
                };
                // Parsed before a single row is written, which is the whole
                // reason this sits above the inserts: tree-sitter failing on
                // this file's own bytes is the text path's one file-shaped
                // failure, and catching it before the rows exist means there is
                // nothing to undo and no window is left holding an id whose row
                // was rolled back. The prepare stage did it already unless the
                // text the store may hold is not the text it read.
                let ready = match (decided, ready) {
                    (Some(redacted), _) => pipeline::take_ready(
                        &path,
                        &redacted,
                        &ctx,
                        None,
                        std::time::Instant::now(),
                    ),
                    (None, Some(ready)) => ready,
                    (None, None) => {
                        pipeline::take_ready(&path, &text, &ctx, None, std::time::Instant::now())
                    }
                };
                drop(text);
                let pipeline::Ready {
                    extraction,
                    chunks,
                    pieces,
                    hashes,
                } = match ready {
                    Ok(ready) => ready,
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
                if chunks.is_empty() {
                    let why = SkipReason::NoText;
                    skip(&mut report, &why);
                    store::refuse(&self.db, &key, why.class(), &why.as_str(), &[], 1, now())?;
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

                // Not yet through it: its bytes are counted as its chunks embed.
                let file_end = report.bytes;
                report.bytes = bytes_before;
                // Said with what reading made of it, so the log reads "12
                // chunks" against the file rather than only that it started.
                say_file(
                    &mut on_file,
                    &report,
                    total,
                    &path,
                    FileOutcome::Indexing,
                    Some(match chunks.len() {
                        1 => "1 chunk".to_string(),
                        n => format!("{n} chunks"),
                    }),
                );

                // Replacing a file: evict its old vectors before adding new ones.
                let timed = std::time::Instant::now();
                for id in store::delete_file(&self.db, &key, now())? {
                    self.index.remove(id)?;
                }
                self.write_parts.add("evict", timed);

                let timed = std::time::Instant::now();
                let file_id = store::insert_file(&self.db, &key, PENDING, len, now())?;
                // Written down the moment it has a row, not when it is finished:
                // a stop can now land inside a file, and the undo has to take the
                // half-embedded file out along with the finished ones.
                written.push(key.clone());
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
                if let Some(units) = units {
                    store::set_file_units(&self.db, file_id, units)?;
                }
                self.write_parts.add("rows", timed);
                let mut spans: Vec<(u32, u32, i64)> = Vec::with_capacity(chunks.len());
                let mut halted = false;
                let count = chunks.len();
                self.awaiting.insert(file_id, (0, hash.clone(), false));
                for (((ord, c), piece), hash) in chunks.iter().enumerate().zip(pieces).zip(hashes) {
                    let timed = std::time::Instant::now();
                    let id = store::insert_chunk(
                        &self.db,
                        file_id,
                        ord,
                        c.start_line,
                        c.end_line,
                        &c.text,
                    )?;
                    self.write_parts.add("rows", timed);
                    spans.push((c.start_line, c.end_line, id));
                    self.file_of.insert(id as u64, file_id);
                    if let Some(left) = self.awaiting.get_mut(&file_id) {
                        left.0 += 1;
                    }
                    report.rows += 1;
                    report.current_rows += 1;
                    window.ids.push(id as u64);
                    window.pieces.push(piece);
                    window.hashes.push(hash);

                    // Handed over by the window, not per file. One 8 MB file
                    // chunks into thousands of pieces, and holding them all makes
                    // peak memory a function of the largest file in the corpus
                    // rather than of the window.
                    if window.ids.len() >= SORT_WINDOW {
                        report.bytes = bytes_before + file_bytes * (ord as u64 + 1) / count as u64;
                        if !self.hand_over(
                            scope,
                            &mut stage,
                            &mut window,
                            &cpu_back,
                            &paused,
                            &ask,
                            &mut clock,
                            &mut |n| {
                                report.batched = n;
                                say_file(
                                    &mut on_file,
                                    &report,
                                    total,
                                    &path,
                                    FileOutcome::Progress,
                                    None,
                                )
                            },
                        )? {
                            halted = true;
                            break;
                        }
                        // The hand-over committed, and the rest of this file —
                        // its remaining rows and its whole graph — went on
                        // without a transaction: a commit, a WAL write and
                        // often a checkpoint per row (#207). The larger the
                        // file, the likelier it crosses a window, and the more
                        // symbols and edges it carries.
                        self.tx_begin()?;
                    }
                }
                report.threads = self.index_threads();
                report.lanes = self.lane_chunks.clone();
                report.bytes = file_end;
                if halted {
                    // Stopped inside this file. Its rows are in `written`, so the
                    // caller's undo takes it out with everything else; nothing of
                    // it is finished, so nothing of it is counted or committed.
                    report.remaining = prefetch.len().0 - seen;
                    report.stopped = true;
                    break;
                }

                // The structure half, on the same changed-file path and inside
                // the same lock. A file whose symbols were extracted by an
                // earlier run had them deleted by `delete_file` above, along with
                // its chunks and the edges leaving them, so this writes a whole
                // fresh set rather than reconciling one.
                let timed = std::time::Instant::now();
                let (symbols, edges) = self.write_graph(extraction, file_id, &spans)?;
                self.write_parts.add("graph", timed);
                report.symbols += symbols;
                report.edges += edges;

                // Committed once its last vector lands (see `land`), not now:
                // its rows are written, its vectors may still be with the lanes.
                // A file with no chunks, or whose last window already landed,
                // is done here.
                if let Some(entry) = self.awaiting.get_mut(&file_id) {
                    entry.2 = true;
                    if entry.0 == 0
                        && let Some((_, hash, _)) = self.awaiting.remove(&file_id)
                    {
                        self.landed.push((file_id, hash));
                    }
                }
                report.indexed += 1;
                report.chunks += count;
                // Written in full: its rows stand for it now.
                report.current_est = 0;
                report.current_rows = 0;
                // Rows reach readers when they are committed, and a window can
                // take seconds to fill on a store of small files. Committed at
                // least this often, so a store filling from cold answers
                // keyword and graph questions about what it has read so far.
                if committed.elapsed() >= ROW_COMMIT {
                    self.tx_commit()?;
                    committed = std::time::Instant::now();
                }

                // Between files, never inside one: a file half-written into the
                // index is a file whose hash must not be committed, and this is
                // the one point in the loop where that cannot be true.
                //
                // Counted, not timed. See `CHECKPOINT_FILES`: a checkpoint lands
                // on the same file every run, so a run's windows are the same
                // windows every time and so are its vectors.
                since_checkpoint += 1;
                if checkpointing && since_checkpoint >= every {
                    // No drain (#203): the windows in flight carry on, the index
                    // is written with every vector that has landed, and only the
                    // files whose every chunk has landed are recorded as indexed.
                    // A file still embedding stays pending, which is what a
                    // crash here must leave it as. Waiting for every window cost
                    // the owner's walk a minute per checkpoint. The window being
                    // filled is handed over now, not waited for, so the chunks
                    // of a small corpus reach a lane before the run ends.
                    if !window.ids.is_empty()
                        && !self.hand_over(
                            scope,
                            &mut stage,
                            &mut window,
                            &cpu_back,
                            &paused,
                            &ask,
                            &mut clock,
                            &mut |n| {
                                report.batched = n;
                                say_file(
                                    &mut on_file,
                                    &report,
                                    total,
                                    &path,
                                    FileOutcome::Progress,
                                    None,
                                )
                            },
                        )?
                    {
                        report.remaining = prefetch.len().0 - seen - 1;
                        report.stopped = true;
                        break;
                    }
                    // What has landed meanwhile, into the index before it is
                    // written.
                    if let Some(stage) = stage.as_mut() {
                        while let Some(done) = stage.next(std::time::Duration::ZERO) {
                            self.land(
                                done.map_err(anyhow::Error::msg)?,
                                &mut report,
                                &mut run_lanes,
                                &mut cached,
                            )?;
                        }
                    }
                    say_phase(
                        &mut on_file,
                        &report,
                        total,
                        progress::Phase::Save,
                        Some(format!(
                            "checkpoint: writing the index to disk ({} chunks still embedding carry on)",
                            progress::grouped(report.rows.saturating_sub(report.embedded) as u64)
                        )),
                    );
                    completed.append(&mut self.landed);
                    self.checkpoint(&mut completed)?;
                    since_checkpoint = 0;
                    say_phase(&mut on_file, &report, total, progress::Phase::Embed, None);
                }
            }
            let last = prefetch
                .path(prefetch.len().0.saturating_sub(1))
                .unwrap_or_default();
            drop(prefetch);

            if !report.stopped {
                if report.rows > report.embedded || !window.ids.is_empty() {
                    say_phase(
                        &mut on_file,
                        &report,
                        total,
                        progress::Phase::Drain,
                        Some(format!(
                            "{} chunks still embedding",
                            // The window's chunks are rows already; adding it
                            // counted them twice (3,918 of an expected 3,879).
                            progress::grouped(report.rows.saturating_sub(report.embedded) as u64)
                        )),
                    );
                }
                let drained = self.hand_over(
                    scope,
                    &mut stage,
                    &mut window,
                    &cpu_back,
                    &paused,
                    &ask,
                    &mut clock,
                    &mut |n| {
                        report.batched = n;
                        say_file(
                            &mut on_file,
                            &report,
                            total,
                            &last,
                            FileOutcome::Progress,
                            None,
                        )
                    },
                )? && self.drain(
                    &mut stage,
                    &mut report,
                    &mut run_lanes,
                    &mut cached,
                    &paused,
                    &ask,
                    &mut clock,
                    &mut |report: &IndexReport| {
                        say_file(
                            &mut on_file,
                            report,
                            total,
                            &last,
                            FileOutcome::Progress,
                            None,
                        )
                    },
                )?;
                if !drained {
                    report.stopped = true;
                }
            }
            // The image lane finishes what it was given; each lands, and on a
            // stop the undo takes them out with the rest of the run's files.
            image_send = None;
            if images_out > 0 {
                say_phase(
                    &mut on_file,
                    &report,
                    total,
                    progress::Phase::Drain,
                    Some(format!(
                        "{} images still with the image model",
                        progress::grouped(images_out as u64)
                    )),
                );
            }
            while images_out > 0 {
                let Ok(done) = image_done.recv() else { break };
                images_out -= 1;
                report.images_flight = images_out;
                if let Some((failed_path, why)) =
                    self.land_image(done, &mut report, &mut written, &mut completed)?
                {
                    say_file(
                        &mut on_file,
                        &report,
                        total,
                        &failed_path,
                        FileOutcome::Failed,
                        Some(why),
                    );
                }
            }
            drop(image_send);
            drop(image_back);
            match image_lane.take() {
                Some(lane) => {
                    if let Ok(model) = lane.join() {
                        self.clip = model;
                    }
                }
                None => {
                    if let Some(model) = clip.take() {
                        self.clip = model;
                    }
                }
            }
            // A stop gives up on what is in flight: those windows' rows are
            // the undo's to remove, and none of their vectors may land.
            if let Some(stage) = stage.as_mut() {
                if report.stopped {
                    stage.cancel();
                } else {
                    stage.close();
                }
            }
            drop(stage);
            drop(cpu_back);
            Ok(())
        })?;
        // The CPU session outlives the run, as the fastembed one always did:
        // a store indexing slice after slice loads it once.
        if let Ok(work) = cpu_returned.try_recv() {
            if let pipeline::CpuWork::Ids { main, .. } = &work {
                self.index_threads = main.threads();
            }
            self.index_session = Some(work);
        }
        self.tx_commit()?;
        report.stages = clock.stages(&ctx.clocks, &run_lanes);
        let tail = std::time::Instant::now();
        report.threads = self.index_threads();
        report.cache_lookups = ctx.lookups.load(std::sync::atomic::Ordering::Relaxed) as usize;
        report.cache_hits = ctx.hits.load(std::sync::atomic::Ordering::Relaxed) as usize;
        // What this call embedded goes into the run's cache, and what it
        // took out is marked used. A cache that cannot be written is a cache
        // that misses next time, never a failed run.
        if let Some(scope) = &ctx.cache
            && report.cache_lookups > 0
            && let Some(mut cache) = ctx.cache_at.as_ref().and_then(cache::Cache::open_in)
        {
            let fresh: Vec<([u8; 32], &'static str, &[f32])> = cached
                .fresh
                .iter()
                .map(|(hash, variant, vector)| {
                    (scope.key(hash, variant), *variant, vector.as_slice())
                })
                .collect();
            let hits: Vec<[u8; 32]> = cached
                .hits
                .iter()
                .map(|(hash, variant)| scope.key(hash, variant))
                .collect();
            let timed = std::time::Instant::now();
            let _ = cache.record(&fresh, &hits, report.cache_lookups as u64);
            self.write_parts.add("cache", timed);
        }

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
            say_phase(
                &mut on_file,
                &report,
                total,
                progress::Phase::Finalize,
                Some("dropping files gone from disk".to_string()),
            );
            for key in store::all_paths(&self.db)? {
                if !Path::new(&key).exists() {
                    for id in store::delete_file(&self.db, &key, now())? {
                        self.index.remove(id)?;
                    }
                    report.removed += 1;
                }
            }
            // A not-indexed row for something no longer on disk says nothing.
            for row in store::refusals(&self.db)? {
                if !Path::new(&row.path).exists() {
                    store::unrefuse(&self.db, &row.path)?;
                }
            }
            if rescan {
                store::set_meta(&self.db, "scan_rules", &SCAN_RULES.to_string())?;
            }
            if regraph {
                store::set_meta(&self.db, "graph_rules", &GRAPH_RULES.to_string())?;
            }
        }

        // A batch that changed nothing must not rewrite index.tv. A watcher
        // sees plenty of events on files whose bytes are identical, and each
        // rewrite is the whole index.
        if report.indexed > 0 || report.removed > 0 || !self.index.exists() {
            say_phase(
                &mut on_file,
                &report,
                total,
                progress::Phase::Save,
                Some("writing the index to disk".to_string()),
            );
            let timed = std::time::Instant::now();
            self.save()?;
            self.write_parts.add("save", timed);
        }

        // Every window has landed or been given up by now: the files whose
        // vectors all arrived are indexed; any left waiting were stopped mid-
        // embed and stay pending for the undo or the next run.
        completed.append(&mut self.landed);
        self.commit_hashes(&mut completed)?;
        // The cache write, the sweep and the last save come after the clock
        // settled, and are the writer's too.
        let tail = tail.elapsed().as_millis() as u64;
        report.stages.wall_ms += tail;
        report.stages.write_ms += tail;
        report.stages.write_parts_ms = self.write_parts.take();

        // The byte rate this store embeds at, for the next scan phase's
        // estimate: a plan can say how long before the model has loaded.
        // A daemon run arrives in slices of a few hundred kilobytes, so any
        // slice of real size counts, averaged with what was known: a floor of
        // one megabyte meant a store built from the portal never had a rate.
        let secs = run_started.elapsed().as_secs_f64();
        if report.indexed > 0 && report.bytes >= 64 * 1024 && secs >= 0.25 {
            let sample = report.bytes as f64 / secs;
            let rate = match store::get_meta(&self.db, "embed_bytes_per_sec")?
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|r| *r > 0.0)
            {
                Some(known) => (known + sample) / 2.0,
                None => sample,
            };
            store::set_meta(&self.db, "embed_bytes_per_sec", &format!("{rate:.0}"))?;
        }

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

        // A file refused for what it now contains had its rows deleted, and
        // `secure_delete` zeroed their pages; the keyword index still carries
        // its terms until its segments merge, and the write-ahead log the
        // pages as they were. Rare, so paid in full here.
        if std::mem::take(&mut self.scrub) {
            store::reclaim(&self.db)?;
        }

        Ok(report)
    }

    /// What a walk found, held to this run's rules: every refusal, exclusion,
    /// credential, generated folder and unreadable entry recorded and reported,
    /// and the paths that remain in the order the run will take them. Returns
    /// them and the walk's file count for progress. A continuation (`each`)
    /// is handed its paths already decided, and holds each file to the rules
    /// as it reaches it instead.
    fn walk_setup(
        &mut self,
        walked: Walked,
        boundary: &ResolvedBoundary,
        each: bool,
        report: &mut IndexReport,
        on_file: &mut dyn FnMut(&Path, IndexProgress),
    ) -> Result<(Vec<PathBuf>, usize)> {
        // Refused before anything is read. A path that names a credential or
        // sits outside this caller's boundary is reported by name with the rule
        // that refused it, rather than dropped from the walk — an agent that
        // asked for a file and got silence cannot tell that from a file that
        // was not there.
        let Walked {
            walk_ms: _,
            files: walked_paths,
            named,
            unreadable: unwalkable,
            generated,
            credentials: hidden_credentials,
            excluded,
        } = walked;
        // A large tree refuses thousands of paths: their rows go in one
        // transaction, not a commit each.
        self.tx_begin()?;
        let (paths, refused): (Vec<PathBuf>, Vec<(PathBuf, Refusal)>) = if each {
            // A continuation is handed the rest of the run in the order the
            // run chose on its first slice; sorting it again would undo that.
            (walked_paths, Vec::new())
        } else {
            let mut allowed = Vec::with_capacity(walked_paths.len() + named.len());
            let mut refused = Vec::new();
            let all = named
                .into_iter()
                .map(|p| (p, false))
                .chain(walked_paths.into_iter().map(|p| (p, true)));
            for (path, walked) in all {
                match boundary.refuses(&path, walked) {
                    Some(why) => refused.push((path, why)),
                    None => allowed.push(path),
                }
            }
            allowed.sort();
            // Where the lanes are used, the files changed most recently go
            // first, so a store filling from cold answers about the code being
            // worked on before it answers about the rest. The library alone
            // keeps path order, which is what its figures were measured on.
            if accel::managed() {
                recent_first(&mut allowed);
            }
            (allowed, refused)
        };
        let total = paths.len() + refused.len() + unwalkable.len();
        for (path, refusal) in &refused {
            self.refuse_file(report, total, path, refusal, &mut *on_file)?;
        }
        report.generated = generated.iter().map(|p| p.display().to_string()).collect();
        for (path, rule) in &excluded {
            let folder = path.is_dir();
            // What `.semlithignore` left out is counted with the run's skips,
            // so the Index page's card says so beside binary and empty (1.8).
            if rule == IGNORE_FILE {
                report.skipped += 1;
                *report
                    .skipped_reasons
                    .entry(IGNORE_FILE.to_string())
                    .or_insert(0) += 1;
            }
            store::refuse(
                &self.db,
                &path.to_string_lossy(),
                store::class::EXCLUDED,
                &format!("left out by {rule}; change the rule rather than accept the file"),
                &[],
                if folder { 0 } else { 1 },
                now(),
            )?;
        }
        // What a rule now leaves out is not what the store keeps holding. A
        // `.semlithignore` or `.gitignore` line added after a folder was
        // indexed left its files searchable for ever, because only a file gone
        // from disk was ever swept: this repository's own store still held the
        // 134 files 0.30.0's `.semlithignore` excluded.
        if !excluded.is_empty() {
            let out: Vec<&Path> = excluded.iter().map(|(p, _)| p.as_path()).collect();
            for key in store::all_paths(&self.db)? {
                if out.iter().any(|o| Path::new(&key).starts_with(o)) {
                    let (chunks, images) = self.evict(&key)?;
                    report.removed += usize::from(chunks + images > 0);
                }
            }
        }
        for path in &hidden_credentials {
            let why = filter::denied(path).map(|d| d.reason()).unwrap_or_default();
            store::refuse(
                &self.db,
                &path.to_string_lossy(),
                store::class::CREDENTIAL,
                &why,
                &[],
                1,
                now(),
            )?;
        }
        for dir in &generated {
            store::refuse(
                &self.db,
                &dir.to_string_lossy(),
                store::class::POLICY,
                "a generated or vendored folder the walk steps over",
                &[],
                0,
                now(),
            )?;
        }
        // Entries the walk could not read. They used to be a line on stderr,
        // which the daemon and the portal never see, so an unreadable
        // directory looked like a tree that simply had nothing in it.
        for (path, why) in &unwalkable {
            store::refuse(
                &self.db,
                &path.to_string_lossy(),
                store::class::UNINDEXABLE,
                why,
                &[],
                1,
                now(),
            )?;
            report
                .failed
                .push((path.display().to_string(), why.clone()));
            report.scanned += 1;
            say_file(
                &mut *on_file,
                report,
                total,
                path,
                FileOutcome::Failed,
                Some(why.clone()),
            );
        }
        self.tx_commit()?;
        Ok((paths, total))
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
        self.tx_commit()?;
        if completed.is_empty() {
            return Ok(());
        }
        let timed = std::time::Instant::now();
        self.save()?;
        self.write_parts.add("save", timed);
        self.commit_hashes(completed)
    }

    /// The vector list with up to [`PENDING_EMBED`] of the keyword list's
    /// chunks that have no vector yet embedded now and merged in by their
    /// similarity. Unchanged when nothing in the store is pending.
    ///
    /// ponytail: the chunk's stored text is embedded, without the heading path
    /// the index pass prepends, because the path is stored nowhere; the vector
    /// it gets when its run reaches it is the real one.
    fn with_pending(
        &mut self,
        query: &[f32],
        keyword: &[u64],
        scores: Vec<f32>,
        ids: Vec<u64>,
    ) -> Result<(Vec<f32>, Vec<u64>)> {
        if store::pending_share(&self.db)?.is_none() {
            return Ok((scores, ids));
        }
        let candidates: Vec<u64> = keyword
            .iter()
            .filter(|id| !ids.contains(id))
            .copied()
            .collect();
        let pending: Vec<(u64, String)> = store::pending_among(&self.db, &candidates)?
            .into_iter()
            .take(PENDING_EMBED)
            .collect();
        if pending.is_empty() {
            return Ok((scores, ids));
        }
        let texts: Vec<String> = pending.iter().map(|(_, text)| text.clone()).collect();
        let vectors = self.embed(texts)?;
        let mut merged: Vec<(u64, f32)> = ids.into_iter().zip(scores).collect();
        for ((id, _), vector) in pending.iter().zip(&vectors) {
            merged.push((*id, index::cosine(query, vector)));
        }
        merged.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (ids, scores) = merged.into_iter().unzip();
        Ok((scores, ids))
    }

    /// The share of this store still being embedded, or `None` when nothing
    /// is. A search mid-run says so beside its answer.
    pub fn pending_share(&self) -> Result<Option<f64>> {
        store::pending_share(&self.db)
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
        let mut ids: std::collections::HashMap<&str, (i64, bool)> =
            std::collections::HashMap::new();
        for symbol in &extraction.symbols {
            let chunk_id = spans
                .iter()
                .find(|(start, end, _)| symbol.start_line >= *start && symbol.start_line <= *end)
                .map(|(_, _, id)| *id);
            let id = store::insert_symbol(&self.db, file_id, chunk_id, symbol)?;
            // A definition outranks the file's module symbol of the same name:
            // `fn brief` in `brief.rs` is where its calls come from, and giving
            // them to the module put every caller of it at line 1.
            let module = symbol.kind == "module";
            match ids.get(symbol.name.as_str()) {
                None => {
                    ids.insert(symbol.name.as_str(), (id, module));
                }
                Some((_, true)) if !module => {
                    ids.insert(symbol.name.as_str(), (id, false));
                }
                _ => {}
            }
        }
        let ids: std::collections::HashMap<&str, i64> =
            ids.into_iter().map(|(k, (id, _))| (k, id)).collect();

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
        let _charged = pipeline::Charge(&self.write_parts, "hashes", std::time::Instant::now());
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

    /// Hand the window to the embed stage, starting the stage with the first
    /// window of the call. `Ok(false)` when a stop was asked for while it
    /// waited for room.
    ///
    /// The window's rows are committed first: its ids are rows' ids, and a
    /// window never goes out carrying an id whose row could still roll back.
    #[allow(clippy::too_many_arguments)]
    fn hand_over<'scope>(
        &mut self,
        scope: &'scope std::thread::Scope<'scope, '_>,
        stage: &mut Option<pipeline::Embedder>,
        window: &mut pipeline::Window,
        cpu_back: &std::sync::mpsc::Sender<pipeline::CpuWork>,
        paused: &std::sync::Arc<std::sync::atomic::AtomicBool>,
        ask: &dyn Fn(&std::sync::atomic::AtomicBool) -> bool,
        clock: &mut pipeline::WriterClock,
        tick: &mut dyn FnMut(usize),
    ) -> Result<bool> {
        if window.ids.is_empty() {
            return Ok(true);
        }
        self.tx_commit()?;
        if stage.is_none() {
            let work = self.index_work()?;
            *stage = Some(pipeline::Embedder::start(
                scope,
                work,
                self.model == Model::Granite,
                cpu_back.clone(),
                std::sync::Arc::clone(paused),
            ));
        }
        let running = stage.as_mut().expect("started above");
        let full = std::mem::replace(
            window,
            pipeline::Window {
                ids: Vec::new(),
                pieces: Vec::new(),
                hashes: Vec::new(),
            },
        );
        let waiting = std::time::Instant::now();
        let mut stopped = false;
        let batched = std::sync::Arc::clone(&running.batched);
        let mut progress = Progress::default();
        let sent = running.send(full, || {
            if !ask(paused) {
                stopped = true;
                return false;
            }
            // Silent while paused: what lands meanwhile is counted, and said
            // with the first line after the resume, so a paused run holds still.
            if !paused.load(std::sync::atomic::Ordering::Relaxed)
                && let Some(n) = progress.due(&batched)
            {
                tick(n);
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            true
        });
        clock.waited_on_embed(waiting);
        if !sent && !stopped {
            bail!("the embed stage ended before the run did");
        }
        Ok(sent)
    }

    /// Wait for every window in flight and put it in the index. `Ok(false)`
    /// when a stop was asked for while waiting.
    #[allow(clippy::too_many_arguments)]
    fn drain(
        &mut self,
        stage: &mut Option<pipeline::Embedder>,
        report: &mut IndexReport,
        run_lanes: &mut std::collections::BTreeMap<String, usize>,
        cached: &mut CacheWrites,
        paused: &std::sync::atomic::AtomicBool,
        ask: &dyn Fn(&std::sync::atomic::AtomicBool) -> bool,
        clock: &mut pipeline::WriterClock,
        tick: &mut dyn FnMut(&IndexReport),
    ) -> Result<bool> {
        let Some(running) = stage.as_mut() else {
            return Ok(true);
        };
        let mut progress = Progress::default();
        while running.outstanding > 0 {
            let waiting = std::time::Instant::now();
            let got = running.next(std::time::Duration::from_millis(20));
            clock.waited_on_embed(waiting);
            match got {
                Some(done) => {
                    self.land(done.map_err(anyhow::Error::msg)?, report, run_lanes, cached)?;
                    if !paused.load(std::sync::atomic::Ordering::Relaxed) {
                        tick(report);
                    }
                }
                None => {
                    if !ask(paused) {
                        return Ok(false);
                    }
                    if !paused.load(std::sync::atomic::Ordering::Relaxed)
                        && let Some(n) = progress.due(&running.batched)
                    {
                        report.batched = n;
                        tick(report);
                    }
                }
            }
        }
        Ok(true)
    }

    /// One embedded window into the index and its sidecar, in the same
    /// breath and from the same values, so the two cannot describe different
    /// vectors. A failure to write the sidecar fails the pass rather than
    /// leaving a store whose rescoring silently reorders by a stale vector.
    /// One image back from the image lane, recorded: its old vector and rows
    /// replaced, its new ones written. `Some((path, why))` when the image
    /// model could not read its bytes, which is that file's failure.
    fn land_image(
        &mut self,
        done: ImageDone,
        report: &mut IndexReport,
        written: &mut Vec<String>,
        completed: &mut Vec<(i64, String)>,
    ) -> Result<Option<(PathBuf, String)>> {
        let ImageDone { job, vector } = done;
        // The writer's own part only (#207): the image model runs on its own
        // thread since #203, and charging its time here made the parts add up
        // to more than the write stage they split.
        let _charged = pipeline::Charge(&self.write_parts, "images", std::time::Instant::now());
        let vector = match vector {
            Ok(vector) => vector,
            Err(why) => {
                failed(report, &job.path, &anyhow::anyhow!("{why}"));
                return Ok(Some((job.path, why)));
            }
        };
        self.tx_begin()?;
        // Replacing an image: its old vector goes before the new one arrives,
        // and the row goes with the file's cascade.
        for id in store::image_ids_of(&self.db, &job.key)? {
            self.images.remove(id as u64)?;
        }
        for id in store::delete_file(&self.db, &job.key, now())? {
            self.index.remove(id)?;
        }
        let file_id =
            store::insert_file(&self.db, &job.key, PENDING, job.bytes.len() as u64, now())?;
        written.push(job.key.clone());
        let image_id = store::insert_image(&self.db, file_id, job.width, job.height)?;
        self.images.add(&vector, &[image_id as u64])?;
        completed.push((file_id, job.hash));
        report.indexed += 1;
        report.images += 1;
        Ok(None)
    }

    fn land(
        &mut self,
        done: pipeline::Embedded,
        report: &mut IndexReport,
        run_lanes: &mut std::collections::BTreeMap<String, usize>,
        cached: &mut CacheWrites,
    ) -> Result<()> {
        let flat: Vec<f32> = done.vectors.iter().flatten().copied().collect();
        anyhow::ensure!(
            flat.len() == done.ids.len() * self.dim,
            "the embed stage handed back {} values for {} chunks",
            flat.len(),
            done.ids.len()
        );
        // The writer is CPU too, and under a cap it waits its turn.
        crate::cpucap::pace();
        self.follow_budget();
        let timed = std::time::Instant::now();
        let (index, exact) = (&mut self.index, &mut self.exact);
        crate::cpucap::within(|| -> Result<()> {
            index.add(&flat, &done.ids)?;
            exact.append(&flat, &done.ids)
        })?;
        self.write_parts.add("vectors", timed);
        for id in &done.ids {
            if let Some(file) = self.file_of.remove(id)
                && let Some(left) = self.awaiting.get_mut(&file)
            {
                left.0 = left.0.saturating_sub(1);
                if left.0 == 0
                    && left.2
                    && let Some((_, hash, _)) = self.awaiting.remove(&file)
                {
                    self.landed.push((file, hash));
                }
            }
        }
        self.record_variants(&done.variants)?;
        for (lane, n) in &done.lanes {
            self.note_lane(lane, *n);
            *run_lanes.entry(lane.to_string()).or_default() += n;
        }
        report.embedded += done.ids.len();
        report.threads = self.index_threads();
        report.lanes = self.lane_chunks.clone();
        self.last_embed = Some(std::time::Instant::now());
        for (at, ((variant, from_cache), hash)) in done.rows.iter().zip(&done.hashes).enumerate() {
            let Some(hash) = hash else { continue };
            if *from_cache {
                cached.hits.push((*hash, variant));
            } else if !variant.is_empty() {
                cached
                    .fresh
                    .push((*hash, variant, done.vectors[at].clone()));
            }
        }
        Ok(())
    }

    /// The CPU session an index pass embeds with, loaded once per store and
    /// kept between slices; rebuilt when the thread count in force changed.
    fn index_work(&mut self) -> Result<pipeline::CpuWork> {
        let threads = embed::threads_in_force();
        if let Some(work) = self.index_session.take() {
            if self.index_threads == threads {
                return Ok(work);
            }
            SESSIONS.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
        }
        let cache = model_cache_dir()?;
        let work = match self.model {
            Model::Granite => {
                let (main, alt) = embed::index_variant();
                pipeline::CpuWork::Ids {
                    main: session::CpuSession::open(&cache, main, threads, self.quiet)?,
                    alt: alt
                        .map(|variant| {
                            session::CpuSession::open(&cache, variant, threads, self.quiet)
                        })
                        .transpose()?,
                }
            }
            Model::Builtin(_) => pipeline::CpuWork::Text(Box::new(self.model.load_variant(
                cache,
                chunk::MAX_CHARS / 2,
                self.quiet,
                threads,
                embed::Variant::Int8,
            )?)),
        };
        self.index_threads = threads;
        SESSIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(work)
    }

    /// The intra-op thread count the index pass's CPU session was built with.
    fn index_threads(&self) -> usize {
        // The CPU lane's own count once it has embedded a batch: it follows a
        // count saved mid-run, and the card shows what the lane runs with.
        match pipeline::CPU_THREADS.load(std::sync::atomic::Ordering::Relaxed) {
            0 => self.index_threads,
            live => live,
        }
    }

    /// Open a transaction if none is open. An index pass writes a file's rows
    /// in one rather than a commit per statement, and commits before it hands
    /// the rows' ids to the embed stage and before every checkpoint.
    fn tx_begin(&self) -> Result<()> {
        if self.db.is_autocommit() {
            self.db.execute_batch("BEGIN")?;
            store::drop_insert_trigger(&self.db)?;
        }
        Ok(())
    }

    fn tx_commit(&self) -> Result<()> {
        let _charged = pipeline::Charge(&self.write_parts, "commit", std::time::Instant::now());
        if !self.db.is_autocommit() {
            self.db.execute_batch("COMMIT")?;
        }
        Ok(())
    }

    /// Count chunks one lane embedded in this pass, for the run card's rate
    /// per lane.
    fn note_lane(&mut self, lane: &'static str, chunks: usize) {
        *self.lane_chunks.entry(lane.to_string()).or_default() += chunks;
    }

    /// Add this window's chunks to the store's count per vector variant.
    ///
    /// int8 on the CPU and fp16 on a GPU are two variants of one model, which
    /// agree at cosine 0.987; a store holding both says how many of each, and
    /// `stats` prints it. One meta row, which an older binary ignores.
    fn record_variants(&self, counts: &[(&'static str, usize)]) -> Result<()> {
        if counts.is_empty() {
            return Ok(());
        }
        let mut variants: std::collections::BTreeMap<String, u64> =
            store::get_meta(&self.db, VARIANTS_KEY)?
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
        for (variant, n) in counts {
            *variants.entry((*variant).to_string()).or_default() += *n as u64;
        }
        store::set_meta(&self.db, VARIANTS_KEY, &serde_json::to_string(&variants)?)
    }

    /// Chunks this store holds per vector variant, where it has counted them.
    pub fn variants(&self) -> std::collections::BTreeMap<String, u64> {
        store::get_meta(&self.db, VARIANTS_KEY)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Take a stopped run's files out of the store, and write the index once.
    ///
    /// Before 0.28.0 the undo called `forget_held` per file, and each call
    /// rewrote the whole index, so undoing a large run took as long as making
    /// it. Returns how many files were taken out.
    pub(crate) fn undo_held(&mut self, keys: &[String]) -> Result<usize> {
        self.writing(|me| {
            for key in keys {
                // The run's files leave without becoming history: they were
                // never the store's, so a later `history` must not list them.
                me.evict_as(key, false)?;
            }
            me.save()?;
            Ok(keys.len())
        })
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
        self.evict_as(key, true)
    }

    /// [`Self::evict`], keeping the file's definitions as history or not.
    fn evict_as(&mut self, key: &str, retire: bool) -> Result<(usize, usize)> {
        // Read before the delete: the cascade that removes the rows is what
        // makes their ids unreadable, and the vectors they address still have
        // to leave the image index.
        let images = store::image_ids_of(&self.db, key)?;
        let ids = if retire {
            store::delete_file(&self.db, key, now())?
        } else {
            store::delete_file_unretired(&self.db, key)?
        };
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
        let resolved = filter::resolve_boundary(roots);
        Ok(store::all_paths(&self.db)?
            .into_iter()
            .filter(|key| !filter::within_resolved(Path::new(key), &resolved))
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

    /// Resolve a filter to the ids the vector index may consider, and every
    /// chunk the filter selects -- the keyword list's set, which also holds
    /// chunks whose vectors are still being made. `None` for no filter.
    fn allowlist(
        &mut self,
        filter: &Filter,
    ) -> Result<(
        Allowlist,
        Option<std::sync::Arc<std::collections::HashSet<u64>>>,
    )> {
        if filter.is_empty() {
            return Ok((Allowlist::All, None));
        }
        // The same filter over the same store is the same answer. "Same
        // store" is the index generation, which every committed write moves,
        // and the newest chunk id, which a row written ahead of its vectors
        // moves before the generation does.
        let newest_chunk = store::newest_chunk(&self.db)?;
        let (generation, groups) = (self.generation, filter.groups());
        self.filter_memo
            .retain(|m| m.generation == generation && m.newest_chunk == newest_chunk);
        if let Some(at) = self.filter_memo.iter().position(|m| m.groups == groups) {
            let memo = self.filter_memo.remove(at).expect("position is in range");
            let out = (memo.allowlist.clone(), Some(memo.selected.clone()));
            self.filter_memo.push_front(memo);
            return Ok(out);
        }
        let candidates = store::filtered_chunk_ids(&self.db, filter.groups())?;
        let mut ids = Vec::with_capacity(candidates.len());
        for &id in &candidates {
            // turbovec panics on an id the index does not hold, and SQLite can
            // hold a chunk the index does not if a run was interrupted between
            // the two. A stale row must not take a search down with it.
            if self.index.contains(id)? {
                ids.push(id);
            }
        }

        let allowlist = if ids.is_empty() {
            Allowlist::Empty
        } else if ids.len() == self.len() {
            // The filter excludes nothing, so skip building a mask the size of
            // the whole index for no benefit.
            Allowlist::All
        } else {
            Allowlist::subset(ids)
        };
        let selected = std::sync::Arc::new(candidates.into_iter().collect());
        self.filter_memo.push_front(FilterMemo {
            groups: filter.groups().to_vec(),
            generation: self.generation,
            newest_chunk,
            allowlist: allowlist.clone(),
            selected: std::sync::Arc::clone(&selected),
        });
        // ponytail: a fixed count, not bytes; a memo of a whole-store-sized
        // subset holds about 9 bytes per chunk, so eight stay small next to
        // the index itself.
        self.filter_memo.truncate(FILTER_MEMOS);
        Ok((allowlist, Some(selected)))
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
        let text = self.model.query_text(query);
        let wanted = embed::query_variant();
        // A query is always embedded by the query variant, never by an index
        // pass's alternation: a mixed store is searched with int8 queries
        // unless the harness asks for another, which is item 1.16's design.
        if embed::index_variant() == (wanted, None) {
            return Ok(self.embed(vec![text])?.remove(0));
        }
        let _lifted = priority::embedding();
        if self.query_embedder.is_none() {
            self.query_embedder = Some(self.model.load_variant(
                model_cache_dir()?,
                chunk::MAX_CHARS / 2,
                self.quiet,
                embed::embed_threads(),
                wanted,
            )?);
        }
        let mut out = self
            .query_embedder
            .as_mut()
            .expect("loaded above")
            .embed(vec![text], Some(1))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut vector = out.remove(0);
        normalize(&mut vector);
        Ok(vector)
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
        // rather than dismissed with the same test. Nor is a store filling from
        // cold, whose rows are readable before any vector is: its keyword and
        // graph halves answer while the index is still empty.
        if k == 0
            || (self.is_empty()
                && store::chunk_count(&self.db)? == 0
                && store::image_count(&self.db)? == 0)
        {
            return Ok(Vec::new());
        }

        // Look deeper than `k` in each half. Fusion can only rank what it is
        // given, and a chunk that is second on one side and absent from the
        // other still deserves to be considered.
        let depth = (k * RANK_DEPTH).max(k);

        // Which half of the fusion this query's own text says to trust.
        let shape = shape_of(query);

        let (allowlist, selected) = self.allowlist(filter)?;
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

        // The definitions of the identifiers a sentence names, as a list of
        // their own. The lift above is for a query that is one identifier; a
        // sentence that names `parse_args` is still asking about it, and the
        // keyword list ranks every mention of the word, not its definition.
        // A list rather than a lift, so it votes beside the others and a
        // wrong guess at what is an identifier costs a place, not the answer.
        let named: Vec<u64> = if shape != Shape::Identifier {
            let names = named_identifiers(query);
            let mut ids = Vec::new();
            for name in &names {
                if let Some(chunk) =
                    store::symbols_by_names(&self.db, std::slice::from_ref(name), filter.groups())?
                        .into_iter()
                        .find_map(|row| row.chunk_id)
                {
                    let id = chunk as u64;
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
            ids
        } else {
            Vec::new()
        };

        let (dense_scores, dense_ids) = self.search_vectors(vector, depth, &allowlist)?;
        let (dense_scores, dense_ids) = self.rescored(vector, dense_scores, dense_ids);
        // The filter's set once, shared: the keyword list used to resolve the
        // filter again in SQL, which was most of a scoped search (#170).
        let keyword_ids = match &selected {
            Some(set) => store::keyword_search_within(&self.db, query, depth, set)?,
            None => store::keyword_search(&self.db, query, depth, &[])?,
        };
        // Mid-run, the keyword half already holds chunks the vector half has
        // not reached. A few of the best of them are embedded here, on the
        // query path, and join the vector list by their own similarity, so a
        // semantic question asked while a store fills still gets a semantic
        // answer from the part that is only rows so far. Not for a query
        // shaped like an identifier: its own text says to trust the keyword
        // half, which already holds them, and the embed is seconds on a cold
        // model while the run has every core.
        let (dense_scores, dense_ids) = if shape == Shape::Identifier {
            (dense_scores, dense_ids)
        } else {
            self.with_pending(vector, &keyword_ids, dense_scores, dense_ids)?
        };

        // The image list. Only when the store actually holds an image: the
        // query has to be embedded a second time, with CLIP's text encoder
        // rather than the store's own model, and a store of source code should
        // not pay for a model it has nothing to compare against.
        let (images, truncated) = self.image_search(query, depth, filter)?;

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
                // A confident image match stands in for the lists a chunk
                // can appear in: an image can only ever be found by this one,
                // and a chunk collects a contribution from each of the
                // others. Without this weight a picture could never place
                // above a passing text match however well CLIP matched it.
                //
                // Below the floor the image is a weak candidate rather than a
                // wrong one, so it sits where that puts it instead of being
                // dropped — and because the list's curve is now steep for
                // every shape (#122), the weak weight is a fraction rather
                // than 1.0, so an unconfident image scores what it scored
                // before: 0.25 / 13 = 0.019 at rank one, against 1 / 61 =
                // 0.016 under the old flat curve. The order among the
                // also-rans is unchanged; only the confident case moved.
                //
                // A query CLIP had to cut short is never confident: the cosine
                // is of its opening words, not of what was asked. An issue or a
                // pasted paragraph runs past 77 tokens, and a screenshot of
                // text matches the opening of almost any technical prose
                // (0.24-0.31 on Django's docs, against 0.39 for a sentence
                // describing a screenshot).
                let weight = if *similarity >= image_floor() && !truncated {
                    TEXT_LISTS
                } else {
                    WEAK_IMAGE
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
            ("named", false, named.iter().map(|id| (*id, 1.0)).collect()),
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
                            symbol_line: None,
                            copies: Vec::new(),
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
                        fresh: true,
                        symbol: None,
                        symbol_kind: None,
                        symbol_line: None,
                        copies: Vec::new(),
                    },
                    similarity,
                ));
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
        // decides which, so the tiebreaks below — staleness, test paths, the
        // caller's preference — still apply to a fused-scale number, and a
        // hit outside the head is never reordered against one inside it.
        if shape != Shape::Identifier && rerank::enabled() && hits.len() > 1 {
            let head = hits.len().min(rerank::RERANK_DEPTH);
            let texts: Vec<String> = hits[..head]
                .iter()
                .map(|(hit, _)| {
                    let text: String = hit.text.chars().take(rerank::RERANK_CHARS).collect();
                    format!("{}\n{text}", hit.path)
                })
                .collect();
            let quiet = self.quiet;
            let cache = model_cache_dir().unwrap_or_default();
            let order = rerank::with(&cache, quiet, |model| {
                model.map(|model| rerank::order(model, query, &texts))
            });
            if let Some(order) = order {
                match order {
                    Ok(order) if order.len() == head => {
                        // Fused with the order it is reordering, not replacing
                        // it.
                        //
                        // Measured, and this is the whole reason: letting the
                        // cross-encoder write the order outright moved sixteen
                        // of seventy-seven questions and gained exactly
                        // nothing — `concept-clip-text-encoder` went from
                        // rank 7 to rank 1 while `concept-configuration-
                        // timeout` went from 1 to 6, and the two cancelled at
                        // every depth. A model that reads the pair is better
                        // than fusion at some questions and worse at others,
                        // which is an argument for two opinions rather than
                        // for replacing one with the other. So both orders
                        // vote, by the same reciprocal-rank rule the lists
                        // themselves are fused with, and a candidate has to
                        // be liked by both to reach the top.
                        let mut scores: Vec<f32> =
                            hits[..head].iter().map(|(hit, _)| hit.score).collect();
                        scores.sort_by(|a, b| b.total_cmp(a));
                        let mut by_fusion: Vec<usize> = (0..head).collect();
                        by_fusion.sort_by(|a, b| hits[*b].0.score.total_cmp(&hits[*a].0.score));
                        let mut fused_rank = vec![0usize; head];
                        for (rank, at) in by_fusion.into_iter().enumerate() {
                            fused_rank[at] = rank;
                        }
                        let mut blended: Vec<(usize, f32)> = order
                            .into_iter()
                            .enumerate()
                            .map(|(rescored, at)| {
                                let vote = 1.0 / (RRF_K + rescored as f32 + 1.0)
                                    + 1.0 / (RRF_K + fused_rank[at] as f32 + 1.0);
                                (at, vote)
                            })
                            .collect();
                        blended.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                        for (rank, (at, _)) in blended.into_iter().enumerate() {
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
        let about_tests = names_tests(query);
        let (prior, named) = project_prior(&hits, query);
        for (((hit, _), share), named) in hits.iter_mut().zip(prior).zip(named) {
            hit.score *= 1.0 + PROJECT_LIFT * share;
            if named {
                hit.score *= 1.0 + NAMED_PROJECT_LIFT;
            }
            if !hit.fresh {
                hit.score *= 1.0 - STALE_PENALTY;
            }
            if !about_tests && is_test_path(&hit.path) {
                hit.score *= 1.0 - TEST_PENALTY;
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
        collapse_copies(&mut hits);
        if !about_tests {
            product_first(&mut hits);
        }
        // Release notes last, for a question that is not about releases. A
        // changelog repeats every name and every bug report's words, one line
        // each, and on the benchmark it filled the budget ahead of the code
        // in 19 of the 41 Rust and TypeScript misses. A stable sort, so the
        // order within each side is the ranking's.
        if !names_releases(query) {
            hits.sort_by_key(|(h, _)| is_release_notes(&h.path));
        }
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
        self.read_within(target, filter, &[])
    }

    /// [`Semlith::read`], with the roots the store indexes, so a path
    /// relative to one of them means that file.
    ///
    /// `src/lib.rs` on a store over a repository that keeps a copy of itself
    /// under `tests/fixtures` used to end two indexed paths and be refused. A
    /// root joined with the path is an exact answer and wins; the suffix match
    /// is what is left for a path that no root holds. Two roots that both hold
    /// the exact relative path are still refused, naming both.
    pub fn read_within(
        &self,
        target: &Target,
        filter: &Filter,
        roots: &[PathBuf],
    ) -> Result<Option<Read>> {
        let (path, start, end) = match target {
            Target::Span { path, start, end } => {
                let exact: Vec<String> = if Path::new(path).is_relative() {
                    let mut found = Vec::new();
                    for root in roots {
                        let joined = root.join(path);
                        let joined = joined.to_string_lossy();
                        for hit in store::files_ending_with(&self.db, &joined, 2)? {
                            if plain(&hit) == plain(&joined) && !found.contains(&hit) {
                                found.push(hit);
                            }
                        }
                    }
                    found
                } else {
                    Vec::new()
                };
                // A locate answer prints a root-relative path, so that is
                // what comes back in. Two indexed files can end with the same
                // suffix, and choosing one of them would be a guess.
                let candidates = if exact.is_empty() {
                    store::files_ending_with(&self.db, path, 8)?
                } else {
                    exact
                };
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
                // `Type::method` narrows to that owner's definition, and a
                // heading or a key is left out when real definitions share
                // the name: `read search_preferring` means the method.
                let (qualifier, bare) = crate::graph::split_qualified(name);
                let mut found = store::symbols_named(&self.db, bare, 200)?;
                if found
                    .iter()
                    .any(|r| !crate::graph::NAVIGATIONAL_KINDS.contains(&r.kind.as_str()))
                {
                    found.retain(|r| !crate::graph::NAVIGATIONAL_KINDS.contains(&r.kind.as_str()));
                }
                if let Some(q) = qualifier
                    && found.iter().any(|r| crate::graph::owned_by(r, q))
                {
                    found.retain(|r| crate::graph::owned_by(r, q));
                }
                match found.len() {
                    // A bare path is a file, not a name nothing defines:
                    // `src/mcp.rs` answered "nothing indexed" for an indexed
                    // file.
                    0 => {
                        let whole = Target::Span {
                            path: name.clone(),
                            start: 1,
                            end: u32::MAX,
                        };
                        return self.read_within(&whole, filter, roots);
                    }
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
            from_disk: false,
            store: None,
        };
        self.name_enclosing_symbol(&mut span)?;
        self.mark_span_freshness(&mut span)?;
        if !span.fresh {
            self.read_from_disk(&mut span, start, end)?;
        }
        Ok(Some(Read::One(span)))
    }

    /// Replace a stale span's text with the file's current lines, when the
    /// file really did change and what is on disk now may be read (1.18).
    ///
    /// Only for a file the store indexed, so this answers for nothing semlith
    /// was not already allowed to read. The current text goes through the
    /// same secret scan indexing does, and a file that now holds something
    /// live-looking is not read: the stale copy stays, marked stale, because
    /// serving a secret the store never held is exactly what reading from
    /// chunks exists to prevent. A file whose bytes hash the same as the row
    /// is not re-read at all — its stamps are behind, not its text (1.4).
    fn read_from_disk(&self, span: &mut Span, start: u32, end: u32) -> Result<()> {
        let Ok(bytes) = std::fs::read(&span.path) else {
            return Ok(());
        };
        if bytes.len() as u64 > chunk::MAX_FILE_BYTES {
            return Ok(());
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        if store::file_hash(&self.db, &span.path)?.as_deref() == Some(hash.as_str()) {
            span.fresh = true;
            return Ok(());
        }
        // Plain text only: a PDF or a spreadsheet goes through a reader at
        // index time, and its current lines are not the file's bytes.
        let Ok(text) = String::from_utf8(bytes) else {
            return Ok(());
        };
        let text = match keyscan::readable(&self.db, &span.path, &text)? {
            Some(text) => text,
            None => return Ok(()),
        };
        let lines: Vec<&str> = text.lines().collect();
        let first = start.max(1);
        let last = end.min(lines.len() as u32);
        if first > last {
            return Ok(());
        }
        span.text = lines[(first - 1) as usize..last as usize].join("\n");
        span.start_line = first;
        span.end_line = last;
        span.from_disk = true;
        Ok(())
    }

    /// The innermost definition a span sits inside, if any.
    fn name_enclosing_symbol(&self, span: &mut Span) -> Result<()> {
        let by_file = store::symbols_in_files(&self.db, std::slice::from_ref(&span.path))?;
        let Some(symbols) = by_file.get(&span.path) else {
            return Ok(());
        };
        // The same rule search uses, for the same reason: a span that begins
        // in a function's doc comment is that function's.
        if let Some((_, name, kind)) = enclosing_definition(symbols, span.start_line, span.end_line)
        {
            span.symbol = Some(name);
            span.symbol_kind = Some(kind);
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
            if let Some((start, name, kind)) =
                enclosing_definition(symbols, hit.start_line, hit.end_line)
            {
                hit.symbol = Some(name);
                hit.symbol_kind = Some(kind);
                hit.symbol_line = Some(start);
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
    /// store's image vectors, and whether CLIP had to cut the query short.
    ///
    /// Empty, and free, for a store that holds no image — which is every store
    /// that has only ever been pointed at source code.
    fn image_search(
        &mut self,
        query: &str,
        depth: usize,
        filter: &Filter,
    ) -> Result<(Vec<(u64, f32)>, bool)> {
        if store::image_count(&self.db)? == 0 {
            return Ok((Vec::new(), false));
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
                return Ok((Vec::new(), false));
            }
            index::Allowlist::subset(ids)
        };

        let (mut vector, truncated) = self.clip.embed_query(query, self.quiet)?;
        normalize(&mut vector);
        let (scores, ids) = self.images.search(&vector, depth, &allowlist)?;
        Ok((
            ids.into_iter()
                .zip(scores.into_iter().chain(std::iter::repeat(0.0)))
                .collect(),
            truncated,
        ))
    }

    /// How many images this store holds.
    pub fn image_count(&self) -> Result<i64> {
        store::image_count(&self.db)
    }

    /// `(files, chunks, indexed bytes)`
    pub fn stats(&self) -> Result<(i64, i64, i64)> {
        store::stats(&self.db)
    }

    /// Whether this store can still be read. See [`store::readable`].
    pub fn readable(&self) -> Result<()> {
        store::readable(&self.db)
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
    if b >= KB * KB * KB {
        format!("{:.1} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.0} KB", b / KB)
    } else {
        format!("{bytes} B")
    }
}

/// [`serialize_plain`] for a list of paths.
pub fn serialize_plain_list<S: serde::Serializer>(
    paths: &[String],
    s: S,
) -> Result<S::Ok, S::Error> {
    s.collect_seq(paths.iter().map(|p| plain(p)))
}

/// [`serialize_plain_list`] for lists grouped by a key.
pub fn serialize_plain_lists<S: serde::Serializer>(
    groups: &std::collections::BTreeMap<String, Vec<String>>,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.collect_map(
        groups
            .iter()
            .map(|(k, paths)| (k, paths.iter().map(|p| plain(p)).collect::<Vec<String>>())),
    )
}

/// Serialize a stored path in the form a person reads, leaving the value in
/// memory as the store's own key. One attribute per field beats a call at every
/// place a struct reaches JSON, and the CLI's `--json`, the portal and the MCP
/// results all serialise these same structs.
pub fn serialize_plain<S: serde::Serializer>(path: &str, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&plain(path))
}

thread_local! {
    /// How many times this process has canonicalised a path.
    ///
    /// Counted because the cost is invisible until it is not: a `canonicalize` is
    /// an opened handle on Windows, and 0.18.0 did three per file — one in the
    /// walk, one on the home directory and one on the path, the last two on every
    /// file for a home that cannot change mid-run. `tests/filter_canonical.rs`
    /// asserts the count over a walk of N files stays near N rather than near 3N.
    // Per thread: an index pass runs on the thread that asked for it, and a
    // process-wide count read the canonicalisations of every other test in the
    // binary running beside the one measuring its own run.
    static CANONICAL_CALLS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// The reading for [`CANONICAL_CALLS`], for the test that pins the cost.
pub fn canonical_calls() -> u64 {
    CANONICAL_CALLS.with(|c| c.get())
}

pub fn canonical(path: &Path) -> PathBuf {
    CANONICAL_CALLS.with(|c| c.set(c.get() + 1));
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// No test or fixture hit ahead of the first product-code hit, for a
/// question that does not name tests.
///
/// Precedence, not a weight — the definition lift's shape. A test that drives
/// a code path names every step of it and the graph list carries it up; a
/// weight large enough to undo that would outvote the query. Only the tests
/// ahead of the first product-code hit move, to just behind it; prose and
/// everything else keep their places.
fn product_first(hits: &mut Vec<(Hit, f32)>) {
    let product = |h: &Hit| filter::is_code(&h.path) && !is_test_path(&h.path);
    let Some(first) = hits.iter().position(|(h, _)| product(h)) else {
        return;
    };
    let held: Vec<usize> = (0..first)
        .filter(|&i| is_test_path(&hits[i].0.path))
        .collect();
    if held.is_empty() {
        return;
    }
    let mut moved: Vec<(Hit, f32)> = Vec::with_capacity(held.len());
    for &i in held.iter().rev() {
        moved.push(hits.remove(i));
    }
    moved.reverse();
    let at = hits
        .iter()
        .position(|(h, _)| product(h))
        .map_or(hits.len(), |p| p + 1);
    for (offset, hit) in moved.into_iter().enumerate() {
        hits.insert(at + offset, hit);
    }
}

/// Fold hits whose text is identical into the best-ranked of them, which
/// then names the others in `copies`.
///
/// A repository that vendors a file, or keeps a fixture copy of itself, has
/// every chunk of that file twice, and the second copy spent a result slot
/// saying nothing new. Exact text only: two similar functions are two answers.
fn collapse_copies(hits: &mut Vec<(Hit, f32)>) {
    let mut first: std::collections::HashMap<blake3::Hash, usize> =
        std::collections::HashMap::new();
    let mut keep: Vec<bool> = Vec::with_capacity(hits.len());
    let mut copies: Vec<(usize, String)> = Vec::new();
    for (i, (hit, _)) in hits.iter().enumerate() {
        if hit.image.is_some() || hit.text.trim().is_empty() {
            keep.push(true);
            continue;
        }
        let key = blake3::hash(hit.text.as_bytes());
        match first.get(&key) {
            Some(&j) if hits[j].0.path != hit.path => {
                copies.push((j, hit.path.clone()));
                keep.push(false);
            }
            Some(_) => keep.push(true),
            None => {
                first.insert(key, i);
                keep.push(true);
            }
        }
    }
    for (j, path) in copies {
        if !hits[j].0.copies.contains(&path) {
            hits[j].0.copies.push(path);
        }
    }
    let mut flags = keep.into_iter();
    hits.retain(|_| flags.next().unwrap_or(true));
}

/// The definition a chunk from `start` to `end` is about, as `(line, name,
/// kind)`.
///
/// A definition that *starts* inside the chunk comes first — the chunker cuts
/// at definitions, so a chunk opening with a doc comment is about the
/// function below it, not the one whose last two lines it overlaps. Then the
/// innermost definition containing the chunk's first line, then the innermost
/// overlapping it at all.
pub(crate) fn enclosing_definition(
    symbols: &[(u32, u32, String, String)],
    start: u32,
    end: u32,
) -> Option<(u32, String, String)> {
    let starts_inside = symbols
        .iter()
        .filter(|(s, _, _, _)| *s >= start && *s <= end)
        .min_by_key(|(s, e, _, _)| (*s, std::cmp::Reverse(e.saturating_sub(*s))));
    let contains = || {
        symbols
            .iter()
            .filter(|(s, e, _, _)| *s <= start && *e >= start)
            .min_by_key(|(s, e, _, _)| e.saturating_sub(*s))
    };
    let overlaps = || {
        symbols
            .iter()
            .filter(|(s, e, _, _)| *s <= end && *e >= start)
            .min_by_key(|(s, e, _, _)| e.saturating_sub(*s))
    };
    starts_inside
        .or_else(contains)
        .or_else(overlaps)
        .map(|(s, _, name, kind)| (*s, name.clone(), kind.clone()))
}

/// Paths written relative to the store root that holds them, with the roots
/// named once.
///
/// An absolute path is most of a locate row: `/Users/…/semlith/src/lib.rs`
/// is fifty characters of which eleven say anything, and an answer of forty
/// rows repeated the other thirty-nine forty times. The roots used are
/// collected as paths are shortened, so [`Shortener::header`] names exactly the
/// ones the answer needs to be turned back into absolute paths — and
/// `semlith_read` resolves a root-relative path against the same roots (1.3).
pub struct Shortener {
    /// Longest first, so a root nested inside another claims its own files.
    roots: Vec<std::path::PathBuf>,
    used: std::cell::RefCell<std::collections::BTreeSet<std::path::PathBuf>>,
}

impl Shortener {
    pub fn new(mut roots: Vec<std::path::PathBuf>) -> Self {
        roots.sort_by_key(|r| std::cmp::Reverse(r.as_os_str().len()));
        Self {
            roots,
            used: Default::default(),
        }
    }

    /// `path` relative to its root, or whole when no root holds it.
    pub fn short(&self, path: &str) -> String {
        let plain_path = plain(path);
        for root in &self.roots {
            let root_text = plain(&root.to_string_lossy());
            if let Some(rest) = plain_path.strip_prefix(root_text.as_str())
                && let Some(rest) = rest.strip_prefix(['/', '\\'])
            {
                self.used.borrow_mut().insert(root.clone());
                return rest.to_string();
            }
        }
        plain_path
    }

    /// The line that says what the relative paths are relative to, or
    /// nothing when every path was printed whole.
    pub fn header(&self) -> Option<String> {
        let used = self.used.borrow();
        if used.is_empty() {
            return None;
        }
        let roots: Vec<String> = used.iter().map(|r| plain(&r.to_string_lossy())).collect();
        Some(if roots.len() == 1 {
            format!("root {}", roots[0])
        } else {
            format!("roots {}", roots.join(", "))
        })
    }

    /// `text` with the header above it, when there is one.
    pub fn with_header(&self, text: String) -> String {
        match self.header() {
            Some(h) if !text.is_empty() => format!("{h}\n{text}"),
            _ => text,
        }
    }
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

/// The file of gitignore patterns a store honours on top of `.gitignore`.
pub const IGNORE_FILE: &str = ".semlithignore";

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

/// How a pass came by its files, which decides when they are held to the
/// boundary and when its time starts.
pub(crate) enum Handed {
    /// Walked or named by this call. Every file is held to the boundary before
    /// any is read, and the deadline, when there is one, is the caller's.
    Walked(Option<std::time::Instant>),
    /// The rest of a run, or a batch of watcher events. Each file is held to
    /// the boundary as the pass reaches it; the budget starts once the pass is
    /// ready to read; `bytes_total` is the run's, when the caller knows it.
    Rest {
        budget: std::time::Duration,
        bytes_total: Option<u64>,
    },
}

/// What a walk found, and what it could not read.
///
/// A pair rather than a bare `Vec` since 0.19.0: the entries the walk gave up
/// on are part of what the run has to report, and a function that returns only
/// the successes gives its caller nothing to report them with.
/// What an index pass is handed to index: a walk already done, or roots to
/// walk, which a pass that uses the lanes walks beside its first files.
pub(crate) enum Walk {
    Done(Walked),
    Roots(Vec<PathBuf>),
}

pub(crate) struct Walked {
    /// Milliseconds the walk took, for the run's stage timings. Zero where
    /// the caller handed over paths it already had.
    pub walk_ms: u64,
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
    /// Credential files by name that the hidden-file rule stepped over, so
    /// they can be listed rather than silently absent.
    pub credentials: Vec<PathBuf>,
    /// Files and folders an ignore rule left out, each with the rule.
    pub excluded: Vec<(PathBuf, String)>,
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

/// The files `semlith cloud push` may send from `root`, and every one it may
/// not, with the reason.
///
/// The index pass's own rules, not a copy of them: the same walk (hidden
/// files, `.gitignore`, `.semlithignore`, generated and vendored folders),
/// the same boundary (the deny-list, credential names), the same secret scan
/// — a file holding a live-looking value is refused, a declared dummy is not
/// — and a size cap of the push's own. The cloud applies the list again;
/// this is what keeps a file from leaving the machine in the first place.
pub fn push_files(root: &Path, cap: u64) -> (Vec<PathBuf>, Vec<(PathBuf, String)>) {
    let walked = walk_allowing(&[root.to_path_buf()], &[], true);
    let home = crate::home::user_home().ok().map(|h| canonical(&h));
    let boundary = Boundary {
        roots: None,
        allow_secrets: false,
    }
    .resolved(home.as_deref());
    let mut ok = Vec::new();
    let mut refused: Vec<(PathBuf, String)> = walked.unreadable;
    for path in walked.credentials {
        refused.push((path, "a credential file by its name".to_string()));
    }
    for path in walked.files.into_iter().chain(walked.named) {
        if let Some(refusal) = boundary.refuses(&path, true) {
            refused.push((path, refusal.why));
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            refused.push((path, "could not be read".to_string()));
            continue;
        };
        if bytes.len() as u64 > cap {
            refused.push((path, format!("over {} MB", cap / (1 << 20))));
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        if let Some(live) = keyscan::scan(&path.to_string_lossy(), &text)
            .into_iter()
            .find(|m| m.dummy.is_none())
        {
            refused.push((
                path,
                format!("holds what looks like {} at line {}", live.kind, live.line),
            ));
            continue;
        }
        ok.push(path);
    }
    refused.sort();
    (ok, refused)
}

#[cfg(test)]
fn walk(roots: &[PathBuf]) -> Walked {
    walk_allowing(roots, &[], true)
}

/// The bytes a run's files count for, one `stat` each: microseconds against
/// the read, hash and embed that follow, and what the remaining-time estimate
/// is taken from. A file over the cap is skipped unread and counts nothing.
fn bytes_of(paths: &[PathBuf]) -> u64 {
    paths
        .iter()
        .map(|p| embeddable_bytes(p.metadata().ok()))
        .sum()
}

/// The chunks a file's text makes, cut where the run will cut it: at the
/// symbols the parser finds, as `pipeline::take_ready` does. Without them a
/// JSON or code file counted a fraction of what it wrote.
fn planned_chunks(path: &Path, text: &str) -> u64 {
    let symbols = contained(|| graph::extract(path, text))
        .ok()
        .and_then(|r| r.ok())
        .flatten()
        .map(|e| e.symbols)
        .unwrap_or_default();
    chunk::chunk_file(path, text, &symbols).len() as u64
}

/// The chunks and images a list of files is expected to hold. See
/// [`progress::estimate`].
fn expected_of(
    paths: &[PathBuf],
    planned: Option<&std::collections::HashMap<String, u64>>,
) -> (u64, usize) {
    let mut chunks = 0;
    let mut images = 0;
    for path in paths {
        let (n, image) = expected_one(path, planned);
        chunks += n;
        images += usize::from(image);
    }
    (chunks, images)
}

fn expected_one(
    path: &Path,
    planned: Option<&std::collections::HashMap<String, u64>>,
) -> (u64, bool) {
    let bytes = embeddable_bytes(path.metadata().ok());
    // With a plan, a file it did not count is one it found unchanged or
    // will not index: nothing to embed. Images are never in its counts.
    let planned = planned.map(|p| p.get(path.to_string_lossy().as_ref()).copied().unwrap_or(0));
    progress::estimate(path, bytes, planned)
}

/// How many recently committed files a pass starts on before its walk is done.
const HEAD_FILES: usize = 48;

/// The files the last commits under `roots` touched, newest first, held to
/// the rules the walk applies — not hidden, not in a generated folder a
/// person has not accepted, not left out by `.semlithignore`, inside the
/// boundary — so a pass can start on them before the walk of the whole tree is
/// done. What `.gitignore` leaves out git does not commit. Empty outside a
/// git repository or without git.
fn recent_head(
    roots: &[PathBuf],
    boundary: &ResolvedBoundary,
    accepted: &[PathBuf],
) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    // A root that is a repository, or a folder of them: the repositories up
    // to two levels down, which is how a projects folder is laid out.
    let mut repos: Vec<PathBuf> = Vec::new();
    for root in roots {
        let root = canonical(root);
        if !root.is_dir() {
            continue;
        }
        let mut level = vec![root];
        for _ in 0..3 {
            let mut next = Vec::new();
            for dir in level {
                if dir.join(".git").exists() {
                    repos.push(dir);
                    continue;
                }
                for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if entry.file_type().is_ok_and(|t| t.is_dir())
                        && !entry.file_name().to_string_lossy().starts_with('.')
                        && !is_generated_dir(&path)
                    {
                        next.push(path);
                    }
                }
            }
            level = next;
        }
    }
    // A share of the head each, so one busy repository does not take it all.
    let each = (HEAD_FILES / repos.len().max(1)).max(4);
    for root in repos {
        let mut taken = 0;
        let Ok(log) = std::process::Command::new("git")
            .arg("-C")
            .arg(&root)
            .args([
                "log",
                "-n",
                "60",
                "--name-only",
                "--format=%x00",
                "--relative",
                "--",
                ".",
            ])
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        // Within one commit every file is as recent as the next. A commit
        // that touched hundreds, as a shallow clone's only commit does, would
        // start the pass on its alphabetical first few, the licence and the
        // changelog; code goes first, which is what an agent asks about.
        let text = String::from_utf8_lossy(&log.stdout);
        let mut lines: Vec<&str> = Vec::new();
        for commit in text.split('\0') {
            let mut names: Vec<&str> = commit.lines().collect();
            names.sort_by_key(|name| !filter::is_code(name.trim()));
            lines.extend(names);
        }
        for line in lines {
            if out.len() >= HEAD_FILES {
                return out;
            }
            if taken >= each {
                break;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let rel = Path::new(line);
            let hidden = rel
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with('.'));
            let path = root.join(rel);
            if hidden || out.contains(&path) || !path.is_file() {
                continue;
            }
            let generated = path
                .ancestors()
                .skip(1)
                .take_while(|a| a.starts_with(&root) && *a != root)
                .any(|a| is_generated_dir(a) && !accepted.iter().any(|x| x.as_path() == a));
            let ignored = path
                .parent()
                .and_then(semlithignore_for)
                .is_some_and(|m| m.matched_path_or_any_parents(&path, false).is_ignore());
            if generated || ignored || boundary.refuses(&path, true).is_some() {
                continue;
            }
            out.push(path);
            taken += 1;
        }
    }
    out
}

/// Put the most recently changed files first: by the later of a file's
/// modification time and the time of the last git commit that touched it.
///
/// Both, because each misses what the other sees. A fresh clone gives every
/// file the same mtime, and git has not heard of an edit nobody committed.
/// Stable, so files changed at the same second keep their path order.
fn recent_first(paths: &mut [PathBuf]) {
    let committed = git_recency(paths);
    let when = |path: &Path| -> i64 {
        let mtime = path
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        mtime.max(committed.get(path).copied().unwrap_or(0))
    };
    let mut keyed: Vec<(i64, PathBuf)> = paths.iter().map(|p| (when(p), p.clone())).collect();
    keyed.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    for (slot, (_, path)) in paths.iter_mut().zip(keyed) {
        *slot = path;
    }
}

/// When each file was last committed, for the repositories the paths sit in:
/// the last few hundred commits of each, which is the part of history that
/// says what is being worked on. Nothing when git is not installed.
fn git_recency(paths: &[PathBuf]) -> std::collections::HashMap<PathBuf, i64> {
    const COMMITS: &str = "400";
    let mut tops: std::collections::BTreeSet<PathBuf> = Default::default();
    let mut seen: std::collections::HashSet<PathBuf> = Default::default();
    for path in paths {
        let mut dir = path.parent();
        while let Some(d) = dir {
            if !seen.insert(d.to_path_buf()) {
                break;
            }
            if d.join(".git").exists() {
                tops.insert(d.to_path_buf());
                break;
            }
            dir = d.parent();
        }
    }
    let mut out = std::collections::HashMap::new();
    for top in tops {
        let Ok(log) = std::process::Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(["log", "-n", COMMITS, "--name-only", "--format=%x00%ct"])
            .stderr(std::process::Stdio::null())
            .output()
        else {
            continue;
        };
        let mut at = 0i64;
        for line in String::from_utf8_lossy(&log.stdout).lines() {
            if let Some(stamp) = line.strip_prefix('\0') {
                at = stamp.trim().parse().unwrap_or(0);
            } else if !line.is_empty() {
                // Newest first, so the first time a file appears is the last
                // time it changed.
                out.entry(top.join(line)).or_insert(at);
            }
        }
    }
    out
}

/// [`walk`], descending into the generated folders a person accepted (2.5).
fn walk_allowing(roots: &[PathBuf], allowed: &[PathBuf], gitignore: bool) -> Walked {
    let mut out = Vec::new();
    let mut named = Vec::new();
    let mut unreadable = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    // What the walk yielded, as it spelled it, so the pass below compares
    // `read_dir`'s spellings with no `canonicalize` per entry — that call is
    // what the index_failure test counts, and what Windows pays for.
    let mut yielded: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
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
        // Resolved once for the root rather than once per file: a
        // `canonicalize` is a walk up every component, and 69 000 of them
        // were most of a five-second walk of the benchmark corpus. A file
        // under the root is the canonical root joined to its relative path,
        // because the walk never follows a directory link; a file that is
        // itself a link is still resolved, since that is where it points.
        let canonical_root = canonical(root);
        let mut builder = ignore::WalkBuilder::new(root);
        builder
            .hidden(true)
            .git_ignore(gitignore)
            .git_global(gitignore)
            .git_exclude(gitignore)
            .parents(true)
            // Honour `.gitignore` even outside a git repo. A notes or docs
            // folder is a perfectly normal thing to index, and a `.gitignore`
            // sitting in it still means "not this".
            .require_git(false)
            // What to leave out of a store that is not what to leave out of
            // git: a repository that keeps a copy of itself under
            // `tests/fixtures` commits it and does not want it searched.
            // Same syntax, same walk, so the watcher and the catch-up — which
            // ask this walk what counts — honour it too.
            .add_custom_ignore_filename(IGNORE_FILE)
            // `.semlith` is the store itself; the rest is generated output
            // that `.gitignore` covers only where somebody wrote one.
            .filter_entry({
                let generated = generated.clone();
                let allowed = allowed.to_vec();
                move |e| {
                    if e.file_name() == ".semlith" {
                        return false;
                    }
                    if e.file_type().is_some_and(|t| t.is_dir())
                        && is_generated_dir(e.path())
                        && !allowed.iter().any(|a| canonical(e.path()) == *a)
                    {
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
            if entry.file_type().is_some_and(|t| t.is_dir()) {
                dirs.push(entry.path().to_path_buf());
            }
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            yielded.insert(entry.path().to_path_buf());
            let path = match entry.path().strip_prefix(root) {
                Ok(rest) if !entry.path_is_symlink() => canonical_root.join(rest),
                _ => canonical(entry.path()),
            };
            if seen.insert(path.clone()) {
                out.push(path);
            }
        }
    }
    // Credential files the hidden rule stepped over — a `.env`, a `.npmrc` —
    // named so the not-indexed list can say so (2.3, class b). One `read_dir`
    // of each directory the walk entered; nothing under them is read.
    // And what an ignore rule left out, beside it: a file or folder directly
    // in a walked directory that the walk did not yield, not hidden and not
    // generated, was excluded by `.gitignore` or `.semlithignore` (2.3, class
    // e). Said per entry, a folder once, never by walking into it.
    let mut credentials = Vec::new();
    let mut excluded: Vec<(PathBuf, String)> = Vec::new();
    let walked_dirs: std::collections::HashSet<&PathBuf> = dirs.iter().collect();
    let generated_now: Vec<PathBuf> = generated.lock().map(|g| g.clone()).unwrap_or_default();
    // Once for the pass: `filter::denied` would resolve the home per entry.
    let home = crate::home::user_home().ok().map(|h| canonical(&h));
    for dir in &dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let semlithignore = semlithignore_for(dir);
        // Resolved once per directory, and only when something in it is
        // recorded: a store key is canonical, a `canonicalize` per entry is
        // what Windows pays for.
        let mut resolved: Option<PathBuf> = None;
        let mut key = |name: &std::ffi::OsStr| -> PathBuf {
            resolved.get_or_insert_with(|| canonical(dir)).join(name)
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if name.starts_with('.') {
                if kind.is_file()
                    && matches!(
                        filter::denied_against(&path, home.as_deref()),
                        Some(filter::Denied::Name(_))
                    )
                {
                    credentials.push(key(&entry.file_name()));
                }
                continue;
            }
            let held = if kind.is_dir() {
                walked_dirs.contains(&path) || generated_now.iter().any(|g| g == &path)
            } else if kind.is_file() {
                yielded.contains(&path)
            } else {
                true
            };
            if held {
                continue;
            }
            let rule = if semlithignore.as_ref().is_some_and(|m| {
                m.matched_path_or_any_parents(&path, kind.is_dir())
                    .is_ignore()
            }) {
                IGNORE_FILE
            } else {
                ".gitignore"
            };
            excluded.push((key(&entry.file_name()), rule.to_string()));
        }
    }
    credentials.sort();
    credentials.dedup();
    excluded.sort();
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
        walk_ms: 0,
        files: out,
        named,
        unreadable,
        generated,
        credentials,
        excluded,
    }
}

/// The `.semlithignore` in force for a directory: the nearest one at or
/// above it, as a matcher.
fn semlithignore_for(dir: &Path) -> Option<ignore::gitignore::Gitignore> {
    let mut at = Some(dir);
    while let Some(d) = at {
        let file = d.join(IGNORE_FILE);
        if file.is_file() {
            return Some(ignore::gitignore::Gitignore::new(&file).0);
        }
        at = d.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two scopes asked in turn both stay resolved (#183): a one-entry memo
    /// resolved each of them again on every call.
    #[test]
    #[ignore = "downloads an embedding model on first run"]
    fn alternating_scopes_both_stay_resolved() {
        let corpus = tempfile::tempdir().unwrap();
        let store = tempfile::tempdir().unwrap();
        let others: Vec<String> = (0..FILTER_MEMOS).map(|i| format!("other{i}")).collect();
        for dir in ["one", "two"]
            .into_iter()
            .chain(others.iter().map(String::as_str))
        {
            std::fs::create_dir(corpus.path().join(dir)).unwrap();
            std::fs::write(
                corpus.path().join(dir).join("retry.rs"),
                format!("fn backoff_{dir}() {{ let delay = base * 2u32.pow(attempt); }}\n"),
            )
            .unwrap();
        }
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
        let one = Filter::new(&["one/**".into()], &[], &[]).unwrap();
        let two = Filter::new(&["two/**".into()], &[], &[]).unwrap();
        for filter in [&one, &two, &one, &two] {
            s.search_filtered("retry backoff", 4, filter).unwrap();
        }
        assert_eq!(s.filter_memo.len(), 2, "each scope resolved once");
        // The one asked last is the newest.
        assert_eq!(s.filter_memo[0].groups, two.groups());
        // A scope past the memo's size pushes the oldest out, not the newest.
        for dir in &others {
            let f = Filter::new(&[format!("{dir}/**")], &[], &[]).unwrap();
            s.search_filtered("retry backoff", 4, &f).unwrap();
        }
        assert_eq!(s.filter_memo.len(), FILTER_MEMOS);
        assert!(s.filter_memo.iter().all(|m| m.groups != one.groups()));
    }

    /// The head finds repositories below a projects folder, and within one
    /// commit takes the code before the licence and the changelog.
    #[test]
    fn the_head_takes_code_first_from_repositories_below_the_root() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("group").join("app");
        std::fs::create_dir_all(repo.join("src")).unwrap();
        for (name, text) in [
            ("CHANGELOG.md", "# Changes\n"),
            ("LICENSE", "MIT\n"),
            ("src/main.rs", "fn main() {}\n"),
        ] {
            std::fs::write(repo.join(name), text).unwrap();
        }
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .output()
                .unwrap()
        };
        if !git(&["init", "-q"]).status.success() {
            return; // no git on this machine: the head is empty by design
        }
        git(&["add", "."]);
        assert!(git(&["commit", "-q", "-m", "one"]).status.success());
        let head = recent_head(
            &[root.path().to_path_buf()],
            &Boundary::default().resolved(Some(Path::new("/nonexistent-home"))),
            &[],
        );
        let names: Vec<String> = head
            .iter()
            .map(|p| {
                p.strip_prefix(canonical(&repo))
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        let main = Path::new("src").join("main.rs").display().to_string();
        assert_eq!(names.first(), Some(&main), "{names:?}");
        assert_eq!(names.len(), 3, "{names:?}");
    }

    /// For a question that does not name tests, no test hit sits above the
    /// first product-code hit; prose keeps its place (1.15).
    #[test]
    fn product_code_comes_before_tests_and_prose_stays_put() {
        let hit = |path: &str| {
            (
                Hit {
                    score: 1.0,
                    path: path.to_string(),
                    start_line: 1,
                    end_line: 2,
                    text: path.to_string(),
                    store: None,
                    lists: Vec::new(),
                    image: None,
                    fresh: true,
                    symbol: None,
                    symbol_kind: None,
                    symbol_line: None,
                    copies: Vec::new(),
                },
                1.0,
            )
        };
        let mut hits = vec![
            hit("/r/README.md"),
            hit("/r/tests/watch.rs"),
            hit("/r/tests/fixtures/x.rs"),
            hit("/r/src/lib.rs"),
            hit("/r/src/mcp.rs"),
        ];
        product_first(&mut hits);
        let order: Vec<&str> = hits.iter().map(|(h, _)| h.path.as_str()).collect();
        assert_eq!(
            order,
            [
                "/r/README.md",
                "/r/src/lib.rs",
                "/r/tests/watch.rs",
                "/r/tests/fixtures/x.rs",
                "/r/src/mcp.rs"
            ]
        );
    }

    /// A `.semlithignore` leaves its patterns out of a walk from above, and a
    /// walk rooted inside an ignored directory still sees that directory's
    /// files: the retrieval harness indexes its pinned corpus from inside the
    /// repository whose `.semlithignore` excludes it.
    #[test]
    fn semlithignore_leaves_a_copy_out_and_a_root_inside_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let root = canonical(dir.path());
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("tests/fixtures/corpus/src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join("tests/fixtures/corpus/src/lib.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join(IGNORE_FILE), "tests/fixtures/corpus/\n").unwrap();

        let whole = walk(std::slice::from_ref(&root)).files;
        assert_eq!(whole, vec![root.join("src/lib.rs")], "{whole:?}");

        let corpus = root.join("tests/fixtures/corpus");
        let inside = walk(std::slice::from_ref(&corpus)).files;
        assert_eq!(inside, vec![corpus.join("src/lib.rs")], "{inside:?}");
    }

    /// Release notes are told apart by name, and only by name.
    #[test]
    fn release_notes_are_named_files_and_changelog_directories() {
        for path in [
            "/w/repo/CHANGELOG.md",
            "/w/repo/CHANGES.rst",
            "/w/repo/HISTORY.md",
            "/w/repo/NEWS",
            "/w/repo/docs/changelogs/v1.2.md",
        ] {
            assert!(is_release_notes(path), "{path}");
        }
        for path in [
            "/w/repo/README.md",
            "/w/repo/src/history.rs",
            "/w/repo/src/news_feed.rs",
        ] {
            assert!(!is_release_notes(path), "{path}");
        }
        assert!(names_releases("what changed in the 1.2 release"));
        assert!(!names_releases("how does the parser recover from an error"));
    }

    /// The identifiers a sentence names: code-shaped words, not English.
    #[test]
    fn a_sentence_names_its_code_shaped_words() {
        assert_eq!(
            named_identifiers("why does `parse_args` call Config::load and buildRequest() twice"),
            ["parse_args", "load", "buildRequest"]
        );
        assert!(named_identifiers("how does the server shut down").is_empty());
        assert_eq!(
            named_identifiers("a_b c_d e_f g_h").len(),
            3,
            "capped at three"
        );
    }

    /// The project prior is nothing on a store of one project, and on a store
    /// of several it favours the project the head agrees on.
    #[test]
    fn the_project_prior_follows_the_head_and_ignores_one_project() {
        let root = tempfile::tempdir().unwrap();
        for repo in ["one", "two"] {
            std::fs::create_dir_all(root.path().join(repo).join(".git")).unwrap();
            std::fs::create_dir_all(root.path().join(repo).join("src")).unwrap();
        }
        let hit = |repo: &str, score: f32| {
            (
                Hit {
                    score,
                    path: root
                        .path()
                        .join(repo)
                        .join("src/a.rs")
                        .display()
                        .to_string(),
                    start_line: 1,
                    end_line: 2,
                    text: String::new(),
                    store: None,
                    lists: vec!["vector"],
                    image: None,
                    fresh: true,
                    symbol: None,
                    symbol_kind: None,
                    symbol_line: None,
                    copies: Vec::new(),
                },
                0.0,
            )
        };
        let hits = vec![hit("one", 3.0), hit("one", 2.0), hit("two", 1.0)];
        let (shares, named) = project_prior(&hits, "how does two handle it");
        assert!(shares[0] > shares[2], "{shares:?}");
        assert!((shares[0] - 5.0 / 6.0).abs() < 1e-6, "{shares:?}");
        assert_eq!(
            named,
            [false, false, false],
            "a word of three letters names nothing"
        );

        let alone = vec![hit("one", 3.0), hit("one", 2.0)];
        let (shares, _) = project_prior(&alone, "anything");
        assert_eq!(shares, [0.0, 0.0]);
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
        // Each penalty stays under 1.6x on its own. The two penalties are
        // about different properties — a file edited since, a file under
        // tests/ — and a stale test is the one case both reach. 0.36.0 removed
        // the third factor, the graph walk's proximity lift, with the graph
        // list it came from.
        for penalty in [STALE_PENALTY, TEST_PENALTY] {
            let weakest = 1.0 - penalty;
            assert!(
                1.0 / weakest < 1.6,
                "the rerank spans 1/{weakest}, which is a ranking rather than a tiebreak"
            );
        }
        const { assert!(STALE_PENALTY > 0.0 && TEST_PENALTY > 0.0) };
        // A stale hit is pushed down, never removed: the excerpt in hand may
        // still be the best answer there is.
        const { assert!(STALE_PENALTY < 1.0 && TEST_PENALTY < 1.0) };
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

    /// `n` empty files under a fresh directory, canonical, in walk order, and
    /// a store confined to that directory.
    fn continuation(n: usize) -> (tempfile::TempDir, tempfile::TempDir, PathBuf, Semlith) {
        let corpus = tempfile::tempdir().unwrap();
        let root = canonical(corpus.path());
        for i in 0..n {
            std::fs::write(root.join(format!("{i:04}.txt")), b"").unwrap();
        }
        let dir = tempfile::tempdir().unwrap();
        let mut s = Semlith::open(dir.path(), None).unwrap();
        s.boundary = Boundary::within(vec![root.clone()]);
        (corpus, dir, root, s)
    }

    /// 0.30.1, item 1: a continuation slice checks the files it reaches, not
    /// the files left. On 0.30.0 every one of the N remaining paths was
    /// canonicalised (and every root with it) before the first file was read,
    /// so a slice that got through K files still paid for N.
    #[test]
    fn a_continuation_slice_checks_only_the_files_it_reaches() {
        const N: usize = 300;
        const K: usize = 10;
        let (_corpus, _dir, root, mut s) = continuation(N);
        let files: Vec<PathBuf> = (0..N).map(|i| root.join(format!("{i:04}.txt"))).collect();

        // The daemon's slice ends on its clock; this one ends on a count, so
        // the test does not depend on how fast the machine is.
        let asked = std::cell::Cell::new(0usize);
        let control = || {
            asked.set(asked.get() + 1);
            if asked.get() > K {
                Flow::Yield
            } else {
                Flow::Run
            }
        };
        let before = canonical_calls();
        let report = s
            .index_rest_held_under(
                files,
                std::time::Duration::from_secs(3600),
                Some(0),
                &control,
                |_, _| {},
            )
            .unwrap();
        let spent = canonical_calls() - before;

        assert_eq!(report.scanned, K);
        assert_eq!(report.pending.len(), N - K);
        // One per file reached, plus the root and the home resolved once.
        assert!(
            spent <= K as u64 + 4,
            "{spent} canonicalisations for a slice that reached {K} of {N} files"
        );
    }

    /// Checked as it is reached is checked just as hard: a path outside the
    /// store's roots handed to a continuation slice is refused, with the reason
    /// a first slice gives it.
    #[test]
    fn a_continuation_slice_refuses_what_a_first_slice_refuses() {
        let (_corpus, _dir, _root, mut s) = continuation(0);
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = canonical(elsewhere.path()).join("outside.txt");
        std::fs::write(&outside, b"not this store's").unwrap();

        let first = s
            .index_paths_within(
                std::slice::from_ref(&outside),
                std::time::Duration::from_secs(3600),
                |_, _| {},
            )
            .unwrap();
        let rest = s
            .index_rest_held_under(
                vec![outside.clone()],
                std::time::Duration::from_secs(3600),
                None,
                &|| Flow::Run,
                |_, _| {},
            )
            .unwrap();

        assert_eq!(first.refused.len(), 1, "{:?}", first.refused);
        assert!(first.refused[0].1.contains("outside this store's roots"));
        assert_eq!(rest.refused, first.refused);
        assert_eq!(rest.indexed, 0);
    }

    /// A slice cut short counts as remaining only the files it did not reach.
    /// It used to subtract its position among the admitted files from a total
    /// that also held the refused ones, so a run's settled total came out one
    /// higher for every refused file and its counter never reached it.
    #[test]
    fn a_cut_slice_counts_only_what_it_did_not_reach_as_remaining() {
        const N: usize = 20;
        const K: usize = 5;
        let (_corpus, _dir, root, mut s) = continuation(N);
        let elsewhere = tempfile::tempdir().unwrap();
        let outside = canonical(elsewhere.path()).join("outside.txt");
        std::fs::write(&outside, b"not this store's").unwrap();

        let asked = std::cell::Cell::new(0usize);
        let control = || {
            asked.set(asked.get() + 1);
            if asked.get() > K {
                Flow::Yield
            } else {
                Flow::Run
            }
        };
        let report = s
            .index_within_held_under(
                &[root, outside],
                std::time::Duration::from_secs(3600),
                &control,
                |_, _| {},
            )
            .unwrap();
        assert_eq!(report.refused.len(), 1);
        assert_eq!(report.pending.len(), N - K);
        assert_eq!(report.remaining, N - K);
        // What the daemon settles the run's total on.
        assert_eq!(report.scanned + report.remaining, N + 1);
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
