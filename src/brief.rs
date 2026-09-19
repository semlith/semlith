//! One call instead of four.
//!
//! An agent that wants to change a function asks four questions today: search
//! for it, read the span that came back, ask what calls it, read those. Four
//! round trips, four context windows, and three of them spent re-establishing
//! what the first one already found.
//!
//! Every part of that answer is already in the store and already free. This
//! module assembles them: the ranked spans, the text of the top ones, and the
//! resolved callers and callees of the symbols those spans sit inside, one hop
//! each way. Nothing here reaches past one hop -- reverse reachability, impact
//! and the path finder are a different release.
//!
//! The assembly is fitted to a token budget the caller sets, counted with the
//! store's own tokenizer through [`crate::ledger::Counter`], the same one the
//! ledger's savings figures are counted with. A budget too small for the whole
//! answer produces a shorter answer that says what it dropped, never an error
//! and never a span cut in half.

use anyhow::Result;
use serde::Serialize;

use crate::{Filter, Hit, Prefer, fleet::Fleet, store::EdgeEnd};

/// The budget a caller gets when it names none.
///
/// Roughly what one search-then-read round costs today, which is the point:
/// the comparison the harness draws is only honest if the two sides are
/// allowed the same room.
pub const DEFAULT_BUDGET: i64 = 4000;

/// How many spans are located before the budget is spent on them.
///
/// The same depth `hit@8` is measured at, so a brief holds what the gate says
/// the ranking put in front of the caller.
pub const SPANS: usize = 8;

/// Most edges reported per enclosing symbol, each way.
///
/// A symbol with two hundred callers is a symbol whose caller list is not an
/// answer. The count of what was left out travels with the brief.
const EDGES_PER_SYMBOL: usize = 8;

/// What one call returns.
#[derive(Debug, Clone, Serialize)]
pub struct Brief {
    pub question: String,
    /// The budget this brief was fitted to, in tokens.
    pub budget: i64,
    /// What it actually spent.
    pub tokens: i64,
    /// `model` when the store's own tokenizer counted, `chars4` when no model
    /// was loaded and four characters stood in for a token. A figure counted
    /// one way is never comparable with one counted the other, so the label
    /// travels with the number, as it does in the ledger.
    pub counted_with: &'static str,
    pub spans: Vec<Span>,
    pub symbols: Vec<Symbol>,
    pub cut: Cut,
}

/// One located span, with its text when the budget reached it.
#[derive(Debug, Clone, Serialize)]
pub struct Span {
    #[serde(serialize_with = "crate::serialize_plain")]
    pub path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Which ranked lists found it: any of `vector`, `keyword`, `graph`. The
    /// same labels a search hit carries, because it is the same hit.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lists: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    /// Whether the file still looks the way it did when it was indexed.
    pub fresh: bool,
    /// The span's text, present when the budget reached it and absent when it
    /// did not. Never truncated: half a function is worse than a locator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// The one-hop neighbourhood of a symbol a span sits inside.
#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub name: String,
    /// What found this part of the answer. Always `graph` -- an edge is not a
    /// ranked guess, and a reader should not have to wonder which it was.
    pub found_by: &'static str,
    pub callers: Vec<EdgeEnd>,
    pub callees: Vec<EdgeEnd>,
    /// Edges of this symbol that the per-symbol cap left out.
    #[serde(skip_serializing_if = "is_zero")]
    pub hidden: usize,
}

/// What the budget could not fit.
///
/// Zero everywhere is the normal case and serialises to an empty object, so a
/// caller reads "nothing was dropped" without counting anything itself.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Cut {
    /// Spans located whose text did not fit.
    #[serde(skip_serializing_if = "is_zero")]
    pub span_text: usize,
    /// Enclosing symbols whose edges did not fit at all.
    #[serde(skip_serializing_if = "is_zero")]
    pub symbols: usize,
    /// Individual edges dropped for the budget, over and above the per-symbol
    /// cap, which reports its own in [`Symbol::hidden`].
    #[serde(skip_serializing_if = "is_zero")]
    pub edges: usize,
}

fn is_zero(n: &usize) -> bool {
    *n == 0
}

impl Cut {
    pub fn is_empty(&self) -> bool {
        self.span_text == 0 && self.symbols == 0 && self.edges == 0
    }
}

/// Everything one question needs, under `budget` tokens.
///
/// The order the budget is spent in is the order a reader needs it:
///
/// 1. Every span's locator. A `path:line` is a handful of tokens and it is the
///    part an agent cannot act without -- so the budget is never so small that
///    the answer says nothing about where to look.
/// 2. The one-hop edges of the symbols those spans sit inside. Small, and the
///    reason this call exists at all: it is what the caller would otherwise
///    spend a second and third round trip asking for.
/// 3. The text of the spans, best-ranked first, until the budget runs out.
///
/// So text is what a tight budget drops, and it drops from the bottom of the
/// ranking up. That order is a decision, not an accident: a locator plus a
/// caller list tells an agent where to go next, while a single span of text
/// with no neighbourhood is what it already had before this call existed.
pub fn brief(
    fleet: &mut Fleet,
    only: Option<&[String]>,
    question: &str,
    budget: i64,
    filter: &Filter,
    prefer: Prefer,
) -> Result<Brief> {
    let hits = fleet.search_preferring(only, question, SPANS, filter, prefer)?;

    // Immutable from here: the counter borrows the fleet, and so does every
    // graph lookup below it.
    let counter = fleet.counter();
    let counted_with = counter.label();

    let mut spans: Vec<Span> = hits.iter().map(Span::locating).collect();
    let mut tokens: i64 = spans.iter().map(|s| counter.count(&s.locator())).sum();

    // 2. Edges, for each distinct enclosing symbol, in span order.
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut cut = Cut::default();
    let mut seen: Vec<String> = Vec::new();
    for hit in &hits {
        let Some(name) = hit.symbol.as_deref() else {
            continue;
        };
        if seen.iter().any(|s| s == name) {
            continue;
        }
        seen.push(name.to_string());

        let found = fleet.neighbours_in(only, name, &[], false)?;
        let (callers, hidden_callers) = cap(found.callers);
        let (callees, hidden_callees) = cap(found.callees);
        if callers.is_empty() && callees.is_empty() {
            continue;
        }
        let symbol = Symbol {
            name: name.to_string(),
            found_by: "graph",
            callers,
            callees,
            hidden: hidden_callers + hidden_callees,
        };
        let cost = counter.count(&symbol.rendered());
        if tokens + cost > budget {
            cut.symbols += 1;
            cut.edges += symbol.callers.len() + symbol.callees.len();
            continue;
        }
        tokens += cost;
        symbols.push(symbol);
    }

    // 3. Span text, best-ranked first.
    for (span, hit) in spans.iter_mut().zip(&hits) {
        let cost = counter.count(&hit.text);
        if tokens + cost > budget {
            cut.span_text += 1;
            continue;
        }
        tokens += cost;
        span.text = Some(hit.text.clone());
    }

    Ok(Brief {
        question: question.to_string(),
        budget,
        tokens,
        counted_with,
        spans,
        symbols,
        cut,
    })
}

/// The per-symbol cap, and how many it left behind.
fn cap(mut edges: Vec<EdgeEnd>) -> (Vec<EdgeEnd>, usize) {
    let hidden = edges.len().saturating_sub(EDGES_PER_SYMBOL);
    edges.truncate(EDGES_PER_SYMBOL);
    (edges, hidden)
}

impl Span {
    /// A hit's locator, before any budget has been spent on its text.
    fn locating(hit: &Hit) -> Self {
        Self {
            path: hit.path.clone(),
            start_line: hit.start_line,
            end_line: hit.end_line,
            lists: hit.lists.clone(),
            symbol: hit.symbol.clone(),
            symbol_kind: hit.symbol_kind.clone(),
            store: hit.store.clone(),
            fresh: hit.fresh,
            text: None,
        }
    }

    /// What a locator costs, counted as the line a reader actually sees rather
    /// than as a guess at the serialised form.
    fn locator(&self) -> String {
        let mut line = format!("{}:{}-{}", self.path, self.start_line, self.end_line);
        if let Some(symbol) = &self.symbol {
            line.push(' ');
            line.push_str(symbol);
        }
        if let Some(kind) = &self.symbol_kind {
            line.push(' ');
            line.push_str(kind);
        }
        for list in &self.lists {
            line.push(' ');
            line.push_str(list);
        }
        line
    }
}

impl Symbol {
    /// What this symbol's block costs, counted the same way.
    fn rendered(&self) -> String {
        let mut out = self.name.clone();
        for edge in self.callers.iter().chain(&self.callees) {
            out.push('\n');
            out.push_str(&edge.symbol.name);
            out.push(' ');
            out.push_str(&edge.symbol.path);
            out.push(' ');
            out.push_str(&edge.kind);
            out.push(' ');
            out.push_str(&edge.confidence);
        }
        out
    }
}
