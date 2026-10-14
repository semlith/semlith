//! S08: the llama-server key never appears in argv, and its port cannot be raced.
//!
//! The llama.cpp worker (`semlith __embed-worker llama <pack>`, on the CPU
//! device so no GPU is needed) is pointed at a pack whose `llama-server` is a
//! shell script: it records its argv and the `LLAMA_API_KEY` it was given,
//! says it could not bind — what the real server says when somebody else got
//! the port first — and waits for the test before it exits. When argv names a
//! port, the test plays the other local user: it binds that port, answers
//! `/health` with 200, and keeps whatever a request brings it.
//!
//! Unix, on the platforms a llama.cpp build is pinned for: the stub is a
//! shell script, and `ps` reading argv is the exposure this is about.

#[allow(unused_imports)]
use super::*;

#[cfg(all(
    unix,
    any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64")
    )
))]
#[test]
fn the_key_is_not_in_argv_and_a_raced_port_gets_nothing() {
    use std::net::TcpListener;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path();
    let pack = home.join("pack");
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(pack.join("granite-embedding-small-english-r2-f16.gguf"), "").unwrap();
    let server = pack.join("llama-server");
    std::fs::write(
        &server,
        "#!/bin/sh\n\
         dir=$(dirname \"$0\")\n\
         printf '%s' \"$LLAMA_API_KEY\" > \"$dir/key\"\n\
         printf '%s\\n' \"$@\" > \"$dir/argv.part\" && mv \"$dir/argv.part\" \"$dir/argv\"\n\
         echo \"couldn't bind HTTP server socket\" >&2\n\
         i=0\n\
         while [ ! -f \"$dir/go\" ] && [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done\n\
         exit 1\n",
    )
    .unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut worker = semlith(home)
        .args(["__embed-worker", "llama"])
        .arg(&pack)
        .env("SEMLITH_LLAMA_DEVICE", "cpu")
        .env_remove("LLAMA_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("the worker runs");

    assert!(
        until(Duration::from_secs(30), || pack.join("argv").exists()),
        "llama-server was never started"
    );
    let argv: Vec<String> = std::fs::read_to_string(pack.join("argv"))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    let port = argv
        .iter()
        .position(|a| a == "--port")
        .and_then(|at| argv.get(at + 1))
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(0);

    // The other local user: the port semlith picked and let go of, bound
    // first, answering `/health` and keeping every request's head.
    let heard = Arc::new(Mutex::new(Vec::<String>::new()));
    if port != 0
        && let Ok(listener) = TcpListener::bind(("127.0.0.1", port))
    {
        let kept = Arc::clone(&heard);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                        break;
                    }
                    head.push_str(&line);
                }
                let health = head.starts_with("GET /health");
                kept.lock().unwrap().push(head);
                let reply = if health {
                    "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"
                } else {
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        // Until the worker sends something past `/health`, or gives up.
        until(Duration::from_secs(30), || {
            heard
                .lock()
                .unwrap()
                .iter()
                .any(|h| !h.starts_with("GET /health"))
                || worker.try_wait().ok().flatten().is_some()
        });
    }
    std::fs::write(pack.join("go"), "").unwrap();
    let ended = until(Duration::from_secs(60), || {
        worker.try_wait().ok().flatten().is_some()
    });
    let _ = worker.kill();
    let _ = worker.wait();
    assert!(ended, "the worker did not end once its server had");

    // Every failure at once, so a red run shows each defect, not the first.
    let key = std::fs::read_to_string(pack.join("key")).unwrap_or_default();
    let mut wrong = Vec::new();
    if argv
        .iter()
        .any(|a| a.contains("api-key") || (!key.is_empty() && a.contains(&key)))
    {
        wrong.push(format!(
            "the key is in argv, where `ps` shows it to every account: {argv:?}"
        ));
    }
    let leaked: Vec<String> = heard
        .lock()
        .unwrap()
        .iter()
        .filter(|h| !h.starts_with("GET /health"))
        .cloned()
        .collect();
    if !leaked.is_empty() {
        wrong.push(format!(
            "a listener on the raced port {port} was trusted and sent: {leaked:?}"
        ));
    }
    if !(key.len() == 32 && key.chars().all(|c| c.is_ascii_hexdigit())) {
        wrong.push(format!(
            "the server was not handed its key in LLAMA_API_KEY: {key:?}"
        ));
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
