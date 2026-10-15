//! `install.sh`: what the one-liner on the README actually does to a machine.
//!
//! The install script is the first code a new user runs, it runs as a shell
//! pipe with no test harness around it, and the failure mode that matters is
//! the one where a truncated or tampered download still lands in `bin`. So the
//! script is exercised end to end here against a fixture HTTP server on
//! loopback: a real archive, a real `SHA256SUMS` signed by a real
//! `ssh-keygen -Y sign`, real `tar` and `shasum`, and the one bit flipped (or
//! the signature missing, or made by another key) that must stop the install
//! dead.
//!
//! The project's private key is not something a test holds, so each fixture
//! signs with a throwaway key and the script copy trusts that key in place of
//! the project's — the same one-line rewrite as the origin.
//!
//! Nothing here touches the network or GitHub — the tag below is deliberately
//! not a release that exists, so a test that somehow escaped the fixture origin
//! would 404 rather than quietly download the real thing.
//!
//! ```sh
//! cargo test --test install
//! ```

#![cfg(unix)]

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Not a real release. See the module comment.
const TAG: &str = "v9.9.9";

/// What the stand-in binary prints, so the test can tell "the archive's file
/// was installed" apart from "something named semlith is on the path".
const MARKER: &str = "fake semlith from the install.sh fixture";

/// The host triple `install.sh` computes from `uname`, computed the same way
/// so the fixture's file names match whichever runner this lands on.
fn target() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => panic!("install.sh supports x86_64 and aarch64; this host is {other}"),
    };
    match std::env::consts::OS {
        "linux" => format!("{arch}-unknown-linux-gnu"),
        // install.sh refuses Intel macOS outright (no ONNX Runtime build), so
        // the only darwin target it can ever ask for is the arm64 one.
        "macos" => "aarch64-apple-darwin".to_string(),
        other => panic!("install.sh supports linux and macos; this host is {other}"),
    }
}

/// The digest, computed by the same tools the script verifies with, because a
/// fixture that disagrees with `shasum` would fail the good case for the wrong
/// reason.
fn sha256(path: &Path) -> String {
    for tool in ["sha256sum", "shasum"] {
        let mut command = Command::new(tool);
        if tool == "shasum" {
            command.args(["-a", "256"]);
        }
        let Ok(out) = command.arg(path).output() else {
            continue;
        };
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            let digest = text
                .split_whitespace()
                .next()
                .unwrap_or_else(|| panic!("{tool} printed no digest: {text:?}"));
            return digest.to_string();
        }
    }
    panic!("need sha256sum or shasum to build the SHA256SUMS fixture; install.sh needs one too");
}

/// Serve `routes` on an ephemeral loopback port and return the origin to hand
/// `SEMLITH_RELEASES_ORIGIN`. The thread lives until the test process exits;
/// the script only makes two requests and never comes back.
fn serve(routes: HashMap<String, Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("an ephemeral port");
    let port = listener.local_addr().expect("the bound address").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let Ok(peek) = stream.try_clone() else {
                continue;
            };
            let mut reader = BufReader::new(peek);
            let mut request = String::new();
            if reader.read_line(&mut request).is_err() {
                continue;
            }
            let path = request.split(' ').nth(1).unwrap_or("").to_string();
            // curl will not read a byte of the response until it has finished
            // sending its headers, so drain them before answering.
            loop {
                let mut header = String::new();
                match reader.read_line(&mut header) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if header == "\r\n" => break,
                    Ok(_) => {}
                }
            }
            let (status, body) = match routes.get(&path) {
                Some(body) => ("200 OK", body.clone()),
                None => ("404 Not Found", Vec::new()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            let _ = stream.flush();
        }
    });
    format!("http://127.0.0.1:{port}")
}

/// The release key `install.sh` ships with; the script copy swaps it out.
const PROJECT_KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAICMTHiHMzeSHzzc+8UDZHUzJcvb6XGUBb+TJdFes91xm";

/// How the fixture's SHA256SUMS is signed.
#[derive(PartialEq)]
enum Signed {
    /// By the key the script copy trusts.
    Trusted,
    /// By a key the script copy does not trust.
    Untrusted,
    /// Not at all: no SHA256SUMS.sig, as every release before v0.40.0.
    Missing,
}

/// Whether `ssh-keygen` can sign and verify. The installer refuses without it,
/// so without it these tests have nothing to say and say that instead.
fn have_ssh_keygen() -> bool {
    let ok = Command::new("ssh-keygen")
        .arg("-?")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stderr).contains("-Y"));
    if !ok {
        eprintln!("skipped: ssh-keygen with -Y support is not installed");
    }
    ok
}

/// A throwaway ed25519 key: its path, and its `ssh-ed25519 <base64>` part.
fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
    let key = dir.join(name);
    let out = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
        .arg(&key)
        .output()
        .expect("run ssh-keygen");
    assert!(out.status.success(), "ssh-keygen failed: {out:?}");
    let public = fs::read_to_string(key.with_extension("pub")).expect("read the public key");
    let public: Vec<&str> = public.split_whitespace().take(2).collect();
    (key, public.join(" "))
}

/// A sandbox holding a release the script can install, plus the origin serving
/// it. `corrupt` flips one character of the published digest.
struct Release {
    _dir: tempfile::TempDir,
    home: PathBuf,
    semlith_home: PathBuf,
    origin: String,
    /// The public key the script copy trusts in place of the project's.
    trusted: String,
}

fn release(corrupt: bool, signed: Signed) -> Release {
    let dir = tempfile::tempdir().expect("a temp dir");
    let root = dir.path();
    let name = format!("semlith-{TAG}-{}", target());
    let archive_name = format!("{name}.tar.gz");

    // The archive's layout is the release workflow's: one top-level directory
    // named for the release, holding the binary.
    let stage = root.join("stage");
    fs::create_dir_all(stage.join(&name)).expect("stage the release directory");
    let fake = stage.join(&name).join("semlith");
    fs::write(&fake, format!("#!/bin/sh\necho '{MARKER}'\nexit 1\n")).expect("write the stand-in");
    // Non-zero exit for every argument, so install.sh's `setup --help` probe
    // fails and the script prints PATH instructions instead of running a setup.
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).expect("make it executable");
    // From 0.14.0 the Linux archives carry ONNX Runtime beside the binary,
    // because that is where a `dynamic-ort` build loads it from. The fixture
    // carries one on every platform: what is under test is that the script
    // puts it next to the binary rather than leaving it in the tarball.
    fs::write(
        stage.join(&name).join("libonnxruntime.so"),
        b"not really a library",
    )
    .expect("stage the runtime");

    let archive = root.join(&archive_name);
    let tar = Command::new("tar")
        .arg("czf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage)
        .arg(&name)
        .status()
        .expect("run tar");
    assert!(tar.success(), "tar failed to build {archive_name}");

    let mut digest = sha256(&archive);
    if corrupt {
        let first = digest.remove(0);
        digest.insert(0, if first == '0' { '1' } else { '0' });
    }

    // GNU sha256sum format: digest, two spaces, file name.
    let sums = root.join("SHA256SUMS");
    fs::write(&sums, format!("{digest}  {archive_name}\n")).expect("write SHA256SUMS");
    let (trusted_key, trusted) = keygen(root, "trusted");
    let (other_key, _) = keygen(root, "other");

    let base = format!("/semlith/semlith/releases/download/{TAG}");
    let mut routes = HashMap::from([
        (
            format!("{base}/{archive_name}"),
            fs::read(&archive).expect("read the archive back"),
        ),
        (
            format!("{base}/SHA256SUMS"),
            fs::read(&sums).expect("read SHA256SUMS back"),
        ),
    ]);
    if signed != Signed::Missing {
        // Exactly the command release.yml runs.
        let key = if signed == Signed::Trusted {
            &trusted_key
        } else {
            &other_key
        };
        let out = Command::new("ssh-keygen")
            .args(["-Y", "sign", "-n", "file", "-f"])
            .arg(key)
            .arg(&sums)
            .output()
            .expect("run ssh-keygen -Y sign");
        assert!(out.status.success(), "ssh-keygen -Y sign failed: {out:?}");
        routes.insert(
            format!("{base}/SHA256SUMS.sig"),
            fs::read(root.join("SHA256SUMS.sig")).expect("read the signature"),
        );
    }

    let home = root.join("home");
    let semlith_home = root.join("semlith-home");
    fs::create_dir_all(&home).expect("create a fake HOME");
    Release {
        _dir: dir,
        home,
        semlith_home,
        origin: serve(routes),
        trusted,
    }
}

/// The shipped script with its one origin rewritten to the fixture's, and its
/// release key to the fixture's.
///
/// The script takes neither from the environment — a `curl … | sh` that read
/// one would install whatever a hostile shell profile pointed it at — so the
/// test edits a copy instead. Everything else about the copy is the shipped
/// script, and the rewrite fails loudly if the line it is looking for moves.
fn script_pointed_at(origin: &str, trusted: &str, into: &Path) -> PathBuf {
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh");
    let text = std::fs::read_to_string(&shipped).expect("reading install.sh");
    assert!(
        text.contains("origin=https://github.com"),
        "install.sh no longer pins one origin; this test rewrites that line"
    );
    assert!(
        text.contains(&format!("signers='semlith-release {PROJECT_KEY}'")),
        "install.sh no longer trusts the project key on one line; this test rewrites it"
    );
    let redirected = text
        .replace("origin=https://github.com", &format!("origin={origin}"))
        .replace(PROJECT_KEY, trusted);
    let copy = into.join("install.sh");
    std::fs::write(&copy, redirected).expect("writing the redirected script");
    copy
}

fn install(release: &Release) -> std::process::Output {
    let script = script_pointed_at(&release.origin, &release.trusted, &release.home);
    Command::new("sh")
        .arg(&script)
        .env("SEMLITH_VERSION", TAG)
        .env("SEMLITH_HOME", &release.semlith_home)
        .env("HOME", &release.home)
        // A fixture has no attestation; a token in the developer's shell would
        // log gh in and make the installer, correctly, refuse it.
        .env_remove("GH_TOKEN")
        .env_remove("GITHUB_TOKEN")
        .output()
        .expect("run install.sh")
}

#[test]
fn installs_a_verified_release() {
    if !have_ssh_keygen() {
        return;
    }
    let release = release(false, Signed::Trusted);
    let out = install(&release);
    assert!(
        out.status.success(),
        "install.sh exited {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let installed = release.semlith_home.join("bin").join("semlith");
    assert!(
        installed.exists(),
        "install.sh reported success but {} is missing\nstdout:\n{}",
        installed.display(),
        String::from_utf8_lossy(&out.stdout)
    );

    let ran = Command::new(&installed)
        .output()
        .expect("run the installed file");
    let printed = String::from_utf8_lossy(&ran.stdout);
    assert!(
        printed.contains(MARKER),
        "the installed file is not the one from the archive; it printed {printed:?}"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("Signature verified:"),
        "the install did not say it verified the signature:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// A binary that loads its runtime from beside itself is a binary the installer
/// has to put a second file next to. One without it cannot embed anything, and
/// the failure would arrive on the first index rather than on the install.
#[test]
fn the_runtime_lands_beside_the_binary() {
    if !have_ssh_keygen() {
        return;
    }
    let release = release(false, Signed::Trusted);
    let out = install(&release);
    assert!(
        out.status.success(),
        "install.sh exited {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    let bin = release.semlith_home.join("bin");
    assert!(bin.join("semlith").exists(), "the binary was not installed");
    assert_eq!(
        fs::read(bin.join("libonnxruntime.so")).expect("the runtime was not installed"),
        b"not really a library",
        "the runtime beside the binary is not the one the archive carried"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("libonnxruntime.so"),
        "the install did not say it placed the runtime:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn refuses_a_release_whose_checksum_does_not_match() {
    if !have_ssh_keygen() {
        return;
    }
    let release = release(true, Signed::Trusted);
    let out = install(&release);
    assert!(
        !out.status.success(),
        "install.sh accepted a bad checksum\nstdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        said.contains("checksum mismatch"),
        "install.sh failed without naming the mismatch:\n{said}"
    );

    // Not "the binary is absent" but "nothing was created at all": the script
    // must bail before it so much as makes the directory it installs into.
    let bin = release.semlith_home.join("bin");
    assert!(
        !bin.exists(),
        "{} was created despite the checksum mismatch",
        bin.display()
    );
}

/// A refusal that names `why` and leaves nothing behind, not even the bin
/// directory.
fn refused(release: &Release, why: &str) {
    let out = install(release);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "install.sh accepted it:\n{said}");
    assert!(
        said.contains(why),
        "install.sh failed without saying {why:?}:\n{said}"
    );
    assert!(
        !release.semlith_home.join("bin").exists(),
        "the bin directory was created despite the refusal:\n{said}"
    );
}

/// Every release before v0.40.0 looks like this, and so does one whose
/// signature an attacker could not produce and simply left out.
#[test]
fn refuses_a_release_with_no_signature() {
    if !have_ssh_keygen() {
        return;
    }
    refused(&release(false, Signed::Missing), "has no SHA256SUMS.sig");
}

/// A SHA256SUMS rewritten to match a replaced archive and signed with the
/// attacker's own key.
#[test]
fn refuses_a_release_signed_by_another_key() {
    if !have_ssh_keygen() {
        return;
    }
    refused(&release(false, Signed::Untrusted), "signature check failed");
}
