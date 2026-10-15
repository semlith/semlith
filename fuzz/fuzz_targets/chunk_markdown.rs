//! The Markdown chunker, the way the index pipeline runs it on a file's text:
//! graph first (src/pipeline.rs `chunk_file` call), then heading cuts.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let path = Path::new("fuzz.md");
    let text = String::from_utf8_lossy(data);
    let graph = semlith::graph::extract(path, &text).ok().flatten();
    let symbols = graph.as_ref().map_or(&[][..], |g| &g.symbols);
    let _ = semlith::chunk::chunk_file(path, &text, symbols);
});
