//! S16: an agent's query cannot become a formula or live HTML in an export.

#[allow(unused_imports)]
use super::*;

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s16_a_query_exports_as_text_not_a_formula_or_html() {
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

    // What a hostile agent would ask: a spreadsheet formula, and markup.
    let formula = r#"=HYPERLINK("http://example.invalid","deploy")"#;
    let markup = "<img src=x onerror=alert(1)> deploy | nightly";
    for query in [formula, markup] {
        let out = run(&home, &["search", query], &[]);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let csv = run(
        &home,
        &["report", "access", "--excerpts", "--format", "csv"],
        &[],
    );
    let csv = String::from_utf8_lossy(&csv.stdout).into_owned();
    assert!(
        csv.contains("HYPERLINK"),
        "the query never reached the report:\n{csv}"
    );
    for line in csv.lines() {
        for cell in line.split(',') {
            let cell = cell.trim_start_matches('"');
            assert!(
                !cell.starts_with("=HYPERLINK"),
                "a formula cell reached the CSV: {line}"
            );
        }
    }

    let md = run(
        &home,
        &["report", "access", "--excerpts", "--format", "markdown"],
        &[],
    );
    let md = String::from_utf8_lossy(&md.stdout).into_owned();
    assert!(
        md.contains("onerror"),
        "the query never reached the report:\n{md}"
    );
    assert!(!md.contains("<img"), "raw HTML reached the Markdown:\n{md}");
}
