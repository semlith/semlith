//! `semlith upgrade`, against a release that is not GitHub.
//!
//! Replacing the binary a user runs is the most damaging thing in this release
//! if it goes wrong, so the three ways it can go wrong are the three tests
//! here: a tampered archive must leave the original in place, a good one must
//! swap and keep the old file, and `--airgap` must refuse without opening a
//! connection at all. The last one is the reason the fixture server counts its
//! requests — "it refused" and "it refused after asking" are different claims.
//!
//! `SEMLITH_RELEASES_ORIGIN` points the redirect, the archive and `SHA256SUMS`
//! at the fixture. It exists for this file and is deliberately not in
//! `docs/compatibility.md`.

#![cfg(unix)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// The version the fixture offers, chosen so it is newer than anything this
/// crate will be called.
const NEWER: &str = "v99.0.0";

struct Fixture {
    port: u16,
    hits: Arc<AtomicUsize>,
}

impl Fixture {
    /// A one-thread HTTP/1.1 server over a fixed route table. Hand-rolled for
    /// the same reason `src/http.rs` is: the daemon answers four verbs on a
    /// loopback socket, and a test of it should not need a framework either.
    fn serve(routes: HashMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        let routes = Arc::new(Mutex::new(routes));

        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                let routes = Arc::clone(&routes);
                answer(stream, &routes.lock().unwrap());
            }
        });

        Self { port, hits }
    }

    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

fn answer(mut stream: TcpStream, routes: &HashMap<String, Vec<u8>>) {
    let Ok(peer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(peer);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();

    // Drain the rest of the request before answering. Replying and closing
    // while the client is still writing its headers makes the peer see a reset
    // instead of the response — on macOS that turned a checksum assertion into
    // "Connection reset by peer" and failed the run on CI.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }

    // The only redirect that matters: `releases/latest` lands on the tag, and
    // `upgrade` reads the tag out of where it landed.
    if path.ends_with("/releases/latest") {
        let to = format!("/semlith/semlith/releases/tag/{NEWER}");
        let _ = write!(
            stream,
            "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        return;
    }
    if path.ends_with(&format!("/releases/tag/{NEWER}")) {
        let _ = write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        return;
    }

    match routes.get(&path) {
        Some(body) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(body);
        }
        None => {
            let _ = write!(
                stream,
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n"
            );
        }
    }
}

/// The triple `upgrade` asks for on this machine, kept in step with
/// `release.yml`'s matrix the same way the command is.
fn target() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("macos", "aarch64") => "aarch64-apple-darwin",
        (os, arch) => panic!("no semlith release target for {os}/{arch}"),
    }
}

struct Release {
    archive_name: String,
    archive: Vec<u8>,
    sums: String,
}

/// A release archive shaped exactly like the one `release.yml` uploads: one
/// top-level directory holding the binary.
fn build_release(work: &Path) -> Release {
    let name = format!("semlith-{NEWER}-{}", target());
    let staged = work.join(&name);
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("semlith"), "#!/bin/sh\necho 'semlith 99.0.0'\n").unwrap();
    chmod_755(&staged.join("semlith"));

    let archive_name = format!("{name}.tar.gz");
    let status = Command::new("tar")
        .arg("czf")
        .arg(work.join(&archive_name))
        .arg("-C")
        .arg(work)
        .arg(&name)
        .status()
        .expect("running tar");
    assert!(status.success(), "tar could not build the fixture archive");

    let archive = std::fs::read(work.join(&archive_name)).unwrap();
    let sums = format!("{}  {archive_name}\n", sha256(&archive));
    Release {
        archive_name,
        archive,
        sums,
    }
}

fn chmod_755(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Shelled out rather than linked: the crate's own sha2 is not a dev-dependency
/// and a test that computed the digest with the same code the command uses
/// would agree with itself no matter what either did.
fn sha256(bytes: &[u8]) -> String {
    let tool = if Command::new("sha256sum")
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success())
    {
        vec!["sha256sum"]
    } else {
        vec!["shasum", "-a", "256"]
    };
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("blob");
    std::fs::write(&file, bytes).unwrap();
    let out = Command::new(tool[0])
        .args(&tool[1..])
        .arg(&file)
        .output()
        .expect("hashing the fixture archive");
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .expect("a digest")
        .to_string()
}

struct Installed {
    _dir: tempfile::TempDir,
    home: PathBuf,
    binary: PathBuf,
}

/// `upgrade` only replaces a binary that lives in the bin directory the install
/// scripts write to, so the fixture has to look like a script install.
fn install() -> Installed {
    let dir = tempfile::tempdir().expect("a temp directory");
    let home = dir.path().join(".semlith");
    let bin = home.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let binary = bin.join("semlith");
    std::fs::copy(env!("CARGO_BIN_EXE_semlith"), &binary).unwrap();
    chmod_755(&binary);
    Installed {
        _dir: dir,
        home,
        binary,
    }
}

fn run(installed: &Installed, origin: &str, args: &[&str], airgap: bool) -> Output {
    let mut command = Command::new(&installed.binary);
    command
        .arg("upgrade")
        .args(args)
        .env("SEMLITH_HOME", &installed.home)
        .env("SEMLITH_RELEASES_ORIGIN", origin);
    if airgap {
        command.env("SEMLITH_AIRGAP", "1");
    }
    command.output().expect("running semlith upgrade")
}

fn routes(release: &Release, sums: &str) -> HashMap<String, Vec<u8>> {
    let base = format!("/semlith/semlith/releases/download/{NEWER}");
    HashMap::from([
        (
            format!("{base}/{}", release.archive_name),
            release.archive.clone(),
        ),
        (format!("{base}/SHA256SUMS"), sums.as_bytes().to_vec()),
    ])
}

#[test]
fn a_tampered_checksum_leaves_the_original_binary_alone() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());

    // One character, which is the whole point: a corrupted download and a
    // substituted one look the same from here.
    let mut tampered = release.sums.clone();
    let first = tampered.remove(0);
    tampered.insert(0, if first == 'a' { 'b' } else { 'a' });

    let fixture = Fixture::serve(routes(&release, &tampered));
    let installed = install();
    let before = std::fs::read(&installed.binary).unwrap();

    let out = run(&installed, &fixture.origin(), &[], false);
    assert!(
        !out.status.success(),
        "a tampered SHA256SUMS should fail the upgrade"
    );
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("checksum mismatch"),
        "the failure should name the mismatch, it said:\n{said}"
    );
    assert_eq!(
        std::fs::read(&installed.binary).unwrap(),
        before,
        "the original binary was modified despite the checksum failing"
    );
    assert!(
        !installed.binary.with_extension("old").exists(),
        "nothing should have been moved aside"
    );
}

#[test]
fn a_matching_checksum_swaps_and_keeps_the_old_binary() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());
    let fixture = Fixture::serve(routes(&release, &release.sums));
    let installed = install();

    let out = run(&installed, &fixture.origin(), &[], false);
    assert!(
        out.status.success(),
        "the upgrade failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let now = Command::new(&installed.binary)
        .output()
        .expect("running the upgraded binary");
    assert_eq!(
        String::from_utf8_lossy(&now.stdout).trim(),
        "semlith 99.0.0",
        "the binary in the bin directory is not the one the release carried"
    );
    assert!(
        installed.binary.with_extension("old").exists(),
        "the previous binary should be left as semlith.old"
    );
}

#[test]
fn airgap_refuses_before_opening_a_connection() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());
    let fixture = Fixture::serve(routes(&release, &release.sums));
    let installed = install();

    for args in [&["--check"][..], &[][..]] {
        let out = run(&installed, &fixture.origin(), args, true);
        assert!(
            !out.status.success(),
            "`semlith upgrade {args:?}` should refuse under --airgap"
        );
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            said.contains("SEMLITH_AIRGAP"),
            "the refusal should name the variable that caused it:\n{said}"
        );
    }

    assert_eq!(
        fixture.hits.load(Ordering::SeqCst),
        0,
        "an air-gapped run reached the network, which is the one thing it promises not to do"
    );
}

/// An archive larger than the cap is refused before the checksum, because the
/// refusal has to happen before the bytes are in memory — a checksum computed
/// over a gigabyte has already cost the gigabyte.
#[test]
fn an_oversized_archive_is_refused_before_it_is_checksummed() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());

    // Past the 256 MiB cap, and compressible, so the fixture holds it cheaply.
    let mut oversized = release.archive.clone();
    oversized.resize(257 * 1024 * 1024, 0);
    let sums = format!("{}  {}\n", sha256(&oversized), release.archive_name);

    let mut table = routes(&release, &sums);
    table.insert(
        format!(
            "/semlith/semlith/releases/download/{NEWER}/{}",
            release.archive_name
        ),
        oversized,
    );
    let fixture = Fixture::serve(table);
    let installed = install();
    let before = std::fs::read(&installed.binary).unwrap();

    let out = run(&installed, &fixture.origin(), &[], false);
    assert!(!out.status.success(), "an oversized archive was accepted");
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(
        said.contains("larger than") && said.contains("nothing was written"),
        "the refusal should name the size, it said:\n{said}"
    );
    assert!(
        !said.contains("checksum mismatch"),
        "the size was found only after the checksum, which means it was all read first:\n{said}"
    );
    assert_eq!(std::fs::read(&installed.binary).unwrap(), before);
}

/// A tag reaches a URL and a path, so it is checked before it reaches either.
/// `--version` is the one a user types; the redirect is the one a server
/// chooses.
#[test]
fn a_malformed_tag_is_refused_before_any_request() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());
    let fixture = Fixture::serve(routes(&release, &release.sums));
    let installed = install();

    for tag in [
        "v9.9.9-/../x",
        "v9.9.9/../../etc",
        "../../etc/passwd",
        "v9.9",
        "v9.9.9.9",
        "vx.y.z",
        "v99.0.0 ",
    ] {
        let out = run(&installed, &fixture.origin(), &["--version", tag], false);
        assert!(
            !out.status.success(),
            "`--version {tag}` was accepted as a version tag"
        );
        let said = String::from_utf8_lossy(&out.stderr);
        assert!(
            said.contains("not a version tag"),
            "`--version {tag}` failed for some other reason:\n{said}"
        );
    }

    assert_eq!(
        fixture.hits.load(Ordering::SeqCst),
        0,
        "a malformed tag reached the network before it was refused"
    );
}

/// A redirect that lands somewhere that is not a version tag is a redirect
/// choosing a path inside this command, so it is refused where it lands.
#[test]
fn a_redirect_that_names_no_tag_is_refused() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());
    let mut table = routes(&release, &release.sums);
    // The fixture redirects `releases/latest` to a fixed tag; this one points
    // it somewhere else entirely.
    table.insert("/redirect-elsewhere".to_string(), Vec::new());
    let fixture = Fixture::serve(table);
    let installed = install();

    let out = run(
        &installed,
        &format!("{}/redirect", fixture.origin()),
        &[],
        false,
    );
    assert!(
        !out.status.success(),
        "an unresolvable release was accepted"
    );
    assert!(
        !std::fs::read(&installed.binary).unwrap().is_empty(),
        "the binary was replaced from a release that could not be resolved"
    );
}

/// The archive carries more than the binary from 0.14.0 — the Linux ones ship
/// `libonnxruntime.so` beside it — so the reader extracts every file rather
/// than picking one out.
#[test]
fn every_file_in_the_archive_is_unpacked_beside_the_binary() {
    let work = tempfile::tempdir().unwrap();
    let name = format!("semlith-{NEWER}-{}", target());
    let staged = work.path().join(&name);
    std::fs::create_dir_all(&staged).unwrap();
    std::fs::write(staged.join("semlith"), "#!/bin/sh\necho 'semlith 99.0.0'\n").unwrap();
    chmod_755(&staged.join("semlith"));
    std::fs::write(staged.join("libonnxruntime.so"), b"not really a library").unwrap();
    std::fs::write(staged.join("README.md"), b"# semlith").unwrap();

    let archive_name = format!("{name}.tar.gz");
    let status = Command::new("tar")
        .arg("czf")
        .arg(work.path().join(&archive_name))
        .arg("-C")
        .arg(work.path())
        .arg(&name)
        .status()
        .expect("running tar");
    assert!(status.success());
    let archive = std::fs::read(work.path().join(&archive_name)).unwrap();
    let release = Release {
        sums: format!("{}  {archive_name}\n", sha256(&archive)),
        archive_name,
        archive,
    };

    let fixture = Fixture::serve(routes(&release, &release.sums));
    let installed = install();
    let out = run(&installed, &fixture.origin(), &[], false);
    assert!(
        out.status.success(),
        "the upgrade failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let bin = installed.binary.parent().unwrap();
    assert_eq!(
        std::fs::read(bin.join("libonnxruntime.so")).unwrap(),
        b"not really a library",
        "the library beside the binary was not unpacked"
    );
    assert!(
        !bin.join("README.md").exists(),
        "the archive's documentation was installed into the bin directory"
    );
    let now = Command::new(&installed.binary).output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&now.stdout).trim(),
        "semlith 99.0.0"
    );
}

/// `--check` is what a script runs, so its exit code is the answer: 0 when
/// current, 10 when there is something newer, and the binary untouched either
/// way.
#[test]
fn check_exits_ten_when_an_upgrade_exists_and_changes_nothing() {
    let work = tempfile::tempdir().unwrap();
    let release = build_release(work.path());
    let fixture = Fixture::serve(routes(&release, &release.sums));
    let installed = install();
    let before = std::fs::read(&installed.binary).unwrap();

    let out = run(&installed, &fixture.origin(), &["--check"], false);
    assert_eq!(
        out.status.code(),
        Some(10),
        "--check should exit 10 when a newer release exists, it said:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("99.0.0"),
        "--check should name the version it found"
    );
    assert_eq!(
        std::fs::read(&installed.binary).unwrap(),
        before,
        "--check changed the binary"
    );
}
