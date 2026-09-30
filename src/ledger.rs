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

use crate::Semlith;
use crate::fleet::Fleet;
use crate::store;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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
                client_version: who.version,
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
///
/// One row per store the reply's headings name, sharing one query id, so a
/// fleet answer is counted the way [`search`] counts one: each store's chain
/// and figures are about its own hits. Until 0.33.0 the whole reply went into
/// the first store open with zero hits — its headings are `label path` with the
/// path relative to a root, which the old reader resolved against the daemon's
/// working directory and never found — so every proxied session in the access
/// report sat under one store as a zero-hit read.
///
/// `scope` is the call's `store` argument. A reply that names no file is still
/// one row, against the first store it was scoped to — every one of them was
/// searched and none answered, so one row keeps it one zero-hit retrieval
/// rather than one per store — or against the first store open when unscoped.
pub fn reply(
    fleet: &Fleet,
    who: &Who<'_>,
    tool: &str,
    query: &str,
    body: &str,
    scope: &[String],
    elapsed: std::time::Duration,
) {
    if !enabled() {
        return;
    }
    let counter = fleet.counter();
    let id = query_id();
    let write = |db: &rusqlite::Connection, paths: &BTreeSet<String>, text: &str| {
        let whole: i64 = paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| counter.count_bytes(m.len()))
            .sum();
        let _ = store::record_retrieval(
            db,
            &store::NewRetrieval {
                client: who.client,
                session: who.session,
                client_version: who.version,
                tool,
                query,
                hits: paths.len() as i64,
                micros: elapsed.as_micros() as i64,
                excerpt_tokens: counter.count(text),
                whole_file_tokens: whole,
                stale_hits: text.matches("· stale").count() as i64,
                tokenizer: counter.label(),
                query_id: &id,
            },
        );
    };

    let members: Vec<(&str, &Semlith)> = fleet.each().collect();
    let parts = shares(&members, &fleet.roots(), body);
    if parts.is_empty() {
        let searched = fleet
            .selected(Some(scope))
            .ok()
            .and_then(|chosen| chosen.into_iter().next())
            .or_else(|| members.first().copied());
        if let Some((_, first)) = searched {
            write(first.db(), &BTreeSet::new(), body);
        }
        return;
    }
    for share in &parts {
        write(members[share.member].1.db(), &share.paths, &share.text);
    }
}

/// One store's part of a rendered reply: the files it answered with, and the
/// lines that showed them, which is what the agent paid for.
#[derive(Debug)]
struct Share {
    /// Index into the members the reply was read against.
    member: usize,
    paths: BTreeSet<String>,
    text: String,
}

/// Split a rendered reply into the stores whose files it names.
///
/// Each heading starts a block that runs to the next one, and the block's
/// lines are that store's share of the reply's cost. The lines above the first
/// heading — which roots the paths are relative to, how the query was read —
/// were paid once and go to the store that answered first, so the shares add
/// up to the reply.
fn shares(members: &[(&str, &Semlith)], roots: &[PathBuf], body: &str) -> Vec<Share> {
    let mut out: Vec<Share> = Vec::new();
    let mut current: Option<usize> = None;
    let mut preamble = String::new();
    for line in body.lines() {
        if let Some((member, path)) = heading(members, roots, line) {
            let at = match out.iter().position(|s| s.member == member) {
                Some(at) => at,
                None => {
                    out.push(Share {
                        member,
                        paths: BTreeSet::new(),
                        text: String::new(),
                    });
                    out.len() - 1
                }
            };
            out[at].paths.insert(path);
            current = Some(at);
        }
        let text = match current {
            Some(at) => &mut out[at].text,
            None => &mut preamble,
        };
        text.push_str(line);
        text.push('\n');
    }
    if let Some(first) = out.first_mut() {
        first.text.insert_str(0, &preamble);
    }
    out
}

/// The store and the file one heading of a rendered reply names.
///
/// Every reply shape puts a file at the margin and indents what is under it:
/// a locate heading is `path` or, across stores, `label path`; a brief span and
/// an exact search's file are `[label] path:start-end …` and `[label] path`; an
/// excerpt is `[n] label path:start-end (score …)`. The path is relative to a
/// store root unless no root holds it, and a candidate is kept only if it is a
/// file that exists, so a line of prose containing a slash is not a read.
///
/// A labelled heading belongs to that store, and of the roots that could hold
/// its path the one that store indexed wins — two stores can both hold a
/// `src/lib.rs`. An unlabelled one belongs to whichever store indexed the
/// file, which in a fleet of one is the one store.
fn heading(members: &[(&str, &Semlith)], roots: &[PathBuf], line: &str) -> Option<(usize, String)> {
    if members.is_empty() || line.starts_with(char::is_whitespace) {
        return None;
    }
    let mut rest = line.trim_end();
    // An excerpt's `[n] ` marker.
    if let Some((n, tail)) = rest.strip_prefix('[').and_then(|r| r.split_once("] "))
        && !n.is_empty()
        && n.bytes().all(|b| b.is_ascii_digit())
    {
        rest = tail;
    }
    let mut named = None;
    for (i, (label, _)) in members.iter().enumerate() {
        let tail = rest
            .strip_prefix('[')
            .and_then(|r| r.strip_prefix(*label))
            .and_then(|r| r.strip_prefix("] "))
            .or_else(|| rest.strip_prefix(*label).and_then(|r| r.strip_prefix(' ')));
        if let Some(tail) = tail {
            named = Some(i);
            rest = tail;
            break;
        }
    }
    // `path:12-40 …` ends at the colon before the line number, which a Windows
    // drive's colon is never followed by.
    let path = match rest
        .char_indices()
        .find(|&(i, c)| c == ':' && rest[i + 1..].starts_with(|d: char| d.is_ascii_digit()))
    {
        Some((i, _)) => &rest[..i],
        None => rest.split(" \u{2014} image").next().unwrap_or(rest),
    };
    // Unlabelled, a path is the whole line; a line with a space in it is prose.
    if path.is_empty() || (named.is_none() && path.contains(char::is_whitespace)) {
        return None;
    }

    let given = Path::new(path);
    let candidates: Vec<PathBuf> = if given.is_absolute() {
        vec![given.to_path_buf()]
    } else {
        roots
            .iter()
            .map(|r| r.join(given))
            .chain([given.to_path_buf()])
            .collect()
    };
    let files: Vec<String> = candidates
        .iter()
        .filter(|c| c.is_file())
        .map(|c| crate::canonical(c).to_string_lossy().into_owned())
        .collect();
    let held = |m: usize, file: &str| {
        store::file_hash(members[m].1.db(), file)
            .ok()
            .flatten()
            .is_some()
    };
    let (member, file) = match named {
        Some(m) => (
            m,
            files
                .iter()
                .find(|f| held(m, f.as_str()))
                .or(files.first())?,
        ),
        None => files
            .iter()
            .find_map(|f| {
                (0..members.len())
                    .find(|&m| held(m, f.as_str()))
                    .map(|m| (m, f))
            })
            .or_else(|| files.first().map(|f| (0, f)))?,
    };
    Some((member, file.clone()))
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
                client_version: who.version,
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
/// hands it the JSON, `heading` finds no file heading at the margin of it, and
/// every brief is recorded with no hits and no saving.
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
                client_version: who.version,
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
                client_version: "",
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
            client_version: "",
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
    /// The client's version, from `clientInfo.version`; empty where none was
    /// given. Kept beside the row, outside the hash chain (see
    /// `store::NewRetrieval::client_version`).
    pub version: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store at `dir` that holds `files`, written straight into its table.
    /// What is under test is which store a reply's row lands in, not indexing,
    /// so no model is loaded.
    fn store_holding(dir: &Path, files: &[PathBuf]) -> Semlith {
        std::fs::create_dir_all(dir).unwrap();
        let s = Semlith::open(dir, None).unwrap();
        store::read_only(s.db(), false).unwrap();
        for f in files {
            s.db()
                .execute(
                    "INSERT INTO files (path, hash, bytes, indexed_at) VALUES (?1, 'h', 1, 0)",
                    [crate::canonical(f).to_string_lossy().into_owned()],
                )
                .unwrap();
        }
        store::read_only(s.db(), true).unwrap();
        s
    }

    fn canon(p: &Path) -> String {
        crate::canonical(p).to_string_lossy().into_owned()
    }

    /// Two roots that both hold a `src/lib.rs`, each indexed by its own store,
    /// and a third file only the first holds.
    fn two_roots(tmp: &Path) -> (PathBuf, PathBuf) {
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        for root in [&a, &b] {
            std::fs::create_dir_all(root.join("src")).unwrap();
            std::fs::write(root.join("src").join("lib.rs"), "fn x() {}\n").unwrap();
        }
        std::fs::write(a.join("notes.md"), "# notes\n").unwrap();
        (a, b)
    }

    /// A fleet's locate reply is split by the store each heading names, and a
    /// relative path is resolved against the root that store indexed — not
    /// against the first root that happens to hold a file of that name.
    #[test]
    fn a_fleet_reply_is_split_by_the_store_each_heading_names() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = two_roots(tmp.path());
        let alpha = store_holding(
            &tmp.path().join("alpha"),
            &[a.join("src").join("lib.rs"), a.join("notes.md")],
        );
        let beta = store_holding(&tmp.path().join("beta"), &[b.join("src").join("lib.rs")]);
        let members = [("alpha", &alpha), ("beta", &beta)];
        let roots = [a.clone(), b.clone()];

        let body = "roots a, b\n\
                    identifier · keyword\n\
                    beta src/lib.rs\n\
                    \x20 1-1 x fn · stale | fn x() {}\n\
                    alpha src/lib.rs\n\
                    \x20 1-1 x fn | fn x() {}\n\
                    alpha notes.md\n\
                    \x20 1-1 | # notes";
        let got = shares(&members, &roots, body);
        assert_eq!(got.len(), 2, "{got:?}");

        assert_eq!(got[0].member, 1, "beta answered first: {got:?}");
        assert_eq!(
            got[0].paths,
            BTreeSet::from([canon(&b.join("src").join("lib.rs"))]),
            "beta's src/lib.rs is the one under beta's root"
        );
        assert!(got[0].text.contains("· stale"));
        assert!(
            got[0].text.starts_with("roots a, b\n"),
            "the lines above the first heading go to the first store: {:?}",
            got[0].text
        );

        assert_eq!(got[1].member, 0);
        assert_eq!(
            got[1].paths,
            BTreeSet::from([
                canon(&a.join("src").join("lib.rs")),
                canon(&a.join("notes.md"))
            ])
        );
        assert!(!got[1].text.contains("stale"));
        assert_eq!(
            got[0].text.len() + got[1].text.len(),
            body.len() + 1,
            "every line of the reply is in exactly one share"
        );
    }

    /// The brief's `[label] path:start-end`, the excerpt's `[n] label
    /// path:start-end`, and a fleet of one's unlabelled headings are all read;
    /// prose that mentions a path is not.
    #[test]
    fn every_reply_shape_names_its_store_and_prose_does_not() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = two_roots(tmp.path());
        let alpha = store_holding(
            &tmp.path().join("alpha"),
            &[a.join("src").join("lib.rs"), a.join("notes.md")],
        );
        let beta = store_holding(&tmp.path().join("beta"), &[b.join("src").join("lib.rs")]);
        let members = [("alpha", &alpha), ("beta", &beta)];
        let roots = [a.clone(), b.clone()];
        let abs = crate::plain(&a.join("notes.md").to_string_lossy());

        let body = format!(
            "[beta] src/lib.rs:1-1 x @1 [vector]\n\
             \x20   fn x() {{}}\n\
             [1] alpha {abs}:1-1 (score 0.500 via vector)\n\
             # notes\n\
             see src/lib.rs for the rest"
        );
        let got = shares(&members, &roots, &body);
        let by = |m: usize| got.iter().find(|s| s.member == m).map(|s| s.paths.clone());
        assert_eq!(
            by(1),
            Some(BTreeSet::from([canon(&b.join("src").join("lib.rs"))]))
        );
        assert_eq!(by(0), Some(BTreeSet::from([canon(&a.join("notes.md"))])));

        // A fleet of one labels nothing, and its headings are its own.
        let one = [("alpha", &alpha)];
        let got = shares(
            &one,
            std::slice::from_ref(&a),
            "src/lib.rs\n  1-1 | x\nnotes.md:1-1 notes\n",
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].paths.len(), 2, "{got:?}");

        assert!(shares(&members, &roots, "No match for that query.").is_empty());
    }

    /// End to end through a fleet: each store that answered gets a row with
    /// its own hits, the rows are one retrieval, and both chains verify.
    ///
    /// Before 0.33.0 this wrote one row, into the first store, with no hits.
    #[test]
    fn a_fleet_reply_writes_a_row_per_store_that_answered() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b) = two_roots(tmp.path());
        let (alpha_dir, beta_dir) = (tmp.path().join("alpha"), tmp.path().join("beta"));
        drop(store_holding(&alpha_dir, &[a.join("notes.md")]));
        drop(store_holding(&beta_dir, &[b.join("src").join("lib.rs")]));

        let fleet = Fleet::open(&[alpha_dir.clone(), beta_dir.clone()]).unwrap();
        assert_eq!(fleet.labels(), vec!["alpha", "beta"]);
        // Absolute, as a fleet whose stores the registry does not know prints
        // them.
        let body = format!(
            "alpha {}\n  1-1 | # notes\nbeta {}\n  1-1 x fn · stale | fn x() {{}}",
            crate::plain(&a.join("notes.md").to_string_lossy()),
            crate::plain(&b.join("src").join("lib.rs").to_string_lossy()),
        );
        let who = Who {
            client: "harness",
            session: "s",
            version: "",
        };
        reply(
            &fleet,
            &who,
            "search",
            "notes",
            &body,
            &[],
            std::time::Duration::from_millis(1),
        );
        drop(fleet);

        let mut ids = Vec::new();
        for dir in [&alpha_dir, &beta_dir] {
            let s = Semlith::open(dir, None).unwrap();
            let rows = store::retrievals(s.db(), 10).unwrap();
            assert_eq!(rows.len(), 1, "{}: {rows:?}", dir.display());
            assert_eq!(rows[0].hits, 1, "{}: {rows:?}", dir.display());
            assert!(rows[0].whole_file_tokens > 0);
            assert_eq!(store::ledger_break(s.db()).unwrap(), None);
            ids.push(rows[0].query_id.clone());
        }
        assert_eq!(ids[0], ids[1], "one reply is one retrieval");
    }

    /// A reply that named nothing is one zero-hit row, in the store the call
    /// was scoped to — or the first store open when it was not scoped.
    #[test]
    fn a_zero_hit_reply_is_one_row_in_the_store_searched() {
        let tmp = tempfile::tempdir().unwrap();
        let (alpha_dir, beta_dir) = (tmp.path().join("alpha"), tmp.path().join("beta"));
        drop(store_holding(&alpha_dir, &[]));
        drop(store_holding(&beta_dir, &[]));
        let fleet = Fleet::open(&[alpha_dir.clone(), beta_dir.clone()]).unwrap();
        let who = Who {
            client: "harness",
            session: "s",
            version: "",
        };
        let nothing = "No match for that query.";
        let at = std::time::Duration::from_millis(1);
        reply(
            &fleet,
            &who,
            "search",
            "q1",
            nothing,
            &["beta".to_string()],
            at,
        );
        reply(&fleet, &who, "search", "q2", nothing, &[], at);
        drop(fleet);

        let queries = |dir: &Path| -> Vec<(String, i64)> {
            let s = Semlith::open(dir, None).unwrap();
            store::retrievals(s.db(), 10)
                .unwrap()
                .into_iter()
                .map(|r| (r.query, r.hits))
                .collect()
        };
        assert_eq!(queries(beta_dir.as_path()), vec![("q1".to_string(), 0)]);
        assert_eq!(queries(alpha_dir.as_path()), vec![("q2".to_string(), 0)]);
    }
}
