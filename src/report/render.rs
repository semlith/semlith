//! Four of the five renderers: Markdown, CSV, JSON and HTML. The PDF is
//! `pdf.rs`. Each reads the same typed structure and nothing else.

use super::{ALL_FORMATS, Block, CSV, Cell, Column, Kind, PDF, Report, Table, num};
use anyhow::{Result, bail};

/// The Content-Security-Policy the portal's report preview is served under
/// (`/api/report/preview`): no script, no network, no form — the embedded
/// stylesheet, fonts and logos are all a report is allowed. `frame-ancestors`
/// lets the portal's own sandboxed frame show it and nothing else.
pub const REPORT_CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; \
     font-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self'";

impl Report {
    /// Render as text: markdown, json or html. CSV is a zip of tables (or one
    /// named table, [`Report::render_table`]) and PDF is bytes, so both go
    /// through [`Report::render_bytes`].
    pub fn render(&self, format: &str) -> Result<String> {
        Ok(match format {
            "markdown" | "md" => self.markdown(),
            "json" => serde_json::to_string_pretty(self)? + "\n",
            "html" => self.html(),
            CSV => bail!(
                "csv is one file per table — name one with --table ({}), or write the zip with --out",
                self.table_ids().join(", ")
            ),
            PDF => bail!("pdf is bytes rather than text; render it with render_bytes"),
            other => bail!(
                "unknown format {other:?}; the formats are {}",
                ALL_FORMATS.join(", ")
            ),
        })
    }

    /// Render in any of the five formats, as the bytes of the file.
    pub fn render_bytes(&self, format: &str) -> Result<Vec<u8>> {
        match format {
            PDF => Ok(self.pdf()),
            CSV => self.csv_zip(),
            other => self.render(other).map(String::into_bytes),
        }
    }

    /// The extension a file of this format is saved under: a report's CSV is
    /// a zip.
    pub fn extension(format: &str) -> &'static str {
        match format {
            CSV => "zip",
            "json" => "json",
            "html" => "html",
            PDF => "pdf",
            _ => "md",
        }
    }

    /// The ids `--table` takes: `summary`, then each table in order.
    pub fn table_ids(&self) -> Vec<&'static str> {
        std::iter::once("summary")
            .chain(self.tables().map(|t| t.id))
            .collect()
    }

    /// One table as a plain CSV: a header of stable snake_case keys, numbers
    /// unformatted, stamps ISO 8601. `summary` is every headline figure as
    /// `key,value,unit,currency,over`.
    pub fn render_table(&self, id: &str) -> Result<String> {
        if id == "summary" {
            return Ok(self.summary_csv());
        }
        match self.tables().find(|t| t.id == id) {
            Some(table) => Ok(table_csv(table)),
            None => bail!(
                "this report has no table {id:?}; its tables are {}",
                self.table_ids().join(", ")
            ),
        }
    }

    fn summary_csv(&self) -> String {
        let mut out = String::from("key,value,unit,currency,over\n");
        let mut push = |label: &str, f: &super::Figure| {
            out.push_str(&format!(
                "{},{},{},{},{}\n",
                snake(label),
                value_cell(&f.value),
                f.unit,
                f.currency.unwrap_or(""),
                csv_cell(&f.over)
            ));
        };
        for kpi in &self.kpis {
            push(&kpi.label, &kpi.figure);
        }
        for block in &self.blocks {
            if let Block::Facts { facts, .. } = block {
                for fact in facts {
                    push(&fact.label, &fact.figure);
                }
            }
        }
        out.push_str(&format!(
            "window,{},text,,{}\n",
            self.window.name,
            csv_cell(&format!(
                "{} to {}",
                self.window.from.as_deref().unwrap_or("the first row"),
                self.window.to
            ))
        ));
        out.push_str(&format!("generated_at,{},stamp,,\n", self.generated_at));
        if let Some(chain) = &self.chain {
            out.push_str(&format!(
                "ledger_chain,{},text,,{}\n",
                chain.state,
                csv_cell(chain.head.as_deref().unwrap_or(""))
            ));
        }
        out
    }

    /// Every table as its own CSV beside `summary.csv`, zipped.
    fn csv_zip(&self) -> Result<Vec<u8>> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for id in self.table_ids() {
            zip.start_file(format!("{}-{id}.csv", self.kind), options)?;
            zip.write_all(self.render_table(id)?.as_bytes())?;
        }
        Ok(zip.finish()?.into_inner())
    }

    // ----------------------------------------------------------- markdown

    fn markdown(&self) -> String {
        let mut out = format!("# {}\n\n", md_text(&self.title));
        let lede: String = self
            .lede
            .iter()
            .map(|r| {
                if r.bold {
                    format!("**{}**", md_text(&r.text))
                } else {
                    md_text(&r.text)
                }
            })
            .collect();
        if !lede.is_empty() {
            out.push_str(&lede);
            out.push_str("\n\n");
        }
        out.push_str("| | |\n|---|---|\n");
        for meta in &self.meta {
            let mut cells: Vec<String> = meta.lines.iter().map(|l| md_cell(l)).collect();
            if !meta.chips.is_empty() {
                cells.insert(
                    0,
                    meta.chips
                        .iter()
                        .map(|c| format!("`{}`", c.text.replace('`', "'")))
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            out.push_str(&format!(
                "| **{}** | {} |\n",
                md_cell(&meta.label),
                cells.join(" · ")
            ));
        }
        out.push('\n');
        if !self.kpis.is_empty() {
            let labels: Vec<String> = self.kpis.iter().map(|k| md_cell(&k.label)).collect();
            out.push_str(&format!("| {} |\n", labels.join(" | ")));
            out.push_str(&format!("|{}\n", "---:|".repeat(self.kpis.len())));
            let values: Vec<String> = self
                .kpis
                .iter()
                .map(|k| format!("**{}**", md_cell(&k.figure.display)))
                .collect();
            out.push_str(&format!("| {} |\n", values.join(" | ")));
            let overs: Vec<String> = self.kpis.iter().map(|k| md_cell(&k.figure.over)).collect();
            out.push_str(&format!("| {} |\n\n", overs.join(" | ")));
        }
        for block in &self.blocks {
            match block {
                Block::Table(table) => out.push_str(&md_table(table)),
                Block::Facts {
                    title,
                    meta,
                    facts,
                    note,
                } => {
                    out.push_str(&format!("## {}\n\n_{}_\n\n", md_text(title), md_text(meta)));
                    out.push_str("| Figure | Value | Over |\n|---|---:|---|\n");
                    for fact in facts {
                        out.push_str(&format!(
                            "| {} | {} | {} |\n",
                            md_cell(&fact.label),
                            md_cell(&fact.figure.display),
                            md_cell(&fact.figure.over)
                        ));
                    }
                    out.push('\n');
                    if let Some(note) = note {
                        out.push_str(&format!("> {}\n\n", md_text(note)));
                    }
                }
                Block::Note { text } => out.push_str(&format!("> {}\n\n", md_text(text))),
                Block::Code {
                    title,
                    comment,
                    lines,
                    ..
                } => {
                    out.push_str(&format!("## {}\n\n```sh\n# {comment}\n", md_text(title)));
                    for line in lines {
                        out.push_str(line);
                        out.push('\n');
                    }
                    out.push_str("```\n\n");
                }
            }
        }
        out.push_str(&format!("---\n\n{}\n", md_text(&self.footer(" · "))));
        out
    }

    /// `semlith 0.42.0 · Generated … · ledger head …`, every format's last line.
    pub(super) fn footer(&self, sep: &str) -> String {
        let mut parts = vec![
            format!("semlith {}", self.version),
            match &self.origin {
                None => format!(
                    "Generated {} on this machine · nothing uploaded",
                    self.generated
                ),
                Some(origin) => format!("Generated {} in {origin}", self.generated),
            },
        ];
        if let Some(head) = self.chain.as_ref().and_then(|c| c.head.as_deref()) {
            parts.push(format!("ledger head {head}"));
        }
        parts.join(sep)
    }

    // --------------------------------------------------------------- html

    fn html(&self) -> String {
        let mut bars = std::collections::BTreeSet::new();
        let mut body = String::new();
        for block in &self.blocks {
            match block {
                Block::Table(table) => body.push_str(&html_table(table, &mut bars)),
                Block::Facts {
                    title,
                    meta,
                    facts,
                    note,
                } => {
                    body.push_str(&card_head(title, meta));
                    body.push_str("<div class=\"facts\">");
                    for fact in facts {
                        body.push_str(&format!(
                            "<div><span class=\"eyebrow\">{}</span><div class=\"v\">{}</div><div class=\"s\">{}</div></div>",
                            esc(&fact.label),
                            esc(&fact.figure.display),
                            esc(&fact.figure.over)
                        ));
                    }
                    body.push_str("</div>");
                    if let Some(note) = note {
                        body.push_str(&format!("<div class=\"card-note\">{}</div>", inline(note)));
                    }
                    body.push_str("</section>\n");
                }
                Block::Note { text } => body.push_str(&format!(
                    "<section class=\"card\"><div class=\"card-note solo\">{}</div></section>\n",
                    inline(text)
                )),
                Block::Code {
                    title,
                    meta,
                    comment,
                    lines,
                } => {
                    body.push_str(&card_head(title, meta));
                    body.push_str(&format!("<pre><span class=\"c\"># {}</span>", esc(comment)));
                    for line in lines {
                        body.push('\n');
                        body.push_str(&esc(line));
                    }
                    body.push_str("</pre></section>\n");
                }
            }
        }
        let lede: String = self
            .lede
            .iter()
            .map(|r| {
                if r.bold {
                    format!("<b>{}</b>", esc(&r.text))
                } else {
                    esc(&r.text)
                }
            })
            .collect();
        let meta: String = self
            .meta
            .iter()
            .map(|m| {
                let mut dd = String::new();
                if !m.chips.is_empty() {
                    dd.push_str("<span class=\"chips\">");
                    for chip in &m.chips {
                        dd.push_str(&pill(&chip.text, chip.tone));
                    }
                    dd.push_str("</span>");
                }
                for (i, line) in m.lines.iter().enumerate() {
                    if i == 0 && m.chips.is_empty() {
                        dd.push_str(&format!("<span class=\"line\">{}</span>", esc(line)));
                    } else {
                        dd.push_str(&format!("<span class=\"line muted\">{}</span>", esc(line)));
                    }
                }
                format!(
                    "<div><dt class=\"eyebrow\">{}</dt><dd>{dd}</dd></div>",
                    esc(&m.label)
                )
            })
            .collect();
        let kpis: String = self
            .kpis
            .iter()
            .map(|k| {
                format!(
                    "<div class=\"kpi{}\"><span class=\"eyebrow\">{}</span><span class=\"v\">{}</span><span class=\"s\">{}</span></div>",
                    if k.lead { " lead" } else { "" },
                    esc(&k.label),
                    esc(&k.figure.display),
                    esc(&k.figure.over)
                )
            })
            .collect();
        let kind_label = super::KINDS
            .iter()
            .find(|(k, _)| *k == self.kind)
            .map_or(self.title.as_str(), |(_, t)| t);
        let footer: String = {
            let mut parts = vec![
                format!("<span>semlith {}</span>", esc(&self.version)),
                format!(
                    "<span>Generated {}{}</span>",
                    esc(&self.generated),
                    self.origin
                        .as_deref()
                        .map(|o| format!(" in {}", esc(o)))
                        .unwrap_or_default()
                ),
            ];
            if let Some(head) = self.chain.as_ref().and_then(|c| c.head.as_deref()) {
                parts.push(format!(
                    "<span class=\"hash\">ledger head {}</span>",
                    esc(head)
                ));
            }
            parts.join("<span aria-hidden=\"true\">·</span>")
        };
        let pill_top = match &self.origin {
            None => pill("generated on this machine · nothing uploaded", "green"),
            Some(origin) => pill(&format!("generated in {origin}"), "blue"),
        };
        format!(
            "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
             <meta name=\"color-scheme\" content=\"light dark\">\
             <meta name=\"generator\" content=\"semlith {version}\">\
             <title>{title} — Semlith</title>\n<style>\n{css}</style></head><body><main class=\"sheet\">\n\
             <header class=\"top\"><img class=\"logo-l\" alt=\"\" src=\"{logo}\"><img class=\"logo-d\" alt=\"\" src=\"{logo_dark}\">\
             <span class=\"word\">Semlith</span><span class=\"vr\"></span><span class=\"kind\">{kind}</span>\
             <span class=\"grow\"></span>{pill_top}</header>\n\
             <section class=\"title\"><div class=\"eyebrow\">{kind} · {audience}</div><h1>{title}</h1><p class=\"lede\">{lede}</p></section>\n\
             <dl class=\"meta\">{meta}</dl>\n<section class=\"kpis k{nk}\">{kpis}</section>\n{body}\
             <footer class=\"foot\">{footer}</footer>\n</main></body></html>\n",
            version = esc(&self.version),
            title = esc(&self.title),
            css = stylesheet(&bars),
            logo = data_uri("image/svg+xml", include_bytes!("../portal/logo.svg")),
            logo_dark = data_uri("image/svg+xml", include_bytes!("../portal/logo-dark.svg")),
            kind = esc(kind_label),
            audience = esc(&self.audience),
            nk = self.kpis.len(),
        )
    }
}

// ------------------------------------------------------------- markdown

/// Rows past which a Markdown table folds into `<details>`, so a PR body
/// stays readable.
const FOLD: usize = 10;

fn md_table(table: &Table) -> String {
    let mut out = format!(
        "## {}\n\n_{}_\n\n",
        md_text(&table.title),
        md_text(&table.meta)
    );
    if table.rows.is_empty() {
        out.push_str(&format!("{}\n\n", md_text(&table.empty)));
        return out;
    }
    let mut grid = String::new();
    let labels: Vec<String> = table.columns.iter().map(|c| md_cell(&c.label)).collect();
    grid.push_str(&format!("| {} |\n", labels.join(" | ")));
    let rules: Vec<&str> = table
        .columns
        .iter()
        .map(|c| match c.kind {
            Kind::Num | Kind::Bar => "---:",
            _ => "---",
        })
        .collect();
    grid.push_str(&format!("| {} |\n", rules.join(" | ")));
    for row in &table.rows {
        grid.push_str(&md_row(&table.columns, row, false));
    }
    if let Some(total) = &table.total {
        grid.push_str(&md_row(&table.columns, total, true));
    }
    if table.rows.len() > FOLD {
        out.push_str(&format!(
            "<details><summary>{} rows</summary>\n\n{grid}\n</details>\n\n",
            num::int(table.rows.len() as i64)
        ));
    } else {
        out.push_str(&grid);
        out.push('\n');
    }
    if table.more > 0 {
        out.push_str(&format!(
            "_{} more not listed._\n\n",
            num::int(table.more as i64)
        ));
    }
    if let Some(note) = &table.note {
        out.push_str(&format!("> {}\n\n", md_text(note)));
    }
    out
}

fn md_row(columns: &[Column], row: &[Cell], bold: bool) -> String {
    let cells: Vec<String> = columns
        .iter()
        .zip(row)
        .map(|(c, cell)| {
            let text = md_cell(&cell.display);
            match (bold, c.kind) {
                (true, _) if !text.is_empty() => format!("**{text}**"),
                (_, Kind::Id) if cell.value.is_string() => {
                    format!("`{}`", cell.display.replace('`', "'"))
                }
                _ => text,
            }
        })
        .collect();
    format!("| {} |\n", cells.join(" | "))
}

/// Prose from a report's own blocks, where an agent's query can appear: HTML
/// is shown as text, never rendered by whoever previews the Markdown.
fn md_text(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// One Markdown table cell: [`md_text`], plus the pipe that would end the
/// cell, the backtick that would open a code span, backslashes first so the
/// escapes stay escapes, and a line break folded so the row stays one row.
pub(super) fn md_cell(v: &str) -> String {
    md_text(&v.replace('\\', "\\\\"))
        .replace('|', "\\|")
        .replace('`', "\\`")
        .replace("\r\n", " ")
        .replace(['\n', '\r'], " ")
}

// ------------------------------------------------------------------ csv

fn table_csv(table: &Table) -> String {
    let mut out = table
        .columns
        .iter()
        .map(|c| c.key)
        .collect::<Vec<_>>()
        .join(",");
    out.push('\n');
    for row in &table.rows {
        out.push_str(
            &row.iter()
                .map(|c| value_cell(&c.value))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

fn value_cell(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => csv_cell(s),
        other => other.to_string(),
    }
}

/// `Net tokens not read` → `net_tokens_not_read`.
fn snake(label: &str) -> String {
    let mut out = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
        }
    }
    out.trim_end_matches('_').to_string()
}

/// One CSV cell. A cell a spreadsheet would read as a formula — one starting
/// with `=`, `+`, `-`, `@`, a tab or a carriage return — gets a leading `'`, so
/// an agent's query of `=HYPERLINK(...)` opens as the text it was (audit S16).
/// A plain number keeps its sign.
pub(super) fn csv_cell(v: &str) -> String {
    let formula =
        v.len() > 1 && v.starts_with(['=', '+', '-', '@', '\t', '\r']) && v.parse::<f64>().is_err();
    let v = if formula {
        format!("'{v}")
    } else {
        v.to_string()
    };
    if v.contains(['"', ',', '\n', '\r']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

// ----------------------------------------------------------------- html

/// An unbroken run too long for any column, marked so print may break it
/// anywhere. Only the long runs: `overflow-wrap:anywhere` on a whole cell
/// lowers the width the table lays the column out at to one character, and
/// a stamp or a tool name then broke mid-word to make room. Printed, a
/// 300-character query in one table cell pushed HITS, EXCERPT TOKENS and
/// WHOLE-FILE past the page (0.42.0); on screen the table still scrolls.
fn breakable(escaped: &str) -> String {
    const LONG: usize = 24;
    let mut out = String::with_capacity(escaped.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        if word.chars().count() > LONG {
            out.push_str(&format!("<span class=\"brk\">{word}</span>"));
        } else {
            out.push_str(word);
        }
        word.clear();
    };
    for c in escaped.chars() {
        if c.is_whitespace() {
            flush(&mut word, &mut out);
            out.push(c);
        } else {
            word.push(c);
        }
    }
    flush(&mut word, &mut out);
    out
}

fn esc(v: &str) -> String {
    v.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Escaped prose with `code` spans drawn in mono.
fn inline(v: &str) -> String {
    let mut out = String::new();
    for (i, part) in v.split('`').enumerate() {
        if i % 2 == 1 {
            out.push_str(&format!("<span class=\"id\">{}</span>", esc(part)));
        } else {
            out.push_str(&esc(part));
        }
    }
    out
}

fn pill(text: &str, tone: &str) -> String {
    let dot = if matches!(tone, "green" | "amber") {
        "<span class=\"dot\"></span>"
    } else {
        ""
    };
    format!(
        "<span class=\"pill{}{tone}\">{dot}{}</span>",
        if tone.is_empty() { "" } else { " " },
        esc(text)
    )
}

fn card_head(title: &str, meta: &str) -> String {
    format!(
        "<section class=\"card\"><div class=\"card-h\"><h2 class=\"card-t\">{}</h2><span class=\"grow\"></span><span class=\"card-meta\">{}</span></div>",
        esc(title),
        inline(meta)
    )
}

fn html_table(table: &Table, bars: &mut std::collections::BTreeSet<u32>) -> String {
    let mut out = card_head(&table.title, &table.meta);
    if table.rows.is_empty() {
        out.push_str(&format!(
            "<div class=\"card-note solo\">{}</div></section>\n",
            esc(&table.empty)
        ));
        return out;
    }
    out.push_str("<div class=\"tw\"><table><thead><tr>");
    for column in &table.columns {
        out.push_str(&format!(
            "<th{}>{}</th>",
            if column.kind == Kind::Num {
                " class=\"r\""
            } else {
                ""
            },
            esc(&column.label)
        ));
    }
    out.push_str("</tr></thead><tbody>");
    let first_bar = std::cell::Cell::new(true);
    let row = |cells: &[Cell], total: bool, bars: &mut std::collections::BTreeSet<u32>| {
        let mut tr = String::from(if total {
            "<tr class=\"total\">"
        } else {
            "<tr>"
        });
        for (column, cell) in table.columns.iter().zip(cells) {
            let text = esc(&cell.display);
            let text = match column.kind {
                Kind::Num | Kind::Bar => text,
                _ => breakable(&text),
            };
            tr.push_str(&match column.kind {
                Kind::Num => format!("<td class=\"r n\">{text}</td>"),
                Kind::Id => format!("<td class=\"id\">{text}</td>"),
                Kind::Strong => format!("<td class=\"strong\">{text}</td>"),
                Kind::Muted => format!("<td class=\"muted\">{text}</td>"),
                Kind::Text => format!("<td>{text}</td>"),
                Kind::Bar if cell.value.is_null() => {
                    format!("<td class=\"n bar-cell\"><span class=\"bar-v\">{text}</span></td>")
                }
                Kind::Bar => {
                    let share = cell.value.as_f64().unwrap_or(0.0).clamp(0.0, 1.0);
                    let width = (share * 100.0).round() as u32;
                    bars.insert(width);
                    let lead = first_bar.replace(false);
                    format!(
                        "<td class=\"n bar-cell\"><span class=\"bar{}\"><i class=\"w{width}\"></i></span><span class=\"bar-v\">{text}</span></td>",
                        if lead { " acc" } else { "" }
                    )
                }
            });
        }
        tr.push_str("</tr>");
        tr
    };
    for cells in &table.rows {
        out.push_str(&row(cells, false, bars));
    }
    if let Some(total) = &table.total {
        out.push_str(&row(total, true, bars));
    }
    out.push_str("</tbody></table></div>");
    if table.more > 0 {
        out.push_str(&format!(
            "<div class=\"card-note\">{} more not listed.</div>",
            num::int(table.more as i64)
        ));
    }
    if let Some(note) = &table.note {
        out.push_str(&format!("<div class=\"card-note\">{}</div>", inline(note)));
    }
    out.push_str("</section>\n");
    out
}

fn data_uri(mime: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    )
}

/// The portal's stylesheet: the one design source the export's colours are
/// read from.
const PORTAL_CSS: &str = include_str!("../portal/style.css");

/// The tokens the reports use, by name (design/reports-v1 §1).
const TOKENS: [&str; 26] = [
    "bg",
    "panel",
    "soft",
    "line",
    "line-soft",
    "ink",
    "ink-2",
    "muted",
    "accent",
    "accent-edge",
    "blue",
    "blue-soft",
    "blue-ink",
    "green",
    "green-soft",
    "green-ink",
    "green-edge",
    "amber-soft",
    "amber-ink",
    "amber-edge",
    "red",
    "red-soft",
    "chip",
    "code-bg",
    "code-ink",
    "code-dim",
];

/// The portal's light tokens, which the PDF's print palette starts from.
pub(super) fn light_tokens() -> String {
    tokens(":root")
}

/// The token declarations of one `selector {` block of the portal stylesheet.
fn tokens(selector: &str) -> String {
    let Some(start) = PORTAL_CSS.find(&format!("{selector} {{")) else {
        return String::new();
    };
    let block = &PORTAL_CSS[start..];
    let block = &block[..block.find('}').unwrap_or(block.len())];
    let mut out = String::new();
    for line in block.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("--") else {
            continue;
        };
        let Some((name, value)) = rest.split_once(':') else {
            continue;
        };
        if TOKENS.contains(&name.trim()) {
            out.push_str(&format!(
                "--{}:{};",
                name.trim(),
                value.trim().trim_end_matches(';')
            ));
        }
    }
    out
}

/// The export stylesheet: the portal's tokens (light, and dark by
/// `prefers-color-scheme`), IBM Plex as the portal ships it, and the
/// reports-v1 components. Generated per render from `style.css`, so a colour
/// changed in the portal changes in every report.
fn stylesheet(bars: &std::collections::BTreeSet<u32>) -> String {
    let face = |family: &str, file: &[u8], weight: u32| {
        format!(
            "@font-face{{font-family:\"{family}\";src:url({}) format(\"woff2\");font-weight:{weight};font-display:swap}}\n",
            data_uri("font/woff2", file)
        )
    };
    let mut css = String::new();
    css.push_str(&face(
        "IBM Plex Sans",
        include_bytes!("../portal/fonts/IBMPlexSans-Regular.woff2"),
        400,
    ));
    css.push_str(&face(
        "IBM Plex Sans",
        include_bytes!("../portal/fonts/IBMPlexSans-Medium.woff2"),
        500,
    ));
    css.push_str(&face(
        "IBM Plex Sans",
        include_bytes!("../portal/fonts/IBMPlexSans-SemiBold.woff2"),
        600,
    ));
    css.push_str(&face(
        "IBM Plex Mono",
        include_bytes!("../portal/fonts/IBMPlexMono-Regular.woff2"),
        400,
    ));
    css.push_str(&face(
        "IBM Plex Mono",
        include_bytes!("../portal/fonts/IBMPlexMono-Medium.woff2"),
        500,
    ));
    css.push_str(&format!(
        ":root{{{}--sans:\"IBM Plex Sans\",\"Helvetica Neue\",Arial,sans-serif;--mono:\"IBM Plex Mono\",ui-monospace,SFMono-Regular,Menlo,monospace;color-scheme:light}}\n",
        tokens(":root")
    ));
    css.push_str(&format!(
        "@media (prefers-color-scheme:dark){{:root{{{}color-scheme:dark}}.top img.logo-l{{display:none}}.top img.logo-d{{display:block}}}}\n",
        tokens(":root[data-theme=\"dark\"]")
    ));
    css.push_str(COMPONENTS);
    for width in bars {
        css.push_str(&format!(".w{width}{{width:{width}%}}"));
    }
    css.push('\n');
    css
}

/// The reports-v1 components (design/reports-v1/README.md §2, §3).
const COMPONENTS: &str = r#"*{box-sizing:border-box}
html{background:var(--bg)}
body{margin:0;font:13.5px/1.5 var(--sans);color:var(--ink);background:var(--bg);-webkit-font-smoothing:antialiased;font-variant-numeric:tabular-nums}
.sheet{max-width:980px;margin:32px auto;padding:0 20px}
.top{display:flex;align-items:center;gap:10px;padding:0 0 14px;border-bottom:1px solid var(--line)}
.top img{width:24px;height:24px;border-radius:5px}
@media not (prefers-color-scheme:dark){.top img.logo-d{display:none}}
.word{font-weight:600;font-size:15px}
.vr{width:1px;height:16px;background:var(--line)}
.kind{color:var(--ink-2)}
.grow{flex:1}
.pill{display:inline-flex;align-items:center;gap:6px;height:22px;padding:0 9px;border-radius:11px;font:11.5px var(--mono);white-space:nowrap;border:1px solid var(--line);background:var(--soft);color:var(--ink-2)}
.pill.green{border-color:var(--green-edge);background:var(--green-soft);color:var(--green-ink)}
.pill.amber{border-color:var(--amber-edge);background:var(--amber-soft);color:var(--amber-ink)}
.pill.blue{border-color:var(--blue);background:var(--blue-soft);color:var(--blue-ink)}
.dot{width:6px;height:6px;border-radius:3px;background:currentColor}
.eyebrow{font:500 10.5px var(--mono);letter-spacing:.08em;text-transform:uppercase;color:var(--muted)}
.title{padding:22px 0 6px}
h1{font-size:26px;line-height:1.2;font-weight:600;margin:6px 0 8px;letter-spacing:-.01em}
.lede{font-size:15px;color:var(--ink-2);max-width:68ch;margin:0}
.lede b{color:var(--ink);font-weight:600}
.meta{display:grid;grid-template-columns:repeat(auto-fit,minmax(200px,1fr));margin:18px 0 20px;background:var(--panel);border:1px solid var(--line);border-radius:8px;overflow:hidden}
.meta>div{padding:10px 14px;border-right:1px solid var(--line-soft);min-width:0}
.meta>div:last-child{border-right:0}
.meta dd{margin:3px 0 0;font:12.5px var(--mono);color:var(--ink);overflow-wrap:anywhere}
.meta .line{display:block}
.chips{display:flex;flex-wrap:wrap;gap:5px;margin:4px 0 3px}
.muted{color:var(--muted)}
.kpis{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px;margin-bottom:16px}
.kpis.k3{grid-template-columns:repeat(3,minmax(0,1fr))}
.kpi{background:var(--panel);border:1px solid var(--line);border-radius:8px;padding:12px 14px;display:flex;flex-direction:column;gap:4px;min-width:0}
.kpi.lead{border:1.5px solid var(--accent)}
.kpi .v{font:600 22px/1.2 var(--mono);white-space:nowrap;overflow:hidden;text-overflow:ellipsis}
.kpi .s{color:var(--muted);font-size:12px}
.card{background:var(--panel);border:1px solid var(--line);border-radius:8px;overflow:hidden;margin-bottom:16px}
.card-h{display:flex;align-items:center;gap:8px;padding:10px 14px;border-bottom:1px solid var(--line-soft);flex-wrap:wrap;min-height:41px}
.card-t{font-weight:600;font-size:13.5px;margin:0}
.card-meta{font:11.5px var(--mono);color:var(--muted)}
.card-note{padding:10px 14px;background:var(--soft);color:var(--ink-2);font-size:12.5px;border-top:1px solid var(--line-soft)}
.card-note.solo{border-top:0}
.tw{overflow-x:auto}
table{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}
th{font:500 10.5px var(--mono);letter-spacing:.08em;text-transform:uppercase;color:var(--muted);text-align:left;padding:8px 14px;white-space:nowrap;background:var(--soft);border-bottom:1px solid var(--line-soft)}
td{padding:9px 14px;border-top:1px solid var(--line-soft);vertical-align:middle}
tbody tr:first-child td{border-top:0}
.r{text-align:right}
.n{font-family:var(--mono);white-space:nowrap}
.id{font:12px var(--mono);white-space:nowrap;color:var(--ink-2)}
.strong{font-weight:600}
tr.total td{background:var(--soft);font-weight:600;border-top:1px solid var(--line)}
.bar-cell{white-space:nowrap}
.bar{display:inline-block;vertical-align:middle;height:6px;border-radius:3px;background:var(--chip);width:120px;overflow:hidden;margin-right:10px}
.bar i{display:block;height:100%;background:var(--blue)}
.bar.acc i{background:var(--accent)}
.bar-v{color:var(--ink-2)}
.facts{display:grid;grid-template-columns:repeat(3,minmax(0,1fr))}
.facts>div{padding:12px 14px;border-top:1px solid var(--line-soft);border-right:1px solid var(--line-soft);min-width:0}
.facts>div:nth-child(3n){border-right:0}
.facts>div:nth-child(-n+3){border-top:0}
.facts .v{font:600 17px var(--mono);margin-top:3px;overflow-wrap:anywhere}
.facts .s{color:var(--muted);font-size:12px}
pre{margin:0;padding:12px 14px;background:var(--code-bg);color:var(--code-ink);font:12px/1.6 var(--mono);overflow-x:auto}
pre .c{color:var(--code-dim)}
.foot{display:flex;gap:6px 12px;flex-wrap:wrap;align-items:center;margin:22px 0 40px;padding-top:12px;border-top:1px solid var(--line);font:11.5px var(--mono);color:var(--muted)}
.foot .hash{overflow-wrap:anywhere}
@media (max-width:720px){.sheet{margin:16px auto;padding:0 16px}.kpis,.kpis.k3{grid-template-columns:repeat(2,minmax(0,1fr))}
.facts{grid-template-columns:repeat(2,minmax(0,1fr))}.facts>div:nth-child(3n){border-right:1px solid var(--line-soft)}
.facts>div:nth-child(2n){border-right:0}.facts>div:nth-child(3){border-top:1px solid var(--line-soft)}
.meta{grid-template-columns:1fr}.meta>div{border-right:0;border-bottom:1px solid var(--line-soft)}.meta>div:last-child{border-bottom:0}
h1{font-size:22px}.top .grow+.pill{display:none}.kpi .v{font-size:19px}}
@media print{:root{--bg:#fff;--panel:#fff;--soft:#f6f8fa;--line:#cfd8df;--line-soft:#e3e9ee;--ink:#1e2a35;--ink-2:#3a4d5e;--muted:#5d7083;--chip:#eef2f5;
--accent:#f0a43c;--blue:#4c7088;--green-soft:#eaf4ef;--green-ink:#1f5c3e;--green-edge:#8cc5a6;--amber-soft:#fbebd2;--amber-ink:#7a4e0a;--amber-edge:#d9a75a;
--code-bg:#f3f6f8;--code-ink:#1e2a35;--code-dim:#5d7083;color-scheme:light}
.top img.logo-l{display:block!important}.top img.logo-d{display:none!important}
@page{size:A4;margin:16mm 14mm 18mm}
body{font-size:10.5pt;-webkit-print-color-adjust:exact;print-color-adjust:exact}
.sheet{max-width:none;margin:0;padding:0}.tw{overflow:visible}
thead{display:table-header-group}tr,.kpi,.card-h,.facts>div{break-inside:avoid}.card-h{break-after:avoid}
th{white-space:normal}th,td{padding:6px 8px}.brk{overflow-wrap:anywhere}pre{white-space:pre-wrap;overflow-wrap:anywhere}table{font-size:9.5pt}.id{white-space:normal;font-size:8.5pt}.card{overflow:visible}
.kpis{grid-template-columns:repeat(4,minmax(0,1fr))}.kpis.k3{grid-template-columns:repeat(3,minmax(0,1fr))}.meta{grid-template-columns:repeat(3,minmax(0,1fr))}}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_formula_cell_exports_as_text() {
        assert_eq!(
            csv_cell(r#"=HYPERLINK("u","x")"#),
            r#""'=HYPERLINK(""u"",""x"")""#
        );
        assert_eq!(csv_cell("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(csv_cell("\tcmd"), "'\tcmd");
        assert_eq!(csv_cell("-12"), "-12");
        assert_eq!(csv_cell("-"), "-");
        assert_eq!(csv_cell("plain"), "plain");
    }

    #[test]
    fn only_a_long_unbroken_run_may_break_anywhere_in_print() {
        assert_eq!(breakable("2026-10-17 12:17:45"), "2026-10-17 12:17:45");
        let long = "w000|w001|w002|w003|w004|w005";
        assert_eq!(
            breakable(&format!("a {long} b")),
            format!("a <span class=\"brk\">{long}</span> b")
        );
        assert!(COMPONENTS.contains(".brk{overflow-wrap:anywhere}"));
    }

    #[test]
    fn a_markdown_cell_cannot_end_the_cell_or_render_html() {
        assert_eq!(
            md_cell("<img src=x> a|b `c`\nd"),
            "&lt;img src=x&gt; a\\|b \\`c\\` d"
        );
        assert_eq!(md_cell(r"a\|b"), r"a\\\|b");
    }

    #[test]
    fn the_stylesheet_is_the_portals_tokens() {
        let light = tokens(":root");
        let dark = tokens(":root[data-theme=\"dark\"]");
        assert!(light.contains("--muted:#64778a;"), "{light}");
        assert!(light.contains("--code-dim:"), "{light}");
        assert!(dark.contains("--panel:#161f27;"), "{dark}");
        for token in TOKENS {
            assert!(
                light.contains(&format!("--{token}:")),
                "light lacks {token}"
            );
        }
        assert!(snake("Net tokens not read") == "net_tokens_not_read");
    }
}
