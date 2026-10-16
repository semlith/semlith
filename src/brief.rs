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
    /// The store its span came from, named only when the brief read several.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
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
/// 1. The best-ranked locator, and the locator of the span the text and the
///    edges belong to. A `path:line` is a handful of tokens and it is the part
///    an agent cannot act without, so the best-ranked one is a floor the
///    budget cannot drop -- an answer that says nothing about where to look is
///    not a smaller answer, it is no answer.
/// 2. The one-hop edges of the definition that span sits inside, edge by
///    edge. Small, and the reason this call exists at all: it is what the
///    caller would otherwise spend a second and third round trip asking for.
/// 3. The other locators, best-ranked first, each only if it fits.
/// 4. The text of the top [`TEXT_SPANS`] spans, for as much of it as the
///    budget has left, and none once an edge has been left out.
///
/// So text is what a tight budget drops, and it drops from the bottom of the
/// ranking up. That order is a decision, not an accident: a locator plus a
/// caller list tells an agent where to go next, while a single span of text
/// with no neighbourhood is what it already had before this call existed.
/// Until 0.42.0 every locator was bought before any edge, and at `-b 600`
/// eight long locators left the edges nothing (2026-10-14 audit, G15).
/// Everything left out is counted in [`Cut`] and named by [`dropped`].
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
    brief_leaning(fleet, only, question, budget, filter, Some(prefer))
}

/// [`brief`] for a caller that may have sent no preference: `None` takes
/// each store's `lean` setting, as [`Fleet::search_leaning`] does.
pub fn brief_leaning(
    fleet: &mut Fleet,
    only: Option<&[String]>,
    question: &str,
    budget: i64,
    filter: &Filter,
    prefer: Option<Prefer>,
) -> Result<Brief> {
    let hits = fleet.search_leaning(only, question, SPANS, filter, prefer)?;

    // Immutable from here: the counter borrows the fleet, and so does every
    // graph lookup below it.
    let counter = fleet.counter();
    let counted_with = counter.label();
    // A locator is counted as it is printed: root-relative, as every surface
    // shows it, not the absolute path the store keeps. Counted absolute, eight
    // locators on a deep checkout cost the whole of a 600-token budget and the
    // edges the help promises were never reached (2026-10-14 audit, G15).
    let paths = fleet.shortener();

    let mut tokens: i64 = 0;
    let mut cut = Cut::default();
    let cost_of =
        |i: usize| counter.count(&Span::locating(&hits[i]).locator(&paths.short(&hits[i].path)));

    // Which span gets the text. For a question about code it is the
    // best-ranked span in a code file: on 2026-09-25 the one text span went to
    // a Markdown section for a question about scoring code, and the answer was
    // built from prose about the function rather than the function. An image
    // has no text to show, so it is never the one.
    let code = crate::code_shaped(question);
    let has_text = |i: usize| hits[i].image.is_none();
    let texted: Option<usize> = if hits.is_empty() {
        None
    } else if code {
        let product =
            |i: usize| crate::filter::is_code(&hits[i].path) && !crate::is_test_path(&hits[i].path);
        let about_tests = question.to_lowercase().contains("test");
        let any = 0..hits.len();
        any.clone()
            .find(|&i| product(i) || (about_tests && crate::filter::is_code(&hits[i].path)))
            .or_else(|| any.clone().find(|&i| crate::filter::is_code(&hits[i].path)))
            .or_else(|| any.clone().find(|&i| has_text(i)))
            .or(Some(0))
    } else {
        (0..hits.len()).find(|&i| has_text(i)).or(Some(0))
    };

    // 1. The best-ranked locator, which is the floor, and the locator of the
    // span the edges and the text belong to, without which neither means
    // anything.
    let mut bought: Vec<usize> = Vec::new();
    let floor = if hits.is_empty() { 0 } else { cost_of(0) };
    if !hits.is_empty() {
        tokens += floor;
        bought.push(0);
    }
    let anchored = match texted {
        Some(0) => true,
        Some(t) if tokens + cost_of(t) <= budget => {
            tokens += cost_of(t);
            bought.push(t);
            true
        }
        _ => false,
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
    for hit in texted
        .filter(|_| anchored)
        .into_iter()
        .take(TEXT_SPANS)
        .map(|t| &hits[t])
    {
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

        // From the span's own store: across a fleet, a name shared by two
        // repositories would otherwise mix their edges under one heading.
        let own = hit.store.as_ref().map(std::slice::from_ref);
        // The edges of the definition this span sits in, not of every
        // definition of the name: flask's `dispatch_request` listed each
        // class holding a `dispatch_request` of its own as a caller (G16).
        // Its owner narrows the callers, the way `Type::method` does for
        // `semlith neighbors`, and the callees are the ones leaving it.
        let here = hit.symbol_line.and_then(|line| {
            fleet
                .symbols_in(own.or(only), name, 200)
                .ok()?
                .into_iter()
                .find(|row| row.path == hit.path && row.start_line == line)
        });
        let asked = match here
            .as_ref()
            .and_then(|row| crate::store::owner_of(&row.qualified))
        {
            Some(owner) => format!("{owner}::{name}"),
            None => name.to_string(),
        };
        let mut found = fleet.neighbours_in(own.or(only), &asked, &[], false, filter)?;
        if let Some(row) = &here {
            found.callees.retain(|e| {
                e.from_path.as_deref().is_none_or(|p| p == row.path)
                    && e.from_line.is_none_or(|l| l == row.start_line)
            });
        }
        if code {
            found.callers.retain(|e| !navigational(&e.symbol.kind));
            found.callees.retain(|e| !navigational(&e.symbol.kind));
        }
        // One row per place: two calls from one function, or a call and a
        // reference, printed the same row twice (G16).
        once(&mut found.callers);
        once(&mut found.callees);
        let (callers, hidden_callers) = cap(found.callers);
        let (callees, hidden_callees) = cap(found.callees);
        if callers.is_empty() && callees.is_empty() {
            continue;
        }
        let mut symbol = Symbol {
            name: name.to_string(),
            store: hit.store.clone(),
            found_by: "graph",
            callers: Vec::new(),
            callees: Vec::new(),
            hidden: hidden_callers + hidden_callees,
        };
        // Bought edge by edge, callers first, so a budget that cannot hold
        // the whole block still holds what it can, and says how many it left.
        let head = counter.count(&symbol.name);
        if tokens + head > budget {
            cut.symbols += 1;
            cut.edges += callers.len() + callees.len();
            continue;
        }
        let mut spent = head;
        let edges = callers
            .into_iter()
            .map(|e| (true, e))
            .chain(callees.into_iter().map(|e| (false, e)));
        for (calling, edge) in edges {
            let cost = counter.count(&edge_row(&edge, &paths));
            if tokens + spent + cost > budget {
                cut.edges += 1;
                continue;
            }
            spent += cost;
            if calling {
                symbol.callers.push(edge);
            } else {
                symbol.callees.push(edge);
            }
        }
        if symbol.callers.is_empty() && symbol.callees.is_empty() {
            cut.symbols += 1;
            continue;
        }
        tokens += spent;
        symbols.push(symbol);
    }

    // 3. The other locators, best-ranked first, each only if it fits.
    for i in 0..hits.len() {
        if bought.contains(&i) {
            continue;
        }
        let cost = cost_of(i);
        if tokens + cost > budget {
            cut.spans += 1;
            continue;
        }
        tokens += cost;
        bought.push(i);
    }
    bought.sort_unstable();
    let mut spans: Vec<Span> = bought.iter().map(|&i| Span::locating(&hits[i])).collect();

    // 4. Span text, for the chosen span only. The others carry none by
    // design, and are not counted as cut: nothing was dropped from them.
    //
    // Text is what a tight budget drops first, so a brief that had to leave
    // an edge out carries no text either, however small the text is.
    let edges_cut = cut.edges > 0 || cut.symbols > 0;
    if let Some(t) = texted.filter(|_| anchored)
        && let Some(at) = bought.iter().position(|&i| i == t)
    {
        let cost = counter.count(&hits[t].text);
        if edges_cut || tokens + cost > budget {
            cut.span_text += 1;
            spans[at].over_budget = true;
        } else {
            tokens += cost;
            spans[at].text = Some(hits[t].text.clone());
        }
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

/// Each place once, by the name, file and line a row prints.
fn once(ends: &mut Vec<EdgeEnd>) {
    let mut seen: Vec<(String, String, u32)> = Vec::new();
    ends.retain(|e| {
        let key = (
            e.symbol.name.clone(),
            e.symbol.path.clone(),
            e.symbol.start_line,
        );
        let first = !seen.contains(&key);
        seen.push(key);
        first
    });
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

    /// `:start-end`, or nothing for an image, whose span is the whole
    /// picture: a brief led with `debugger.png:0-0` (G13).
    pub fn range(&self) -> String {
        if self.start_line == 0 && self.end_line == 0 {
            String::new()
        } else {
            format!(":{}-{}", self.start_line, self.end_line)
        }
    }

    /// What a locator costs, counted as the line a reader actually sees rather
    /// than as a guess at the serialised form: `shown` is the path as printed.
    fn locator(&self, shown: &str) -> String {
        let mut line = format!("{shown}{}", self.range());
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

/// One edge as it is counted: the name, the root-relative path, the kind and
/// the confidence.
fn edge_row(edge: &EdgeEnd, paths: &crate::Shortener) -> String {
    format!(
        "{} {} {} {}",
        edge.symbol.name,
        paths.short(&edge.symbol.path),
        edge.kind,
        edge.confidence
    )
}

/// What a span without text says under its locator: the span that carries
/// the text, by its number, so "(text: top span only)" under the first span
/// never stands for the third span's text (G16).
pub fn untexted_note(spans: &[Span]) -> String {
    match spans.iter().position(|s| s.text.is_some()) {
        Some(at) => format!("(locator only; span {} carries the text)", at + 1),
        None => "(locator only)".to_string(),
    }
}

/// What a brief left out, as the words after `-- dropped:`. Every count the
/// cut holds is named: `-b 150` dropped five locators and printed an empty
/// list (G15).
pub fn dropped(cut: &Cut) -> Vec<String> {
    let plural =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut out = Vec::new();
    if cut.spans > 0 {
        out.push(plural(cut.spans, "locator", "locators"));
    }
    if cut.symbols > 0 {
        out.push(plural(cut.symbols, "symbol's edges", "symbols' edges"));
    }
    if cut.edges > 0 {
        out.push(plural(cut.edges, "edge", "edges"));
    }
    if cut.span_text > 0 {
        out.push(plural(cut.span_text, "span's text", "spans' text"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every count a cut holds is said, locators included (G15).
    #[test]
    fn every_drop_is_named() {
        let cut = Cut {
            spans: 5,
            span_text: 1,
            symbols: 1,
            edges: 2,
        };
        assert_eq!(
            dropped(&cut),
            ["5 locators", "1 symbol's edges", "2 edges", "1 span's text"]
        );
        assert!(dropped(&Cut::default()).is_empty());
    }

    /// The note under a span with no text names the span that has it, and an
    /// image's locator carries no `0-0` (G13, G16).
    #[test]
    fn a_span_without_text_names_the_one_with_it() {
        let span = |text: Option<&str>, start: u32| Span {
            path: "a.png".into(),
            start_line: start,
            end_line: start,
            lists: Vec::new(),
            symbol: None,
            symbol_kind: None,
            symbol_line: None,
            store: None,
            fresh: true,
            text: text.map(str::to_string),
            over_budget: false,
        };
        let spans = [span(None, 0), span(None, 3), span(Some("x"), 7)];
        assert_eq!(
            untexted_note(&spans),
            "(locator only; span 3 carries the text)"
        );
        assert_eq!(untexted_note(&spans[..2]), "(locator only)");
        assert_eq!(spans[0].range(), "");
        assert_eq!(spans[0].locator("a.png"), "a.png");
        assert_eq!(spans[1].range(), ":3-3");
    }
}
