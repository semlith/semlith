//! Turning a file on disk into embeddable chunks of text.

use crate::formats;
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

/// Split text into chunks, starting a new one at every line in `cuts`.
///
/// `cuts` are 1-based line numbers, sorted. A budget still ends a chunk, so a
/// definition longer than [`MAX_CHARS`] is split the way anything else is —
/// what a cut buys is that a definition never *begins* in the middle of a
/// chunk, and so is never separated from the doc comment above it.
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
        // The `len == 0` arm guarantees progress: a line wider than the whole
        // budget is still taken, then hard-split below. The cut test is second
        // so that a cut on the first line of a chunk does not end it before it
        // has a line in it.
        while i < lines.len() && (len == 0 || (len + lines[i].len() < MAX_CHARS && !cut(i))) {
            len += lines[i].len() + 1;
            i += 1;
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
/// Three cases, in the order they are tried. Markdown cuts at its headings and
/// carries the heading path as embedding context. Anything tree-sitter found
/// definitions in cuts at those definitions. Everything else — a plain text
/// file, a language with no grammar, a document a reader turned into prose —
/// keeps the fixed window, which is what it always had.
pub fn chunk_file(path: &Path, text: &str, definitions: &[(u32, u32)]) -> Vec<Chunk> {
    // MEASUREMENT TOGGLE — removed before the pull request. 2.1 and 2.2 each
    // need their own before/after pair against the pinned corpus, and they are
    // one commit; this turns 2.2 off so 2.1 can be measured alone.
    if std::env::var_os("SEMLITH_FIXED_WINDOWS").is_some() {
        return chunk_text(text);
    }
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
    if definitions.is_empty() {
        return chunk_text(text);
    }
    chunk_at(text, &definition_cuts(text, definitions))
}

/// The lines a definition starts on, doc comment and attributes included.
///
/// `spans` are the symbols tree-sitter found, as `(start_line, end_line)`. A
/// grammar's node for an item does not include the comment above it — `///` is
/// a sibling, not a child — so a cut taken at the node's own first line would
/// leave the doc comment at the end of the previous chunk, which is where it
/// was before this release and is the reason for it. The walk upwards stops at
/// a blank line, so it takes the comment block attached to this definition and
/// not the tail of whatever is above it.
///
/// The comment markers are the ones the corpus actually uses rather than a
/// table per language: a line that begins with any of them, directly above a
/// definition and with no blank line between, belongs to that definition in
/// every language this indexes. Being wrong here costs two lines of context in
/// one chunk, which is why it is a list and not a parser.
pub fn definition_cuts(text: &str, spans: &[(u32, u32)]) -> Vec<u32> {
    const ATTACHED: [&str; 10] = [
        "///", "//!", "//", "#[", "#'", "--", "/*", "*", "@", "\"\"\"",
    ];
    let lines: Vec<&str> = text.lines().collect();
    let mut cuts: Vec<u32> = Vec::with_capacity(spans.len());
    for (start, _) in spans {
        let mut at = *start as usize; // 1-based
        while at > 1 {
            let above = lines.get(at - 2).map(|l| l.trim()).unwrap_or("");
            if above.is_empty() || !ATTACHED.iter().any(|m| above.starts_with(m)) {
                break;
            }
            at -= 1;
        }
        cuts.push(at as u32);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

/// The lines Markdown headings start on.
pub fn heading_cuts(text: &str) -> Vec<u32> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| line.starts_with('#') && line.trim_start_matches('#').starts_with(' '))
        .map(|(at, _)| at as u32 + 1)
        .collect()
}

/// The heading path a line sits under, as `Store format > Shards`.
///
/// Handed to the embedding model in front of the chunk, never stored as part of
/// it — see [`Chunk::context`]. A section three levels down is usually written
/// as though the reader has the two above it in mind, because they do.
pub fn heading_path(text: &str, line: u32) -> String {
    let mut path: Vec<(usize, String)> = Vec::new();
    for (at, raw) in text.lines().enumerate() {
        if at as u32 + 1 > line {
            break;
        }
        let hashes = raw.len() - raw.trim_start_matches('#').len();
        if hashes == 0 || !raw[hashes..].starts_with(' ') {
            continue;
        }
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

    /// The miss this release's chunking exists to close: a constant and the
    /// comment that says what it is for, in one chunk rather than two halves
    /// of two.
    #[test]
    fn a_definition_keeps_the_doc_comment_above_it() {
        let text = "\
use std::path::Path;

fn earlier() {
    // padding so the constant is not the first thing in the file
}

/// How many nodes a traversal may visit.
///
/// The bound that keeps peak memory flat as the corpus grows.
pub const MAX_NODES: usize = 4000;

fn later() {}
";
        // As tree-sitter would report them: the node's own lines, without the
        // comment above it.
        let definitions = [(3, 5), (10, 10), (12, 12)];
        let cuts = definition_cuts(text, &definitions);
        assert!(
            cuts.contains(&7),
            "the cut should be at the doc comment, not at the const: {cuts:?}"
        );

        let chunks = chunk_at(text, &cuts);
        let holding = chunks
            .iter()
            .find(|c| c.text.contains("MAX_NODES"))
            .expect("some chunk holds the constant");
        assert!(
            holding
                .text
                .contains("How many nodes a traversal may visit"),
            "the constant is separated from its doc comment:\n{}",
            holding.text
        );
        assert!(
            holding.text.starts_with("/// How many nodes"),
            "the chunk should begin at the doc comment:\n{}",
            holding.text
        );
    }

    #[test]
    fn a_cut_ends_a_chunk_even_under_budget() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        let chunks = chunk_at(text, &[1, 2, 3]);
        assert_eq!(chunks.len(), 3, "{chunks:#?}");
        assert_eq!(
            chunks.iter().map(|c| c.start_line).collect::<Vec<_>>(),
            vec![1, 2, 3],
            "a cut did not start a chunk"
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

    #[test]
    fn text_with_no_structure_keeps_the_fixed_window() {
        let text: String = (1..=200).map(|i| format!("line {i}\n")).collect();
        assert_eq!(
            chunk_file(Path::new("notes.txt"), &text, &[]),
            chunk_text(&text)
        );
    }
}
