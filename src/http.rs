//! A synchronous HTTP/1.1 server, just large enough to serve the portal.
//!
//! Nothing here is a general-purpose web server and nothing here should become
//! one. It answers requests from one browser on one machine over the loopback
//! interface, which is why it is a few hundred lines of `std::net` rather than
//! a dependency: every HTTP crate that does chunked responses and a thread pool
//! brings a tree semlith would then be shipping to every user of a local search
//! tool, and the parts of HTTP this needs are the parts that fit on a page.
//!
//! Three rules are enforced here, before any handler sees a request, because
//! they are the product's privacy claim and a claim enforced in one place is
//! one that can be read in one place:
//!
//! - **Loopback only.** [`Server::bind`] binds `127.0.0.1` and there is no flag
//!   to change it.
//! - **A per-run token.** Generated at start, printed once in the URL, and
//!   required on every request as a `SameSite=Strict` cookie. A request without
//!   it gets 401 and an empty body — not a login page, not an error object,
//!   nothing that tells a prober what is here.
//! - **A `Host` header that is localhost.** A page on another origin cannot
//!   read a cross-origin response, but it can *send* the request, and a DNS
//!   name that resolves to 127.0.0.1 would otherwise reach this server with the
//!   browser's cookies attached. Only `localhost` and `127.0.0.1`, with an
//!   optional port, are answered.
//!
//! Every response also carries a Content-Security-Policy that allows only
//! `'self'`, and no CORS header is ever emitted — there is no origin this
//! server wants to be readable from.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// The port `semlith start` listens on unless told otherwise.
///
/// 7365 spells SEML on a phone keypad, is registered to nothing in the IANA
/// list, sits below every OS ephemeral range, and is nowhere near the ports
/// local development already uses — 3000, 4200, 5000, 5173, 8000, 8080, 8888,
/// 9000, and the usual database and model-server ports. A stable port is what
/// lets the URL be a bookmark.
pub const DEFAULT_PORT: u16 = 7365;

/// Overrides [`DEFAULT_PORT`].
pub const PORT_ENV: &str = "SEMLITH_PORT";

/// The cookie the token travels in.
pub const TOKEN_COOKIE: &str = "semlith_token";

/// Threads answering requests.
///
/// A browser opens up to six connections to one origin, and this server closes
/// each connection after its response, so eight is enough for one portal with
/// a long-running index stream on one of them and nothing queueing behind it.
const WORKERS: usize = 8;

/// The largest request line and header block accepted. A browser's is under
/// 2 KiB; anything near this is not a browser.
const MAX_HEADERS: usize = 16 * 1024;

/// The largest request body accepted. The only bodies this server takes are
/// small JSON objects naming paths.
const MAX_BODY: usize = 1024 * 1024;

/// How long a connection may sit idle mid-request before it is dropped, so a
/// half-open socket cannot hold a worker forever.
const IO_TIMEOUT: Duration = Duration::from_secs(30);

/// Why a request was refused, for the one-line-per-class count the daemon logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Refusal {
    /// No token, or the wrong one.
    Unauthorized,
    /// A `Host` header that is not loopback.
    ForeignHost,
    /// Malformed, oversized, or a method this server does not answer.
    Malformed,
}

impl Refusal {
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Unauthorized => "unauthorized",
            Refusal::ForeignHost => "foreign host",
            Refusal::Malformed => "malformed",
        }
    }
}

/// One parsed request.
pub struct Request {
    pub method: String,
    /// Path with the query string removed, percent-decoded.
    pub path: String,
    pub query: BTreeMap<String, String>,
    /// Header names lowercased, so a lookup never depends on a client's casing.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn query(&self, key: &str) -> Option<&str> {
        self.query.get(key).map(String::as_str)
    }

    /// Every value a repeated query key has, for `?lang=rust&lang=go`.
    ///
    /// The map holds them joined by `\u{1}`, which cannot appear in a
    /// percent-decoded query value that a browser produced.
    pub fn query_all(&self, key: &str) -> Vec<String> {
        match self.query.get(key) {
            Some(v) => v.split('\u{1}').map(str::to_string).collect(),
            None => Vec::new(),
        }
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    pub fn json(&self) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

/// What a handler writes a streaming body through.
///
/// Each [`Chunks::send`] is one HTTP chunk, flushed immediately, which is what
/// makes an index run's progress visible while it is still running rather than
/// all at once when it ends.
pub struct Chunks<'a> {
    out: &'a mut dyn Write,
}

impl Chunks<'_> {
    pub fn send(&mut self, line: &str) -> std::io::Result<()> {
        write!(self.out, "{:x}\r\n", line.len())?;
        self.out.write_all(line.as_bytes())?;
        self.out.write_all(b"\r\n")?;
        self.out.flush()
    }
}

type Streamer = Box<dyn FnOnce(&mut Chunks) -> std::io::Result<()> + Send>;

enum Body {
    Bytes(Vec<u8>),
    /// Sent with `Transfer-Encoding: chunked`, for a response whose length is
    /// not known when it starts.
    Stream(Streamer),
}

pub struct Response {
    status: u16,
    content_type: &'static str,
    extra: Vec<(String, String)>,
    body: Body,
}

impl Response {
    pub fn new(status: u16, content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status,
            content_type,
            extra: Vec::new(),
            body: Body::Bytes(body),
        }
    }

    pub fn json(value: &serde_json::Value) -> Self {
        Self::new(
            200,
            "application/json; charset=utf-8",
            serde_json::to_vec(value).unwrap_or_else(|_| b"{}".to_vec()),
        )
    }

    /// A refusal a handler decided on: still JSON, because the portal shows it.
    pub fn error(status: u16, message: &str) -> Self {
        Self::new(
            status,
            "application/json; charset=utf-8",
            serde_json::to_vec(&serde_json::json!({ "error": message }))
                .unwrap_or_else(|_| b"{}".to_vec()),
        )
    }

    pub fn asset(content_type: &'static str, bytes: &'static [u8]) -> Self {
        Self::new(200, content_type, bytes.to_vec())
    }

    /// A response whose body is produced as it goes.
    pub fn stream(f: impl FnOnce(&mut Chunks) -> std::io::Result<()> + Send + 'static) -> Self {
        Self {
            status: 200,
            content_type: "application/x-ndjson; charset=utf-8",
            extra: Vec::new(),
            body: Body::Stream(Box::new(f)),
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.extra.push((name.to_string(), value.into()));
        self
    }

    /// Hand the browser the token, so the URL is the only place it appears.
    ///
    /// `SameSite=Strict` means a page on any other origin cannot cause the
    /// browser to attach it, `HttpOnly` keeps it out of reach of script, and
    /// `Path=/` covers every route. No `Secure`: this is `http://127.0.0.1`
    /// by design, and `Secure` would stop the cookie being set at all.
    pub fn with_token(self, token: &str) -> Self {
        self.header(
            "Set-Cookie",
            format!("{TOKEN_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict"),
        )
    }
}

/// A handler: everything the daemon serves, behind one function.
pub type Handler = Arc<dyn Fn(&Request) -> Response + Send + Sync>;

pub struct Server {
    listener: TcpListener,
    port: u16,
    /// Behind a lock so the Privacy page's Rotate button can replace it while
    /// the server is running.
    token: Arc<Mutex<String>>,
}

impl Server {
    /// Bind the loopback interface, or explain what is already on the port.
    ///
    /// Never falls back to another port. A URL that moves is not a bookmark,
    /// and from 0.11.0 it is also a client's MCP endpoint — so a taken port is
    /// an error naming the flag, not a silent reassignment.
    pub fn bind(port: u16) -> Result<Self> {
        let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
        let listener = TcpListener::bind(addr).with_context(|| {
            format!(
                "port {port} is already in use — stop whatever is listening on it, \
                 or choose another with --port or {PORT_ENV}"
            )
        })?;
        let port = listener.local_addr().map(|a| a.port()).unwrap_or(port);
        Ok(Self {
            listener,
            port,
            token: Arc::new(Mutex::new(new_token())),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> String {
        self.token.lock().expect("the token lock").clone()
    }

    /// Invalidate the current token and return the new one.
    pub fn rotate(&self) -> String {
        let fresh = new_token();
        *self.token.lock().expect("the token lock") = fresh.clone();
        fresh
    }

    /// The URL to print. The token appears here and nowhere else.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/?token={}", self.port, self.token())
    }

    /// Serve until `stop` is set, calling `refused` once per refused request.
    ///
    /// `stop` is checked between accepts, so shutdown waits at most one accept
    /// timeout rather than for the next request to arrive.
    pub fn serve(
        &self,
        handler: Handler,
        stop: &AtomicBool,
        refused: impl Fn(Refusal) + Send + Sync + 'static,
    ) -> Result<()> {
        self.listener
            .set_nonblocking(true)
            .context("putting the listener in non-blocking mode")?;

        let (tx, rx) = mpsc::channel::<TcpStream>();
        let rx = Arc::new(Mutex::new(rx));
        let refused = Arc::new(refused);

        let mut workers = Vec::with_capacity(WORKERS);
        for _ in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let handler = Arc::clone(&handler);
            let token = Arc::clone(&self.token);
            let refused = Arc::clone(&refused);
            workers.push(std::thread::spawn(move || {
                loop {
                    // The receiver is behind a mutex only for the recv; a
                    // worker that took a connection does not hold up the next.
                    let next = {
                        let guard = rx.lock().expect("the queue lock");
                        guard.recv()
                    };
                    let Ok(stream) = next else { return };
                    let want = token.lock().expect("the token lock").clone();
                    if let Some(class) = answer(stream, &handler, &want) {
                        refused(class);
                    }
                }
            }));
        }

        while !stop.load(Ordering::Relaxed) {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                    if tx.send(stream).is_err() {
                        break;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e).context("accepting a connection"),
            }
        }

        // Dropping the sender ends every worker's `recv`, so shutdown finishes
        // the requests in flight and starts no more.
        drop(tx);
        for worker in workers {
            let _ = worker.join();
        }
        Ok(())
    }
}

/// Answer one connection. Returns why it was refused, if it was.
fn answer(mut stream: TcpStream, handler: &Handler, want: &str) -> Option<Refusal> {
    let request = match read_request(&mut stream) {
        Ok(Some(r)) => r,
        Ok(None) => return None,
        Err(_) => {
            let _ = write_response(&mut stream, Response::new(400, "text/plain", Vec::new()));
            return Some(Refusal::Malformed);
        }
    };

    // Host first: a request from a foreign origin is refused before the token
    // is even compared, so a wrong guess and a right one cost the same.
    if !loopback_host(request.header("host")) {
        let _ = write_response(&mut stream, Response::new(400, "text/plain", Vec::new()));
        return Some(Refusal::ForeignHost);
    }

    let from_cookie = cookie(request.header("cookie"), TOKEN_COOKIE);
    let by_cookie = from_cookie.as_deref().is_some_and(|t| same(t, want));
    // The URL the daemon prints carries the token in the query, and that is
    // the one request that may arrive without the cookie — it is what sets it.
    let by_query = request.query("token").is_some_and(|t| same(t, want));

    if !by_cookie && !by_query {
        // Empty body on purpose: a prober learns that something refused it and
        // nothing else.
        let _ = write_response(&mut stream, Response::new(401, "text/plain", Vec::new()));
        return Some(Refusal::Unauthorized);
    }

    // HEAD is GET without the body. Answered by running the handler and
    // dropping what it produced, so a probe sees the real status and the real
    // headers rather than the 404 an unhandled method would give it.
    let head_only = request.method == "HEAD";
    let request = if head_only {
        Request {
            method: "GET".to_string(),
            ..request
        }
    } else {
        request
    };

    let mut response = handler(&request);
    if by_query && !by_cookie {
        response = response.with_token(want);
    }
    if head_only {
        response.body = Body::Bytes(Vec::new());
    }
    let _ = write_response(&mut stream, response);
    None
}

/// Whether `Host` is this machine talking to itself.
///
/// A missing header is refused: HTTP/1.1 requires one, and the clients that
/// omit it are not browsers.
fn loopback_host(host: Option<&str>) -> bool {
    let Some(host) = host else { return false };
    let name = host.rsplit_once(':').map_or(host, |(n, _)| n);
    let name = name.trim_start_matches('[').trim_end_matches(']');
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "::1"
}

/// One cookie's value out of a `Cookie` header.
fn cookie(header: Option<&str>, name: &str) -> Option<String> {
    header?.split(';').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key.trim() == name).then(|| value.trim().to_string())
    })
}

/// Compare in time that does not depend on where the first wrong byte is.
///
/// The token is a secret and this is the check that guards it. The difference
/// is unmeasurable across a loopback socket, but a constant-time compare costs
/// nothing and removes the question.
fn same(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn read_request(stream: &mut TcpStream) -> Result<Option<Request>> {
    let mut reader = BufReader::new(stream.try_clone()?);

    let mut head = String::new();
    let mut read = 0;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            // The peer closed before sending anything: a browser probing a
            // connection it then did not use. Not a refusal, not an error.
            return Ok(head.is_empty().then_some(None).flatten());
        }
        read += n;
        anyhow::ensure!(read <= MAX_HEADERS, "request headers too large");
        if line == "\r\n" || line == "\n" {
            break;
        }
        head.push_str(&line);
    }

    let mut lines = head.lines();
    let start = lines.next().context("empty request")?;
    let mut parts = start.split_whitespace();
    let method = parts.next().context("no method")?.to_string();
    let target = parts.next().context("no target")?.to_string();

    let mut headers = BTreeMap::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let (raw_path, raw_query) = match target.split_once('?') {
        Some((p, q)) => (p, q),
        None => (target.as_str(), ""),
    };

    let mut body = Vec::new();
    if let Some(len) = headers.get("content-length").and_then(|v| v.parse().ok()) {
        let len: usize = len;
        anyhow::ensure!(len <= MAX_BODY, "request body too large");
        body.resize(len, 0);
        reader.read_exact(&mut body)?;
    }

    Ok(Some(Request {
        method,
        path: percent_decode(raw_path),
        query: parse_query(raw_query),
        headers,
        body,
    }))
}

/// `a=1&b=2` to a map, with a repeated key's values joined by `\u{1}`.
fn parse_query(raw: &str) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for pair in raw.split('&').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = percent_decode(&key.replace('+', " "));
        let value = percent_decode(&value.replace('+', " "));
        out.entry(key)
            .and_modify(|existing| {
                existing.push('\u{1}');
                existing.push_str(&value);
            })
            .or_insert(value);
    }
    out
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_response(stream: &mut TcpStream, response: Response) -> std::io::Result<()> {
    let Response {
        status,
        content_type,
        extra,
        body,
    } = response;

    let mut head = format!("HTTP/1.1 {status} {}\r\n", reason(status));
    head.push_str(&format!("Content-Type: {content_type}\r\n"));
    // Everything the page loads is served by this server, nothing may frame
    // it, and no form may post anywhere else. There is no origin this server
    // wants to be readable from, so there is no CORS header here or anywhere.
    head.push_str(
        "Content-Security-Policy: default-src 'self'; \
         img-src 'self' data:; \
         style-src 'self'; \
         script-src 'self'; \
         font-src 'self'; \
         connect-src 'self'; \
         form-action 'none'; \
         frame-ancestors 'none'; \
         base-uri 'none'\r\n",
    );
    head.push_str("X-Content-Type-Options: nosniff\r\n");
    head.push_str("Referrer-Policy: no-referrer\r\n");
    // The token lives in the first URL. Nothing here may be stored by a proxy
    // that does not exist, but a browser's back-forward cache holding a
    // rotated token is a real thing to avoid.
    head.push_str("Cache-Control: no-store\r\n");
    head.push_str("Connection: close\r\n");
    for (name, value) in &extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }

    match body {
        Body::Bytes(bytes) => {
            head.push_str(&format!("Content-Length: {}\r\n\r\n", bytes.len()));
            stream.write_all(head.as_bytes())?;
            stream.write_all(&bytes)?;
            stream.flush()
        }
        Body::Stream(f) => {
            head.push_str("Transfer-Encoding: chunked\r\n\r\n");
            stream.write_all(head.as_bytes())?;
            stream.flush()?;
            let result = {
                let mut chunks = Chunks { out: stream };
                f(&mut chunks)
            };
            // The terminating chunk goes out whether or not the producer
            // failed: a client reading a stream that simply stops has no way
            // to tell a finished run from a dropped connection.
            stream.write_all(b"0\r\n\r\n")?;
            stream.flush()?;
            result
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

/// A 256-bit token, hex encoded.
///
/// The bytes come from blake3 over the machine's clock, this process's id and
/// the address of a fresh heap allocation — which is already a dependency, is
/// not a predictable sequence, and needs no RNG crate. The token guards a
/// loopback socket for one process's lifetime; it is not a long-lived secret.
fn new_token() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let entropy = Box::new(0u8);
    let address = &*entropy as *const u8 as usize;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&now.to_le_bytes());
    hasher.update(&std::process::id().to_le_bytes());
    hasher.update(&address.to_le_bytes());
    hasher.update(&std::time::Instant::now().elapsed().as_nanos().to_le_bytes());
    hasher.finalize().to_hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Host check is what stops a page on another origin reaching this
    /// server with the browser's cookies attached, so the names it accepts are
    /// worth stating exactly.
    #[test]
    fn only_loopback_hosts_are_answered() {
        assert!(loopback_host(Some("localhost")));
        assert!(loopback_host(Some("localhost:7365")));
        assert!(loopback_host(Some("127.0.0.1:7365")));
        assert!(loopback_host(Some("LOCALHOST:7365")));
        assert!(loopback_host(Some("[::1]:7365")));

        assert!(!loopback_host(Some("evil.example")));
        assert!(!loopback_host(Some("evil.example:7365")));
        // A name that merely ends in localhost is a different host.
        assert!(!loopback_host(Some("notlocalhost")));
        assert!(!loopback_host(Some("localhost.evil.example")));
        // 127.0.0.1 by another spelling is still a foreign Host header: the
        // check is on the name the browser sent, not on where it resolves.
        assert!(!loopback_host(Some("127.1")));
        assert!(!loopback_host(None));
    }

    #[test]
    fn a_cookie_is_read_out_of_a_header_with_others_in_it() {
        let header = Some("theme=dark; semlith_token=abc123; other=1");
        assert_eq!(cookie(header, TOKEN_COOKIE), Some("abc123".to_string()));
        assert_eq!(cookie(Some("theme=dark"), TOKEN_COOKIE), None);
        assert_eq!(cookie(None, TOKEN_COOKIE), None);
        // A token that is a prefix of the cookie name must not match.
        assert_eq!(cookie(Some("semlith_tokens=x"), TOKEN_COOKIE), None);
    }

    #[test]
    fn tokens_compare_by_value_and_by_length() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
        assert!(!same("", "a"));
    }

    /// `?lang=rust&lang=go` is how the portal sends a repeated filter, and a
    /// map that kept only the last one would silently narrow the search.
    #[test]
    fn a_repeated_query_key_keeps_every_value() {
        let q = parse_query("lang=rust&lang=go&k=5");
        let request = Request {
            method: "GET".into(),
            path: "/".into(),
            query: q,
            headers: BTreeMap::new(),
            body: Vec::new(),
        };
        assert_eq!(request.query_all("lang"), vec!["rust", "go"]);
        assert_eq!(request.query("k"), Some("5"));
        assert!(request.query_all("ext").is_empty());
    }

    #[test]
    fn percent_and_plus_both_decode() {
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%2Fsrc%2Flib.rs"), "/src/lib.rs");
        // A stray percent is left alone rather than eating the next bytes.
        assert_eq!(percent_decode("100%"), "100%");
        let q = parse_query("query=Fleet%3A%3Awritable+opens");
        assert_eq!(q.get("query").unwrap(), "Fleet::writable opens");
    }

    /// Two tokens from one process must differ, or a rotate would hand back
    /// the token it was asked to invalidate.
    #[test]
    fn every_token_is_new() {
        let a = new_token();
        let b = new_token();
        assert_ne!(a, b);
        assert_eq!(a.len(), 64, "256 bits, hex encoded");
    }
}
