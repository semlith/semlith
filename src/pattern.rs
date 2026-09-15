//! `semlith pattern`: a tree-sitter structural query over the indexed files of
//! one language.
//!
//! The question this answers is the one a regex cannot: "every call whose
//! receiver is `self.index`", "every struct with a lifetime parameter". The
//! graph already parses these files with these grammars; this exposes the
//! parser to a caller who knows what shape they are looking for.
//!
//! Two things keep it honest. The grammar comes from [`crate::graph`]'s own
//! table, so a language `pattern` accepts is a language the extractor accepts
//! and there is no second list to drift. And the text comes from the store's
//! chunks rather than from disk, for the reason [`crate::Semlith::read`] does:
//! the boundary that decided what may be indexed is the boundary that decides
//! what may be read back.

use crate::filter::Filter;
use crate::store;
use anyhow::{Result, bail};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

/// How many captures one pattern will return before it stops and says so.
///
/// A pattern over a large store can match tens of thousands of nodes, and the
/// answer stops being useful long before it stops growing — the same reasoning
/// `graph::MAX_NODES` records for a traversal.
pub const MAX_MATCHES: usize = 200;

/// How many files one pattern will parse.
///
/// Parsing is the cost here: the store holds the text already, so the budget
/// is on syntax trees built rather than on bytes read.
pub const MAX_FILES: usize = 2000;

/// One captured node.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Match {
    pub path: String,
    /// The capture's own name in the pattern, without the `@`.
    pub capture: String,
    pub start_line: u32,
    pub end_line: u32,
    /// The captured text, cut to one line's worth so a pattern that captures a
    /// whole `impl` block does not return the block.
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub store: Option<String>,
}

/// What one pattern run produced.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Matches {
    pub language: String,
    pub matches: Vec<Match>,
    /// Files parsed, so an empty answer can be told from an answer about
    /// nothing: "no file of that language is indexed" and "none of them match"
    /// are different facts.
    pub files: usize,
    /// True when [`MAX_MATCHES`] or [`MAX_FILES`] cut the answer short.
    pub truncated: bool,
}

/// Run `source` as a tree-sitter query over every indexed file of `language`.
///
/// The pattern is compiled once, before any file is read, so a syntax error in
/// it costs nothing and is reported as the parser's own message rather than as
/// an empty result — an agent that mistyped a pattern and was handed "no
/// matches" would conclude the code does not contain the shape.
pub fn run(
    db: &rusqlite::Connection,
    language: &str,
    source: &str,
    filter: &Filter,
) -> Result<Matches> {
    let language = language.trim().to_ascii_lowercase();
    let Some(grammar) = crate::graph::language_grammar(&language) else {
        bail!(
            "no grammar for {language:?}; the languages with one are {}",
            crate::graph::languages().join(", ")
        );
    };
    let query = Query::new(&grammar, source)
        .map_err(|e| anyhow::anyhow!("that is not a valid tree-sitter pattern: {e:?}"))?;
    let names = query.capture_names().to_vec();

    // The language filter and whatever the caller asked for, intersected the
    // way every other surface intersects them — one id set, one predicate.
    let scoped = filter.and_language(&language)?;
    let files = store::file_rows(db, scoped.groups(), store::FileSort::Path, false, i64::MAX)?;

    let mut parser = Parser::new();
    parser.set_language(&grammar)?;

    let mut out = Vec::new();
    let mut parsed = 0usize;
    let mut truncated = false;
    for file in files.iter().take(MAX_FILES) {
        if out.len() >= MAX_MATCHES {
            truncated = true;
            break;
        }
        let Some(text) = file_text(db, &file.path)? else {
            continue;
        };
        let Some(tree) = parser.parse(&text, None) else {
            // tree-sitter returns a tree for anything it can hold; `None` is a
            // parser that was cancelled or given a language it cannot use, and
            // neither is this file's fault.
            continue;
        };
        parsed += 1;

        let mut cursor = QueryCursor::new();
        let mut hits = cursor.matches(&query, tree.root_node(), text.as_bytes());
        while let Some(m) = hits.next() {
            for capture in m.captures() {
                if out.len() >= MAX_MATCHES {
                    truncated = true;
                    break;
                }
                let node = capture.node;
                out.push(Match {
                    path: file.path.clone(),
                    capture: names
                        .get(capture.index as usize)
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                    start_line: node.start_position().row as u32 + 1,
                    end_line: node.end_position().row as u32 + 1,
                    text: first_line(&text[node.byte_range()]),
                    store: None,
                });
            }
        }
    }
    if files.len() > MAX_FILES {
        truncated = true;
    }

    Ok(Matches {
        language,
        matches: out,
        files: parsed,
        truncated,
    })
}

/// One file's text, stitched from the chunks the store holds.
///
/// Chunks overlap by two lines, so they are joined by line number rather than
/// concatenated — the seam would otherwise appear twice and every line after
/// it would be numbered wrongly, which for a tool whose whole output is line
/// numbers is the only thing that matters.
fn file_text(db: &rusqlite::Connection, path: &str) -> Result<Option<String>> {
    let chunks = store::chunks_overlapping(db, path, 1, u32::MAX)?;
    if chunks.is_empty() {
        return Ok(None);
    }
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

    // The stitched text has to start at line 1 for the parser's row numbers to
    // mean what they say. A store that never held the head of the file cannot
    // be parsed into line numbers a reader can use.
    if lines.first().map(|(n, _)| *n) != Some(1) {
        return Ok(None);
    }
    Ok(Some(
        lines
            .into_iter()
            .map(|(_, line)| line)
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

/// The first line of a captured node, trimmed.
///
/// A pattern that captures an `impl` block captures hundreds of lines, and a
/// caller who wanted those has `semlith read` and the span this row carries.
fn first_line(raw: &str) -> String {
    const WIDTH: usize = 120;
    let line = raw.lines().next().unwrap_or("").trim();
    if line.chars().count() <= WIDTH {
        return line.to_string();
    }
    line.chars().take(WIDTH).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pattern that does not compile is the caller's mistake and has to be
    /// reported as one. Handing back "no matches" would have the caller
    /// conclude the code does not contain the shape they asked about.
    #[test]
    fn an_invalid_pattern_is_an_error_rather_than_an_empty_answer() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(store::SCHEMA).unwrap();
        let err = run(&db, "rust", "(this is not a pattern", &Filter::default())
            .expect_err("an unbalanced pattern does not compile");
        assert!(
            err.to_string().contains("not a valid tree-sitter pattern"),
            "{err}"
        );
    }

    /// Every language the filter advertises compiles a pattern, because
    /// `pattern` and the extractor read the same grammar table.
    ///
    /// A pattern is the one surface where a missing grammar is an error a user
    /// sees rather than a silent absence of symbols, so it is asserted for all
    /// forty-six rather than assumed from the extractor's own gate.
    #[test]
    fn every_advertised_language_compiles_a_pattern() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(store::SCHEMA).unwrap();
        let mut broken: Vec<String> = Vec::new();
        for entry in crate::filter::LANGUAGES {
            if !crate::graph::has_graph(entry.name) {
                continue;
            }
            if let Err(e) = run(&db, entry.name, "(_) @node", &Filter::default()) {
                broken.push(format!("{}: {e}", entry.name));
            }
        }
        assert!(broken.is_empty(), "{}", broken.join("\n"));
    }

    /// A language with no grammar is named, with the ones that have one, so a
    /// caller can correct it in one step.
    #[test]
    fn a_language_with_no_grammar_says_which_have_one() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(store::SCHEMA).unwrap();
        let err = run(&db, "cobol", "(identifier) @x", &Filter::default())
            .expect_err("cobol carries no grammar in this release");
        assert!(err.to_string().contains("rust"), "{err}");
    }
}
