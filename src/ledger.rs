//! Writing the retrieval ledger, from wherever a retrieval happened.
//!
//! Until 0.15.0 there was one caller: the portal's own search box. Every
//! retrieval an agent made — over stdio, over the daemon's `/mcp` endpoint,
//! from the command line — went unrecorded, so the ledger the product exists
//! to keep counted the one user who was not the point. The 2026-09-14 study
//! found the table empty on real installations for exactly that reason.
//!
//! So the write moved here, out of `routes.rs`, and the three surfaces call
//! it. One place decides what a row says, which is what keeps a search from
//! the CLI and the same search from an agent from being counted differently.
//!
//! Nothing here leaves the machine. A row is written into the store it came
//! from, beside the chunks, and deleting every one of them is one `DELETE`.

use crate::fleet::Fleet;
use crate::store;

/// The label on a row counted at four characters per token.
///
/// Both sides of a ratio are always counted the same way, and the row says
/// which way, so rows counted two different ways are never summed together.
pub const CHARS4: &str = "chars4";

/// The environment variable that turns recording off for good.
///
/// For a machine that should never keep a record: a shared build box, a
/// container, somebody else's laptop. Read on every write rather than cached,
/// so setting it takes effect without restarting anything.
pub const OFF_ENV: &str = "SEMLITH_LEDGER";

/// Whether a retrieval may be recorded at all.
///
/// `SEMLITH_LEDGER=0` is the answer to "I never want this recorded anywhere";
/// `semlith start --no-ledger` is the answer to "not in this session". The two
/// exist separately because they are different promises.
pub fn enabled() -> bool {
    !matches!(
        std::env::var(OFF_ENV).as_deref(),
        Ok("0") | Ok("off") | Ok("false")
    )
}

/// Four characters to a token: a rough rule, said to be one.
pub fn estimate(text: &str) -> i64 {
    text.len().div_ceil(4) as i64
}

/// What counted a row's two token figures.
///
/// A savings ratio is a comparison, so what matters is that both sides were
/// counted the same way — and what matters *across* rows is that a row counted
/// one way is never added to a row counted the other. Hence the label, stored
/// per row rather than inferred from the row's age.
#[derive(Clone, Copy)]
pub enum Counter<'a> {
    /// The tokenizer the store's own embedding model uses. The honest count.
    Model(&'a tokenizers::Tokenizer),
    /// Four characters to a token, when no model has been loaded. A graph-only
    /// session never loads one, and an airgapped machine with no cached model
    /// never can.
    Chars4,
}

impl Counter<'_> {
    pub fn count(&self, text: &str) -> i64 {
        match self {
            Counter::Model(tokenizer) => match tokenizer.encode(text, false) {
                Ok(encoded) => encoded.len() as i64,
                // A tokenizer that refuses a string is not a reason to lose the
                // row; the estimate is labelled for what it is.
                Err(_) => estimate(text),
            },
            Counter::Chars4 => estimate(text),
        }
    }

    /// Bytes on disk, which are never tokenized because reading a file to
    /// count it would cost more than the saving being measured.
    ///
    /// Four characters per token either way, and the label says `chars4` on
    /// any row where the denominator was counted this way — which is every row
    /// whose denominator is a file size.
    pub fn count_bytes(&self, bytes: u64) -> i64 {
        bytes.div_ceil(4) as i64
    }

    pub fn label(&self) -> &'static str {
        match self {
            Counter::Model(_) => MODEL,
            Counter::Chars4 => CHARS4,
        }
    }
}

/// The label on a row whose excerpt side was counted by the store's own
/// tokenizer.
pub const MODEL: &str = "model";

/// Record one search-shaped retrieval into every store that answered it.
///
/// `whole_file_tokens` is what reading those files whole would have cost,
/// which is the honest denominator: the ratio is measured against a real
/// alternative rather than an imagined one.
///
/// A search that found nothing is still recorded, and credited nothing. A
/// ledger that only remembers its successes is a marketing document.
pub fn search(
    fleet: &Fleet,
    who: &Who<'_>,
    query: &str,
    hits: &[crate::Hit],
    elapsed: std::time::Duration,
) {
    let counter = fleet.counter();
    if !enabled() {
        return;
    }
    let excerpt: i64 = hits.iter().map(|h| counter.count(&h.text)).sum();

    for (label, store) in fleet.each() {
        // Only the stores this answer actually came from, so a search across
        // three stores does not write three identical rows. A search that
        // found nothing came from none of them and is recorded once, against
        // the first, so the zero-hit row exists to be counted.
        let mine: Vec<&crate::Hit> = hits
            .iter()
            .filter(|h| h.store.as_deref().is_none_or(|s| s == label))
            .collect();
        if mine.is_empty() && !hits.is_empty() {
            continue;
        }
        let paths: std::collections::BTreeSet<&str> =
            mine.iter().map(|h| h.path.as_str()).collect();
        let whole: i64 = paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| counter.count_bytes(m.len()))
            .sum();
        let stale = mine.iter().filter(|h| !h.fresh).count() as i64;
        let _ = store::record_retrieval(
            store.db(),
            &store::NewRetrieval {
                client: who.client,
                session: who.session,
                tool: "search",
                query,
                hits: mine.len() as i64,
                micros: elapsed.as_micros() as i64,
                excerpt_tokens: excerpt,
                whole_file_tokens: whole,
                stale_hits: stale,
                tokenizer: counter.label(),
            },
        );
        if hits.is_empty() {
            break;
        }
    }
}

/// Record a search whose hits are no longer in hand, from its reply.
///
/// The MCP path renders its hits into text and drops them, so what is left to
/// measure is the reply itself - which is the honest thing to measure anyway:
/// the reply is what the agent paid for. The denominator is the files that
/// reply names, read whole.
pub fn reply(
    fleet: &Fleet,
    who: &Who<'_>,
    tool: &str,
    query: &str,
    body: &str,
    elapsed: std::time::Duration,
) {
    if !enabled() {
        return;
    }
    let counter = fleet.counter();
    let paths = paths_in(body);
    let whole: i64 = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| counter.count_bytes(m.len()))
        .sum();
    let hits = paths.len() as i64;
    // One row for one answer. A reply that has already been rendered to text
    // cannot be attributed store by store, so it is recorded against the first
    // store open rather than duplicated across all of them.
    if let Some((_, store)) = fleet.each().next() {
        let _ = store::record_retrieval(
            store.db(),
            &store::NewRetrieval {
                client: who.client,
                session: who.session,
                tool,
                query,
                hits,
                micros: elapsed.as_micros() as i64,
                excerpt_tokens: counter.count(body),
                whole_file_tokens: whole,
                stale_hits: body.matches("· stale").count() as i64,
                tokenizer: counter.label(),
            },
        );
    }
}

/// The file paths a rendered reply names.
///
/// A locate reply puts a bare path on its own line and indents the rows under
/// it; an excerpt reply puts `path:start-end` after a `[n]` marker. Both are
/// read here, and a candidate is only kept if it is a file that exists, so a
/// line of prose that happens to contain a slash is not counted as a read.
fn paths_in(body: &str) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for line in body.lines() {
        let trimmed = line
            .trim_start_matches(|c: char| c == '[' || c.is_ascii_digit() || c == ']')
            .trim();
        if trimmed.is_empty() {
            continue;
        }
        // A locate reply's file heading: a bare path, alone on its line.
        if !trimmed.contains(' ') && std::path::Path::new(trimmed).is_file() {
            out.insert(trimmed.to_string());
            continue;
        }
        // An excerpt row: `path:start-end (score ...)`.
        if let Some((path, _)) = trimmed.split_once(':')
            && std::path::Path::new(path).is_file()
        {
            out.insert(path.to_string());
        }
    }
    out
}

/// Record one graph answer: `neighbors`, `path` or `symbol`.
///
/// A graph answer has no hits and no excerpts, and it still replaced work. The
/// work it replaced is a grep for the name and a read of whatever came back,
/// so that is the denominator: the bytes of every file that defines the name
/// or points at it. `reply` is what the agent was handed instead.
pub fn graph(
    fleet: &Fleet,
    who: &Who<'_>,
    tool: &str,
    name: &str,
    reply: &str,
    found: bool,
    elapsed: std::time::Duration,
) {
    if !enabled() {
        return;
    }
    let counter = fleet.counter();
    for (_, store) in fleet.each() {
        let whole = match store::grep_cost(store.db(), name) {
            Ok(bytes) => counter.count_bytes(bytes as u64),
            Err(_) => 0,
        };
        // A store that knows nothing about the name did not answer, and a row
        // against it would credit this store with work it did not do.
        if whole == 0 {
            continue;
        }
        let _ = store::record_retrieval(
            store.db(),
            &store::NewRetrieval {
                client: who.client,
                session: who.session,
                tool,
                query: name,
                hits: i64::from(found),
                micros: elapsed.as_micros() as i64,
                excerpt_tokens: counter.count(reply),
                whole_file_tokens: whole,
                // A graph answer quotes no file, so none of it can be stale.
                stale_hits: 0,
                tokenizer: counter.label(),
            },
        );
    }
}

/// Who made a retrieval, and which conversation it belonged to.
#[derive(Debug, Clone, Copy)]
pub struct Who<'a> {
    /// The client's own name for itself: `claude-code`, `cursor`, `cli`,
    /// `portal`. Taken from the MCP handshake where there is one, so the
    /// ledger says who rather than saying "an agent".
    pub client: &'a str,
    /// One conversation. Lets a session be read, and credited, as a unit.
    pub session: &'a str,
}
