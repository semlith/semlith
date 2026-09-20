//! Five reports, generated from this machine's ledger, index and graph.
//!
//! Nothing here reaches the network, nothing here is a model's opinion, and
//! nothing here is a number without its denominator. A report is a title and
//! a list of blocks — a paragraph, or a table with named columns — and the
//! four renderers below turn that one structure into Markdown, CSV, JSON or
//! print-styled HTML. One structure rather than four writers, so the formats
//! cannot disagree about what a report says, and so `semlith report` and the
//! portal's export produce the same bytes.
//!
//! PDF is the browser's own Print-to-PDF over the HTML, which is why the
//! HTML carries a print stylesheet and why no PDF dependency is in the
//! binary.

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
    pub stores: Vec<String>,
    pub blocks: Vec<Block>,
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

/// The output formats. PDF is the browser's print of the HTML.
pub const FORMATS: [&str; 4] = ["markdown", "csv", "json", "html"];

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
pub fn check_format(format: &str) -> Result<()> {
    if FORMATS.contains(&format) || format == "md" {
        return Ok(());
    }
    bail!(
        "unknown format {format:?}; the formats are {}",
        FORMATS.join(", ")
    )
}

/// Generate one report over the chosen stores.
pub fn generate(fleet: &crate::fleet::Fleet, kind: &str, model: &str) -> Result<Report> {
    let (kind, title) = kind_of(kind)?;
    let stores: Vec<String> = fleet.each().map(|(label, _)| label.to_string()).collect();
    let blocks = match kind {
        "savings" => savings(fleet, model)?,
        "access" => access(fleet)?,
        "change" => change(fleet)?,
        "health" => health(fleet)?,
        _ => gaps(fleet)?,
    };
    Ok(Report {
        kind,
        title: title.to_string(),
        generated: crate::clock::local_stamp(unix_now()),
        stores,
        blocks,
    })
}

/// What was not read, what that would have cost, and how much of the ledger
/// the figure covers.
fn savings(fleet: &crate::fleet::Fleet, model: &str) -> Result<Vec<Block>> {
    let (name, per_million) = price_of(model);
    let (mut net, mut credited, mut total, mut measured) = (0i64, 0i64, 0i64, true);
    let (mut refunds, mut zero_hit, mut refunds_measured) = (0i64, 0i64, false);
    let (mut excerpt, mut whole) = (0i64, 0i64);
    let (mut p50, mut p95) = (0i64, 0i64);
    let mut head = None;
    let mut intact = true;
    for (_, store) in fleet.each() {
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
fn access(fleet: &crate::fleet::Fleet) -> Result<Vec<Block>> {
    let mut rows = Vec::new();
    let mut intact = true;
    for (label, store) in fleet.each() {
        for session in crate::store::ledger_sessions(store.db(), 200)? {
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
        intact = intact && crate::store::ledger_break(store.db())?.is_none();
    }
    rows.sort_by(|a, b| b[0].cmp(&a[0]));
    Ok(vec![
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
    ])
}

/// What this machine has read lately.
fn change(fleet: &crate::fleet::Fleet) -> Result<Vec<Block>> {
    const WINDOW_DAYS: i64 = 7;
    let since = unix_now() - WINDOW_DAYS * 24 * 60 * 60;
    let mut rows = Vec::new();
    let mut retired = 0i64;
    for (label, store) in fleet.each() {
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
                "Files this machine re-read in the last {WINDOW_DAYS} days, newest first. \
                 Re-read, not rewritten: the store records when it read a file, not when anybody \
                 changed it."
            ),
        },
        Block::Facts {
            facts: vec![
                (
                    "Files re-read".into(),
                    n(rows.len() as i64),
                    format!("in {WINDOW_DAYS} days"),
                ),
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
fn health(fleet: &crate::fleet::Fleet) -> Result<Vec<Block>> {
    let mut rows = Vec::new();
    let (mut files, mut chunks, mut bytes) = (0i64, 0i64, 0i64);
    let (mut unresolved, mut several) = (0i64, 0i64);
    for (label, store) in fleet.each() {
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
fn gaps(fleet: &crate::fleet::Fleet) -> Result<Vec<Block>> {
    let mut asked = Vec::new();
    let mut missing = Vec::new();
    let mut several = 0i64;
    for (label, store) in fleet.each() {
        for (query, count) in crate::store::ledger_zero_hit_queries(store.db(), 20)? {
            asked.push(vec![query, n(count), label.to_string()]);
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

impl Report {
    /// Render in one of [`FORMATS`].
    pub fn render(&self, format: &str) -> Result<String> {
        Ok(match format {
            "markdown" | "md" => self.markdown(),
            "csv" => self.csv(),
            "json" => serde_json::to_string_pretty(self)? + "\n",
            "html" => self.html(),
            other => bail!(
                "unknown format {other:?}; the formats are {}",
                FORMATS.join(", ")
            ),
        })
    }

    fn header(&self) -> String {
        format!(
            "{}\n\nGenerated {} on this machine, over {}. Nothing left it.",
            self.title,
            self.generated,
            if self.stores.is_empty() {
                "no open store".to_string()
            } else {
                self.stores.join(", ")
            }
        )
    }

    fn markdown(&self) -> String {
        let mut out = format!("# {}\n\n", self.title);
        out.push_str(&format!(
            "Generated {} on this machine, over {}. Nothing left it.\n",
            self.generated,
            if self.stores.is_empty() {
                "no open store".to_string()
            } else {
                self.stores.join(", ")
            }
        ));
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
        // Print-styled on purpose: PDF is this page printed by the reader's
        // own browser, which is why no PDF writer is in the binary.
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
        let e = report.render("pdf").unwrap_err().to_string();
        assert!(e.contains("markdown"), "{e}");
        assert!(e.contains("html"), "{e}");
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
