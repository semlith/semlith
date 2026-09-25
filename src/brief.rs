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

/// How many spans carry their text.
///
/// "The text of the top ones", which is a different instruction from "as much
/// text as the budget will hold" -- and the harness is what told the two apart.
/// Filling the budget cost 3 660 tokens per answered question against
/// search-then-read's 708 on the same questions, because a brief that spends
/// every token it is allowed is a brief that spends every token it is allowed,
/// whatever the question needed.
///
/// A budget is a ceiling, not a target, and one is what the measurement says
/// the ceiling should be.
///
/// Three was the 0.23.0 answer and 0.24.0 measured what it cost: 1.00 calls
/// and 2 209 tokens per answered question against search-then-read's 2.63
/// calls and 724 tokens on the same questions. A call that saves two round
/// trips and spends three times the tokens is not the cheap path this command
/// exists to be — an agent that reads one span, its neighbourhood and the
/// locators of the rest has what it needs to decide, and the second and third
/// texts are what it was going to skim past.
const TEXT_SPANS: usize = 1;

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
    /// What the best-ranked locator costs, which is the one thing a budget
    /// cannot drop.
    ///
    /// An answer that says nothing about where to look is not a smaller answer,
    /// it is no answer -- so a budget below this buys this and stops, and
    /// `tokens` is then `floor` rather than `budget`. Reported rather than
    /// implied, so a caller comparing `tokens` against `budget` is never left
    /// wondering why the smaller number lost.
    pub floor: i64,
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
    /// Where `symbol` is defined. See [`crate::Hit::symbol_line`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
    /// Whether the file still looks the way it did when it was indexed.
    pub fresh: bool,
    /// The span's text, present when the budget reached it and absent when it
    /// did not. Never truncated: half a function is worse than a locator.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// This span was chosen for text and the budget could not fit it — as
    /// opposed to every other span, which carries no text by design
    /// ([`TEXT_SPANS`]). The two used to share one label, so a brief a fifth
    /// spent said seven spans were "left out for the budget".
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub over_budget: bool,
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
    /// Spans the ranking found that the budget could not even locate.
    #[serde(skip_serializing_if = "is_zero")]
    pub spans: usize,
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
        self.spans == 0 && self.span_text == 0 && self.symbols == 0 && self.edges == 0
    }
}

/// Everything one question needs, under `budget` tokens.
///
/// The order the budget is spent in is the order a reader needs it:
///
/// 1. The locators, best-ranked first. A `path:line` is a handful of tokens and
///    it is the part an agent cannot act without, so the best-ranked one is a
///    floor the budget cannot drop -- an answer that says nothing about where
///    to look is not a smaller answer, it is no answer. Every locator after it
///    is bought only if it fits.
/// 2. The one-hop edges of the symbols those spans sit inside. Small, and the
///    reason this call exists at all: it is what the caller would otherwise
///    spend a second and third round trip asking for.
/// 3. The text of the top [`TEXT_SPANS`] spans, best-ranked first, for as much
///    of it as the budget has left.
///
/// So text is what a tight budget drops, and it drops from the bottom of the
/// ranking up. That order is a decision, not an accident: a locator plus a
/// caller list tells an agent where to go next, while a single span of text
/// with no neighbourhood is what it already had before this call existed.
///
/// The budget is never a target. A brief that filled its 4 000 tokens because
/// it was allowed to cost five times what searching and reading cost for the
/// same answers, which the harness measured and is why the cap exists.
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

    // 1. Locators, best-ranked first, with the first one as the floor.
    let mut spans: Vec<Span> = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    let mut tokens: i64 = 0;
    let mut floor: i64 = 0;
    let mut cut = Cut::default();
    for (i, hit) in hits.iter().enumerate() {
        let span = Span::locating(hit);
        let cost = counter.count(&span.locator());
        if spans.is_empty() {
            floor = cost;
        } else if tokens + cost > budget {
            cut.spans += 1;
            continue;
        }
        tokens += cost;
        spans.push(span);
        kept.push(i);
    }

    // Which span gets the text. For a question about code it is the
    // best-ranked span in a code file: on 2026-09-25 the one text span went to
    // a Markdown section for a question about scoring code, and the answer was
    // built from prose about the function rather than the function.
    let code = crate::code_shaped(question);
    let texted: usize = if code {
        let product =
            |i: usize| crate::filter::is_code(&hits[i].path) && !crate::is_test_path(&hits[i].path);
        let about_tests = question.to_lowercase().contains("test");
        kept.iter()
            .position(|&i| product(i) || (about_tests && crate::filter::is_code(&hits[i].path)))
            .or_else(|| {
                kept.iter()
                    .position(|&i| crate::filter::is_code(&hits[i].path))
            })
            .unwrap_or(0)
    } else {
        0
    };
    let navigational = |kind: &str| crate::graph::NAVIGATIONAL_KINDS.contains(&kind);

    // 2. Edges, for the symbols enclosing the spans this brief actually shows.
    //
    // Not for all eight. The neighbourhood of a span whose text was never
    // included is a list of names with nothing to attach them to, and eight
    // symbols at sixteen edges each is a data dump rather than an answer -- it
    // was two thirds of what a brief cost when the harness first measured one.
    let mut symbols: Vec<Symbol> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for hit in kept.iter().skip(texted).take(TEXT_SPANS).map(|&i| &hits[i]) {
        let Some(name) = hit.symbol.as_deref() else {
            continue;
        };
        // A heading is a place in a document, and "What fusion means here
        // called by Search" is not a fact about code.
        if code && hit.symbol_kind.as_deref().is_some_and(navigational) {
            continue;
        }
        if seen.iter().any(|s| s == name) {
            continue;
        }
        seen.push(name.to_string());

        let mut found = fleet.neighbours_in(only, name, &[], false)?;
        if code {
            found.callers.retain(|e| !navigational(&e.symbol.kind));
            found.callees.retain(|e| !navigational(&e.symbol.kind));
        }
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

    // 3. Span text, for the chosen span only. The others carry none by
    // design, and are not counted as cut: nothing was dropped from them.
    for (i, (span, hit)) in spans
        .iter_mut()
        .zip(kept.iter().map(|&i| &hits[i]))
        .enumerate()
    {
        if i < texted || i >= texted + TEXT_SPANS {
            continue;
        }
        let cost = counter.count(&hit.text);
        if tokens + cost > budget {
            cut.span_text += 1;
            span.over_budget = true;
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
        floor,
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
            symbol_line: hit.symbol_line,
            over_budget: false,
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
