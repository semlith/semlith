//! S03: an agent key reads by default; tools/call obeys its scope.

use super::*;

const WRITES: [&str; 3] = ["semlith_add", "semlith_index", "semlith_forget"];

fn call(tool: &str, args: Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    })
    .to_string()
}

fn args_for(tool: &str) -> Value {
    match tool {
        "semlith_add" => serde_json::json!({ "url": "https://example.com/" }),
        "semlith_index" => serde_json::json!({ "paths": ["/nonexistent-semlith-test"] }),
        _ => serde_json::json!({ "path": "/nonexistent-semlith-test/a.txt" }),
    }
}

fn refused(body: &str) -> bool {
    body.contains("this agent key is read-only")
}

/// A home with one empty store, so the daemon has a fleet to answer from.
fn home_with_store(dir: &Path) -> PathBuf {
    let home = dir.join("home");
    let corpus = dir.join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    home
}

#[test]
#[ignore = "opens a store, which loads the embedding model"]
fn s03_a_read_key_cannot_call_a_write_tool_over_http_or_stdio() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_with_store(dir.path());
    let daemon = Daemon::start(&home, &[("SEMLITH_MCP_TOOLS", "all")]);
    let key = daemon.key();
    let bearer = format!("Authorization: Bearer {key}\r\n");

    // HTTP /mcp with the key: every write tool refused, naming the fix.
    for tool in WRITES {
        let (status, body) = daemon.send("POST", "/mcp", &bearer, &call(tool, args_for(tool)));
        assert_eq!(status, 200, "{tool}: {body}");
        assert!(
            refused(&body),
            "{tool} was not refused to a read key: {body}"
        );
        assert!(
            body.contains("semlith key --scope write"),
            "{tool}: the refusal names no fix: {body}"
        );
    }
    // A read tool still answers.
    let (_, body) = daemon.send(
        "POST",
        "/mcp",
        &bearer,
        &call("semlith_stats", serde_json::json!({})),
    );
    assert!(!refused(&body), "a read tool was refused: {body}");

    // The listing under SEMLITH_MCP_TOOLS=all shows no write tool to a read key.
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    let (_, body) = daemon.send("POST", "/mcp", &bearer, list);
    assert!(body.contains("semlith_search"), "{body}");
    for tool in WRITES {
        assert!(
            !body.contains(&format!("\"{tool}\"")),
            "{tool} is listed to a read key"
        );
    }

    // stdio: `semlith mcp` forwards to this daemon and gets the same answer.
    let mut child = semlith(&home)
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        "{}",
        call("semlith_forget", args_for("semlith_forget"))
    )
    .unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(refused(&said), "stdio let a read key forget: {said}");

    // The refusals are ledger rows.
    let db = walk(&home.join(".semlith"))
        .into_iter()
        .find(|p| p.file_name().is_some_and(|f| f == "store.db"))
        .expect("a store");
    drop(daemon);
    let conn = rusqlite::Connection::open(db).unwrap();
    let rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM retrievals WHERE tool = 'refused'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        rows >= 4,
        "{rows} refusal rows, expected one per refused call"
    );
}

#[test]
#[ignore = "opens a store, which loads the embedding model"]
fn s03_a_write_scoped_key_may_write() {
    let dir = tempfile::tempdir().unwrap();
    let home = home_with_store(dir.path());
    let out = run(&home, &["key", "--scope", "write"], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let daemon = Daemon::start(&home, &[("SEMLITH_MCP_TOOLS", "all")]);
    let bearer = format!("Authorization: Bearer {}\r\n", daemon.key());
    let (_, body) = daemon.send(
        "POST",
        "/mcp",
        &bearer,
        &call("semlith_forget", args_for("semlith_forget")),
    );
    assert!(!refused(&body), "a write key was refused: {body}");
    let (_, body) = daemon.send(
        "POST",
        "/mcp",
        &bearer,
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
    );
    assert!(
        body.contains("\"semlith_forget\""),
        "the write key is not shown write tools"
    );
}
