//! A stand-in Semlith Cloud host: a loopback HTTP/1.1 server that answers each
//! request from a closure and remembers every request it saw, so a test can
//! assert what left the binary as well as what came back.
//!
//! Shared by `tests/cloud.rs` and `src/cloud.rs`'s unit tests (through
//! `#[path]`), so there is one stub rather than two that drift.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

/// One request as the stub received it.
#[derive(Debug, Clone)]
pub struct Seen {
    pub method: String,
    /// The path with its query string.
    pub path: String,
    /// Header names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Seen {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// What the stub answers: status, extra headers, body.
pub type Answer = (u16, Vec<(String, String)>, Vec<u8>);

pub fn json(status: u16, value: serde_json::Value) -> Answer {
    (
        status,
        vec![
            ("Content-Type".into(), "application/json".into()),
            ("Semlith-Cloud-Version".into(), "0.3.0".into()),
        ],
        serde_json::to_vec(&value).unwrap(),
    )
}

pub fn error(status: u16, code: &str, message: &str) -> Answer {
    json(
        status,
        serde_json::json!({ "error": { "code": code, "message": message } }),
    )
}

pub struct Stub {
    /// `http://127.0.0.1:<port>`, no trailing slash.
    pub url: String,
    pub seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    pub fn start(answer: impl Fn(&Seen) -> Answer + Send + Sync + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
        let log = Arc::clone(&seen);
        let answer = Arc::new(answer);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let log = Arc::clone(&log);
                let answer = Arc::clone(&answer);
                std::thread::spawn(move || {
                    let _ = serve(stream, &log, &*answer);
                });
            }
        });
        Stub { url, seen }
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    /// Every request whose path starts with `prefix`.
    pub fn to(&self, prefix: &str) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.path.starts_with(prefix))
            .collect()
    }
}

fn serve(
    stream: std::net::TcpStream,
    log: &Mutex<Vec<Seen>>,
    answer: &(dyn Fn(&Seen) -> Answer + Send + Sync),
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h)? == 0 || h.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok());
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.contains("chunked"));
    let mut body = Vec::new();
    if let Some(n) = length {
        body.resize(n, 0);
        reader.read_exact(&mut body)?;
    } else if chunked {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size)?;
            let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            let mut chunk = vec![0; n + 2];
            reader.read_exact(&mut chunk)?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
    let seen = Seen {
        method,
        path,
        headers,
        body,
    };
    log.lock().unwrap().push(seen.clone());
    let (status, extra, out) = answer(&seen);
    let mut head = format!(
        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n",
        out.len()
    );
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let mut stream = stream;
    stream.write_all(head.as_bytes())?;
    stream.write_all(&out)?;
    stream.flush()
}
