//! Five reports, generated from this machine's ledger, index and graph.
//!
//! Nothing here reaches the network, nothing here is a model's opinion, and
//! nothing here is a number without its denominator. A report is a title and
//! a list of blocks — a paragraph, or a table with named columns — and the
//! five renderers below turn that one structure into Markdown, CSV, JSON,
//! print-styled HTML or PDF. One structure rather than five writers, so the
//! formats cannot disagree about what a report says, and so `semlith report`
//! and the portal's export produce the same bytes.
//!
//! PDF used to be the browser's own Print-to-PDF over the HTML. From 0.27.0 it
//! is typeset here, because a saved report that only exists if a browser is
//! open is not a saved report — a daemon on a build box, an agent over MCP and
//! `semlith report --out` all have blocks and no browser. It is a typeset
//! document, not a render of the portal's CSS: the HTML keeps its print
//! stylesheet and the two are allowed to look different, because they are
//! renderers over the same blocks rather than one being a picture of the
//! other.

use anyhow::{Result, bail};

/// A block of a report.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "block", rename_all = "lowercase")]
pub enum Block {
    /// A sentence, or a few. Never a number without what it is over.
    Text { text: String },
    /// A named table. `columns` are its headers, in order.
    Table {
        title: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// A row of counted facts: label, value, and what the value is over.
    Facts {
        facts: Vec<(String, String, String)>,
    },
}

/// One generated report.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    pub kind: &'static str,
    pub title: String,
    /// Local time with the offset, the same stamp the ledger prints.
    pub generated: String,
    /// The stores this was generated over, so a figure has a subject.
    ///
    /// This is the scope, after narrowing: a report over one store of six says
    /// so here, and every renderer prints it.
    pub stores: Vec<String>,
    /// The span this report covers, in words, and whether it was applied.
    ///
    /// Carried on the report rather than left to the caller because three of
    /// the five reports cannot honour a window at all, and a heading claiming
    /// seven days over a lifetime total is exactly the number-without-its-
    /// denominator this module exists to refuse.
    pub window: String,
    pub blocks: Vec<Block>,
}

/// How far back a report looks.
///
/// Days, because the only two things in a report that carry a date carry it as
/// a second since the epoch: the change brief's `files_indexed_since` and the
/// access audit's "last seen". The ledger's savings, latency and coverage
/// figures are lifetime aggregates with no date on them at all, and the index
/// and graph counts the health and gaps reports are built from describe what
/// is on disk now. So the vocabulary is days and the three unwindowable
/// reports say so rather than pretending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// No window was named. Every report keeps the span it has always had:
    /// seven days for the change brief, everything for the rest. This is what
    /// a caller who names no window gets, and what every caller got before
    /// 0.27.0 — which is the whole of why it is a value of its own rather than
    /// a synonym for `All` or for `Days(7)`.
    Unset,
    /// Everything recorded, the change brief included.
    All,
    /// The last N days.
    Days(i64),
}

/// The window vocabulary, and the only spellings [`window_of`] accepts.
pub const WINDOWS: [(&str, Window); 5] = [
    ("all", Window::All),
    ("day", Window::Days(1)),
    ("week", Window::Days(7)),
    ("month", Window::Days(30)),
    ("quarter", Window::Days(90)),
];

/// The change brief's own span, and the one [`Window::Unset`] keeps for it.
const CHANGE_DAYS: i64 = 7;

/// Resolve a window name, or say which there are. `None` is [`Window::Unset`].
pub fn window_of(name: Option<&str>) -> Result<Window> {
    let Some(name) = name else {
        return Ok(Window::Unset);
    };
    match WINDOWS.iter().find(|(n, _)| *n == name) {
        Some((_, window)) => Ok(*window),
        None => bail!(
            "unknown window {name:?}; the windows are {}",
            WINDOWS
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

impl Window {
    /// The epoch second a dated row must be at or after, for a block that has
    /// no span of its own.
    fn since(self, now: i64) -> Option<i64> {
        match self {
            Window::Days(days) => Some(now - days * 24 * 60 * 60),
            _ => None,
        }
    }

    /// The same, for a block that already had one. [`Window::Unset`] keeps it.
    fn since_or(self, now: i64, own_days: i64) -> Option<i64> {
        match self {
            Window::Unset => Some(now - own_days * 24 * 60 * 60),
            _ => self.since(now),
        }
    }

    /// What a report says it was taken over.
    ///
    /// `windowed` is whether this report's blocks carry a date at all. One
    /// that does not says so in the same sentence, because the alternative is
    /// a report headed "the last 7 days" over a figure counted since the store
    /// was created.
    fn note(self, windowed: bool) -> String {
        let span = match self {
            Window::Unset if windowed => "each block's own span".to_string(),
            Window::Unset | Window::All => "everything recorded".to_string(),
            Window::Days(1) => "the last day".to_string(),
            Window::Days(days) => format!("the last {days} days"),
        };
        if windowed || matches!(self, Window::Unset) {
            span
        } else {
            format!(
                "{span} — this report counts over a store's whole history and has no date to narrow by"
            )
        }
    }
}

/// The five reports, and nothing else.
///
/// The ones the v4 design keeps. Team roll-up, the compliance pack, CI impact
/// mode, signed reports and build attestation need a team ledger or a shared
/// container, so they belong to Semlith Cloud rather than to this binary.
pub const KINDS: [(&str, &str); 5] = [
    ("savings", "Retrieval savings"),
    ("access", "AI access audit"),
    ("change", "Change brief"),
    ("health", "Index health"),
    ("gaps", "Knowledge gaps"),
];

/// The output formats [`Report::render`] returns text for.
pub const FORMATS: [&str; 4] = ["markdown", "csv", "json", "html"];

/// The fifth format.
///
/// Not in [`FORMATS`] because [`Report::render`] returns a `String` and a PDF
/// is bytes. [`Report::render_bytes`] is the one call that serves all five,
/// and it is a renderer over the same blocks as the other four — there is no
/// second data path behind it.
pub const PDF: &str = "pdf";

/// The five formats, in the order a caller is offered them.
pub const ALL_FORMATS: [&str; 5] = ["markdown", "csv", "json", "html", PDF];

/// What a million tokens costs to read, by model. Input pricing, because a
/// retrieval is something an agent reads.
///
/// Written down rather than fetched: a report must generate on a machine
/// with no network, and a price whose source a reader cannot see is worse
/// than one they can argue with.
pub const PRICES: [(&str, f64); 3] = [("Sonnet 5", 3.0), ("Opus 5", 15.0), ("Haiku 4.5", 1.0)];

/// A model name reduced to what is worth comparing: letters and digits only,
/// folded to lower case. `Opus 5`, `opus-5` and `opus_5` are one model, and a
/// caller that has the slug should not price at the wrong one for punctuation.
fn key_of(model: &str) -> String {
    model
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The price of a model by name, if it is one this binary prices.
///
/// Separate from [`price_of`] because a caller taking the name from a user has
/// to be able to refuse an unknown one: this is a cost in money, and a
/// spelling nobody prices must not quietly come back as the default's figure
/// under the default's name.
pub fn price_named(model: &str) -> Option<(&'static str, f64)> {
    let wanted = key_of(model);
    PRICES
        .iter()
        .find(|(name, _)| key_of(name) == wanted)
        .copied()
}

/// The price of a model by name, or the first one.
pub fn price_of(model: &str) -> (&'static str, f64) {
    price_named(model).unwrap_or(PRICES[0])
}

/// The three names, for an error message that says what would have worked.
pub fn price_names() -> String {
    PRICES
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Seconds since the epoch, for the one stamp a report carries.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `1 234 567` — the same grouping every other surface uses.
fn n(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    if value < 0 { format!("-{out}") } else { out }
}

/// Resolve a report name, or say which five there are.
///
/// Separate from [`generate`] so a typo is caught before a store is opened:
/// `semlith report team-rollup` in a directory that is not a store should
/// answer about the report name, not about the directory.
pub fn kind_of(kind: &str) -> Result<(&'static str, &'static str)> {
    match KINDS.iter().find(|(k, _)| *k == kind) {
        Some((kind, title)) => Ok((kind, title)),
        None => bail!(
            "unknown report {kind:?}; the reports are {}",
            KINDS.iter().map(|(k, _)| *k).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// Check a format name before anything is generated, for the same reason.
///
/// The four that render to text. A caller that can hand back bytes — the CLI
/// writing a file, the route answering a request, the schedule runner — wants
/// [`check_any_format`] instead, which is the same check over all five.
pub fn check_format(format: &str) -> Result<()> {
    if FORMATS.contains(&format) || format == "md" {
        return Ok(());
    }
    bail!(
        "unknown format {format:?}; the formats are {}",
        FORMATS.join(", ")
    )
}

/// Check a format name against all five, PDF included.
///
/// Two checks rather than one because the two sets are genuinely different: a
/// PDF is bytes and [`Report::render`] returns a `String`, so a surface that
/// can only print text may not accept `pdf`. Both read their list from the
/// constants above, so there is no third place that knows what a format is.
pub fn check_any_format(format: &str) -> Result<()> {
    if ALL_FORMATS.contains(&format) || format == "md" {
        return Ok(());
    }
    bail!(
        "unknown format {format:?}; the formats are {}",
        ALL_FORMATS.join(", ")
    )
}

/// The stores one report is over: the scope, already narrowed.
type Scope<'a> = [(&'a str, &'a crate::Semlith)];

/// Generate one report over every open store, with no window.
///
/// The 0.26.x signature, kept so it still means what it meant: `semlith
/// report` and `semlith_report` ask for exactly this.
pub fn generate(fleet: &crate::fleet::Fleet, kind: &str, model: &str) -> Result<Report> {
    generate_over(fleet, kind, model, Window::Unset, &[])
}

/// Generate one report over a window and a scope.
///
/// `only` is store labels; empty is every open store. A label no open store
/// answers to is refused rather than dropped — a report headed with five
/// stores when the caller asked for six is a figure with the wrong subject,
/// and nothing in the output would say so.
/// The builder's two content toggles.
///
/// Not three: the design draws a `Sign the report` toggle as well, and signing
/// needs a key this product does not have and a decision about where it lives.
/// A toggle that is drawn and does nothing is worse than one that is not drawn,
/// so that one is rendered as unavailable and said to be, rather than shipped
/// as a switch with no wire behind it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Attach the individual retrievals, not only the session totals.
    pub excerpts: bool,
    /// Replace every query text with a digest of it.
    pub redact: bool,
}

/// A query text as it appears in a report: itself, or a digest of itself.
///
/// The same blake3 the ledger chains its rows with, truncated to sixteen
/// characters — long enough that two different questions do not collide in any
/// corpus a person will read, short enough to sit in a table cell. Redaction is
/// report-wide rather than per-block on purpose: the access report is not the
/// only place a query reaches paper, and a toggle that hid it in one table and
/// printed it in another would be worse than no toggle at all.
fn said(query: &str, options: Options) -> String {
    if !options.redact {
        return query.to_string();
    }
    format!("hash:{}", &blake3::hash(query.as_bytes()).to_hex()[..16])
}

pub fn generate_over(
    fleet: &crate::fleet::Fleet,
    kind: &str,
    model: &str,
    window: Window,
    only: &[String],
) -> Result<Report> {
    generate_with(fleet, kind, model, window, only, Options::default())
}

/// Generate one report with the builder's toggles applied.
pub fn generate_with(
    fleet: &crate::fleet::Fleet,
    kind: &str,
    model: &str,
    window: Window,
    only: &[String],
    options: Options,
) -> Result<Report> {
    let (kind, title) = kind_of(kind)?;
    let open: Vec<&str> = fleet.each().map(|(label, _)| label).collect();
    for name in only {
        if !open.contains(&name.as_str()) {
            bail!(
                "no open store is named {name:?}; the open stores are {}",
                if open.is_empty() {
                    "none".to_string()
                } else {
                    open.join(", ")
                }
            );
        }
    }
    let scope: Vec<(&str, &crate::Semlith)> = fleet
        .each()
        .filter(|(label, _)| only.is_empty() || only.iter().any(|n| n == label))
        .collect();
    let stores: Vec<String> = scope.iter().map(|(label, _)| label.to_string()).collect();
    let blocks = match kind {
        "savings" => savings(&scope, model)?,
        "access" => access(&scope, window, options)?,
        "change" => change(&scope, window)?,
        "health" => health(&scope)?,
        _ => gaps(&scope, options)?,
    };
    Ok(Report {
        kind,
        title: title.to_string(),
        generated: crate::clock::local_stamp(unix_now()),
        stores,
        window: window.note(matches!(kind, "access" | "change")),
        blocks,
    })
}

/// What was not read, what that would have cost, and how much of the ledger
/// the figure covers.
fn savings(scope: &Scope<'_>, model: &str) -> Result<Vec<Block>> {
    let (name, per_million) = price_of(model);
    let (mut net, mut credited, mut total, mut measured) = (0i64, 0i64, 0i64, true);
    let (mut refunds, mut zero_hit, mut refunds_measured) = (0i64, 0i64, false);
    let (mut excerpt, mut whole) = (0i64, 0i64);
    let (mut p50, mut p95) = (0i64, 0i64);
    let mut head = None;
    let mut intact = true;
    for (_, store) in scope {
        let s = crate::store::ledger_savings(store.db())?;
        net += s.net;
        credited += s.credited;
        total += s.total;
        measured = measured && s.measured;
        let misses = crate::store::ledger_misses(store.db())?;
        refunds += misses.refunds;
        zero_hit += misses.zero_hit;
        refunds_measured = refunds_measured || misses.measured;
        let (_, _, e, w) = crate::store::ledger_totals(store.db())?;
        excerpt += e;
        whole += w;
        let (a, b) = crate::store::ledger_latency(store.db())?;
        p50 = p50.max(a);
        p95 = p95.max(b);
        if head.is_none() {
            head = crate::store::ledger_head(store.db())?;
        }
        intact = intact && crate::store::ledger_break(store.db())?.is_none();
    }
    let coverage = if total == 0 {
        0
    } else {
        credited * 100 / total
    };
    let tier = if measured && credited > 0 {
        "measured"
    } else {
        "modelled"
    };
    let cost = |tokens: i64| format!("${:.2}", (tokens as f64 / 1e6) * per_million);

    Ok(vec![
        Block::Text {
            text: format!(
                "Over {} recorded retrievals, {} of which are credited ({coverage}% coverage, {tier}). \
                 Every figure below is counted on this machine and against that denominator.",
                n(total),
                n(credited)
            ),
        },
        Block::Table {
            title: format!("What was not read, at {name} prices"),
            columns: vec![
                "line".into(),
                "tokens".into(),
                format!("cost at {name}"),
                "what it counts".into(),
            ],
            // Three counterfactuals, never summed: they answer three
            // different questions and adding them would count one saved read
            // up to three times.
            rows: vec![
                vec![
                    "Whole files not read".into(),
                    n(whole),
                    cost(whole),
                    "what reading every file a retrieval answered from would have cost".into(),
                ],
                vec![
                    "Excerpts read instead".into(),
                    n(excerpt),
                    cost(excerpt),
                    "what the answers actually cost".into(),
                ],
                vec![
                    "Refunds".into(),
                    n(refunds),
                    String::new(),
                    if refunds_measured {
                        "files an agent read whole anyway, seen by the steering hook — measured"
                            .into()
                    } else {
                        "a floor: no steering hook reports here, so reads semlith never served are uncounted".to_string()
                    },
                ],
            ],
        },
        Block::Facts {
            facts: vec![
                (
                    "Net".into(),
                    n(net),
                    format!(
                        "whole-file less excerpt over {} credited retrievals",
                        n(credited)
                    ),
                ),
                ("Net cost avoided".into(), cost(net), format!("at {name}")),
                (
                    "Coverage".into(),
                    format!("{coverage}%"),
                    format!("{} of {} retrievals", n(credited), n(total)),
                ),
                (
                    "Tier".into(),
                    tier.into(),
                    "measured when the store's own tokenizer counted every credited row".into(),
                ),
                (
                    "Zero-hit".into(),
                    n(zero_hit),
                    "retrievals that found nothing, recorded and credited nothing".into(),
                ),
                (
                    "Refund rate".into(),
                    if total == 0 {
                        "0%".into()
                    } else {
                        format!("{}%", refunds * 100 / total)
                    },
                    "of recorded retrievals".into(),
                ),
                (
                    "p50".into(),
                    format!("{} ms", p50 / 1000),
                    "as the ledger recorded it".into(),
                ),
                (
                    "p95".into(),
                    format!("{} ms", p95 / 1000),
                    "as the ledger recorded it".into(),
                ),
                (
                    "Ledger chain".into(),
                    if intact {
                        "verifies".into()
                    } else {
                        "broken".to_string()
                    },
                    head.unwrap_or_else(|| "no rows yet".into()),
                ),
            ],
        },
        Block::Text {
            text: "The three counterfactual lines answer three different questions and are never \
                   summed. A refund is an agent that did not ask; a zero hit is semlith that did \
                   not reach the answer. They call for opposite things."
                .into(),
        },
    ])
}

/// Which agents read this machine's corpus, how often, and what they missed.
fn access(scope: &Scope<'_>, window: Window, options: Options) -> Result<Vec<Block>> {
    let mut rows = Vec::new();
    let mut attached: Vec<Vec<String>> = Vec::new();
    let mut intact = true;
    // `ledger_sessions` answers newest first, so narrowing the list it returns
    // can only drop sessions older than the window — a session the window
    // covers cannot have been pushed out of the 200 by an older one.
    let since = window.since(unix_now());
    for (label, store) in scope {
        for session in crate::store::ledger_sessions(store.db(), 200)? {
            if since.is_some_and(|since| session.last < since) {
                continue;
            }
            rows.push(vec![
                crate::clock::local_stamp(session.last),
                if session.session.is_empty() {
                    "—".into()
                } else {
                    session.session.clone()
                },
                session.client.clone(),
                label.to_string(),
                n(session.retrievals),
                n(session.zero_hit),
                n(session.net),
                session.tier().to_string(),
            ]);
        }
        if options.excerpts {
            for row in crate::store::ledger_retrievals(store.db(), since.unwrap_or(0), ATTACHED)? {
                attached.push(vec![
                    crate::clock::local_stamp(row.at),
                    row.client,
                    if row.tool.is_empty() {
                        "—".into()
                    } else {
                        row.tool
                    },
                    said(&row.query, options),
                    n(row.hits),
                    n(row.excerpt_tokens),
                    n(row.whole_file_tokens),
                    label.to_string(),
                ]);
            }
        }
        intact = intact && crate::store::ledger_break(store.db())?.is_none();
    }
    rows.sort_by(|a, b| b[0].cmp(&a[0]));
    attached.sort_by(|a, b| b[0].cmp(&a[0]));
    attached.truncate(ATTACHED);
    let mut blocks = vec![
        Block::Text {
            text: format!(
                "Every agent session recorded on this machine. The rows are hash-chained and the \
                 chain {}. Nothing here has left the store it was written into.",
                if intact {
                    "verifies"
                } else {
                    "does not verify"
                }
            ),
        },
        Block::Table {
            title: "Sessions".into(),
            columns: vec![
                "last seen".into(),
                "session".into(),
                "agent".into(),
                "store".into(),
                "reads".into(),
                "zero-hit".into(),
                "net tokens".into(),
                "tier".into(),
            ],
            rows,
        },
    ];
    if options.excerpts {
        blocks.push(Block::Text {
            text: format!(
                "Every retrieval behind the sessions above, newest first, to a ceiling of \
                 {ATTACHED}. A session line says an agent asked forty times; this is which \
                 forty.{}",
                if options.redact {
                    " The query column is a digest rather than the text, so this says who asked, \
                     when, and what it cost, and not what was asked."
                } else {
                    ""
                }
            ),
        });
        blocks.push(Block::Table {
            title: "Retrievals".into(),
            columns: vec![
                "at".into(),
                "agent".into(),
                "tool".into(),
                "query".into(),
                "hits".into(),
                "excerpt tokens".into(),
                "whole-file tokens".into(),
                "store".into(),
            ],
            rows: attached,
        });
    }
    Ok(blocks)
}

/// The most individual retrievals one access report attaches.
///
/// A ceiling rather than everything: a ledger is unbounded, and a report nobody
/// can open is a report nobody reads. The sessions table above is the whole
/// count either way, so what this bounds is the detail and not the total.
const ATTACHED: usize = 500;

/// What this machine has read lately.
fn change(scope: &Scope<'_>, window: Window) -> Result<Vec<Block>> {
    // Zero rather than "no bound": `files_indexed_since` takes a second, and
    // no file was indexed before the epoch.
    let since = window.since_or(unix_now(), CHANGE_DAYS).unwrap_or(0);
    let span = window.note(true);
    let mut rows = Vec::new();
    let mut retired = 0i64;
    for (label, store) in scope {
        for (path, at) in crate::store::files_indexed_since(store.db(), since, 200)? {
            rows.push(vec![
                crate::clock::local_stamp(at),
                crate::plain(&path),
                label.to_string(),
            ]);
        }
        retired += crate::store::symbols_past_count(store.db()).unwrap_or(0);
    }
    rows.sort_by(|a, b| b[0].cmp(&a[0]));
    Ok(vec![
        Block::Text {
            text: format!(
                "Files this machine re-read over {span}, newest first. \
                 Re-read, not rewritten: the store records when it read a file, not when anybody \
                 changed it."
            ),
        },
        Block::Facts {
            facts: vec![
                ("Files re-read".into(), n(rows.len() as i64), span.clone()),
                (
                    "Definitions replaced".into(),
                    n(retired),
                    "symbols a re-index retired, over the store's whole history".into(),
                ),
            ],
        },
        Block::Table {
            title: "Re-read".into(),
            columns: vec!["when".into(), "file".into(), "store".into()],
            rows,
        },
    ])
}

/// What the index holds and what the graph made of it.
fn health(scope: &Scope<'_>) -> Result<Vec<Block>> {
    let mut rows = Vec::new();
    let (mut files, mut chunks, mut bytes) = (0i64, 0i64, 0i64);
    let (mut unresolved, mut several) = (0i64, 0i64);
    for (label, store) in scope {
        let (f, c, b) = crate::store::stats(store.db())?;
        files += f;
        chunks += c;
        bytes += b;
        let (_, distinct) = crate::store::unresolved_targets(store.db(), 5)?;
        unresolved += distinct;
        several += crate::store::names_with_several_definitions(store.db())?;
        for row in crate::store::coverage_by_language(store.db())? {
            if row.files == 0 && row.definitions == 0 {
                continue;
            }
            rows.push(vec![
                label.to_string(),
                row.language.clone(),
                n(row.files as i64),
                n(row.parser_failed as i64),
                n(row.definitions as i64),
                n(row.extracted as i64),
                n(row.resolved as i64),
                n(row.ambiguous as i64),
                n(row.unresolved as i64),
                format!("{}%", row.settled_share()),
            ]);
        }
    }
    Ok(vec![
        Block::Facts {
            facts: vec![
                ("Files".into(), n(files), "indexed".into()),
                ("Chunks".into(), n(chunks), "embedded".into()),
                ("Bytes".into(), n(bytes), "read".into()),
                (
                    "Call targets with no definition here".into(),
                    n(unresolved),
                    "distinct names the graph points at and cannot find".into(),
                ),
                (
                    "Names with several definitions".into(),
                    n(several),
                    "the population of every ambiguous edge".into(),
                ),
            ],
        },
        Block::Table {
            title: "What the graph covers, per language".into(),
            columns: vec![
                "store".into(),
                "language".into(),
                "files".into(),
                "unparsed".into(),
                "definitions".into(),
                "extracted".into(),
                "resolved".into(),
                "ambiguous".into(),
                "unresolved".into(),
                "settled".into(),
            ],
            rows,
        },
        Block::Text {
            text: "An absent edge and a file the parser gave up on are different problems, which \
                   is why they are different columns. `semlith stats` prints these same rows."
                .into(),
        },
    ])
}

/// What was asked and not answered, and the names that mislead.
fn gaps(scope: &Scope<'_>, options: Options) -> Result<Vec<Block>> {
    let mut asked = Vec::new();
    let mut missing = Vec::new();
    let mut several = 0i64;
    for (label, store) in scope {
        for (query, count) in crate::store::ledger_zero_hit_queries(store.db(), 20)? {
            // Redaction is not the access report's private arrangement: this is
            // the other place a query text reaches paper, and a toggle that hid
            // it in one table and printed it in another would be a promise
            // broken by the same document that made it.
            asked.push(vec![said(&query, options), n(count), label.to_string()]);
        }
        let (targets, _) = crate::store::unresolved_targets(store.db(), 20)?;
        for (name, edges) in targets {
            missing.push(vec![name, n(edges), label.to_string()]);
        }
        several += crate::store::names_with_several_definitions(store.db())?;
    }
    Ok(vec![
        Block::Text {
            text: "Two kinds of gap, kept apart. A question that found nothing is a document this \
                   corpus does not hold, or words nobody wrote. A call target with no definition \
                   is code this corpus does not contain."
                .into(),
        },
        Block::Table {
            title: "Asked and not answered".into(),
            columns: vec!["query".into(), "times".into(), "store".into()],
            rows: asked,
        },
        Block::Table {
            title: "Called and not found".into(),
            columns: vec!["name".into(), "edges".into(), "store".into()],
            rows: missing,
        },
        Block::Facts {
            facts: vec![(
                "Names that mislead".into(),
                n(several),
                "defined more than once, so a walk refuses to cross them and a search cannot say \
                 which one you meant"
                    .into(),
            )],
        },
    ])
}

// ------------------------------------------------------------------ typeset
//
// The PDF's page geometry. US Letter, because the reports price in dollars and
// a fixed page is what makes the column arithmetic below exact rather than a
// guess.
const PAGE_W: f32 = 612.0;
const PAGE_H: f32 = 792.0;
const MARGIN: f32 = 54.0;
const TITLE_PT: f32 = 16.0;
const HEADING_PT: f32 = 11.0;
const BODY_PT: f32 = 9.0;
const META_PT: f32 = 8.0;

/// How many characters of a given size fit between the margins.
///
/// Courier is a fixed-width face and every glyph in it is 0.6 em, so a line's
/// width *is* its length. That is the whole layout engine, and it is the
/// reason the face was chosen: wrapping a paragraph or fitting a table in a
/// proportional base-14 font means carrying the 224 AFM widths of each of
/// them, which is a metrics table this crate would then have to keep correct
/// for ever. Here, nothing can run off the page edge because nothing can be
/// wider than the number of characters that fit.
fn columns_at(size: f32) -> usize {
    ((PAGE_W - 2.0 * MARGIN) / (size * 0.6)) as usize
}

/// One thing to put on a page, before anything knows which page that is.
///
/// Laying out to pieces first and paginating second is what keeps the page
/// break out of the block loop: a renderer that emitted directly would have to
/// ask "am I near the bottom" in five places.
enum Piece {
    /// Vertical space, dropped at the top of a page.
    Gap(f32),
    /// A horizontal rule under a table's header row.
    Rule,
    Text {
        text: String,
        bold: bool,
        size: f32,
    },
}

/// A report's prose typeset as ASCII.
///
/// A base-14 font is WinAnsi-encoded, so an em dash could be written as one
/// byte — but then every reader and every extractor has to agree about the
/// encoding to get it back, and the one thing a saved report must survive is
/// being read. The handful of characters the five reports actually use are
/// transliterated, and anything else outside ASCII becomes `?`, which is
/// visible rather than silent.
fn ascii(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\t' => out.push(' '),
            '—' | '–' => out.push_str("--"),
            '·' | '•' => out.push('-'),
            '…' => out.push_str("..."),
            '‘' | '’' => out.push('\''),
            '“' | '”' => out.push('"'),
            '×' => out.push('x'),
            '\n' | ' ' => out.push(c),
            c if c.is_ascii_graphic() => out.push(c),
            // Every other control character, dropped rather than drawn.
            c if c.is_ascii() => {}
            _ => out.push('?'),
        }
    }
    out
}

/// Break text to a column count, on word boundaries where there are any.
///
/// A word longer than the column count — a path, a query, a symbol name — is
/// cut rather than allowed past the margin, because the margin is the promise.
fn wrap(text: &str, cols: usize) -> Vec<String> {
    let cols = cols.max(1);
    let text = ascii(text);
    let mut out = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word;
            while word.len() > cols {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                }
                out.push(word[..cols].to_string());
                word = &word[cols..];
            }
            if line.is_empty() {
                line = word.to_string();
            } else if line.len() + 1 + word.len() <= cols {
                line.push(' ');
                line.push_str(word);
            } else {
                out.push(std::mem::replace(&mut line, word.to_string()));
            }
        }
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// Column widths that add up to no more than the page holds.
///
/// Each column starts at its widest cell and the widest column gives up a
/// character at a time until the row fits. Shrinking costs height rather than
/// content: a cell narrower than its text wraps inside its column.
fn column_widths(columns: &[String], rows: &[Vec<String>], total: usize) -> Vec<usize> {
    const GUTTER: usize = 2;
    const MIN: usize = 6;
    let budget = total.saturating_sub(GUTTER * columns.len().saturating_sub(1));
    let mut widths: Vec<usize> = columns
        .iter()
        .map(|c| ascii(c).len().clamp(3, budget.max(3)))
        .collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(i) {
                *width = (*width).max(ascii(cell).len()).min(budget.max(MIN));
            }
        }
    }
    while widths.iter().sum::<usize>() > budget {
        let Some(widest) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, width)| **width)
            .map(|(i, _)| i)
        else {
            break;
        };
        if widths[widest] <= MIN {
            break;
        }
        widths[widest] -= 1;
    }
    widths
}

/// One table row as the physical lines it occupies.
fn table_lines(cells: &[String], widths: &[usize]) -> Vec<String> {
    let parts: Vec<Vec<String>> = widths
        .iter()
        .enumerate()
        .map(|(i, width)| wrap(cells.get(i).map(String::as_str).unwrap_or(""), *width))
        .collect();
    let height = parts.iter().map(Vec::len).max().unwrap_or(1).max(1);
    (0..height)
        .map(|line| {
            let mut out = String::new();
            for (i, (part, width)) in parts.iter().zip(widths).enumerate() {
                if i > 0 {
                    out.push_str("  ");
                }
                let cell = part.get(line).map(String::as_str).unwrap_or("");
                out.push_str(cell);
                out.push_str(&" ".repeat(width.saturating_sub(cell.len())));
            }
            out.trim_end().to_string()
        })
        .collect()
}

impl Report {
    /// Render in one of [`FORMATS`].
    pub fn render(&self, format: &str) -> Result<String> {
        Ok(match format {
            "markdown" | "md" => self.markdown(),
            "csv" => self.csv(),
            "json" => serde_json::to_string_pretty(self)? + "\n",
            "html" => self.html(),
            // Named rather than folded into the list below, because "the
            // formats are markdown, csv, json, html" is a lie about a format
            // this binary does produce — just not as text.
            PDF => bail!("pdf is bytes rather than text; render it with render_bytes"),
            other => bail!(
                "unknown format {other:?}; the formats are {}",
                ALL_FORMATS.join(", ")
            ),
        })
    }

    /// Render in one of [`ALL_FORMATS`], as the bytes that format is.
    ///
    /// The one call that serves all five. PDF is the only one that is not
    /// UTF-8, and it is still a renderer over `self.blocks` — nothing here
    /// reaches past the block structure the other four read.
    pub fn render_bytes(&self, format: &str) -> Result<Vec<u8>> {
        match format {
            PDF => Ok(self.pdf()),
            other => self.render(other).map(String::into_bytes),
        }
    }

    /// The report as a flat list of things to draw, before pagination.
    fn pieces(&self) -> Vec<Piece> {
        let text = |out: &mut Vec<Piece>, body: &str, bold: bool, size: f32| {
            for line in wrap(body, columns_at(size)) {
                out.push(Piece::Text {
                    text: line,
                    bold,
                    size,
                });
            }
        };
        let mut out = Vec::new();
        text(&mut out, &self.title, true, TITLE_PT);
        out.push(Piece::Gap(4.0));
        text(&mut out, &self.meta(), false, META_PT);
        for block in &self.blocks {
            out.push(Piece::Gap(10.0));
            match block {
                Block::Text { text: body } => text(&mut out, body, false, BODY_PT),
                Block::Facts { facts } => {
                    for (label, value, over) in facts {
                        text(
                            &mut out,
                            &format!("{label}: {value} — {over}"),
                            false,
                            BODY_PT,
                        );
                    }
                }
                Block::Table {
                    title,
                    columns,
                    rows,
                } => {
                    text(&mut out, title, true, HEADING_PT);
                    out.push(Piece::Gap(3.0));
                    if rows.is_empty() {
                        text(&mut out, "Nothing to report.", false, BODY_PT);
                        continue;
                    }
                    let widths = column_widths(columns, rows, columns_at(BODY_PT));
                    for line in table_lines(columns, &widths) {
                        out.push(Piece::Text {
                            text: line,
                            bold: true,
                            size: BODY_PT,
                        });
                    }
                    out.push(Piece::Rule);
                    for row in rows {
                        for line in table_lines(row, &widths) {
                            out.push(Piece::Text {
                                text: line,
                                bold: false,
                                size: BODY_PT,
                            });
                        }
                    }
                }
            }
        }
        out
    }

    /// The same blocks, typeset and paginated.
    fn pdf(&self) -> Vec<u8> {
        use pdf_writer::{Content, Finish, Name, Pdf, Rect, Ref, Str};

        const REGULAR: Name = Name(b"F1");
        const BOLD: Name = Name(b"F2");
        const TOP: f32 = PAGE_H - MARGIN;

        let mut finished: Vec<Vec<u8>> = Vec::new();
        let mut content = Content::new();
        let mut y = TOP;
        for piece in self.pieces() {
            let height = match &piece {
                Piece::Gap(gap) => *gap,
                Piece::Rule => 6.0,
                // 1.35 em of leading: enough that a wrapped table row reads as
                // rows rather than as a block of text.
                Piece::Text { size, .. } => size * 1.35,
            };
            if let Piece::Gap(_) = piece {
                // Never before the first line of the document.
                if y < TOP {
                    y -= height;
                }
                continue;
            }
            if y - height < MARGIN {
                finished.push(
                    std::mem::replace(&mut content, Content::new())
                        .finish()
                        .to_vec(),
                );
                y = TOP;
            }
            y -= height;
            match piece {
                Piece::Gap(_) => unreachable!("handled above"),
                Piece::Rule => {
                    content.set_line_width(0.5);
                    content.move_to(MARGIN, y + 3.0);
                    content.line_to(PAGE_W - MARGIN, y + 3.0);
                    content.stroke();
                }
                Piece::Text { text, bold, size } => {
                    content.begin_text();
                    content.set_font(if bold { BOLD } else { REGULAR }, size);
                    // Absolute, because the text matrix is the identity at
                    // every `begin_text` and `Td` is relative to it.
                    content.next_line(MARGIN, y);
                    content.show(Str(text.as_bytes()));
                    content.end_text();
                }
            }
        }
        finished.push(content.finish().to_vec());

        let mut pdf = Pdf::new();
        let catalog = Ref::new(1);
        let tree = Ref::new(2);
        let regular = Ref::new(3);
        let bold = Ref::new(4);
        let page_id = |i: usize| Ref::new(5 + 2 * i as i32);
        let body_id = |i: usize| Ref::new(6 + 2 * i as i32);

        pdf.catalog(catalog).pages(tree);
        pdf.pages(tree)
            .kids((0..finished.len()).map(page_id))
            .count(finished.len() as i32);
        // The two base-14 faces every reader has. No font is embedded, so the
        // binary grows by nothing and there is no font licence to carry.
        pdf.type1_font(regular)
            .base_font(Name(b"Courier"))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
        pdf.type1_font(bold)
            .base_font(Name(b"Courier-Bold"))
            .encoding_predefined(Name(b"WinAnsiEncoding"));
        for (i, body) in finished.iter().enumerate() {
            let mut page = pdf.page(page_id(i));
            page.parent(tree);
            page.media_box(Rect::new(0.0, 0.0, PAGE_W, PAGE_H));
            page.contents(body_id(i));
            page.resources()
                .fonts()
                .pair(REGULAR, regular)
                .pair(BOLD, bold);
            page.finish();
            pdf.stream(body_id(i), body);
        }
        pdf.finish()
    }

    fn header(&self) -> String {
        format!("{}\n\n{}", self.title, self.meta())
    }

    /// The one line every renderer puts under the title: when, over what, and
    /// over how long. The scope and the window are printed rather than
    /// implied, because a narrowed report that does not say it is narrowed is
    /// a figure whose subject the reader has to guess.
    fn meta(&self) -> String {
        format!(
            "Generated {} on this machine, over {}, covering {}. Nothing left it.",
            self.generated,
            if self.stores.is_empty() {
                "no open store".to_string()
            } else {
                self.stores.join(", ")
            },
            self.window,
        )
    }

    fn markdown(&self) -> String {
        let mut out = format!("# {}\n\n", self.title);
        out.push_str(&self.meta());
        out.push('\n');
        for block in &self.blocks {
            match block {
                Block::Text { text } => out.push_str(&format!("\n{text}\n")),
                Block::Facts { facts } => {
                    out.push('\n');
                    for (label, value, over) in facts {
                        out.push_str(&format!("- **{label}:** {value} — {over}\n"));
                    }
                }
                Block::Table {
                    title,
                    columns,
                    rows,
                } => {
                    out.push_str(&format!("\n## {title}\n\n"));
                    if rows.is_empty() {
                        out.push_str("Nothing to report.\n");
                        continue;
                    }
                    out.push_str(&format!("| {} |\n", columns.join(" | ")));
                    out.push_str(&format!(
                        "| {} |\n",
                        columns
                            .iter()
                            .map(|_| "---")
                            .collect::<Vec<_>>()
                            .join(" | ")
                    ));
                    for row in rows {
                        out.push_str(&format!("| {} |\n", row.join(" | ")));
                    }
                }
            }
        }
        out
    }

    fn csv(&self) -> String {
        // A report is several tables, so the CSV is several tables one after
        // another, each named on its own line. A spreadsheet reads that; a
        // single flattened table would have to invent a column that means
        // "which table is this row from".
        let quote = |v: &str| -> String {
            if v.contains('"') || v.contains(',') || v.contains('\n') {
                format!("\"{}\"", v.replace('"', "\"\""))
            } else {
                v.to_string()
            }
        };
        let mut out = format!("{}\n", quote(&self.header().replace('\n', " ")));
        for block in &self.blocks {
            match block {
                Block::Text { text } => out.push_str(&format!("\n{}\n", quote(text))),
                Block::Facts { facts } => {
                    out.push_str("\nfact,value,over\n");
                    for (label, value, over) in facts {
                        out.push_str(&format!(
                            "{},{},{}\n",
                            quote(label),
                            quote(value),
                            quote(over)
                        ));
                    }
                }
                Block::Table {
                    title,
                    columns,
                    rows,
                } => {
                    out.push_str(&format!("\n{}\n", quote(title)));
                    out.push_str(&format!(
                        "{}\n",
                        columns
                            .iter()
                            .map(|c| quote(c))
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                    for row in rows {
                        out.push_str(&format!(
                            "{}\n",
                            row.iter().map(|c| quote(c)).collect::<Vec<_>>().join(",")
                        ));
                    }
                }
            }
        }
        out
    }

    fn html(&self) -> String {
        let escape = |v: &str| {
            v.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
        };
        let mut body = String::new();
        for block in &self.blocks {
            match block {
                Block::Text { text } => body.push_str(&format!("<p>{}</p>\n", escape(text))),
                Block::Facts { facts } => {
                    body.push_str("<dl class=\"facts\">\n");
                    for (label, value, over) in facts {
                        body.push_str(&format!(
                            "<div><dt>{}</dt><dd><b>{}</b> <span>{}</span></dd></div>\n",
                            escape(label),
                            escape(value),
                            escape(over)
                        ));
                    }
                    body.push_str("</dl>\n");
                }
                Block::Table {
                    title,
                    columns,
                    rows,
                } => {
                    body.push_str(&format!("<h2>{}</h2>\n", escape(title)));
                    if rows.is_empty() {
                        body.push_str("<p>Nothing to report.</p>\n");
                        continue;
                    }
                    body.push_str("<table><thead><tr>");
                    for column in columns {
                        body.push_str(&format!("<th>{}</th>", escape(column)));
                    }
                    body.push_str("</tr></thead><tbody>\n");
                    for row in rows {
                        body.push_str("<tr>");
                        for cell in row {
                            body.push_str(&format!("<td>{}</td>", escape(cell)));
                        }
                        body.push_str("</tr>\n");
                    }
                    body.push_str("</tbody></table>\n");
                }
            }
        }
        // Print-styled on purpose, and still worth having now that a PDF
        // renderer exists beside it: this is the format a reader opens in a
        // browser, sends to somebody, and prints if they want to. The PDF is
        // typeset from the blocks rather than printed from this.
        format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\n\
             <title>{title}</title>\n<style>\n\
             :root {{ color-scheme: light dark; }}\n\
             body {{ font: 14px/1.5 system-ui, sans-serif; margin: 2rem auto; max-width: 52rem; padding: 0 1rem; }}\n\
             h1 {{ font-size: 1.5rem; }} h2 {{ font-size: 1.05rem; margin-top: 2rem; }}\n\
             table {{ border-collapse: collapse; width: 100%; font-size: 12.5px; }}\n\
             th, td {{ border-bottom: 1px solid #ccc; padding: 4px 8px; text-align: left; }}\n\
             th {{ font-weight: 600; }}\n\
             .facts {{ display: grid; gap: .4rem; }} .facts div {{ display: flex; gap: .5rem; }}\n\
             .facts dt {{ min-width: 16rem; color: #555; }} .facts dd {{ margin: 0; }}\n\
             .facts span {{ color: #555; }}\n\
             .meta {{ color: #555; }}\n\
             @media print {{ body {{ margin: 0; max-width: none; }} h2 {{ break-after: avoid; }} tr {{ break-inside: avoid; }} }}\n\
             </style></head><body>\n<h1>{title}</h1>\n<p class=\"meta\">{meta}</p>\n{body}</body></html>\n",
            title = escape(&self.title),
            meta = escape(
                &self
                    .header()
                    .split_once("\n\n")
                    .map(|(_, rest)| rest.to_string())
                    .unwrap_or_default()
                    .replace('\n', " ")
            ),
            body = body,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Report {
        Report {
            kind: "health",
            title: "Index health".into(),
            generated: "2026-09-20 10:00:00 +05:30".into(),
            stores: vec!["semlith".into()],
            window: Window::Unset.note(false),
            blocks: vec![
                Block::Text {
                    text: "One sentence.".into(),
                },
                Block::Facts {
                    facts: vec![("Files".into(), "3".into(), "indexed".into())],
                },
                Block::Table {
                    title: "Rows".into(),
                    columns: vec!["a".into(), "b".into()],
                    rows: vec![vec!["one".into(), "two, with a comma".into()]],
                },
            ],
        }
    }

    #[test]
    fn every_format_carries_the_same_facts() {
        let report = sample();
        for format in FORMATS {
            let out = report.render(format).unwrap();
            assert!(out.contains("Index health"), "{format} lost the title");
            assert!(out.contains("two, with a comma"), "{format} lost a cell");
            assert!(out.contains('3'), "{format} lost a fact");
        }
    }

    #[test]
    fn a_comma_in_a_cell_does_not_become_a_column() {
        let csv = sample().render("csv").unwrap();
        let row = csv
            .lines()
            .find(|line| line.starts_with("one,"))
            .expect("the table row");
        assert_eq!(row, "one,\"two, with a comma\"");
    }

    #[test]
    fn an_unknown_report_or_format_says_what_there_is() {
        let report = sample();
        let e = report.render("xml").unwrap_err().to_string();
        assert!(e.contains("markdown"), "{e}");
        assert!(e.contains("html"), "{e}");
        assert!(e.contains("pdf"), "the fifth format is not named: {e}");
        // `render` returns text, so it refuses the one format that is not
        // text — but it says why rather than calling it unknown.
        let e = report.render(PDF).unwrap_err().to_string();
        assert!(e.contains("render_bytes"), "{e}");
    }

    #[test]
    fn a_window_name_resolves_or_says_which_there_are() {
        assert_eq!(window_of(None).unwrap(), Window::Unset);
        assert_eq!(window_of(Some("week")).unwrap(), Window::Days(7));
        assert_eq!(window_of(Some("all")).unwrap(), Window::All);
        let e = window_of(Some("fortnight")).unwrap_err().to_string();
        for wanted in ["all", "day", "week", "month", "quarter"] {
            assert!(e.contains(wanted), "{e} does not name {wanted}");
        }
    }

    #[test]
    fn a_window_a_report_cannot_honour_is_said_rather_than_printed() {
        // Three of the five count over a store's whole history. A heading
        // reading "the last 7 days" over a lifetime total is the number
        // without its denominator this module exists to refuse.
        assert_eq!(Window::Days(7).note(true), "the last 7 days");
        assert!(Window::Days(7).note(false).contains("whole history"));
        assert_eq!(Window::Unset.note(true), "each block's own span");
        assert_eq!(Window::Unset.note(false), "everything recorded");
        // Every value says something different, so a reader can tell which
        // was applied from the document alone.
        let said: std::collections::BTreeSet<String> = WINDOWS
            .iter()
            .flat_map(|(_, w)| [w.note(true), w.note(false)])
            .collect();
        assert_eq!(said.len(), WINDOWS.len() * 2);
    }

    #[test]
    fn nothing_a_table_holds_runs_off_the_page() {
        // The margin is the promise, and a path or a query is the thing that
        // breaks it: neither has a space to wrap at.
        let wide = "/a/very/long/path/without/any/spaces/in/it/at/all/that/is/far/wider/than/the/page/is.rs";
        let columns = vec!["when".to_string(), "file".to_string(), "store".to_string()];
        let rows = vec![vec![
            "2026-09-20 10:00:00".to_string(),
            wide.to_string(),
            "semlith".to_string(),
        ]];
        let widths = column_widths(&columns, &rows, columns_at(BODY_PT));
        for line in table_lines(&rows[0], &widths) {
            assert!(
                line.len() <= columns_at(BODY_PT),
                "{} characters in a {}-column page: {line}",
                line.len(),
                columns_at(BODY_PT)
            );
        }
        // And nothing was thrown away to get there.
        let laid = table_lines(&rows[0], &widths).join("");
        assert!(
            laid.replace(' ', "").contains(&wide[wide.len() - 20..]),
            "the path was cut rather than wrapped: {laid}"
        );
    }

    #[test]
    fn the_pdf_is_a_renderer_over_the_same_blocks() {
        // Not a second data path: every cell the other four formats carry
        // reaches the typesetter, through `pieces`, from `self.blocks`.
        let report = sample();
        let drawn: String = report
            .pieces()
            .into_iter()
            .filter_map(|piece| match piece {
                Piece::Text { text, .. } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(drawn.contains("Index health"));
        assert!(drawn.contains("two, with a comma"));
        assert!(drawn.contains("Files: 3"));

        let bytes = report.render_bytes(PDF).unwrap();
        assert!(bytes.starts_with(b"%PDF-"), "not a PDF");
        assert!(bytes.ends_with(b"%%EOF\n") || bytes.ends_with(b"%%EOF"));
    }

    #[test]
    fn prose_outside_ascii_is_transliterated_rather_than_dropped() {
        // A base-14 font is WinAnsi. The reports are full of em dashes, and a
        // dash that silently became nothing would read as a missing word.
        assert_eq!(ascii("net — over 3 rows"), "net -- over 3 rows");
        assert_eq!(ascii("a · b"), "a - b");
        assert_eq!(ascii("\u{4f60}"), "?");
    }

    #[test]
    fn a_model_is_priced_however_its_name_is_punctuated() {
        // The portal, the CLI and the tool each spell these differently —
        // `Opus 5`, `opus-5`, `opus_5`. Matching on the display name alone
        // meant the other two fell through to the first price and the report
        // then said "at Sonnet 5" over an Opus figure.
        for spelling in ["Opus 5", "opus 5", "opus-5", "opus_5", "OPUS5"] {
            let (name, per_million) = price_of(spelling);
            assert_eq!(name, "Opus 5", "{spelling} priced as {name}");
            assert_eq!(per_million, 15.0);
        }
    }

    #[test]
    fn a_model_nobody_prices_is_refused_rather_than_defaulted() {
        // The figure is money. A name this binary does not price must not come
        // back as Sonnet 5's number under Sonnet 5's name, because the caller
        // asked for something else and nothing would tell them.
        assert!(price_named("gpt-9").is_none());
        assert!(price_named("").is_none());
        assert_eq!(price_named("haiku 4.5").map(|(n, _)| n), Some("Haiku 4.5"));
        let names = price_names();
        for wanted in ["Sonnet 5", "Opus 5", "Haiku 4.5"] {
            assert!(names.contains(wanted), "{names} does not name {wanted}");
        }
    }

    #[test]
    fn the_html_is_escaped_and_printable() {
        let mut report = sample();
        report.blocks.push(Block::Text {
            text: "<script>alert(1)</script>".into(),
        });
        let html = report.render("html").unwrap();
        assert!(!html.contains("<script>"), "a cell reached the markup");
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("@media print"), "no print stylesheet");
    }
}
