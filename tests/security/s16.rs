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
    //
    // Asked over MCP, as an agent. Until 0.42.0 this test asked through
    // `semlith search`; 37d9422 ("one aggregation") keeps the CLI's and the
    // portal's own retrievals out of the agents' figures and out of the access
    // report, so a CLI query no longer reaches it by design.
    let formula = r#"=HYPERLINK("http://example.invalid","deploy")"#;
    let markup = "<img src=x onerror=alert(1)> deploy | nightly";
    let mut script = vec![serde_json::json!({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "clientInfo": { "name": "hostile-agent", "version": "1" }
        }
    })];
    for (id, query) in [formula, markup].into_iter().enumerate() {
        script.push(serde_json::json!({
            "jsonrpc": "2.0", "id": id + 1, "method": "tools/call",
            "params": { "name": "semlith_search", "arguments": { "query": query } }
        }));
    }
    let mut child = semlith(&home)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for message in &script {
            writeln!(stdin, "{message}").unwrap();
        }
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let csv = run(
        &home,
        // From 0.42.0 (3b6dbe1, typed reports) a report's CSV is a zip of one
        // file per table unless one table is named; the queries are in
        // `retrievals`.
        &[
            "report",
            "access",
            "--excerpts",
            "--format",
            "csv",
            "--table",
            "retrievals",
        ],
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
