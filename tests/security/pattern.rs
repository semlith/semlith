//! A pattern parses stored text in the parse worker, as indexing does: a
//! grammar that crashes on a file costs one worker, never the daemon or the
//! command. cargo-fuzz found a Markdown input that faults tree-sitter-md's C
//! code (0.41.0); `semlith_pattern` used to parse stored Markdown in-process.

#[allow(unused_imports)]
use super::*;

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn a_parser_crash_during_a_pattern_costs_a_worker_not_the_process() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(
        corpus.join("notes.md"),
        "# Notes\n\nThe deploy runs nightly.\n",
    )
    .unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let clean = run(
        &home,
        &["pattern", "--lang", "markdown", "(atx_heading) @h"],
        &[],
    );
    let said = String::from_utf8_lossy(&clean.stdout).into_owned();
    assert!(
        clean.status.success() && said.contains("notes.md"),
        "{said}"
    );

    // Debug builds abort the worker on a request named for the language.
    let out = run(
        &home,
        &["pattern", "--lang", "markdown", "(atx_heading) @h"],
        &[("SEMLITH_FAULT_ABORT", "markdown")],
    );
    assert!(
        out.status.success(),
        "a parser abort took the command down: {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    ); // And the parse did cross the worker: the file it crashed on is passed
    // over, where an in-process parse would never have reached the hook.
    let said = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !said.contains("notes.md"),
        "the pattern parsed in-process:\n{said}"
    );
}
