//! S23: the small surface issues — a repeated security header, the status
//! codes, a session id that walks out of its folder.

#[allow(unused_imports)]
use super::*;

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s23_the_daemon_refuses_repeated_headers_and_says_404_and_400() {
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
    let daemon = Daemon::start(&home, &[]);

    // Host twice, the bad one first: a proxy might honour either.
    let (status, _) = daemon.send(
        "GET",
        "/api/stores",
        &format!("Host: evil.example\r\nSemlith-Token: {}\r\n", daemon.token),
        "",
    );
    assert_eq!(status, 400, "a repeated Host header was answered");

    let token = format!("Semlith-Token: {}\r\n", daemon.token);
    let (status, body) = daemon.send("GET", "/api/search?query=deploy&store=nope", &token, "");
    assert_eq!(status, 404, "a missing store: {body}");
    let (status, body) = daemon.send("GET", "/api/report?kind=nope", &token, "");
    assert_eq!(status, 400, "an unknown report kind: {body}");

    // Every status line names its own status.
    let mut stream = TcpStream::connect(("127.0.0.1", daemon.port)).unwrap();
    stream
        .write_all(
            format!(
                "POST /api/index HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{token}\
                 Origin: http://127.0.0.1:{other}\r\nSec-Fetch-Site: same-site\r\n\
                 Content-Type: application/json\r\nContent-Length: 2\r\n\r\n{{}}",
                port = daemon.port,
                other = daemon.port + 1,
            )
            .as_bytes(),
        )
        .unwrap();
    let mut first = String::new();
    BufReader::new(stream).read_line(&mut first).unwrap();
    assert!(first.starts_with("HTTP/1.1 403 Forbidden"), "{first:?}");
}
