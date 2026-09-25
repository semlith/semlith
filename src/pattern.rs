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
    /// Matches this run passed over to honour the caller's `offset`.
    ///
    /// Reported so a fleet can carry one offset across several stores: the
    /// second store's offset is the caller's minus what the first one already
    /// skipped, and without this number there is nothing to subtract.
    pub skipped: usize,
}

/// Run `source` as a tree-sitter query over every indexed file of `language`.
///
/// The pattern is compiled once, before any file is read, so a syntax error in
/// it costs nothing and is reported as the parser's own message rather than as
/// an empty result — an agent that mistyped a pattern and was handed "no
/// matches" would conclude the code does not contain the shape.
/// `offset` continues a listing the cap cut short. The order is total — files
/// are sorted and the matches within a file are tree-sitter's — so the same
/// call with the same offset returns the same matches, which is what makes
/// paging through a cap meaningful rather than a second sample.
pub fn run(
    db: &rusqlite::Connection,
    language: &str,
    source: &str,
    filter: &Filter,
    offset: usize,
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
    let mut skipped = 0usize;
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
                // Skipped after the match is found, not before the file is
                // read: the offset is into the match sequence, and a file
                // holding forty matches contributes forty to it whether or not
                // this call renders them.
                if skipped < offset {
                    skipped += 1;
                    continue;
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
        skipped,
    })
}

/// Every indexed line matching `source`, as `grep -E` would find it.
///
/// The sweep an agent reached for grep to make: every marker comment, every struct
/// literal, every use of a string. Answered from the store's text, the boundary
/// [`run`] reads through, so a refused file stays unread. Each line carries
/// the definition it sits in, which is the next thing a grep user runs `awk`
/// to find. A query that is not a valid regular expression is taken as
/// literal text, because `Row {` is what someone typing a literal means.
///
/// `files` counts files searched and `truncated` means [`MAX_MATCHES`] cut the
/// listing, as for a pattern; `offset` pages through it the same way.
pub fn grep(
    db: &rusqlite::Connection,
    source: &str,
    filter: &Filter,
    offset: usize,
) -> Result<Matches> {
    let regex = regex::Regex::new(source)
        .or_else(|_| regex::Regex::new(&regex::escape(source)))
        .map_err(|e| anyhow::anyhow!("that query cannot be searched for: {e}"))?;
    let files = store::file_rows(db, filter.groups(), store::FileSort::Path, false, i64::MAX)?;
    let mut out = Vec::new();
    let mut searched = 0usize;
    let mut truncated = false;
    let mut skipped = 0usize;
    'files: for file in &files {
        let Some(text) = file_text(db, &file.path)? else {
            continue;
        };
        searched += 1;
        let mut symbols = None;
        for (i, line) in text.lines().enumerate() {
            if !regex.is_match(line) {
                continue;
            }
            if out.len() >= MAX_MATCHES {
                truncated = true;
                break 'files;
            }
            if skipped < offset {
                skipped += 1;
                continue;
            }
            // Read once per file with a match, not per file searched.
            if symbols.is_none() {
                symbols = Some(
                    store::symbols_in_files(db, std::slice::from_ref(&file.path))?
                        .remove(&file.path)
                        .unwrap_or_default(),
                );
            }
            let defs = symbols.as_deref().unwrap_or_default();
            let at = i as u32 + 1;
            out.push(Match {
                path: file.path.clone(),
                capture: crate::enclosing_definition(defs, at, at)
                    .map(|(_, name, _)| name)
                    .unwrap_or_default(),
                start_line: at,
                end_line: at,
                text: first_line(line),
                store: None,
            });
        }
    }
    Ok(Matches {
        language: String::new(),
        matches: out,
        files: searched,
        truncated,
        skipped,
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
    // Keyed by line number: a scan of the lines so far for each new one was
    // quadratic in the file, which a grep over every file in the store pays.
    let mut lines: std::collections::BTreeMap<u32, String> = Default::default();
    for chunk in &chunks {
        for (offset, line) in chunk.text.lines().enumerate() {
            lines
                .entry(chunk.start_line + offset as u32)
                .or_insert_with(|| line.to_string());
        }
    }

    // The stitched text has to start at line 1 for the parser's row numbers to
    // mean what they say. A store that never held the head of the file cannot
    // be parsed into line numbers a reader can use.
    if lines.keys().next() != Some(&1) {
        return Ok(None);
    }
    Ok(Some(lines.into_values().collect::<Vec<_>>().join("\n")))
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

    /// A grep answers every matching line with the definition around it, and
    /// a query that is not a regular expression is searched as literal text.
    #[test]
    fn grep_finds_every_line_and_names_its_definition() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(store::SCHEMA).unwrap();
        db.execute_batch(
            "INSERT INTO files (id, path, hash, bytes, indexed_at) VALUES (1, '/r/a.rs', 'h', 1, 0);
             INSERT INTO chunks (id, file_id, ord, start_line, end_line, text)
               VALUES (1, 1, 0, 1, 4, 'fn one() {\n    // NOTE first\n}\nlet h = Row {');
             INSERT INTO chunks (id, file_id, ord, start_line, end_line, text)
               VALUES (2, 1, 1, 3, 6, '}\nlet h = Row {\nfn two() { // HACK second }\n');
             INSERT INTO symbols (file_id, name, qualified, kind, start_line, end_line)
               VALUES (1, 'one', 'one', 'function', 1, 3), (1, 'two', 'two', 'function', 5, 5);",
        )
        .unwrap();
        let found = grep(&db, r"\b(NOTE|HACK)\b", &Filter::default(), 0).unwrap();
        let rows: Vec<_> = found
            .matches
            .iter()
            .map(|m| (m.start_line, m.capture.as_str()))
            .collect();
        assert_eq!(rows, [(2, "one"), (5, "two")]);
        let literal = grep(&db, "Row {", &Filter::default(), 0).unwrap();
        assert_eq!(literal.matches.len(), 1, "{:?}", literal.matches);
        assert_eq!(literal.matches[0].start_line, 4);
    }

    /// A pattern that does not compile is the caller's mistake and has to be
    /// reported as one. Handing back "no matches" would have the caller
    /// conclude the code does not contain the shape they asked about.
    #[test]
    fn an_invalid_pattern_is_an_error_rather_than_an_empty_answer() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(store::SCHEMA).unwrap();
        let err = run(&db, "rust", "(this is not a pattern", &Filter::default(), 0)
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
            if let Err(e) = run(&db, entry.name, "(_) @node", &Filter::default(), 0) {
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
        let err = run(&db, "cobol", "(identifier) @x", &Filter::default(), 0)
            .expect_err("cobol carries no grammar in this release");
        assert!(err.to_string().contains("rust"), "{err}");
    }
}

#[cfg(test)]
mod offset_tests {
    use super::*;

    /// A store holding one Rust file with `calls` distinct call expressions,
    /// written straight into the schema — no embedder, because a pattern run
    /// reads chunk text and never a vector.
    fn store_of(calls: usize) -> rusqlite::Connection {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch(store::SCHEMA).unwrap();
        let mut body = String::from("fn main() {\n");
        for i in 0..calls {
            body.push_str(&format!("    call_{i}();\n"));
        }
        body.push_str("}\n");
        let lines = body.lines().count() as u32;
        let file = store::insert_file(&db, "/work/api/src/main.rs", "hash", 1, 0).unwrap();
        store::insert_chunk(&db, file, 0, 1, lines, &body).unwrap();
        db
    }

    /// The cap is only useful if there is a way past it, and the way past it
    /// is only useful if the order is total.
    #[test]
    fn offset_continues_the_listing_and_repeats_exactly() {
        let db = store_of(MAX_MATCHES + 40);
        let pattern = "(call_expression function: (identifier) @f)";

        let first = run(&db, "rust", pattern, &Filter::default(), 0).unwrap();
        assert_eq!(first.matches.len(), MAX_MATCHES);
        assert!(first.truncated);

        let next = run(&db, "rust", pattern, &Filter::default(), MAX_MATCHES).unwrap();
        assert_eq!(next.skipped, MAX_MATCHES);
        assert_eq!(next.matches.len(), 40);
        assert_eq!(
            next.matches[0].text, "call_200",
            "the continuation starts at the match after the cap"
        );
        assert!(
            !first.matches.iter().any(|m| m.text == "call_200"),
            "the two pages must not overlap"
        );

        // Two identical calls, two identical answers. Without this the offset
        // is a second sample rather than a continuation.
        let again = run(&db, "rust", pattern, &Filter::default(), MAX_MATCHES).unwrap();
        let texts: Vec<&str> = next.matches.iter().map(|m| m.text.as_str()).collect();
        let repeat: Vec<&str> = again.matches.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, repeat);
    }

    /// The same glob filter every other surface takes, which is the other way
    /// past the cap.
    #[test]
    fn a_path_filter_narrows_which_files_are_parsed() {
        let db = store_of(3);
        let pattern = "(call_expression function: (identifier) @f)";

        let all = run(&db, "rust", pattern, &Filter::default(), 0).unwrap();
        assert_eq!(all.files, 1);

        let elsewhere = Filter::new(&["src/other/**".to_string()], &[], &[]).unwrap();
        let none = run(&db, "rust", pattern, &elsewhere, 0).unwrap();
        assert_eq!(none.files, 0);
        assert!(none.matches.is_empty());

        let here = Filter::new(&["**/src/**".to_string()], &[], &[]).unwrap();
        let some = run(&db, "rust", pattern, &here, 0).unwrap();
        assert_eq!(some.matches.len(), 3);
    }
}
