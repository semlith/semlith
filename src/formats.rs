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
//! Seven of the thirteen formats are ZIP archives of XML, which is why they
//! cost one archive reader and one tag scanner between them rather than seven
//! parsers. EPUB is the seventh and the cheapest of all of them: a book is a
//! ZIP of XHTML, so it is the archive reader and the HTML reader already here,
//! joined by the spine order its own manifest states.

use std::io::{Cursor, Read};

/// Extensions this module reads. [`crate::chunk::extract`] dispatches on this
/// list before it looks at the bytes, so a corpus with none of these formats
/// pays one string comparison per file and nothing else.
const HANDLED: &[&str] = &[
    "ipynb", "html", "htm", "docx", "pptx", "xlsx", "odt", "odp", "ods", "epub", "rtf", "eml",
    "mbox",
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
        "epub" => epub(bytes),
        "rtf" => rtf(bytes),
        "eml" => eml(bytes),
        "mbox" => mbox(bytes),
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
    Some(html_text(&String::from_utf8_lossy(bytes)))
}

/// The body of [`html`], over text that has already been decoded.
///
/// Split out because an EPUB chapter arrives as a `String` from the archive
/// reader rather than as bytes, and running it back through a lossy decode to
/// reach the same scanner would be a copy for nothing.
fn html_text(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;

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
    decode_entities(&out)
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
    // What the entry says it expands to, before a byte of it is inflated. An
    // archive is free to lie here, which is why the read below is bounded too —
    // but an honest one that is simply enormous costs nothing to refuse.
    if file.size() > *budget {
        return None;
    }
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

// --------------------------------------------------------------------- EPUB

/// An EPUB book as its chapters, in the order the book says to read them.
///
/// The spine is the whole reason this is not "every XHTML file in the archive".
/// Chapters are named `part0012.xhtml` as often as `chapter-three.xhtml`, so
/// sorted-by-filename is a different book — and often a book whose first
/// chapter is the copyright page. The manifest maps an id to a file and the
/// spine lists those ids in reading order; both are stated by the book rather
/// than guessed from it.
fn epub(bytes: &[u8]) -> Option<String> {
    let mut zip = archive(bytes)?;
    let mut budget = MAX_ARCHIVE_TEXT;

    // The one path the specification fixes. Everything else in an EPUB is
    // found by following this file.
    let container = entry(&mut zip, "META-INF/container.xml", &mut budget)?;
    let opf_path = rootfile(&container)?;
    let opf = entry(&mut zip, &opf_path, &mut budget)?;

    // Manifest hrefs are relative to the OPF, which normally sits a directory
    // below the archive root.
    let base = opf_path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("");

    let mut out = String::new();
    for href in spine(&opf) {
        // A spine entry the archive does not hold is a broken book, not a
        // reason to throw away the chapters that are fine.
        let Some(source) = entry(&mut zip, &join(base, &href), &mut budget) else {
            continue;
        };
        let text = html_text(&source);
        if text.trim().is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("# {href}\n"));
        out.push_str(text.trim_end());
        out.push('\n');
    }
    (!out.is_empty()).then_some(out)
}

/// Where `META-INF/container.xml` says the package document is.
fn rootfile(xml: &str) -> Option<String> {
    let mut path = None;
    scan(xml, |event| {
        if let Event::Open { name, attrs } = event
            && local(name) == "rootfile"
            && path.is_none()
        {
            path = attr(attrs, "full-path");
        }
    });
    path.filter(|p| !p.is_empty())
}

/// Chapter hrefs in reading order: the spine's `idref`s, each resolved through
/// the manifest to the file it names.
fn spine(opf: &str) -> Vec<String> {
    let mut manifest: Vec<(String, String)> = Vec::new();
    let mut order: Vec<String> = Vec::new();

    scan(opf, |event| {
        if let Event::Open { name, attrs } = event {
            match local(name) {
                "item" => {
                    if let (Some(id), Some(href)) = (attr(attrs, "id"), attr(attrs, "href")) {
                        manifest.push((id, href));
                    }
                }
                "itemref" => {
                    if let Some(idref) = attr(attrs, "idref") {
                        order.push(idref);
                    }
                }
                _ => {}
            }
        }
    });

    order
        .iter()
        .filter_map(|wanted| {
            manifest
                .iter()
                .find(|(id, _)| id == wanted)
                .map(|(_, href)| href.clone())
        })
        .collect()
}

/// An element's name without its namespace prefix. EPUB's own files are
/// written both ways — `<package>` in one book, `<opf:package>` in the next —
/// and the prefix is chosen by whoever built the book, so matching on it would
/// read some books and not others.
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// A manifest href resolved against the OPF's own directory.
///
/// ZIP entry names are literal strings, so a path still holding `..` or a
/// percent escape matches nothing and the chapter silently disappears.
fn join(base: &str, href: &str) -> String {
    let href = unpercent(href.split('#').next().unwrap_or(href));
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for segment in href.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// `%20` back to a space, and every other `%hh` back to its byte.
///
/// Shared with [`crate::add`], which needs the same decoding for the opposite
/// reason: here it is so a chapter whose name has a space in it is found in the
/// archive, and there it is so a percent-encoded `..` cannot slip past the path
/// sanitiser as one opaque segment.
pub(crate) fn unpercent(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        if byte != b'%' || rest.len() < 2 {
            out.push(byte);
            continue;
        }
        match hex_byte(&rest[..2]) {
            Some(decoded) => {
                out.push(decoded);
                rest = &rest[2..];
            }
            None => out.push(byte),
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Two ASCII hex digits as the byte they name.
fn hex_byte(pair: &[u8]) -> Option<u8> {
    let text = std::str::from_utf8(pair).ok()?;
    u8::from_str_radix(text, 16).ok()
}

// ---------------------------------------------------------------------- RTF

/// Control words that open a group holding something other than the
/// document's text.
///
/// This list is what makes a reader this small possible. RTF interleaves its
/// prose with font and colour tables, style sheets, embedded pictures and
/// revision metadata, and all of them live in a group named by its own first
/// control word — so they can be skipped whole rather than filtered out of the
/// text afterwards. `\*` covers every destination not named here, which is how
/// a writer's private extensions stay out without this list having to know
/// them.
const RTF_SKIPPED: &[&str] = &[
    "fonttbl",
    "colortbl",
    "stylesheet",
    "info",
    "pict",
    "filetbl",
    "listtable",
    "listoverridetable",
    "rsidtbl",
    "generator",
    "themedata",
    "colorschememapping",
    "latentstyles",
    "datastore",
    "objdata",
    "xmlnstbl",
];

/// The 32 code points Windows-1252 puts where Latin-1 has control characters.
///
/// This range is the entire practical difference between the two, and it holds
/// the characters a word processor actually emits — curly quotes, the dashes,
/// the ellipsis. Reading them as Latin-1 controls turns every quotation mark in
/// a document into an invisible character.
const CP1252_HIGH: [char; 32] = [
    '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
    '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}', '\u{017D}', '\u{008F}',
    '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
    '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
];

/// An RTF document as the text a word processor would show.
///
/// RTF is a stream of three things: literal characters, `{}` groups, and
/// `\control` words. Almost none of a real file is text, so the reader is
/// mostly a list of what to throw away — the destinations in [`RTF_SKIPPED`],
/// and every control word that is neither a line break nor a character.
///
/// It cannot loop: the scan only ever moves forward through the input, so
/// unbalanced braces and a file that stops mid-control-word both end it.
fn rtf(bytes: &[u8]) -> Option<String> {
    let source = String::from_utf8_lossy(bytes);
    // The signature, rather than the extension, decides. Something else saved
    // as `.rtf` would otherwise be emitted as its own markup.
    if !source.trim_start().starts_with("{\\rtf") {
        return None;
    }

    let mut out = String::new();
    let mut depth: i32 = 0;
    // The depth at which a skipped destination opened, if one is open.
    let mut skipping: Option<i32> = None;
    // Characters still to be swallowed as a `\u` escape's ASCII fallback.
    let mut fallback = 0usize;
    // How many of them each `\u` is followed by, per `\uc`.
    let mut fallback_width = 1usize;
    let mut codepage = 1252u32;
    // A group's first control word is the one that can name it a destination.
    let mut at_group_start = false;

    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                depth += 1;
                at_group_start = true;
                fallback = 0;
            }
            '}' => {
                depth -= 1;
                at_group_start = false;
                fallback = 0;
                if skipping.is_some_and(|opened| depth < opened) {
                    skipping = None;
                }
            }
            // A newline in the source is layout, not text: RTF marks its own
            // line breaks with `\par` and `\line`.
            '\r' | '\n' => at_group_start = false,
            '\\' => {
                let first_in_group = at_group_start;
                at_group_start = false;
                match control(&mut chars) {
                    Control::Literal(c) => {
                        emit(&mut out, c, &skipping, &mut fallback);
                    }
                    Control::Byte(byte) => {
                        emit(
                            &mut out,
                            ansi_char(byte, codepage),
                            &skipping,
                            &mut fallback,
                        );
                    }
                    Control::Word { word, param } => {
                        if first_in_group && RTF_SKIPPED.contains(&word.as_str()) {
                            skipping = Some(depth);
                            continue;
                        }
                        match word.as_str() {
                            // `\*` marks the group it opens as a destination
                            // whose contents a reader that does not know the
                            // word must not show.
                            "*" => skipping = Some(depth),
                            "ansicpg" => codepage = param.unwrap_or(1252) as u32,
                            "uc" => fallback_width = param.unwrap_or(1).max(0) as usize,
                            "u" => {
                                // Negative parameters are how RTF writes a code
                                // point above 32767 in a signed 16-bit field.
                                let code = param.unwrap_or(0);
                                let code = if code < 0 { code + 65536 } else { code };
                                if let Some(c) = u32::try_from(code).ok().and_then(char::from_u32) {
                                    emit(&mut out, c, &skipping, &mut fallback);
                                }
                                fallback = fallback_width;
                            }
                            "par" | "line" | "sect" | "page" | "row" => {
                                emit(&mut out, '\n', &skipping, &mut fallback)
                            }
                            "tab" | "cell" => emit(&mut out, '\t', &skipping, &mut fallback),
                            "emdash" => emit(&mut out, '\u{2014}', &skipping, &mut fallback),
                            "endash" => emit(&mut out, '\u{2013}', &skipping, &mut fallback),
                            "bullet" => emit(&mut out, '\u{2022}', &skipping, &mut fallback),
                            "lquote" => emit(&mut out, '\u{2018}', &skipping, &mut fallback),
                            "rquote" => emit(&mut out, '\u{2019}', &skipping, &mut fallback),
                            "ldblquote" => emit(&mut out, '\u{201C}', &skipping, &mut fallback),
                            "rdblquote" => emit(&mut out, '\u{201D}', &skipping, &mut fallback),
                            _ => {}
                        }
                    }
                    Control::End => break,
                }
            }
            _ => {
                at_group_start = false;
                emit(&mut out, c, &skipping, &mut fallback);
            }
        }
    }

    let text: String = out
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then_some(text)
}

/// Append one character, unless a skipped destination is open or the character
/// is part of a `\u` escape's fallback.
fn emit(out: &mut String, c: char, skipping: &Option<i32>, fallback: &mut usize) {
    if skipping.is_some() {
        return;
    }
    if *fallback > 0 {
        // The fallback is the same character written again for a reader that
        // cannot do Unicode. Showing both would double every accented letter.
        *fallback -= 1;
        return;
    }
    out.push(c);
}

/// What followed a backslash.
enum Control {
    /// An escaped literal: `\\`, `\{`, `\}`, a non-breaking space.
    Literal(char),
    /// A `\'hh` byte, still to be read through the document's codepage.
    Byte(u8),
    Word {
        word: String,
        param: Option<i32>,
    },
    /// The file ended mid-escape.
    End,
}

/// One control sequence, consumed from the character stream.
fn control(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Control {
    let Some(&next) = chars.peek() else {
        return Control::End;
    };

    if !next.is_ascii_alphabetic() {
        chars.next();
        return match next {
            '\\' | '{' | '}' => Control::Literal(next),
            '~' => Control::Literal('\u{00A0}'),
            '_' => Control::Literal('\u{2011}'),
            // A backslash directly before a line ending is how some writers
            // spell a paragraph break.
            '\r' | '\n' => Control::Literal('\n'),
            '\'' => {
                let hex: String = chars.by_ref().take(2).collect();
                match hex_byte(hex.as_bytes()) {
                    Some(byte) => Control::Byte(byte),
                    None => Control::End,
                }
            }
            other => Control::Word {
                word: other.to_string(),
                param: None,
            },
        };
    }

    let mut word = String::new();
    while let Some(&c) = chars.peek() {
        if !c.is_ascii_alphabetic() {
            break;
        }
        word.push(c);
        chars.next();
    }

    let mut digits = String::new();
    if chars.peek() == Some(&'-') {
        digits.push('-');
        chars.next();
    }
    while let Some(&c) = chars.peek() {
        if !c.is_ascii_digit() {
            break;
        }
        digits.push(c);
        chars.next();
    }

    // A single space after a control word delimits it and is not text. A
    // second space is.
    if chars.peek() == Some(&' ') {
        chars.next();
    }

    Control::Word {
        word,
        param: digits.parse().ok(),
    }
}

/// A `\'hh` byte as a character, through the document's declared codepage.
fn ansi_char(byte: u8, codepage: u32) -> char {
    if byte < 0x80 {
        return byte as char;
    }
    if codepage == 1252 && byte <= 0x9F {
        return CP1252_HIGH[(byte - 0x80) as usize];
    }
    // Every other codepage is read as Latin-1, where the byte is the code
    // point. Wrong for a Cyrillic or Greek document, and still far better than
    // dropping the character: the surrounding text stays searchable either way.
    byte as char
}

// --------------------------------------------------------------------- mail

/// The headers kept, in the order they are emitted.
///
/// Fixed rather than "all of them": a real message carries thirty headers and
/// twenty-five of them are routing, spam scoring and client fingerprints.
/// Indexed, they bury the five a person is actually searching for under a
/// screen of `X-` lines that are the same in every message they own.
const MAIL_HEADERS: [&str; 5] = ["From", "To", "Cc", "Date", "Subject"];

/// How deep a `multipart/*` may nest before the reader stops following it. Real
/// mail reaches three; anything past this is a message built to be walked
/// rather than read.
const MAX_MAIL_DEPTH: usize = 8;

/// One RFC 5322 message: its five headers, then its body.
fn eml(bytes: &[u8]) -> Option<String> {
    message(&String::from_utf8_lossy(bytes)).map(|(_, text)| text)
}

/// An mbox archive as every message in it, in file order.
///
/// The `From ` separator is the whole of the format's structure, so a file that
/// does not open with one is not an mbox. Reading it as one anyway would
/// produce a single message out of whatever it actually is, which looks like a
/// successful parse and is not one.
fn mbox(bytes: &[u8]) -> Option<String> {
    let source = String::from_utf8_lossy(bytes);
    if !source.starts_with("From ") {
        return None;
    }

    let mut out = String::new();
    let mut number = 0u32;
    for raw in mbox_messages(&source) {
        let Some((subject, text)) = message(&raw) else {
            continue;
        };
        number += 1;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("# Message {number}: {subject}\n"));
        out.push_str(text.trim_end());
        out.push('\n');
    }
    (!out.trim().is_empty()).then_some(out)
}

/// The archive split at its separators, each message without the separator
/// line that introduced it.
fn mbox_messages(source: &str) -> Vec<String> {
    let mut messages = Vec::new();
    let mut current = String::new();

    for line in source.lines() {
        if line.starts_with("From ") {
            if !current.is_empty() {
                messages.push(std::mem::take(&mut current));
            }
            continue;
        }
        // A body line that would look like a separator was escaped with `>`
        // when the archive was written. What the message said is the line
        // without it.
        let line = match line.strip_prefix('>') {
            Some(rest) if rest.starts_with("From ") => rest,
            _ => line,
        };
        current.push_str(line);
        current.push('\n');
    }

    if !current.is_empty() {
        messages.push(current);
    }
    messages
}

/// One message as its decoded subject and the text to index.
fn message(raw: &str) -> Option<(String, String)> {
    let (head, body) = split_head(raw)?;
    let fields = fields(head);

    let mut out = String::new();
    let mut subject = String::new();
    for name in MAIL_HEADERS {
        let Some(value) = field(&fields, name) else {
            continue;
        };
        let decoded = decode_words(value);
        if name == "Subject" {
            subject = decoded.clone();
        }
        out.push_str(&format!("{name}: {decoded}\n"));
    }

    out.push('\n');
    out.push_str(body_text(&fields, body, 0).trim_end());
    out.push('\n');

    (!out.trim().is_empty()).then_some((subject, out))
}

/// The header block and the body, split at the first empty line.
///
/// A message with no empty line has no body by definition, and is far more
/// likely to be a truncated file than a real message — so it is `None`, which
/// is a skipped file rather than an error.
fn split_head(raw: &str) -> Option<(&str, &str)> {
    let crlf = raw.find("\r\n\r\n").map(|at| (at, 4));
    let lf = raw.find("\n\n").map(|at| (at, 2));
    let (at, width) = match (crlf, lf) {
        (Some(a), Some(b)) => {
            if a.0 <= b.0 {
                a
            } else {
                b
            }
        }
        (Some(a), None) => a,
        (None, Some(b)) => b,
        (None, None) => return None,
    };
    Some((&raw[..at], &raw[at + width..]))
}

/// The header block as name/value pairs, unfolded, names lowercased.
fn fields(head: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in head.lines() {
        // A line starting with whitespace continues the one before it. That is
        // how a long Subject or a list of twenty recipients crosses lines, and
        // reading it as a header of its own loses the rest of the value.
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            out.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    out
}

fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    let wanted = name.to_ascii_lowercase();
    fields
        .iter()
        .find(|(key, _)| *key == wanted)
        .map(|(_, value)| value.as_str())
}

/// The readable text of one part, following `multipart/*` into its children.
fn body_text(fields: &[(String, String)], body: &str, depth: usize) -> String {
    let content_type = field(fields, "content-type").unwrap_or_default();
    let mime = mime_of(content_type);

    if mime.starts_with("multipart/") {
        if depth >= MAX_MAIL_DEPTH {
            return String::new();
        }
        return match parameter(content_type, "boundary") {
            Some(boundary) => multipart(body, &boundary, depth + 1),
            None => String::new(),
        };
    }

    let encoding = field(fields, "content-transfer-encoding").unwrap_or_default();
    let decoded = decode_transfer(body, encoding.trim());
    match mime.as_str() {
        "text/html" => html_text(&decoded),
        _ => decoded,
    }
}

/// A `multipart/*` body as the one part worth indexing, plus the names of any
/// attachments.
///
/// The plain part is preferred over the HTML one because they say the same
/// thing and the plain one says it without markup. The HTML one is the
/// fallback rather than an addition, so a `multipart/alternative` is not
/// indexed twice.
fn multipart(body: &str, boundary: &str, depth: usize) -> String {
    let mut plain: Option<String> = None;
    let mut html: Option<String> = None;
    let mut attachments: Vec<String> = Vec::new();

    for part in parts(body, boundary) {
        let Some((head, part_body)) = split_head(part) else {
            continue;
        };
        let fields = fields(head);
        let content_type = field(&fields, "content-type").unwrap_or_default();
        let disposition = field(&fields, "content-disposition").unwrap_or_default();
        let mime = mime_of(content_type);

        if disposition
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("attachment")
        {
            // Named, never decoded. Running a base64 blob back through a second
            // format reader is a matrix of its own, and the filename is both
            // what makes the attachment findable and what a person remembers
            // about it.
            attachments.push(
                parameter(disposition, "filename")
                    .or_else(|| parameter(content_type, "name"))
                    .map(|name| decode_words(&name))
                    .unwrap_or_else(|| "attachment".to_string()),
            );
            continue;
        }

        let text = body_text(&fields, part_body, depth);
        match mime.as_str() {
            "text/html" if html.is_none() => html = Some(text),
            "text/plain" | "" if plain.is_none() => plain = Some(text),
            _ if mime.starts_with("multipart/") && plain.is_none() && !text.trim().is_empty() => {
                plain = Some(text)
            }
            _ => {}
        }
    }

    let mut out = plain.or(html).unwrap_or_default().trim_end().to_string();
    for name in attachments {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format!("# Attachment: {name}"));
    }
    out
}

/// The pieces between a boundary and the closing `--boundary--`.
fn parts<'a>(body: &'a str, boundary: &str) -> Vec<&'a str> {
    let delimiter = format!("--{boundary}");
    let mut out = Vec::new();
    for piece in body.split(delimiter.as_str()).skip(1) {
        // Two dashes straight after the boundary close the multipart; the
        // epilogue after it belongs to no part.
        if piece.starts_with("--") {
            break;
        }
        out.push(piece.trim_start_matches(['\r', '\n']));
    }
    out
}

/// The type and subtype of a Content-Type, lowercased, without its parameters.
fn mime_of(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// One `; key=value` parameter of a header value, unquoted.
fn parameter(value: &str, key: &str) -> Option<String> {
    for piece in value.split(';').skip(1) {
        let Some((name, raw)) = piece.split_once('=') else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case(key) {
            continue;
        }
        let raw = raw.trim();
        let unquoted = raw
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(raw);
        return Some(unquoted.to_string());
    }
    None
}

/// A body decoded from whatever `Content-Transfer-Encoding` it declared.
fn decode_transfer(body: &str, encoding: &str) -> String {
    let bytes = match encoding.to_ascii_lowercase().as_str() {
        "quoted-printable" => quoted_printable(body, false),
        "base64" => base64(body),
        // `7bit`, `8bit` and `binary` all mean the bytes are already the text.
        _ => return body.to_string(),
    };
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Quoted-printable, as bytes.
///
/// Bytes rather than a `String` because an encoded-word may be in a charset
/// that is not UTF-8, and deciding that is the caller's job.
fn quoted_printable(s: &str, underscore_is_space: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut rest = s.as_bytes();
    while let Some((&byte, tail)) = rest.split_first() {
        rest = tail;
        match byte {
            b'=' => {
                // `=` at the end of a line is a soft break: the line continues
                // and neither the marker nor the break is part of the text.
                if let Some(after) = rest.strip_prefix(b"\r\n".as_slice()) {
                    rest = after;
                    continue;
                }
                if let Some(after) = rest.strip_prefix(b"\n".as_slice()) {
                    rest = after;
                    continue;
                }
                match rest.get(..2).and_then(hex_byte) {
                    Some(decoded) => {
                        out.push(decoded);
                        rest = &rest[2..];
                    }
                    // A stray `=` that is not an escape is a literal one.
                    None => out.push(b'='),
                }
            }
            b'_' if underscore_is_space => out.push(b' '),
            _ => out.push(byte),
        }
    }
    out
}

/// Base64, as bytes, ignoring everything outside the alphabet.
///
/// Line breaks, padding and whitespace are all simply not alphabet characters,
/// so skipping anything that is not one handles all three without a case for
/// each.
fn base64(s: &str) -> Vec<u8> {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let mut accumulator = 0u32;
    let mut bits = 0u32;
    for byte in s.bytes() {
        let Some(value) = ALPHABET.iter().position(|&c| c == byte) else {
            continue;
        };
        accumulator = (accumulator << 6) | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((accumulator >> bits) as u8);
        }
    }
    out
}

/// RFC 2047 encoded-words in a header value, decoded.
///
/// A Subject with a non-ASCII character in it does not travel as that
/// character. It travels as `=?utf-8?Q?caf=C3=A9?=`, which is exactly what a
/// person searching for the word will never type.
fn decode_words(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    let mut previous_was_word = false;

    while let Some(at) = rest.find("=?") {
        let (before, from) = rest.split_at(at);
        match encoded_word(from) {
            Some((text, tail)) => {
                // Whitespace between two adjacent encoded-words is how a long
                // value was folded, not part of what it says. Whitespace
                // anywhere else is the value's own.
                if !(previous_was_word && before.trim().is_empty()) {
                    out.push_str(before);
                }
                out.push_str(&text);
                rest = tail;
                previous_was_word = true;
            }
            None => {
                out.push_str(before);
                out.push_str("=?");
                rest = &from[2..];
                previous_was_word = false;
            }
        }
    }

    out.push_str(rest);
    out
}

/// One `=?charset?encoding?text?=` and whatever follows it.
fn encoded_word(s: &str) -> Option<(String, &str)> {
    let body = s.strip_prefix("=?")?;
    // `?` has to be encoded inside the text, so the first `?=` is the end.
    let end = body.find("?=")?;
    let (inside, tail) = (&body[..end], &body[end + 2..]);

    let mut parts = inside.splitn(3, '?');
    let charset = parts.next()?.to_ascii_lowercase();
    let encoding = parts.next()?.to_ascii_lowercase();
    let text = parts.next()?;

    let bytes = match encoding.as_str() {
        "b" => base64(text),
        "q" => quoted_printable(text, true),
        _ => return None,
    };

    let decoded = match charset.as_str() {
        "utf-8" | "utf8" | "us-ascii" | "ascii" => String::from_utf8_lossy(&bytes).into_owned(),
        // Every other charset is read as Latin-1, where the byte is the code
        // point. Wrong for a Cyrillic or Greek subject, and still better than
        // leaving the raw `=?...?=` in place, which is searchable by nobody.
        _ => bytes.iter().map(|&b| b as char).collect(),
    };
    Some((decoded, tail))
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
