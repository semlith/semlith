//! The tree-sitter graph and the code chunker, as the index pipeline runs them:
//! `graph::extract` for the symbols, then `chunk::chunk_file` with them. The
//! first byte picks the language.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;

const PATHS: &[&str] = &[
    "fuzz.rs",
    "fuzz.py",
    "fuzz.js",
    "fuzz.ts",
    "fuzz.go",
    "fuzz.yaml",
];

fuzz_target!(|data: &[u8]| {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let path = Path::new(PATHS[pick as usize % PATHS.len()]);
    let text = String::from_utf8_lossy(rest);
    let graph = semlith::graph::extract(path, &text).ok().flatten();
    let symbols = graph.as_ref().map_or(&[][..], |g| &g.symbols);
    let _ = semlith::chunk::chunk_file(path, &text, symbols);
});
