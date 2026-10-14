//! The parse worker, driven as the daemon drives it: a file that aborts a
//! parser, hangs one or allocates past the limit costs one worker and comes
//! back as a reason, and the next file is read by a fresh worker.
//!
//! One test in its own binary, because it sets the environment the workers
//! inherit before the first one starts. No model, so it runs in the default
//! set on every OS.

use semlith::worker::{self, Stop};
use std::path::Path;

#[test]
fn a_parser_that_dies_costs_one_worker_and_names_the_reason() {
    // SAFETY: the only test in this binary, before any worker is started or
    // any other thread reads the environment.
    unsafe {
        std::env::set_var("SEMLITH_NO_NESTING_CAP", "1");
        std::env::set_var("SEMLITH_FAULT_HANG", "hang.txt");
        std::env::set_var("SEMLITH_FAULT_ALLOC", "alloc.txt");
        std::env::set_var("SEMLITH_PARSE_DEADLINE_MS", "3000");
        std::env::set_var(worker::MEMORY_ENV, "256");
    }
    worker::enable(env!("CARGO_BIN_EXE_semlith").into());

    // A file that reads cleanly comes back with its text and its graph.
    let ok = worker::read(Path::new("ok.rs"), b"fn main() {}\n", true).expect("a clean read");
    let (text, _) = ok.text.unwrap().unwrap();
    assert_eq!(text, "fn main() {}\n");
    assert!(
        ok.graph.unwrap().unwrap().is_some(),
        "a Rust file has a graph"
    );

    // The real trigger: tree-sitter-md's external scanner overflows its
    // 1024-byte buffer and the C assert aborts the process.
    let quotes = "> ".repeat(300) + "x\n";
    match worker::read(Path::new("deep.md"), quotes.as_bytes(), true) {
        Err(Stop::Crashed(reason)) => {
            assert!(reason.starts_with("parser crashed"), "{reason}");
            #[cfg(unix)]
            assert_eq!(reason, "parser crashed: SIGABRT");
        }
        other => panic!("the abort was not contained as a crash: {other:?}"),
    }

    // The next file is read by a fresh worker.
    assert!(worker::read(Path::new("after.txt"), b"still here\n", false).is_ok());

    match worker::read(Path::new("hang.txt"), b"x", false) {
        Err(Stop::Crashed(reason)) => assert_eq!(reason, "parser timed out after 3 s"),
        other => panic!("a hang was not cut off: {other:?}"),
    }

    match worker::read(Path::new("alloc.txt"), b"x", false) {
        Err(Stop::Crashed(reason)) => assert!(
            reason.starts_with("parser ran past its memory limit (256 MiB"),
            "{reason}"
        ),
        other => panic!("an allocation past the limit was not contained: {other:?}"),
    }

    // A graph asked for on text already decided goes through a worker too.
    let graph = worker::graph(Path::new("deep.yaml"), &("- ".repeat(300) + "x\n"));
    assert!(matches!(graph, Err(Stop::Crashed(_))), "{graph:?}");
    assert!(
        worker::graph(Path::new("fine.yaml"), "a: 1\n")
            .unwrap()
            .is_ok()
    );
}
