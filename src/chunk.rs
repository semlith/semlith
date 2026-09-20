//! Turning a file on disk into embeddable chunks of text.

use crate::formats;
use crate::graph::Symbol;
use std::path::Path;

/// Soft upper bound on chunk size, in characters.
///
/// At roughly 3-4 characters per token this lands around 200-270 tokens, well
/// inside the embedding model's window. Smaller chunks are not just cheaper to
/// embed — transformer cost grows faster than linearly in sequence length —
/// they also retrieve more precisely and cost an agent fewer tokens to read.
pub const MAX_CHARS: usize = 800;

/// Lines repeated from the previous chunk, so a match that straddles a chunk
/// boundary still has some context on at least one side.
pub const OVERLAP_LINES: usize = 2;

/// Files above this are skipped: usually generated, vendored, or a blob that
/// nobody wants to read the middle of anyway.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chunk {
    /// 1-based, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
    /// What the embedding model is shown in front of `text`, and nothing else
    /// is.
    ///
    /// A Markdown chunk two headings deep says what it is about in its
    /// ancestors and not in its own words — "it is rewritten on the next index
    /// pass" means nothing until you know the section is about the store
    /// format. The ancestors are not contiguous lines, so they cannot be part
    /// of `text`: `Semlith::read` maps every line of a chunk to
    /// `start_line + offset`, and one invented line at the front would shift
    /// every span the store can answer with by one. So the model sees the path
    /// and the store keeps the file's own bytes.
    pub context: String,
}

impl Chunk {
    /// What is embedded: the context, then the text.
    pub fn embedded(&self) -> String {
        if self.context.is_empty() {
            self.text.clone()
        } else {
            format!("{}\n{}", self.context, self.text)
        }
    }
}

/// Turn already-read file contents into text. `None` means "deliberately
/// skipped", not an error: binary, an unreadable PDF, or a document that could
/// not be opened.
///
/// The extension decides, and it decides before anything looks at the bytes.
/// That ordering is what lets a document be read at all — `.docx` and its
/// relatives are ZIP archives, so the binary check below would reject every one
/// of them — and it is also what keeps a corpus of ordinary source from paying
/// for formats it does not contain.
///
/// `path` is only consulted for its extension; the caller has the bytes
/// already because it needs them to hash the file anyway.
pub fn extract(path: &Path, bytes: &[u8]) -> Result<String, crate::SkipReason> {
    use crate::SkipReason;
    if bytes.is_empty() {
        return Err(SkipReason::Empty);
    }
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    // An `Option` before 0.19.0, which is why the index loop's branch for it
    // reported nothing: there was nothing to report. "A reader ran and got
    // nothing out" and "these bytes are not text" are different answers and
    // the person chasing a missing file needs to be told which.
    match ext.as_deref() {
        Some("pdf") => extract_pdf(bytes).ok_or(SkipReason::NoText),
        Some(ext) if formats::handles(ext) => {
            guard(|| formats::extract(ext, bytes)).ok_or(SkipReason::NoText)
        }
        _ => {
            if is_binary(bytes) {
                return Err(SkipReason::Binary);
            }
            Ok(String::from_utf8_lossy(bytes).into_owned())
        }
    }
}

/// Which reader turned this file into text.
///
/// The portal's Files view shows it, because "that .docx came out empty" and
/// "that .docx was read as binary and skipped" look identical in a file list
/// and are two entirely different problems. Derived from the same extension
/// dispatch [`extract`] uses, so it cannot describe a reader that did not run.
pub fn reader_of(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("pdf") => "pdf",
        Some("ipynb") => "notebook",
        Some("html" | "htm") => "html",
        Some("docx") => "word",
        Some("pptx") => "powerpoint",
        Some("xlsx") => "excel",
        Some("odt" | "odp" | "ods") => "opendocument",
        Some("epub") => "epub",
        Some("rtf") => "rtf",
        Some("eml" | "mbox") => "mail",
        // Not a reader at all in the sense the others are: an image is not
        // turned into text, it is embedded as a picture. The Files page says
        // `image` so that a row with no lines and no chunks reads as a
        // deliberate kind of file rather than as one that failed to parse.
        Some(ext) if crate::image::EXTENSIONS.contains(&ext) => "image",
        _ => "text",
    }
}

/// pdf-extract can panic on malformed input, and one bad PDF should not take
/// down a whole indexing run.
fn extract_pdf(bytes: &[u8]) -> Option<String> {
    let text = guard(|| pdf_extract::extract_text_from_mem(bytes).ok())?;
    (!text.trim().is_empty()).then_some(text)
}

/// Run an extractor so that a panic inside it is a skipped file rather than a
/// dead process.
///
/// The readers in [`crate::formats`] are written not to panic and are tested
/// against malformed input for every format, but they sit downstream of a
/// decompressor and a document somebody else wrote. A panic here would take out
/// an indexing run that is otherwise minutes from finishing, so the cheap
/// insurance is worth its one line.
fn guard(extract: impl FnOnce() -> Option<String>) -> Option<String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(extract)).ok()?
}

/// A NUL byte in the first 8 KiB is the same heuristic git uses.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

/// Split text into line-aligned chunks of at most [`MAX_CHARS`] characters.
///
/// Lines longer than the budget on their own (minified JS, embedded base64)
/// are hard-split on a char boundary rather than emitted oversized.
///
/// The fallback for text with no structure. Where the structure is known,
/// [`chunk_at`] is given the lines a chunk must start on.
pub fn chunk_text(text: &str) -> Vec<Chunk> {
    chunk_at(text, &[])
}

/// Split text into chunks, ending one at every line in `cuts`.
///
/// `cuts` are 1-based line numbers, sorted: the lines sections start on. A
/// section longer than [`MAX_CHARS`] is still split by the budget the way
/// anything else is; what a cut buys is that a chunk never spans two sections,
/// so the heading path it carries is true of all of it.
///
/// This is the whole of the fix for the miss class the 0.22.0 contract calls a
/// chunk boundary. A fixed 800-character window put `MAX_NODES` and the six
/// lines explaining what it bounds in different chunks, so the chunk holding
/// the constant said nothing about what it was for and the chunk that said it
/// did not hold the constant. Neither answered a question about `MAX_NODES`.
///
/// The overlap is dropped where the next chunk starts on a cut. Two repeated
/// lines exist so a match straddling an arbitrary boundary keeps some context;
/// a boundary that is a definition's own first line is not arbitrary, and
/// repeating the tail of the previous definition into the front of this one is
/// exactly the blurring the cut is for.
pub fn chunk_at(text: &str, cuts: &[u32]) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let cut = |line_index: usize| -> bool { cuts.binary_search(&(line_index as u32 + 1)).is_ok() };
    let mut chunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let start = i;
        let mut len = 0;
        // The first cut inside this chunk, which is where it ends.
        let mut first_cut: Option<usize> = None;
        // The `len == 0` arm guarantees progress: a line wider than the whole
        // budget is still taken, then hard-split below.
        while i < lines.len() && (len == 0 || len + lines[i].len() < MAX_CHARS) {
            if i > start && cut(i) {
                first_cut = first_cut.or(Some(i));
            }
            len += lines[i].len() + 1;
            i += 1;
        }
        // A cut ends the chunk wherever it falls. A section is the unit a
        // reader and a writer both think in, and a chunk spanning two of them
        // would carry the first one's heading path over the second one's text.
        if let Some(boundary) = first_cut {
            i = i.min(boundary);
        }

        let body = lines[start..i].join("\n");
        if body.trim().is_empty() {
            continue;
        }

        if body.chars().count() > MAX_CHARS {
            // Single over-long line: slice it up, all pieces share the line span.
            let line = start as u32 + 1;
            for piece in split_chars(&body, MAX_CHARS) {
                chunks.push(Chunk {
                    start_line: line,
                    end_line: line,
                    text: piece,
                    context: String::new(),
                });
            }
        } else {
            chunks.push(Chunk {
                start_line: start as u32 + 1,
                end_line: i as u32,
                text: body,
                context: String::new(),
            });
        }

        // Step back so the next chunk repeats a couple of lines of context —
        // unless it is about to start on a cut, which is a boundary that means
        // something.
        if i < lines.len() && !cut(i) {
            i = i.saturating_sub(OVERLAP_LINES).max(start + 1);
        }
    }

    chunks
}

/// Chunk one file, cutting where its own structure says to.
///
/// Markdown cuts at its headings and carries the heading path as embedding
/// context. Everything else keeps the fixed window, which is what it always
/// had.
///
/// Code is deliberately not cut at its definitions, and that is a measurement
/// rather than an opinion. Cutting there was built twice and measured twice on
/// the pinned corpus. Forcing a boundary at every definition gave 45/53/64
/// against 51/57/66 without it; aligning the boundary to the budget recovered
/// most of that and still gave 46/61/67 against 53/61/67 for Markdown cuts
/// alone. Question by question the reason is plain: the Markdown heading path
/// wins concept questions — `concept-no-cookie` 5 to 1, `concept-stale-index-
/// on-save` 7 to 2, two that missed entirely coming back — while cutting code
/// loses identifier questions — `id-dependency-kinds` 1 to 3, `id-token-header`
/// 1 to 3, `id-key-grace` 3 to 6. They were one scope item and they pull in
/// opposite directions, so only the half that wins is here.
pub fn chunk_file(path: &Path, text: &str, symbols: &[Symbol]) -> Vec<Chunk> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if matches!(ext.as_deref(), Some("md" | "markdown" | "mdx")) {
        let cuts = heading_cuts(text);
        if cuts.is_empty() {
            return chunk_text(text);
        }
        let mut chunks = chunk_at(text, &cuts);
        for chunk in &mut chunks {
            chunk.context = heading_path(text, chunk.start_line);
        }
        return chunks;
    }
    let mut chunks = chunk_text(text);
    if !symbols.is_empty() {
        let lines: Vec<&str> = text.lines().collect();
        for chunk in &mut chunks {
            chunk.context = code_context(&lines, symbols, chunk.start_line);
        }
    }
    chunks
}

/// What a code chunk is shown in front of its text: the definition it sits
/// inside, and the first line of that definition's doc comment.
///
/// The same argument as the Markdown heading path, and the same restraint. A
/// window cut out of the middle of a function says what it does and never says
/// whose body it is — "the receiver of a qualified call" is a line about
/// `Edge::hint` that never names `Edge` or `hint`, so a question phrased the
/// way a person asks it reaches nothing. The signature and the doc line say
/// both, they are already in the file, and they go in front of the embedded
/// text only: `Semlith::read` maps every line of a chunk to `start_line +
/// offset`, so an invented line inside `text` would shift every span the store
/// can answer with.
///
/// Innermost wins. A method inside an `impl` inside a module is described by
/// the method, because that is the smallest true statement about the lines in
/// hand.
fn code_context(lines: &[&str], symbols: &[Symbol], start_line: u32) -> String {
    let Some(symbol) = symbols
        .iter()
        .filter(|s| s.start_line <= start_line && start_line <= s.end_line)
        .max_by_key(|s| s.start_line)
    else {
        return String::new();
    };

    let at = symbol.start_line.saturating_sub(1) as usize;
    let Some(signature) = lines.get(at) else {
        return String::new();
    };
    let signature = signature.trim();
    if signature.is_empty() {
        return String::new();
    }

    // The doc comment above the signature, if the file has one: the first line
    // of the contiguous comment block, which is the line that says what the
    // definition is for. The rest of the block is detail the body usually
    // repeats.
    let mut doc = None;
    let mut above = at;
    while above > 0 {
        above -= 1;
        let line = lines[above].trim();
        if let Some(said) = comment_body(line) {
            if !said.is_empty() {
                doc = Some(said.to_string());
            }
            continue;
        }
        break;
    }

    let signature = truncate_chars(signature, CONTEXT_CHARS);
    match doc {
        Some(doc) => format!("{signature}\n{}", truncate_chars(&doc, CONTEXT_CHARS)),
        None => signature.to_string(),
    }
}

/// What a comment line says, or `None` when the line is not a comment.
///
/// Every marker the indexed languages write a doc comment with, and nothing
/// clever: a line that is not a comment ends the block, which is what stops
/// this walking out of one definition and into the code above it.
fn comment_body(line: &str) -> Option<&str> {
    for marker in [
        "///", "//!", "//", "#!", "#", "--", "*/", "/**", "/*", "*", ";;", "%",
    ] {
        if let Some(rest) = line.strip_prefix(marker) {
            return Some(rest.trim());
        }
    }
    None
}

/// Cut to a character budget on a character boundary, so one enormous
/// generated signature cannot become most of what the model reads.
fn truncate_chars(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => text[..at].to_string(),
        None => text.to_string(),
    }
}

/// The budget for each half of a code chunk's context.
///
/// A signature and a doc line are short by nature; the cap is for generated
/// code, where one definition can be a kilobyte of type parameters and would
/// otherwise crowd out the chunk it is meant to describe.
const CONTEXT_CHARS: usize = 200;

/// The lines Markdown headings start on.
///
/// Fenced code is skipped, because a great deal of the prose this indexes is
/// shell transcripts and `# install semlith` is a comment rather than a
/// heading. Taken as a heading it would cut a chunk in the middle of a command
/// and, worse, reset the heading path to a line the section is not about.
pub fn heading_cuts(text: &str) -> Vec<u32> {
    let mut cuts = Vec::new();
    let mut fenced = false;
    for (at, line) in text.lines().enumerate() {
        if line.starts_with("```") || line.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if !fenced && is_heading(line) {
            cuts.push(at as u32 + 1);
        }
    }
    cuts
}

/// A line that opens an ATX heading: one to six hashes, then a space.
fn is_heading(line: &str) -> bool {
    let hashes = line.len() - line.trim_start_matches('#').len();
    (1..=6).contains(&hashes) && line[hashes..].starts_with(' ')
}

/// The heading path a line sits under, as `Store format > Shards`.
///
/// Handed to the embedding model in front of the chunk, never stored as part of
/// it — see [`Chunk::context`]. A section three levels down is usually written
/// as though the reader has the two above it in mind, because they do.
pub fn heading_path(text: &str, line: u32) -> String {
    let mut path: Vec<(usize, String)> = Vec::new();
    let mut fenced = false;
    for (at, raw) in text.lines().enumerate() {
        if at as u32 + 1 > line {
            break;
        }
        if raw.starts_with("```") || raw.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || !is_heading(raw) {
            continue;
        }
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        path.retain(|(level, _)| *level < hashes);
        path.push((hashes, raw[hashes..].trim().to_string()));
    }
    path.into_iter()
        .map(|(_, title)| title)
        .collect::<Vec<_>>()
        .join(" > ")
}

fn split_chars(s: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    chars.chunks(n).map(|c| c.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_cover_every_line_and_stay_under_budget() {
        let text: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        let chunks = chunk_text(&text);

        assert!(chunks.len() > 1, "200 lines should not fit in one chunk");
        for c in &chunks {
            assert!(c.text.chars().count() <= MAX_CHARS, "chunk over budget");
            assert!(c.start_line >= 1 && c.end_line >= c.start_line);
        }
        // Every source line lands in at least one chunk.
        for i in 1..=200 {
            let needle = format!("line {i}\n");
            assert!(
                chunks.iter().any(|c| c.text.contains(needle.trim_end())),
                "line {i} missing from all chunks"
            );
        }
        // Chunks advance; an off-by-one in the overlap step-back would loop forever.
        for w in chunks.windows(2) {
            assert!(
                w[1].start_line > w[0].start_line,
                "chunking did not advance"
            );
        }
    }

    #[test]
    fn over_long_single_line_is_split() {
        let text = "x".repeat(MAX_CHARS * 3);
        let chunks = chunk_text(&text);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| c.text.chars().count() <= MAX_CHARS));
    }

    #[test]
    fn blank_input_yields_nothing() {
        assert!(chunk_text("").is_empty());
        assert!(chunk_text("\n\n   \n").is_empty());
    }

    #[test]
    fn binary_is_detected() {
        assert!(is_binary(b"abc\0def"));
        assert!(!is_binary(b"fn main() {}"));
    }

    /// A forced cut is what Markdown takes, and it does end a chunk wherever it
    /// falls — a section is the unit, and a chunk spanning two would carry the
    /// first one's heading path over the second one's text.
    #[test]
    fn a_cut_ends_a_chunk_even_under_budget() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let chunks = chunk_at(text, &[1, 2, 3]);
        assert_eq!(
            chunks.iter().map(|c| c.start_line).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "a forced cut did not start a chunk"
        );
    }

    /// A definition longer than the budget is still split. What a cut buys is
    /// that it never *begins* mid-chunk.
    #[test]
    fn an_over_long_definition_is_still_split() {
        let body: String = (1..=100)
            .map(|i| format!("    let x{i} = {i};\n"))
            .collect();
        let text = format!("fn big() {{\n{body}}}\n");
        let chunks = chunk_at(&text, &[1]);
        assert!(chunks.len() > 1, "a 100-line function fit in one chunk?");
        assert_eq!(chunks[0].start_line, 1);
        for chunk in &chunks {
            assert!(chunk.text.chars().count() <= MAX_CHARS);
        }
    }

    #[test]
    fn a_markdown_chunk_carries_its_heading_path() {
        let text = "\
# Architecture

Prose.

## Store format

More prose.

### Shards

The part a reader needs the two headings above to understand.
";
        let chunks = chunk_file(Path::new("docs/architecture.md"), text, &[]);
        let deepest = chunks
            .iter()
            .find(|c| c.text.contains("the two headings above"))
            .expect("the deepest section is a chunk");
        assert_eq!(deepest.context, "Architecture > Store format > Shards");
        assert!(
            deepest
                .embedded()
                .starts_with("Architecture > Store format > Shards\n"),
            "the model should see the path first: {:?}",
            deepest.embedded()
        );
        // And the stored text is still the file's own bytes, because
        // `Semlith::read` maps line `start_line + offset`.
        assert!(
            !deepest.text.contains("Architecture > Store format"),
            "the path leaked into the stored text, which would shift every span \
             this store can answer with by one line"
        );
    }

    /// A shell comment in a fenced block is not a heading. Most of the prose
    /// this indexes is documentation full of transcripts.
    #[test]
    fn a_hash_inside_a_code_fence_is_not_a_heading() {
        let text = "\
# Install

```sh
# install semlith
curl -fsSL https://example.invalid/install.sh | sh
```

Text after the fence.
";
        assert_eq!(
            heading_cuts(text),
            vec![1],
            "a fenced comment was cut as a heading"
        );
        let after = text
            .lines()
            .position(|l| l.starts_with("Text after"))
            .unwrap() as u32
            + 1;
        assert_eq!(heading_path(text, after), "Install");
    }

    #[test]
    fn a_code_chunk_carries_the_definition_it_sits_inside() {
        let mut text = String::from(
            "/// How many nodes a traversal may visit before it gives up.\n\
             /// Four thousand, measured on the pinned corpus.\n\
             pub fn walk(graph: &Graph, max_nodes: usize) -> Vec<Node> {\n",
        );
        // Long enough that the chunk is a window out of the middle of the
        // body, which is the case the context exists for.
        for i in 1..=60 {
            text.push_str(&format!("    let step_{i} = visit(graph, {i});\n"));
        }
        text.push_str("}\n");

        let symbols = crate::graph::extract(Path::new("walk.rs"), &text)
            .expect("the file parses")
            .expect("Rust has a grammar")
            .symbols;
        let chunks = chunk_file(Path::new("walk.rs"), &text, &symbols);
        let inside = chunks
            .iter()
            .find(|c| c.text.contains("let step_40"))
            .expect("the body is chunked");

        assert_eq!(
            inside.context,
            format!(
                "{}\n{}",
                "pub fn walk(graph: &Graph, max_nodes: usize) -> Vec<Node> {",
                "How many nodes a traversal may visit before it gives up."
            ),
            "a window out of a function body must say whose body it is"
        );
        assert!(
            inside.embedded().starts_with("pub fn walk("),
            "the model is shown the definition first"
        );
        assert!(
            !inside.text.contains("pub fn walk("),
            "the stored bytes are the file's own: an invented line would shift every span"
        );
        assert_eq!(
            chunk_file(Path::new("walk.rs"), &text, &[])
                .iter()
                .map(|c| c.text.clone())
                .collect::<Vec<_>>(),
            chunks.iter().map(|c| c.text.clone()).collect::<Vec<_>>(),
            "the context changes what is embedded and nothing about where the cuts fall"
        );
    }

    #[test]
    fn text_with_no_structure_keeps_the_fixed_window() {
        let text: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        assert_eq!(
            chunk_file(Path::new("notes.txt"), &text, &[]),
            chunk_text(&text)
        );
    }
}
