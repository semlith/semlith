//! The ZIP readers (Word, Excel, PowerPoint, OpenDocument, EPUB), through `chunk::extract_counted` — the call the
//! parse worker's `serve_read` makes (src/worker.rs), so this is what it runs.
//! The first byte picks the extension, which picks the reader.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;

const PATHS: &[&str] = &[
    "fuzz.docx",
    "fuzz.xlsx",
    "fuzz.pptx",
    "fuzz.odt",
    "fuzz.odp",
    "fuzz.ods",
    "fuzz.epub",
];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let _ = semlith::chunk::extract_counted(Path::new(PATHS[pick as usize % PATHS.len()]), rest);
});
