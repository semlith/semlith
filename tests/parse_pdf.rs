//! SEC-008: the fonts embedded in an indexed PDF are untrusted input, and they
//! are read inside the parse worker, where a reader that dies costs one
//! worker and one refused file, never the daemon.
//!
//! ttf-parser, the unmaintained crate the finding names, is not on this path:
//! lopdf calls it only in `FontData::new` (lopdf 0.42.0 `src/font.rs:69`),
//! which builds a font for a PDF being written, and pdf-extract never calls
//! it. The embedded-font readers pdf-extract does run are
//! `type1_encoding_parser` on a Type 1 `/FontFile` (pdf-extract 0.12.1
//! `src/lib.rs:391`) and `cff_parser` on a `/FontFile3` of subtype Type1C
//! (`src/lib.rs:413`); a TrueType `/FontFile2` is decompressed and dropped
//! unread (`src/lib.rs:396-400`). Both readers are fed a malformed program
//! here, one that overflows the stack and one that panics.
//!
//! Its own binary, with no environment set in this process, so it can sit
//! beside `parse_worker.rs`'s single test. It indexes, so it needs the
//! embedding model; the model cache is the user's, everything else a scratch
//! home.

use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

fn model_cache() -> PathBuf {
    std::env::var_os("SEMLITH_MODEL_CACHE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .unwrap(),
            )
            .join(".cache")
            .join("semlith")
            .join("models")
        })
}

fn semlith(home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_semlith"));
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("SEMLITH_HOME", home.join(".semlith"))
        .env("SEMLITH_MODEL_CACHE", model_cache())
        .env_remove("SEMLITH_STORE")
        .env_remove("SEMLITH_PORT")
        .current_dir(home);
    cmd
}

fn run(home: &Path, args: &[&str]) -> Output {
    semlith(home).args(args).output().expect("semlith runs")
}

struct Daemon {
    child: Child,
    port: u16,
    token: String,
}

impl Daemon {
    fn start(home: &Path) -> Self {
        let mut child = semlith(home)
            .args(["start", "--port", "0"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("semlith start runs");
        let mut line = String::new();
        BufReader::new(child.stdout.as_mut().unwrap())
            .read_line(&mut line)
            .expect("the daemon prints its URL");
        let rest = line
            .trim()
            .strip_prefix("http://127.0.0.1:")
            .unwrap_or_else(|| panic!("no loopback URL in {line:?}"));
        let (port, token) = rest.split_once("/?token=").expect("a token in the URL");
        Self {
            child,
            port: port.parse().unwrap(),
            token: token.to_string(),
        }
    }

    fn alive(&mut self) -> bool {
        self.child.try_wait().ok().flatten().is_none()
    }

    fn get(&self, path: &str) -> (u16, Value) {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", self.port)) else {
            return (0, Value::Null);
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        let request = format!(
            "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nSemlith-Token: {}\r\n\
             Connection: close\r\n\r\n",
            self.port, self.token
        );
        if stream.write_all(request.as_bytes()).is_err() {
            return (0, Value::Null);
        }
        let mut raw = Vec::new();
        let _ = stream.read_to_end(&mut raw);
        let text = String::from_utf8_lossy(&raw);
        let status = text
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = text.split_once("\r\n\r\n").map_or("", |(_, b)| b);
        (status, serde_json::from_str(body).unwrap_or(Value::Null))
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait for `done` to hold, polling, up to `limit`.
fn until(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    done()
}

/// A one-page PDF that shows a line of text in font `/F1`. With `embedded`,
/// that font is a simple font of `subtype` whose descriptor carries
/// `program` under `key`, the stream's own dictionary entries being `extra`.
fn pdf(embedded: Option<(&str, &str, &str, &[u8])>) -> Vec<u8> {
    let content = b"BT /F1 12 Tf 72 712 Td (Hello from a PDF) Tj ET";
    let stream = |extra: &str, data: &[u8]| {
        [
            format!("<< {extra} /Length {} >>\nstream\n", data.len()).as_bytes(),
            data,
            b"\nendstream",
        ]
        .concat()
    };
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
          /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_vec(),
    ];
    match embedded {
        None => {
            objects.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec());
            objects.push(stream("", content));
        }
        Some((subtype, key, extra, program)) => {
            objects.push(
                format!(
                    "<< /Type /Font /Subtype /{subtype} /BaseFont /Bad /FontDescriptor 6 0 R >>"
                )
                .into_bytes(),
            );
            objects.push(stream("", content));
            objects.push(
                format!(
                    "<< /Type /FontDescriptor /FontName /Bad /Flags 32 \
                     /FontBBox [0 0 1000 1000] /ItalicAngle 0 /Ascent 800 /Descent -200 \
                     /CapHeight 700 /StemV 80 /{key} 7 0 R >>"
                )
                .into_bytes(),
            );
            objects.push(stream(extra, program));
        }
    }
    let mut out = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(object);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// `(class, rule)` of the refused row for the file named `name`.
fn refusal(refused: &Value, name: &str) -> Option<(String, String)> {
    refused["stores"]
        .as_array()?
        .iter()
        .flat_map(|s| s["rows"].as_array().cloned().unwrap_or_default())
        .find(|r| r["path"].as_str().is_some_and(|p| p.ends_with(name)))
        .map(|r| {
            (
                r["class"].as_str().unwrap_or_default().to_string(),
                r["rule"].as_str().unwrap_or_default().to_string(),
            )
        })
}

fn listed(files: &Value, name: &str) -> bool {
    files["files"].as_array().is_some_and(|f| {
        f.iter()
            .any(|r| r["path"].as_str().is_some_and(|p| p.ends_with(name)))
    })
}

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn a_pdf_whose_embedded_font_kills_its_reader_is_refused_and_the_daemon_stays_up() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(corpus.join("notes.md"), "# Notes\n\nAn ordinary file.\n").unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()]);
    assert!(out.status.success(), "{:?}", out.status);

    // Added while nothing runs, so the daemon's catch-up is what reads them.
    // A Type 1 program of 100 000 open procedures: type1-encoding-parser
    // recurses once per brace and overflows the stack, an abort no
    // catch_unwind can stop. Measured: SIGABRT in debug and release workers.
    let deep = vec![b'{'; 100_000];
    std::fs::write(
        corpus.join("font.pdf"),
        pdf(Some(("Type1", "FontFile", "", &deep))),
    )
    .unwrap();
    // A Type1C program that is not CFF: cff_parser's parse fails and
    // pdf-extract unwraps it — a panic, caught where it happens.
    std::fs::write(
        corpus.join("cff.pdf"),
        pdf(Some((
            "Type1",
            "FontFile3",
            "/Subtype /Type1C",
            b"\x01\x00\x04\x04not a font program",
        ))),
    )
    .unwrap();
    std::fs::write(corpus.join("good.pdf"), pdf(None)).unwrap();

    let mut daemon = Daemon::start(&home);
    let settled = until(Duration::from_secs(180), || {
        let refused = daemon.get("/api/refused").1;
        refusal(&refused, "font.pdf").is_some()
            && refusal(&refused, "cff.pdf").is_some()
            && listed(&daemon.get("/api/files").1, "good.pdf")
    });
    assert!(daemon.alive(), "the daemon died reading an embedded font");
    let refused = daemon.get("/api/refused").1;
    assert!(settled, "the catch-up did not settle: {refused}");

    // Only a parse worker that died says this: the stack overflowed in the
    // worker, and the daemon read its stderr.
    assert_eq!(
        refusal(&refused, "font.pdf"),
        Some(("crashed".into(), "parser overflowed its stack".into())),
        "{refused}"
    );
    // The panic costs no worker: the PDF has no text, and says so.
    assert_eq!(
        refusal(&refused, "cff.pdf"),
        Some(("unindexable".into(), "no text in this document".into())),
        "{refused}"
    );

    // The next file, and the daemon, are fine.
    let files = daemon.get("/api/files").1;
    assert!(listed(&files, "good.pdf"), "{files}");
    assert!(!listed(&files, "font.pdf"), "{files}");
    let (status, _) = daemon.get("/api/about");
    assert_eq!(status, 200, "the daemon does not answer");
    assert!(daemon.alive());
}
