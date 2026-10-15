//! S14: a daemon whose stdout is a log file writes no session token there,
//! and a token an older daemon left in the service log is scrubbed.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
#[test]
fn s14_a_daemon_logging_to_a_file_leaves_no_token_in_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let logs = home.join(".semlith").join("logs");
    std::fs::create_dir_all(&logs).unwrap();
    let log = logs.join("daemon.log");
    // What 0.40.0 left behind on every service start.
    std::fs::write(&log, "http://127.0.0.1:7365/?token=zqoldtoken123\n").unwrap();

    // A store, so the daemon has somewhere to write its discovery file.
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&corpus).unwrap();
    let made = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(
        made.status.success(),
        "{}",
        String::from_utf8_lossy(&made.stderr)
    );

    let out = std::fs::OpenOptions::new().append(true).open(&log).unwrap();
    let mut child = semlith(&home)
        .args(["start", "--port", "0"])
        .stdout(out)
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let token = || -> Option<String> {
        walk(&home)
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n == "daemon.json"))
            .find_map(|p| {
                let v: Value = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
                v["token"].as_str().map(str::to_string)
            })
    };
    let ready = until(Duration::from_secs(60), || {
        token().is_some() && std::fs::read_to_string(&log).is_ok_and(|t| t.contains("127.0.0.1:"))
    });
    let token = token();
    let _ = child.kill();
    let _ = child.wait();
    assert!(ready, "the daemon never came up");
    let token = token.unwrap();

    let text = std::fs::read_to_string(&log).unwrap();
    assert!(
        !text.contains("zqoldtoken123"),
        "the old token is still in the log:\n{text}"
    );
    for path in walk(&home) {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name == "daemon.json" || name == "agent.key" {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap_or_default();
        assert!(
            !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
            "the session token is in {}",
            path.display()
        );
    }
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode & 0o077, 0, "daemon.log is {mode:o}");
}
