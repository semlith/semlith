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
        std::env::set_var("SEMLITH_FAULT_GRAPH_HANG", "graph-hang.rs");
        std::env::set_var("SEMLITH_FAULT_ALLOC", "alloc.txt");
        std::env::set_var("SEMLITH_PARSE_DEADLINE_MS", "3000");
        std::env::set_var(worker::MEMORY_ENV, "8");
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

    // Out of time while the graph is being made: the text survives, without
    // a graph, rather than the file being refused.
    let slow = worker::read(Path::new("graph-hang.rs"), b"fn main() {}\n", true)
        .expect("the text is read again alone");
    assert_eq!(slow.text.unwrap().unwrap().0, "fn main() {}\n");
    assert!(matches!(slow.graph, Some(Ok(None))), "{:?}", slow.graph);

    match worker::read(Path::new("alloc.txt"), b"x", false) {
        Err(Stop::Crashed(reason)) => assert!(
            reason.starts_with("parser ran past its memory limit (8 MiB"),
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

    // macOS cannot lower RLIMIT_AS, so tree-sitter's own mallocs — which the
    // counting allocator never sees — are bounded by the parent watching the
    // worker's footprint. 600 KB of `a;` is about 2 MB of Rust and, measured
    // on an M1, 191 MB of parse tree in a release build. The limit is 8 MiB
    // because tree-sitter ends a parse at two seconds: on a loaded machine a
    // debug parse reached only 14 MB in that time and slipped under 16 MiB.
    #[cfg(target_os = "macos")]
    {
        // What one 25 ms poll lets a parse add before the worker is killed:
        // 1 to 3 MiB measured in a debug build, about 13 MiB at a release
        // parse's 500 MB/s.
        const MARGIN_MB: u64 = 32;
        let big = "fn f() {\n".to_string() + &"a;".repeat(300_000) + "\n}\n";
        match worker::read(Path::new("big.rs"), big.as_bytes(), true) {
            Err(Stop::Crashed(reason)) => {
                assert!(
                    reason.starts_with("parser ran past its memory limit (8 MiB"),
                    "{reason}"
                );
                // "...: the worker peaked at P MiB against a cap of C MiB"
                let mib = |after: &str| -> u64 {
                    let at = reason.find(after).expect(after) + after.len();
                    reason[at..].split(' ').next().unwrap().parse().unwrap()
                };
                let (peak, cap) = (mib("peaked at "), mib("cap of "));
                assert!(
                    (8..8 + 64).contains(&cap),
                    "the cap is the limit and the worker's idle footprint: {reason}"
                );
                assert!(peak < cap + MARGIN_MB, "{reason}");
            }
            Ok(_) => panic!("native allocations past the cap were not contained: it parsed"),
            Err(other) => panic!("not refused for memory: {other:?}"),
        }
        // The next file parses, in a fresh worker.
        let next = worker::read(Path::new("ok.rs"), b"fn main() {}\n", true).expect("a clean read");
        assert!(next.graph.unwrap().unwrap().is_some());
    }
}
