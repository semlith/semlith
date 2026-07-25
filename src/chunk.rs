//! Turning a file on disk into embeddable chunks of text.

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

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// 1-based, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    pub text: String,
}

/// Turn already-read file contents into text. `None` means "deliberately
/// skipped", not an error: binary, or an unreadable PDF.
///
/// `path` is only consulted for its extension; the caller has the bytes
/// already because it needs them to hash the file anyway.
pub fn extract(path: &Path, bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return extract_pdf(bytes);
    }
    if is_binary(bytes) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// pdf-extract can panic on malformed input, and one bad PDF should not take
/// down a whole indexing run.
fn extract_pdf(bytes: &[u8]) -> Option<String> {
    let text = std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes))
        .ok()?
        .ok()?;
    (!text.trim().is_empty()).then_some(text)
}

/// A NUL byte in the first 8 KiB is the same heuristic git uses.
fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&b| b == 0)
}

/// Split text into line-aligned chunks of at most [`MAX_CHARS`] characters.
///
/// Lines longer than the budget on their own (minified JS, embedded base64)
/// are hard-split on a char boundary rather than emitted oversized.
pub fn chunk_text(text: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = text.lines().collect();
    let mut chunks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let start = i;
        let mut len = 0;
        // The `len == 0` arm guarantees progress: a line wider than the whole
        // budget is still taken, then hard-split below.
        while i < lines.len() && (len == 0 || len + lines[i].len() < MAX_CHARS) {
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
                });
            }
        } else {
            chunks.push(Chunk {
                start_line: start as u32 + 1,
                end_line: i as u32,
                text: body,
            });
        }

        // Step back so the next chunk repeats a couple of lines of context.
        if i < lines.len() {
            i = i.saturating_sub(OVERLAP_LINES).max(start + 1);
        }
    }

    chunks
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
}
