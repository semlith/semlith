//! The notebook reader, through `chunk::extract_counted` — the call the parse
//! worker's `serve_read` makes (src/worker.rs), so this is what it runs.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(|data: &[u8]| {
    let _ = semlith::chunk::extract_counted(Path::new("fuzz.ipynb"), data);
});
