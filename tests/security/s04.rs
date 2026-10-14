//! S04: cloud login never opens a URL the server picked unless it is on the configured host.
//!
//! A stub cloud on loopback answers the device grant with a planted
//! `verification_uri_complete`, then denies the code at the first poll so the
//! login ends. On macOS and Linux `cloud login` runs under `script`, so it has
//! the terminal it opens a browser for, with `open` and `xdg-open` on its PATH
//! replaced by a stub that records its arguments. Windows has no `script`; a
//! debug build there records the opener's command line to the file
//! `SEMLITH_TEST_OPENED` names instead of running it.

#[allow(unused_imports)]
use super::*;
use std::net::TcpListener;

/// A cloud host on loopback whose device grant names `page` and `complete`.
///
/// It answers `authorization_pending` until the opener stub under `home` has
/// recorded something, then denies the code so the login ends. A login that
/// ended at once would take the terminal down with it, and the opener it had
/// just started, before the stub wrote a line. Nothing opened at all is the
/// outcome being proved, so that wait is bounded.
fn stub_host(home: &Path, page: &str, complete: &str, code: &str) -> String {
    let opened = home.join("opened.log");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let host = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let grant = serde_json::json!({
        "device_code": "d",
        "user_code": code,
        "verification_uri": page.replace("{host}", &host),
        "verification_uri_complete": complete.replace("{host}", &host),
        "interval": 0,
        "expires_in": 60,
    })
    .to_string();
    std::thread::spawn(move || {
        let mut asked = Instant::now();
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            let _ = reader.read_line(&mut line);
            let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
            let mut length = 0;
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header.trim().is_empty() {
                    break;
                }
                if let Some((k, v)) = header.split_once(':')
                    && k.eq_ignore_ascii_case("content-length")
                {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let decided = opened.exists() || asked.elapsed() > Duration::from_secs(5);
            let (status, reply) = if path == "/v1/cli/authorize" {
                asked = Instant::now();
                ("200 OK", grant.clone())
            } else if !decided {
                (
                    "400 Bad Request",
                    r#"{"error":{"code":"authorization_pending","message":"Waiting."}}"#
                        .to_string(),
                )
            } else {
                (
                    "400 Bad Request",
                    r#"{"error":{"code":"access_denied","message":"Denied."}}"#.to_string(),
                )
            };
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                reply.len()
            );
        }
    });
    host
}

/// Run `cloud login --host <host>` as a person at a terminal would. Returns
/// what it printed and what it handed the OS's opener.
fn login(home: &Path, host: &str) -> (String, String) {
    let opened = home.join("opened.log");
    let _ = std::fs::remove_file(&opened);
    let mut cmd;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let stubs = home.join("bin");
        std::fs::create_dir_all(&stubs).unwrap();
        for name in ["open", "xdg-open"] {
            let stub = stubs.join(name);
            std::fs::write(
                &stub,
                format!(
                    "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n",
                    opened.display()
                ),
            )
            .unwrap();
            std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bin = bin().display().to_string();
        let args = [bin.as_str(), "cloud", "login", "--host", host];
        cmd = Command::new("script");
        if cfg!(target_os = "macos") {
            cmd.args(["-q", "/dev/null"]).args(args);
        } else {
            let line: Vec<String> = args.iter().map(|a| format!("'{a}'")).collect();
            cmd.args(["-qec", &line.join(" "), "/dev/null"]);
        }
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![stubs];
        paths.extend(std::env::split_paths(&path));
        cmd.env("PATH", std::env::join_paths(paths).unwrap());
    }
    #[cfg(windows)]
    {
        cmd = Command::new(bin());
        cmd.args(["cloud", "login", "--host", host])
            .env("SEMLITH_TEST_OPENED", &opened);
    }
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("SEMLITH_HOME", home.join(".semlith"))
        .env_remove("SEMLITH_AIRGAP")
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("cloud login runs");
    let ended = until(Duration::from_secs(60), || {
        child.try_wait().ok().flatten().is_some()
    });
    if !ended {
        let _ = child.kill();
    }
    let out = child.wait_with_output().unwrap();
    assert!(ended, "cloud login did not end");
    let printed =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    (
        printed,
        std::fs::read_to_string(&opened).unwrap_or_default(),
    )
}

/// The host's own page is opened — the control that proves the stub opener is
/// reached at all, so the refusals below are not vacuous.
#[test]
fn the_hosts_own_page_is_opened() {
    let scratch = tempfile::tempdir().unwrap();
    let host = stub_host(
        scratch.path(),
        "{host}/cli",
        "{host}/cli?code=WDJB-MJHT",
        "WDJB-MJHT",
    );
    let (printed, opened) = login(scratch.path(), &host);
    assert!(
        opened.contains(&format!("{host}/cli?code=WDJB-MJHT")),
        "the host's own page was not opened; opener got {opened:?}; printed {printed}"
    );
    if cfg!(windows) {
        assert!(
            opened.starts_with("rundll32 url.dll,FileProtocolHandler "),
            "{opened}"
        );
    }
}

/// A page the server planted is printed, never opened: another origin, a
/// local app, or the host's own origin carrying a shell metacharacter.
#[test]
fn a_planted_page_is_printed_not_opened() {
    for planted in [
        "https://evil.example/cli?code=A",
        "/System/Applications/Calculator.app",
        "{host}/cli?code=A&calc",
    ] {
        let scratch = tempfile::tempdir().unwrap();
        let host = stub_host(scratch.path(), "{host}/cli", planted, "WDJB-MJHT");
        let planted = planted.replace("{host}", &host);
        let (printed, opened) = login(scratch.path(), &host);
        assert!(
            opened.is_empty(),
            "{planted} was handed to the opener: {opened:?}"
        );
        assert!(
            printed.contains(&planted),
            "{planted} was not printed for the person to see: {printed}"
        );
    }
}

/// Escape sequences in what the host sends never reach the terminal.
#[test]
fn the_hosts_strings_carry_no_terminal_escapes() {
    let scratch = tempfile::tempdir().unwrap();
    let host = stub_host(
        scratch.path(),
        "{host}/cli\u{1b}]0;owned\u{7}",
        "{host}/cli",
        "WDJB\u{1b}[2J-MJHT",
    );
    let (printed, _) = login(scratch.path(), &host);
    assert!(
        printed.contains("WDJB"),
        "the code was not printed: {printed}"
    );
    for escape in ["\u{1b}]0;owned", "\u{1b}[2J"] {
        assert!(
            !printed.contains(escape),
            "{escape:?} reached the terminal: {printed:?}"
        );
    }
}
