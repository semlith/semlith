//! `semlith add`, against a server that is not the internet.
//!
//! Almost every test here is a refusal, and that ratio is the point. `add` is
//! the first outbound connection semlith makes that is not the model download
//! or an upgrade, so each rule it holds to — https only, five redirects, 32
//! MiB, a type a reader handles, no credential, nothing outside the downloads
//! directory — is a hole in the "nothing leaves this machine" claim if it is
//! wrong. A rule that is only asserted in a doc comment is not a rule.
//!
//! The fixture binds 127.0.0.1 on a port the OS picks and counts its requests,
//! because "it refused" and "it refused after asking" are different claims and
//! only the first one is worth anything under `--airgap`.
//!
//! `SEMLITH_ADD_ORIGIN` names the one plain-HTTP origin that may be fetched, so
//! the fixture does not have to speak TLS to test rules that have nothing to do
//! with TLS. It exists for this file and is deliberately not in
//! `docs/compatibility.md`.

#![cfg(unix)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// What the fixture answers for one path.
#[derive(Clone)]
struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
    /// Sent as `Location` when this is a redirect.
    location: Option<String>,
}

impl Reply {
    fn ok(content_type: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type,
            body: body.into(),
            location: None,
        }
    }

    fn redirect(to: impl Into<String>) -> Self {
        Self {
            status: 302,
            content_type: "text/plain",
            body: Vec::new(),
            location: Some(to.into()),
        }
    }

    fn status(status: u16) -> Self {
        Self {
            status,
            content_type: "text/plain",
            body: b"no".to_vec(),
            location: None,
        }
    }
}

struct Fixture {
    port: u16,
    /// Requests per path rather than a single total. The tests share one
    /// server — the allowed-origin environment variable is process-wide, so a
    /// server per test would have them overwriting each other's port — and a
    /// shared total would be whatever the other tests happened to be doing.
    hits: Arc<Mutex<HashMap<String, usize>>>,
}

impl Fixture {
    /// A one-thread HTTP/1.1 server over a fixed route table, hand-rolled for
    /// the same reason `tests/upgrade.rs`'s is: a test of a loopback fetch
    /// should not need a framework either.
    fn serve(routes: HashMap<String, Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
        let port = listener.local_addr().unwrap().port();
        let hits: Arc<Mutex<HashMap<String, usize>>> = Arc::new(Mutex::new(HashMap::new()));
        let counter = Arc::clone(&hits);
        let routes = Arc::new(Mutex::new(routes));

        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let routes = Arc::clone(&routes);
                let counter = Arc::clone(&counter);
                // One connection at a time is enough: every fetch here is
                // sequential within its test, and a thread per connection would
                // only add a way for the counts to interleave.
                answer(stream, &routes.lock().unwrap(), &counter);
            }
        });

        Self { port, hits }
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin())
    }

    /// How many times one path has been asked for.
    fn requests(&self, path: &str) -> usize {
        *self.hits.lock().unwrap().get(path).unwrap_or(&0)
    }
}

fn answer(
    mut stream: TcpStream,
    routes: &HashMap<String, Reply>,
    hits: &Arc<Mutex<HashMap<String, usize>>>,
) {
    let Ok(peer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(peer);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
    *hits.lock().unwrap().entry(path.clone()).or_insert(0) += 1;

    // Drain the request before answering. Replying and closing while the client
    // is still writing its headers makes the peer see a reset instead of the
    // response — the same failure `tests/upgrade.rs` records.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    let reply = routes
        .get(&path)
        .cloned()
        .unwrap_or_else(|| Reply::status(404));
    let mut head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    if let Some(location) = &reply.location {
        head.push_str(&format!("Location: {location}\r\n"));
    }
    head.push_str("\r\n");

    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}

/// The fixture every test below shares, with one route per rule.
fn routes(port_placeholder: &str) -> HashMap<String, Reply> {
    let mut routes = HashMap::new();

    routes.insert(
        "/page.html".into(),
        Reply::ok(
            "text/html; charset=utf-8",
            "<html><body><p>The pangolin ferry timetable was revised in March.</p></body></html>",
        ),
    );
    routes.insert(
        "/paper".into(),
        // A minimal but real PDF, so the PDF reader is the thing being exercised.
        Reply::ok("application/pdf", minimal_pdf()),
    );
    routes.insert(
        "/lib.rs".into(),
        Reply::ok("text/plain", "fn main() { println!(\"hello\"); }"),
    );

    // A chain of three, which is inside the limit, and a chain of six, which is
    // not. Each hop is a separate route so the count is exact.
    routes.insert("/hop1".into(), Reply::redirect("/hop2"));
    routes.insert("/hop2".into(), Reply::redirect("/hop3"));
    routes.insert("/hop3".into(), Reply::redirect("/page.html"));
    for n in 1..=7 {
        routes.insert(
            format!("/loop{n}"),
            Reply::redirect(format!("/loop{}", n + 1)),
        );
    }

    // A redirect that leaves the one origin allowed to be plain HTTP.
    routes.insert(
        "/offsite".into(),
        Reply::redirect(format!("http://{port_placeholder}/elsewhere")),
    );

    routes.insert(
        "/huge".into(),
        Reply::ok("text/html", vec![b'a'; (32 * 1024 * 1024) + 64]),
    );
    routes.insert(
        "/image".into(),
        Reply::ok("image/png", b"\x89PNG\r\n".to_vec()),
    );
    routes.insert("/untyped".into(), Reply::ok("", b"something".to_vec()));
    routes.insert(
        "/liar".into(),
        // Says PDF, is not one. Handing this to the PDF reader would produce
        // nothing and look like an empty document rather than a wrong one.
        Reply::ok("application/pdf", b"<html>not a pdf at all</html>".to_vec()),
    );
    routes.insert("/locked".into(), Reply::status(401));
    routes.insert("/forbidden".into(), Reply::status(403));
    routes.insert("/missing".into(), Reply::status(404));

    // A path that tries to climb out of the downloads directory.
    routes.insert(
        "/../../../../etc/passwd".into(),
        Reply::ok("text/html", b"<p>root</p>".to_vec()),
    );

    routes
}

fn minimal_pdf() -> Vec<u8> {
    let body = "%PDF-1.4\n\
        1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n\
        2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n\
        3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj\n\
        4 0 obj<</Length 74>>stream\n\
        BT /F1 12 Tf 72 720 Td (The okapi siding was relaid last autumn.) Tj ET\n\
        endstream endobj\n\
        5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj\n\
        trailer<</Root 1 0 R>>\n";
    body.as_bytes().to_vec()
}

/// The one server every test in this file shares.
///
/// One rather than one each, because the environment variable naming the
/// allowed plain-HTTP origin is process-wide: a fixture per test would have
/// each of them pointing that variable at its own port, and whichever ran last
/// would win for all of them. That is precisely the bug this arrangement had
/// before it was written down.
fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        // The offsite redirect needs a host that is not the fixture's own, and
        // the port is not known until the listener binds — so it is written
        // with a placeholder the route map is built around.
        let fixture = Fixture::serve(routes("127.0.0.2:1"));
        unsafe { std::env::set_var("SEMLITH_ADD_ORIGIN", fixture.origin()) };
        // The fixture is on loopback, which `semlith add` refuses from 0.14.0
        // unless this says otherwise. It is the same opt-in a developer
        // indexing an intranet host uses — the tests do not get a private door.
        unsafe { std::env::set_var("SEMLITH_ADD_ALLOW_PRIVATE", "1") };
        fixture
    })
}

/// The shared fixture and a store of this test's own.
struct Harness {
    fixture: &'static Fixture,
    store: tempfile::TempDir,
}

impl Harness {
    fn new() -> Self {
        Self {
            fixture: fixture(),
            store: tempfile::tempdir().unwrap(),
        }
    }

    fn dir(&self) -> &Path {
        self.store.path()
    }

    fn downloads(&self) -> PathBuf {
        self.dir().join("downloads")
    }

    fn fetch(&self, path: &str) -> anyhow::Result<semlith::add::Fetched> {
        semlith::add::fetch(&self.fixture.url(path), self.dir())
    }
}

/// What a page, a paper and a source file each become on disk.
#[test]
fn a_page_a_pdf_and_a_text_file_each_land_under_their_host() {
    let h = Harness::new();

    let page = h.fetch("/page.html").expect("the page");
    assert!(page.path.starts_with(h.downloads()), "{:?}", page.path);
    assert_eq!(page.path.extension().unwrap(), "html");
    let written = std::fs::read_to_string(&page.path).unwrap();
    assert!(written.contains("pangolin ferry timetable"), "{written}");

    let paper = h.fetch("/paper").expect("the paper");
    // Typed a PDF and starting with %PDF-, so it is saved as one even though
    // the URL carries no extension of its own.
    assert_eq!(paper.path.extension().unwrap(), "pdf");

    // A text type keeps the extension the URL gave it, so `--lang rust` still
    // finds what was fetched.
    let source = h.fetch("/lib.rs").expect("the source file");
    assert_eq!(source.path.extension().unwrap(), "rs");

    // Laid out by host, so a store that has collected fifty pages stays
    // navigable and two sites' index.html are two files.
    assert!(
        page.path
            .parent()
            .unwrap()
            .to_string_lossy()
            .contains("127.0.0.1"),
        "{:?}",
        page.path
    );
}

/// Everything `add` will not do. Each one exits with a reason and writes
/// nothing, and the second half of that claim is the half worth testing.
#[test]
fn every_refusal_refuses_and_leaves_nothing_behind() {
    let h = Harness::new();

    let cases: &[(&str, &str)] = &[
        ("/loop1", "more than"),
        ("/offsite", "not https"),
        ("/huge", "larger than"),
        ("/image", "no reader for"),
        ("/untyped", "did not say what it is"),
        ("/liar", "does not start with %PDF-"),
        ("/locked", "credentials"),
        ("/forbidden", "credentials"),
        ("/missing", "404"),
    ];

    for (path, expected) in cases {
        let error = match h.fetch(path) {
            Ok(fetched) => panic!("{path} was fetched to {:?}", fetched.path),
            Err(e) => format!("{e:#}"),
        };
        assert!(
            error.contains(expected),
            "{path} was refused, but not for the stated reason: {error}"
        );
    }

    // Not one of them wrote a byte. A refusal that leaves a partial file behind
    // is a refusal the next indexing run undoes.
    let written = walk(&h.downloads());
    assert!(
        written.is_empty(),
        "a refusal left files behind: {written:?}"
    );

    // A plain http URL that is not the one allowed origin is refused too, which
    // is what keeps the test escape from being a hole.
    let error = semlith::add::fetch("http://example.com/x", h.dir())
        .map(|f| format!("fetched {:?}", f.path))
        .unwrap_err()
        .to_string();
    assert!(error.contains("not https"), "{error}");
}

/// Redirects are followed to the document, and counted.
#[test]
fn a_redirect_chain_is_followed_to_its_end_and_no_further() {
    let h = Harness::new();

    let fetched = h.fetch("/hop1").expect("three hops is inside the limit");
    let written = std::fs::read_to_string(&fetched.path).unwrap();
    assert!(written.contains("pangolin ferry timetable"), "{written}");
    // The reported URL is where the chain ended, not where it started: what was
    // indexed is the document, and saying otherwise would be a lie in the log.
    assert!(fetched.url.ends_with("/page.html"), "{}", fetched.url);

    // Each hop asked for exactly once, and the page at the end reached. Counted
    // per path, so another test fetching the page cannot inflate it.
    for hop in ["/hop1", "/hop2", "/hop3"] {
        assert_eq!(
            h.fixture.requests(hop),
            1,
            "{hop} was not asked for exactly once"
        );
    }

    // And the chain stopped where it was told to: the limit is five, so the
    // seventh route is never reached however many times /loop1 is tried.
    assert_eq!(
        h.fixture.requests("/loop7"),
        0,
        "the redirect limit did not hold"
    );
}

/// The path a document is written to is derived from a URL, which makes it the
/// one piece of attacker-influenced input that reaches a filesystem write.
#[test]
fn a_url_cannot_write_outside_the_downloads_directory() {
    let h = Harness::new();

    // The server is willing to serve it; the point is where it lands.
    if let Ok(fetched) = h.fetch("/../../../../etc/passwd") {
        assert!(
            fetched.path.starts_with(h.downloads()),
            "a traversing URL escaped to {:?}",
            fetched.path
        );
        assert!(
            !fetched.path.to_string_lossy().contains(".."),
            "a .. survived into {:?}",
            fetched.path
        );
    }

    // Nothing was created outside the store at all.
    assert!(
        !Path::new("/etc/passwd.html").exists(),
        "a file was written outside the store"
    );
}

/// Fetching the same URL twice keeps both. The first copy is already indexed,
/// and a page that changed is the reason somebody fetched it again.
#[test]
fn a_second_fetch_of_the_same_url_does_not_replace_the_first() {
    let h = Harness::new();

    let first = h.fetch("/page.html").expect("the first fetch");
    let second = h.fetch("/page.html").expect("the second fetch");

    assert_ne!(first.path, second.path);
    assert!(first.path.exists() && second.path.exists());
    assert!(
        second.path.to_string_lossy().contains("page-2"),
        "the second copy is not suffixed: {:?}",
        second.path
    );
}

/// The refusal that has to happen before the socket, not after it.
///
/// `--airgap`'s whole claim is that the process can be shown never to have
/// reached the network, so "it refused" is not the assertion — "it refused and
/// the server saw nothing" is. Run as a subprocess because the flag is read
/// from the environment and the other tests in this file share a process.
#[test]
fn airgap_refuses_before_a_single_request_is_made() {
    let h = Harness::new();
    // Its own route, so the count is this test's alone.
    let path = "/airgap.html";

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["add", "--airgap", &h.fixture.url(path)])
        .env("SEMLITH_HOME", h.dir())
        .env("SEMLITH_ADD_ORIGIN", h.fixture.origin())
        .env("SEMLITH_ADD_ALLOW_PRIVATE", "1")
        .env("SEMLITH_STORE", h.dir())
        .output()
        .expect("running semlith add --airgap");

    assert!(!output.status.success(), "--airgap exited zero");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--airgap"),
        "the error does not name the flag that caused it: {stderr}"
    );
    assert_eq!(
        h.fixture.requests(path),
        0,
        "--airgap refused, but only after opening a connection"
    );
}

/// The round trip the command exists for: fetch a page, and find it by asking a
/// question about what it said.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_added_page_is_searchable_with_a_locator() {
    let h = Harness::new();
    let fetched = h.fetch("/page.html").expect("the page");

    let mut store = semlith::Semlith::open(h.dir(), None).unwrap();
    store.quiet = true;
    let report = store
        .index_paths(std::slice::from_ref(&fetched.path), |_, _| {})
        .unwrap();
    assert!(report.chunks > 0, "the page produced no chunks: {report:?}");

    let hits = store.search("pangolin ferry timetable", 3).unwrap();
    let top = hits.first().expect("the added page found nothing");
    assert!(
        top.path.ends_with("page.html"),
        "the added page did not rank first: {top:?}"
    );
    assert!(top.start_line >= 1, "a hit with no locator: {top:?}");
}

/// Every file under a directory, or nothing when it was never created.
fn walk(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walk(&path));
        } else {
            out.push(path);
        }
    }
    out
}

/// The case this rule exists for: a tool that runs on your machine, told to
/// fetch a URL, is a way to read what only your machine can reach.
#[test]
fn an_address_that_is_not_on_the_internet_is_refused_at_every_hop() {
    let h = Harness::new();

    for url in [
        "https://127.0.0.1:1/x",
        "https://10.0.0.1/",
        "https://169.254.169.254/latest/meta-data/",
        "https://192.168.1.1/",
        "https://172.16.0.1/",
        "https://100.64.0.1/",
        "https://[::1]/x",
        "https://[fd00::1]/x",
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
            .args(["add", url])
            .env("SEMLITH_HOME", h.dir())
            .env("SEMLITH_STORE", h.dir())
            .env_remove("SEMLITH_ADD_ALLOW_PRIVATE")
            .output()
            .expect("running semlith add");
        assert!(!out.status.success(), "{url} was fetched");
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            said.contains("not on the public internet"),
            "{url} was refused for some other reason:\n{said}"
        );
        // The refusal names the address, so somebody reading it knows which
        // hop of a chain was the problem.
        assert!(
            said.contains("resolves to"),
            "the refusal does not name the address:\n{said}"
        );
    }

    // And the opt-in works, which is what makes the rule a default rather than
    // a wall: the fixture server is on loopback.
    let allowed = std::process::Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["add", &h.fixture.url("/page.html")])
        .env("SEMLITH_HOME", h.dir())
        .env("SEMLITH_STORE", h.dir())
        .env("SEMLITH_ADD_ORIGIN", h.fixture.origin())
        .env("SEMLITH_ADD_ALLOW_PRIVATE", "1")
        .output()
        .expect("running semlith add");
    assert!(
        allowed.status.success(),
        "the opt-in did not work:\n{}",
        String::from_utf8_lossy(&allowed.stderr)
    );
}
