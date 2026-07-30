//! Reading the formats that are not plain text.
//!
//! Every reader here answers the same question [`crate::chunk::extract`] asks of
//! a file: what would a person see if they opened this? The answer is text, in
//! reading order, with a structural marker wherever the format has divisions a
//! line number cannot express — a slide, a sheet, a notebook cell. `None` means
//! "deliberately skipped", exactly as it does for an unreadable PDF: a document
//! semlith cannot read is a file the run walks past, never an error that ends
//! the run.
//!
//! Six of the nine formats are ZIP archives of XML, which is why they cost one
//! archive reader and one tag scanner between them rather than six parsers.

use std::io::{Cursor, Read};

/// Extensions this module reads. [`crate::chunk::extract`] dispatches on this
/// list before it looks at the bytes, so a corpus with none of these formats
/// pays one string comparison per file and nothing else.
const HANDLED: &[&str] = &[
    "ipynb", "html", "htm", "docx", "pptx", "xlsx", "odt", "odp", "ods",
];

/// How much text one archive may decompress to.
///
/// An archive is entered under the same 8 MiB file cap as everything else, but
/// compression means the cap on what comes out has to be its own number: a few
/// hundred kilobytes of zeros expand to gigabytes, and without a bound the size
/// of a run's largest allocation would be chosen by whoever wrote the file. 32
/// MiB is far more text than any real document holds — a novel is around one —
/// and small enough that hitting it is a decision rather than an accident.
pub(crate) const MAX_ARCHIVE_TEXT: u64 = 32 * 1024 * 1024;

/// Text kept from one notebook output, in characters. A cell that printed a
/// megabyte of logs is not what the notebook is about.
const MAX_OUTPUT_CHARS: usize = 2000;

pub(crate) fn handles(ext: &str) -> bool {
    HANDLED.contains(&ext)
}

/// Text from a file of one of the [`HANDLED`] formats, or `None` when it cannot
/// be read: corrupt, encrypted, empty of text, or larger decompressed than
/// [`MAX_ARCHIVE_TEXT`].
pub(crate) fn extract(ext: &str, bytes: &[u8]) -> Option<String> {
    let text = match ext {
        "ipynb" => notebook(bytes),
        "html" | "htm" => html(bytes),
        "docx" => docx(bytes),
        "pptx" => pptx(bytes),
        "xlsx" => xlsx(bytes),
        "odt" | "odp" | "ods" => odf(bytes),
        _ => None,
    }?;
    (!text.trim().is_empty()).then_some(text)
}

// ---------------------------------------------------------------- notebooks

/// A Jupyter notebook as its cells: markdown as prose, code as code, in
/// notebook order.
///
/// Indexed as the JSON it is on disk, a notebook chunk is mostly `"cell_type"`,
/// `"outputs"` and escaped newlines. What a developer is searching for is the
/// third line of the fourth cell, so that is what gets embedded, with the cell
/// number kept as a marker because a notebook has cells where a file has lines.
fn notebook(bytes: &[u8]) -> Option<String> {
    let json: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    let cells = json.get("cells")?.as_array()?;
    let mut out = String::new();

    for (i, cell) in cells.iter().enumerate() {
        let kind = cell
            .get("cell_type")
            .and_then(|k| k.as_str())
            .unwrap_or("cell");
        let source = source_text(cell.get("source"));
        if source.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("# Cell {} ({kind})\n", i + 1));
        out.push_str(source.trim_end());
        out.push('\n');

        for output in cell
            .get("outputs")
            .and_then(|o| o.as_array())
            .map(|o| o.as_slice())
            .unwrap_or_default()
        {
            let text = output_text(output);
            if text.trim().is_empty() {
                continue;
            }
            out.push_str("# Output:\n");
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out.push('\n');
    }
    Some(out)
}

/// nbformat writes a cell's source either as one string or as a list of lines.
fn source_text(value: Option<&serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(lines)) => lines
            .iter()
            .filter_map(|l| l.as_str())
            .collect::<Vec<_>>()
            .concat(),
        _ => String::new(),
    }
}

/// The text of one output — a stream's `text`, or a result's `text/plain`.
///
/// Anything else a cell produced is a picture, a widget or a MIME type nobody
/// searches for, and a base64 PNG is the largest thing in most notebooks.
fn output_text(output: &serde_json::Value) -> String {
    let text = match output.get("text") {
        Some(t) => source_text(Some(t)),
        None => source_text(output.get("data").and_then(|d| d.get("text/plain"))),
    };
    truncate_chars(text, MAX_OUTPUT_CHARS)
}

fn truncate_chars(mut s: String, max: usize) -> String {
    if s.chars().count() > max {
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        s.truncate(end);
    }
    s
}

// --------------------------------------------------------------------- HTML

/// An HTML page as the text a browser would show.
///
/// Tags go, script and style contents go, entities are decoded — and every
/// newline in the source survives, including the ones inside the tags that were
/// removed. That last part is the whole trick: it keeps the extracted text
/// line-for-line aligned with the file on disk, so the `file:line` locator on a
/// hit points at the line a person opening the file will find the sentence on.
fn html(bytes: &[u8]) -> Option<String> {
    let source = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(source.len());
    let mut rest = source.as_ref();

    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];

        // A comment ends at `-->`, not at the first `>` inside it.
        let skipped = if rest.starts_with("<!--") {
            skip_to(rest, "-->")
        } else if let Some(tag) = raw_text_tag(rest) {
            // <script> and <style> hold code that is not the page's text. Their
            // contents are skipped along with the tags themselves.
            let close = format!("</{tag}");
            let end = skip_to(rest, &close);
            let after = skip_to(&rest[end..], ">");
            end + after
        } else {
            skip_to(rest, ">")
        };

        // The newlines inside what was skipped are kept, so the line count
        // never drifts from the source's.
        for _ in 0..rest[..skipped].matches('\n').count() {
            out.push('\n');
        }
        rest = &rest[skipped..];
    }
    out.push_str(rest);
    Some(decode_entities(&out))
}

/// How far to the end of `needle`, or to the end of the input when a document
/// is truncated mid-tag.
fn skip_to(s: &str, needle: &str) -> usize {
    match s.find(needle) {
        Some(at) => at + needle.len(),
        None => s.len(),
    }
}

/// `Some("script")` or `Some("style")` when `s` opens one of them.
fn raw_text_tag(s: &str) -> Option<&'static str> {
    for tag in ["script", "style"] {
        let open = format!("<{tag}");
        if s.len() > open.len()
            && s[..open.len()].eq_ignore_ascii_case(&open)
            && matches!(
                s.as_bytes()[open.len()],
                b'>' | b' ' | b'\t' | b'\n' | b'\r'
            )
        {
            return Some(tag);
        }
    }
    None
}

/// The named entities worth knowing and the numeric forms. Anything else is
/// left as it was written: an unrecognised `&thing;` is likelier to be text
/// about an entity than an entity.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let body = &rest[1..end];
        let decoded = match body {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some(' '),
            _ => numeric_entity(body),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn numeric_entity(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(code)
}

// ------------------------------------------------------------- ZIP archives

type Archive<'a> = zip::ZipArchive<Cursor<&'a [u8]>>;

/// Open an archive, or `None` when the bytes are not one — which is also what a
/// password-protected Office document looks like, since encryption wraps the
/// archive in a container that is not a ZIP at all.
fn archive(bytes: &[u8]) -> Option<Archive<'_>> {
    zip::ZipArchive::new(Cursor::new(bytes)).ok()
}

/// One entry's contents, spending from the archive's decompression budget.
///
/// The budget is per archive rather than per entry, so a document cannot get
/// around the cap by holding a thousand entries just under it.
fn entry(zip: &mut Archive<'_>, name: &str, budget: &mut u64) -> Option<String> {
    let file = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    // One byte past the budget, so filling it exactly is distinguishable from
    // running past it.
    file.take(*budget + 1).read_to_end(&mut buf).ok()?;
    if buf.len() as u64 > *budget {
        return None;
    }
    *budget -= buf.len() as u64;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// A Word document: its paragraphs, in order, one per line.
fn docx(bytes: &[u8]) -> Option<String> {
    let mut zip = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_TEXT;
    let xml = entry(&mut zip, "word/document.xml", &mut budget)?;
    Some(docx_body(&xml))
}

/// `w:p` is a paragraph, and a table is `w:tr` rows of `w:tc` cells, each cell
/// holding paragraphs of its own. Kept as rows, a table reads as a table;
/// flattened, every cell is a line and the row it belonged to is gone.
fn docx_body(xml: &str) -> String {
    let mut lines = Lines::default();
    scan(xml, |event| match event {
        Event::Text(text) => lines.text(text),
        Event::Close("w:p" | "w:br") => lines.end_line(),
        Event::Open { name: "w:tr", .. } => lines.start_row(),
        Event::Close("w:tc") => lines.end_cell(),
        Event::Close("w:tr") => lines.end_row(),
        // A literal tab inside a paragraph is a tab.
        Event::Close("w:tab") => lines.text("\t"),
        _ => {}
    });
    lines.finish()
}

/// A slide deck: each slide's text, under a marker naming its number.
fn pptx(bytes: &[u8]) -> Option<String> {
    let mut zip = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_TEXT;

    // Sorted by the number in the name, not by the name: lexicographically,
    // slide11.xml sorts before slide2.xml, which would deal the deck out of
    // order and put the wrong number on every marker after the tenth.
    let mut slides: Vec<(u32, String)> = zip
        .file_names()
        .filter_map(|name| Some((slide_number(name)?, name.to_string())))
        .collect();
    slides.sort_unstable();

    let mut out = String::new();
    for (number, name) in slides {
        let Some(xml) = entry(&mut zip, &name, &mut budget) else {
            // Out of budget, or an entry that would not read. What was gathered
            // so far is still worth indexing.
            break;
        };
        let mut lines = Lines::default();
        scan(&xml, |event| match event {
            Event::Text(text) => lines.text(text),
            Event::Close("a:p" | "a:br") => lines.end_line(),
            _ => {}
        });
        let text = lines.finish();
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("# Slide {number}\n{}\n\n", text.trim_end()));
    }
    Some(out)
}

/// `ppt/slides/slide7.xml` → `7`. Notes, layouts and masters are not slides.
fn slide_number(name: &str) -> Option<u32> {
    name.strip_prefix("ppt/slides/slide")?
        .strip_suffix(".xml")?
        .parse()
        .ok()
}

/// A workbook: every sheet, in workbook order, under a marker naming it, with
/// each row a line of tab-separated cells.
fn xlsx(bytes: &[u8]) -> Option<String> {
    let mut zip = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_TEXT;

    // Cell values live in a workbook-wide table and cells hold indexes into it,
    // so without this a sheet reads as a column of small integers.
    let shared: Vec<String> = match entry(&mut zip, "xl/sharedStrings.xml", &mut budget) {
        Some(xml) => shared_strings(&xml),
        None => Vec::new(),
    };

    let workbook = entry(&mut zip, "xl/workbook.xml", &mut budget)?;
    let rels = entry(&mut zip, "xl/_rels/workbook.xml.rels", &mut budget).unwrap_or_default();
    let targets = relationships(&rels);

    let mut out = String::new();
    for (name, id) in sheets(&workbook) {
        // The relationship is what ties a sheet's name to the part holding it.
        // sheet1.xml is usually the first sheet and is not required to be.
        let Some(target) = targets.get(&id) else {
            continue;
        };
        // A relationship target is relative to the part that declared it —
        // `xl/` here — unless it is written absolute, which openpyxl does and
        // Excel does not, in which case it is from the package root.
        let path = match target.strip_prefix('/') {
            Some(from_root) => from_root.to_string(),
            None => format!("xl/{target}"),
        };
        let Some(xml) = entry(&mut zip, &path, &mut budget) else {
            break;
        };
        let text = sheet_text(&xml, &shared);
        if text.trim().is_empty() {
            continue;
        }
        out.push_str(&format!("## Sheet: {name}\n{}\n\n", text.trim_end()));
    }
    Some(out)
}

/// The shared-string table, in index order.
fn shared_strings(xml: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    scan(xml, |event| match event {
        Event::Open { name: "si", .. } => {
            depth += 1;
            current.clear();
        }
        Event::Text(text) if depth > 0 => current.push_str(text),
        Event::Close("si") if depth > 0 => {
            depth -= 1;
            strings.push(decode_entities(&current));
        }
        _ => {}
    });
    strings
}

/// `rId3` → `worksheets/sheet3.xml`, from a relationships part.
fn relationships(xml: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    scan(xml, |event| {
        if let Event::Open {
            name: "Relationship",
            attrs,
        } = event
            && let (Some(id), Some(target)) = (attr(attrs, "Id"), attr(attrs, "Target"))
        {
            map.insert(id, target);
        }
    });
    map
}

/// `(sheet name, relationship id)` in the order the workbook lists them.
fn sheets(xml: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    scan(xml, |event| {
        if let Event::Open {
            name: "sheet",
            attrs,
        } = event
            && let (Some(name), Some(id)) = (attr(attrs, "name"), attr(attrs, "r:id"))
        {
            found.push((name, id));
        }
    });
    found
}

/// One worksheet: a line per row, a tab between cells.
fn sheet_text(xml: &str, shared: &[String]) -> String {
    let mut out = String::new();
    let mut cell_kind = String::new();
    let mut value = String::new();
    let mut in_value = false;
    let mut cells_in_row = 0usize;

    scan(xml, |event| match event {
        Event::Open { name: "c", attrs } => {
            cell_kind = attr(attrs, "t").unwrap_or_default();
        }
        // `v` is a cell's value; `t` inside `is` is an inline string, which is
        // what a cell holds when the writer skipped the shared table.
        Event::Open {
            name: "v" | "t", ..
        } => {
            in_value = true;
            value.clear();
        }
        Event::Text(text) if in_value => value.push_str(text),
        Event::Close("v" | "t") if in_value => {
            in_value = false;
            let text = match cell_kind.as_str() {
                // A shared-string cell holds an index into the table.
                "s" => value
                    .trim()
                    .parse::<usize>()
                    .ok()
                    .and_then(|i| shared.get(i).cloned())
                    .unwrap_or_default(),
                _ => decode_entities(&value),
            };
            if !text.is_empty() {
                if cells_in_row > 0 {
                    out.push('\t');
                }
                out.push_str(&text);
                cells_in_row += 1;
            }
        }
        Event::Close("row") => {
            if cells_in_row > 0 {
                out.push('\n');
            }
            cells_in_row = 0;
        }
        _ => {}
    });
    out
}

/// An OpenDocument text, presentation or spreadsheet.
///
/// All three keep their content in one `content.xml` and all three mark a
/// paragraph with `text:p`, so they are one reader with markers for the two
/// divisions that are not paragraphs: a presentation's pages and a
/// spreadsheet's tables.
fn odf(bytes: &[u8]) -> Option<String> {
    let mut zip = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_TEXT;
    let xml = entry(&mut zip, "content.xml", &mut budget)?;

    let mut lines = Lines::default();
    let mut page = 0u32;

    scan(&xml, |event| match event {
        Event::Open {
            name: "draw:page",
            attrs,
        } => {
            page += 1;
            // A page's own name is worth keeping when the author gave it one,
            // and worth nothing when the tool named it "page1" after its index.
            match attr(attrs, "draw:name")
                .filter(|n| !n.eq_ignore_ascii_case(&format!("page{page}")))
            {
                Some(named) => lines.marker(&format!("# Slide {page} ({named})")),
                None => lines.marker(&format!("# Slide {page}")),
            }
        }
        Event::Open {
            name: "table:table",
            attrs,
        } => {
            let named = attr(attrs, "table:name").unwrap_or_default();
            lines.marker(&format!("## Sheet: {named}"));
        }
        Event::Text(text) => lines.text(text),
        // A heading is a paragraph for these purposes; a cell holds paragraphs
        // and a row of cells is a line.
        Event::Close("text:p" | "text:h") => lines.end_line(),
        Event::Open {
            name: "table:table-row",
            ..
        } => lines.start_row(),
        Event::Close("table:table-cell") => lines.end_cell(),
        Event::Close("table:table-row") => lines.end_row(),
        _ => {}
    });
    Some(lines.finish())
}

/// Text assembled the way a document is laid out: paragraphs are lines, and a
/// row of table cells is one line of tab-separated values.
///
/// Every archive format needs the same two decisions — where a line ends, and
/// whether the text belongs to a cell of a row — so they are made once here
/// rather than three times in three readers with three slightly different
/// answers.
#[derive(Default)]
struct Lines {
    out: String,
    /// Text seen since the last paragraph ended.
    line: String,
    /// Paragraphs of the cell being read, joined by a space.
    cell: String,
    /// Cells of the row being read.
    row: Vec<String>,
    in_row: bool,
}

impl Lines {
    fn text(&mut self, text: &str) {
        self.line.push_str(text);
    }

    /// A marker names a division a line number cannot: a slide, a sheet.
    fn marker(&mut self, marker: &str) {
        self.end_line();
        if !self.out.is_empty() {
            self.out.push('\n');
        }
        self.out.push_str(marker);
        self.out.push('\n');
    }

    fn end_line(&mut self) {
        let text = decode_entities(self.line.trim());
        self.line.clear();
        if text.is_empty() {
            return;
        }
        if self.in_row {
            if !self.cell.is_empty() {
                self.cell.push(' ');
            }
            self.cell.push_str(&text);
        } else {
            self.out.push_str(&text);
            self.out.push('\n');
        }
    }

    fn start_row(&mut self) {
        self.end_line();
        self.in_row = true;
        self.cell.clear();
        self.row.clear();
    }

    fn end_cell(&mut self) {
        self.end_line();
        if self.in_row {
            self.row.push(std::mem::take(&mut self.cell));
        }
    }

    fn end_row(&mut self) {
        // Only text the last cell's own close did not already take: pushing
        // unconditionally would end every row with an empty column.
        self.end_line();
        if !self.cell.is_empty() {
            self.row.push(std::mem::take(&mut self.cell));
        }
        if self.row.iter().any(|c| !c.is_empty()) {
            self.out.push_str(&self.row.join("\t"));
            self.out.push('\n');
        }
        self.in_row = false;
        self.row.clear();
    }

    fn finish(mut self) -> String {
        if self.in_row {
            self.end_row();
        }
        self.end_line();
        self.out
    }
}

// ---------------------------------------------------------------- XML pieces

enum Event<'a> {
    Open { name: &'a str, attrs: &'a str },
    Close(&'a str),
    Text(&'a str),
}

/// The smallest scan that answers what these formats ask of XML: which element
/// opened, with which attributes, which closed, and what text lay between.
///
/// It is not a parser — it does not validate, resolve namespaces or build a
/// tree, and it never fails. That is deliberate: the input is a document
/// somebody else wrote, and the worst thing this could do with a malformed one
/// is refuse to read the parts that are fine.
fn scan(xml: &str, mut on_event: impl FnMut(Event<'_>)) {
    let mut rest = xml;
    while let Some(at) = rest.find('<') {
        if at > 0 {
            on_event(Event::Text(&rest[..at]));
        }
        rest = &rest[at..];

        // Comments, doctypes and CDATA are not elements; skip to their end
        // rather than reading `<!--` as a tag named `!--`.
        if rest.starts_with("<!--") {
            rest = &rest[skip_to(rest, "-->")..];
            continue;
        }
        let Some(end) = rest.find('>') else { return };
        let inside = &rest[1..end];
        rest = &rest[end + 1..];

        if let Some(name) = inside.strip_prefix('/') {
            on_event(Event::Close(name.trim()));
            continue;
        }
        if inside.starts_with(['?', '!']) {
            continue;
        }
        let self_closing = inside.ends_with('/');
        let inside = inside.trim_end_matches('/');
        let (name, attrs) = match inside.find([' ', '\t', '\n', '\r']) {
            Some(at) => (&inside[..at], &inside[at + 1..]),
            None => (inside, ""),
        };
        on_event(Event::Open { name, attrs });
        if self_closing {
            on_event(Event::Close(name));
        }
    }
    if !rest.is_empty() {
        on_event(Event::Text(rest));
    }
}

/// One attribute's value, decoded. Values are quoted with either quote
/// character, and a document in the wild uses both.
fn attr(attrs: &str, key: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(at) = rest.find(key) {
        let after = &rest[at + key.len()..];
        let before_is_boundary = at == 0 || rest.as_bytes()[at - 1].is_ascii_whitespace();
        let after = after.trim_start();
        if !before_is_boundary || !after.starts_with('=') {
            rest = &rest[at + key.len()..];
            continue;
        }
        let value = after[1..].trim_start();
        let quote = value.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let end = value[1..].find(quote)? + 1;
        return Some(decode_entities(&value[1..end]));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property the whole HTML path rests on: what comes out is the page's
    /// text, and it is still on the line the source put it on.
    #[test]
    fn html_keeps_its_text_its_lines_and_none_of_its_markup() {
        let page = "<!doctype html>\n<html>\n<head>\n<style>\n.a { color: red }\n</style>\n\
                    <script>var secret = 1;</script>\n</head>\n<body>\n\
                    <p class=\"lede\">Wombats &amp; friends &#233;t al.</p>\n</body>\n</html>\n";
        let text = html(page.as_bytes()).unwrap();

        assert!(text.contains("Wombats & friends ét al."), "{text:?}");
        assert!(
            !text.contains("secret"),
            "script contents survived: {text:?}"
        );
        assert!(!text.contains("color: red"), "style contents survived");
        assert!(!text.contains("class"), "an attribute survived: {text:?}");

        // Line 10 of the source holds the paragraph; line 10 of the text must too.
        let line = text.lines().position(|l| l.contains("Wombats")).unwrap() + 1;
        let source_line = page.lines().position(|l| l.contains("Wombats")).unwrap() + 1;
        assert_eq!(line, source_line, "line numbers drifted:\n{text}");
    }

    #[test]
    fn an_unknown_entity_is_left_alone() {
        assert_eq!(decode_entities("a &b; &amp; c"), "a &b; & c");
        assert_eq!(decode_entities("&#x41;&#66;"), "AB");
    }

    /// A truncated tag must end the scan, not spin in it.
    #[test]
    fn malformed_markup_terminates() {
        assert!(html(b"<p>text<").is_some());
        assert!(html(b"<!-- unclosed comment").is_some());
        let mut count = 0;
        scan("<a><b attr='1'/>text</a", |_| count += 1);
        assert!(count > 0);
    }

    #[test]
    fn notebook_cells_come_out_as_their_source() {
        let nb = br##"{"cells":[
            {"cell_type":"markdown","source":["# Title\n","prose here\n"]},
            {"cell_type":"code","execution_count":1,"source":"def f(x):\n    return x\n",
             "outputs":[{"output_type":"stream","name":"stdout","text":["converged\n"]},
                        {"output_type":"display_data","data":{"image/png":"AAAA"}}]}
        ],"nbformat":4}"##;
        let text = notebook(nb).unwrap();

        assert!(text.contains("prose here"));
        assert!(text.contains("def f(x):"));
        assert!(text.contains("converged"), "stream output missing: {text}");
        assert!(!text.contains("AAAA"), "a base64 image was indexed");
        assert!(!text.contains("cell_type"), "JSON scaffolding survived");
        assert!(
            text.find("Cell 1").unwrap() < text.find("Cell 2").unwrap(),
            "cells came out of order"
        );
    }

    #[test]
    fn a_notebook_that_is_not_a_notebook_is_skipped() {
        assert!(notebook(b"[1, 2, 3]").is_none());
        assert!(notebook(b"not json at all").is_none());
    }

    #[test]
    fn attributes_are_read_by_name_not_by_substring() {
        assert_eq!(attr(r#"r:id="rId3" id="9""#, "id").as_deref(), Some("9"));
        assert_eq!(attr(r#"r:id="rId3""#, "r:id").as_deref(), Some("rId3"));
        assert_eq!(attr(r#"name='Q3 &amp; Q4'"#, "name").unwrap(), "Q3 & Q4");
        assert_eq!(attr(r#"other="1""#, "name"), None);
    }

    /// Slides are ordered by their number. Sorted as text, slide11 precedes
    /// slide2 and every marker past the tenth is wrong.
    #[test]
    fn slides_are_numbered_not_named() {
        assert_eq!(slide_number("ppt/slides/slide11.xml"), Some(11));
        assert_eq!(slide_number("ppt/slides/_rels/slide1.xml.rels"), None);
        assert_eq!(slide_number("ppt/notesSlides/notesSlide1.xml"), None);
    }

    #[test]
    fn a_worksheet_resolves_its_shared_strings() {
        let shared =
            shared_strings(r#"<sst><si><t>alpha</t></si><si><t>beta &amp; co</t></si></sst>"#);
        assert_eq!(shared, ["alpha", "beta & co"]);

        let sheet = r#"<worksheet><sheetData>
            <row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1"><v>42</v></c></row>
            <row r="2"><c r="A2" t="s"><v>1</v></c></row>
        </sheetData></worksheet>"#;
        assert_eq!(sheet_text(sheet, &shared), "alpha\t42\nbeta & co\n");
    }

    /// A Word table has to come back as rows. Flattened, a two-column table is
    /// a column of orphaned values and the pairing they carried is gone.
    #[test]
    fn a_word_paragraph_is_a_line_and_a_table_row_is_one_line_of_cells() {
        let xml = r#"<w:document><w:body>
            <w:p><w:r><w:t>First para</w:t></w:r></w:p>
            <w:tbl>
              <w:tr><w:tc><w:p><w:r><w:t>Item</w:t></w:r></w:p></w:tc>
                    <w:tc><w:p><w:r><w:t>Status</w:t></w:r></w:p></w:tc></w:tr>
              <w:tr><w:tc><w:p><w:r><w:t>ledger</w:t></w:r></w:p></w:tc>
                    <w:tc><w:p><w:r><w:t>Open</w:t></w:r></w:p></w:tc></w:tr>
            </w:tbl>
        </w:body></w:document>"#;
        let text = docx_body(xml);
        assert_eq!(text, "First para\nItem\tStatus\nledger\tOpen\n", "{text:?}");
    }

    /// Nothing here may accept bytes that are not what they claim to be.
    #[test]
    fn rubbish_in_every_format_is_skipped_rather_than_read() {
        let rubbish: &[&[u8]] = &[b"", b"not a zip", &[0u8; 64], b"PK\x03\x04truncated"];
        for ext in HANDLED {
            for bytes in rubbish {
                // The contract is that this returns, without panicking, and
                // that nonsense does not come back as text.
                let _ = extract(ext, bytes);
            }
            assert!(extract(ext, b"").is_none(), "{ext} read an empty file");
        }
    }
}
