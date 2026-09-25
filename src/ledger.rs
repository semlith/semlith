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

/// An id for one retrieval, shared by every row it writes.
///
/// A cross-store search writes a row in each store that answered it, because
/// that is what keeps each store's hash chain its own and its token figures
/// about its own hits. Without something tying those rows together they are
/// counted as several retrievals, which is how the Ledger page came to report
/// sixty-three queries for about a dozen searches — every figure on it
/// multiplied by the number of stores that happened to be open.
///
/// The clock and a counter, because two retrievals in the same nanosecond are
/// possible and two in the same nanosecond in the same process are not.
fn query_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{at:x}-{:x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Record one search-shaped retrieval into every store that answered it./// Record one search-shaped retrieval into every store that answered it.
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
    let id = query_id();

    // Whether the hits know which store they came from. The fleet labels them
    // when it merges, and a single-store search does not — so `None` means
    // "the one store this came from", not "every store". Read once for the
    // answer rather than once per hit per store: treating an unlabelled hit as
    // belonging to whichever store was being considered is what wrote the same
    // row into all six of them.
    let labelled = hits.iter().any(|h| h.store.is_some());

    let mut first = true;
    for (label, store) in fleet.each() {
        // Only the stores this answer actually came from, so a search across
        // three stores does not write three identical rows. A search that
        // found nothing came from none of them and is recorded once, against
        // the first, so the zero-hit row exists to be counted.
        let mine: Vec<&crate::Hit> = if labelled {
            hits.iter()
                .filter(|h| h.store.as_deref() == Some(label))
                .collect()
        } else if first {
            hits.iter().collect()
        } else {
            Vec::new()
        };
        first = false;
        if mine.is_empty() && !hits.is_empty() {
            continue;
        }
        // This store's excerpt cost, not the whole answer's. Both sides of the
        // ratio describe the same hits or the row describes nothing.
        let excerpt: i64 = mine.iter().map(|h| counter.count(&h.text)).sum();
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
                query_id: &id,
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
                query_id: &query_id(),
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
    let id = query_id();
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
                query_id: &id,
            },
        );
    }
}

/// Record one brief, from the brief itself rather than from its rendering.
///
/// [`reply`] recovers the files an answer named by reading them back out of the
/// rendered text, which works for the MCP tools whose rendering it was written
/// against and silently does not for `semlith brief` at a terminal: the CLI
/// hands it the JSON, `paths_in` finds no bare path or `path:start-end` row in
/// it, and every brief is recorded with no hits and no saving.
///
/// The cost of that was not a missing row. It was a ledger in which the one
/// command 0.23.0 exists for was counted, 107 times out of 107, as a retrieval
/// that answered nothing — so Coverage read 0 %, the saving read zero, and the
/// figures this release is built on described the opposite of what happened.
///
/// A brief already knows which files it named. This asks it instead of parsing
/// its own output back.
pub fn brief(
    fleet: &Fleet,
    who: &Who<'_>,
    question: &str,
    brief: &crate::brief::Brief,
    body: &str,
    elapsed: std::time::Duration,
) {
    if !enabled() {
        return;
    }
    let counter = fleet.counter();
    let paths: std::collections::BTreeSet<&str> =
        brief.spans.iter().map(|s| s.path.as_str()).collect();
    let whole: i64 = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok())
        .map(|m| counter.count_bytes(m.len()))
        .sum();
    if let Some((_, store)) = fleet.each().next() {
        let _ = store::record_retrieval(
            store.db(),
            &store::NewRetrieval {
                client: who.client,
                session: who.session,
                tool: "brief",
                query: question,
                hits: paths.len() as i64,
                micros: elapsed.as_micros() as i64,
                excerpt_tokens: counter.count(body),
                whole_file_tokens: whole,
                stale_hits: 0,
                tokenizer: counter.label(),
                query_id: &query_id(),
            },
        );
    }
}

/// The tool name on a row the steering hook wrote.
///
/// Its own name rather than a column of its own: the ledger is one table with a
/// `tool`, and a raw read is a retrieval semlith did not get to serve.
pub const RAW_READ: &str = "raw-read";

/// Record one whole-file read an agent made instead of asking semlith.
///
/// This is what turns Refunds from an estimate into a measurement. Everything
/// else in the ledger counts what semlith answered, which flatters the ratio by
/// leaving out every question it was never asked; the steering hook sees those
/// and this is where they land.
///
/// `hits` is zero, deliberately. A raw read is a retrieval that returned
/// nothing from semlith, so it is uncredited: it lowers Coverage and is
/// excluded from the saving, which is exactly what happened.
///
/// The row goes into the store whose roots cover the file. A file no open store
/// holds is not recorded at all, and `false` says so — Refunds counts reads of
/// files semlith could have answered, and nothing else.
pub fn raw_read(fleet: &Fleet, client: &str, session: &str, path: &str) -> bool {
    if !enabled() {
        return false;
    }
    let counter = fleet.counter();
    let whole = std::fs::metadata(path)
        .map(|m| counter.count_bytes(m.len()))
        .unwrap_or(0);
    for (_, store) in fleet.each() {
        if !store::holds_path(store.db(), path).unwrap_or(false) {
            continue;
        }
        let recorded = store::record_retrieval(
            store.db(),
            &store::NewRetrieval {
                client,
                session,
                tool: RAW_READ,
                query: path,
                // Nothing was retrieved from semlith. This is the row's whole
                // point, and crediting it would make the ledger count a read it
                // failed to prevent as a saving.
                hits: 0,
                micros: 0,
                excerpt_tokens: 0,
                whole_file_tokens: whole,
                stale_hits: 0,
                // The denominator is a file size, which is counted the same way
                // on every row whose denominator is one.
                tokenizer: CHARS4,
                query_id: &query_id(),
            },
        );
        return recorded.is_ok();
    }
    false
}

/// Record that a person accepted or revoked one refused file (2.6).
///
/// Hash-chained like every other row, with the path, class, mode and
/// confidence in the query column — never the value, which the acceptance
/// itself never holds either. `source` is `portal` or `cli`, which is what
/// the client column says.
pub fn acceptance(
    db: &rusqlite::Connection,
    a: &store::Acceptance,
    action: &str,
) -> anyhow::Result<()> {
    if !enabled() {
        return Ok(());
    }
    let query = serde_json::json!({
        "path": crate::plain(&a.path),
        "class": a.class,
        "mode": a.mode,
        "confidence": a.confidence,
    })
    .to_string();
    store::record_retrieval(
        db,
        &store::NewRetrieval {
            client: &a.source,
            session: "",
            tool: action,
            query: &query,
            hits: 0,
            micros: 0,
            excerpt_tokens: 0,
            whole_file_tokens: 0,
            stale_hits: 0,
            tokenizer: CHARS4,
            query_id: &query_id(),
        },
    )
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
