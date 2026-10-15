//! The PDF wrapper (`extract_pdf`, src/chunk.rs), through `chunk::extract_counted`
//! — the call the parse worker's `serve_read` makes (src/worker.rs), so this is
//! what it runs.
//!
//! pdf-extract panics on malformed input as a matter of course (an `expect` on
//! every dangling object reference), and `extract_pdf` runs it under
//! `catch_unwind`, so a panic there is a skipped file, not a dead process. The
//! init block replaces libFuzzer's abort-on-panic hook so this target sees what
//! production sees: a contained panic is not a crash; an abort, an overflowed
//! stack, an allocation past the RSS limit, a hang, or a panic that escapes
//! the guard still is.
#![no_main]
use libfuzzer_sys::fuzz_target;
use std::path::Path;

fuzz_target!(
    init: std::panic::set_hook(Box::new(|_| {})),
    |data: &[u8]| {
        let _ = semlith::chunk::extract_counted(Path::new("fuzz.pdf"), data);
    }
);
