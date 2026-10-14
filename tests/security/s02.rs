//! S02: no lock is held across a tool call, and every outbound fetch has a
//! deadline.

use super::*;
use std::net::TcpListener;

/// A server that accepts, then sends a response one byte a second, forever:
/// the slowest thing a fetch can meet that is still technically answering.
fn drip() -> (u16, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut stream = stream;
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                for b in b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n"
                    .iter()
                    .cycle()
                {
                    if stream.write_all(&[*b]).is_err() {
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }
            });
        }
    });
    (port, handle)
}

fn call(id: u32, tool: &str, args: Value) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": tool, "arguments": args }
    })
    .to_string()
}

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s02_a_slow_add_and_an_index_do_not_stall_other_agents_or_the_portal() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    let more = dir.path().join("more");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::create_dir_all(&more).unwrap();
    std::fs::write(corpus.join("a.md"), "# Alpha\n\nThe answer is forty-two.\n").unwrap();
    for i in 0..300 {
        std::fs::write(
            more.join(format!("f{i}.md")),
            format!("# File {i}\n\nText {i}.\n"),
        )
        .unwrap();
    }
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success());
    // An old binary has no scope flag and refuses it; it needs none to write.
    let _ = run(&home, &["key", "--scope", "write"], &[]);

    let (port, _drip) = drip();
    let origin = format!("http://127.0.0.1:{port}");
    let daemon = std::sync::Arc::new(Daemon::start(
        &home,
        &[
            ("SEMLITH_ADD_ORIGIN", &origin),
            ("SEMLITH_ADD_ALLOW_PRIVATE", "1"),
            ("SEMLITH_MCP_TOOLS", "all"),
        ],
    ));
    let bearer = format!("Authorization: Bearer {}\r\n", daemon.key());
    // One search first, so the measured one is not the one that opens the
    // fleet and loads the query model: the claim is about waiting on other
    // calls, not about a cold start.
    let (status, body) = daemon.send(
        "POST",
        "/mcp",
        &bearer,
        &call(
            9,
            "semlith_search",
            serde_json::json!({ "query": "warm up" }),
        ),
    );
    assert_eq!(status, 200, "{body}");

    // One agent's add, held by the drip server.
    let (d, b, o) = (daemon.clone(), bearer.clone(), origin.clone());
    std::thread::spawn(move || {
        let _ = d.send(
            "POST",
            "/mcp",
            &b,
            &call(
                1,
                "semlith_add",
                serde_json::json!({ "url": format!("{o}/slow") }),
            ),
        );
    });
    // Another's index of three hundred files.
    let (d, b, m) = (daemon.clone(), bearer.clone(), more.clone());
    std::thread::spawn(move || {
        let _ = d.send(
            "POST",
            "/mcp",
            &b,
            &call(
                2,
                "semlith_index",
                serde_json::json!({ "path": [m.to_str().unwrap()] }),
            ),
        );
    });
    // Wait until the add has reached the drip server's socket rather than for
    // a guessed interval: a second request is in flight once the run shows.
    let busy = until(Duration::from_secs(30), || {
        daemon.api("/api/runs").to_string().contains("running")
            || daemon.api("/api/agents")["connected"].as_u64().unwrap_or(0) > 0
    });
    assert!(busy, "nothing in flight to stall on");

    let started = Instant::now();
    let (status, _) = daemon.send(
        "GET",
        "/api/about",
        &format!("Semlith-Token: {}\r\n", daemon.token),
        "",
    );
    assert_eq!(status, 200);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the portal took {:?}",
        started.elapsed()
    );

    let started = Instant::now();
    let (status, body) = daemon.send(
        "POST",
        "/mcp",
        &bearer,
        &call(
            3,
            "semlith_search",
            serde_json::json!({ "query": "the answer" }),
        ),
    );
    assert_eq!(status, 200, "{body}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a second session's search took {:?} behind the add and the index",
        started.elapsed()
    );
}
