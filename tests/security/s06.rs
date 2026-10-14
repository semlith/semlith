//! S06: upgrade and both installers refuse a release not signed by the project key.
//!
//! `semlith upgrade` is pointed at a loopback release through
//! `SEMLITH_RELEASES_ORIGIN`, which only a debug build reads. Against a release
//! binary the override is ignored, so every test here first asserts the fixture
//! was actually asked: a run that went to github.com and failed there would
//! otherwise pass as a refusal. The installers' half is `tests/install.rs`.
//!
//! The accept path needs a release signed by a trusted key, and the project's
//! private key is not something a test holds. `SEMLITH_EXTRA_RELEASE_KEY` adds
//! one throwaway public key to the trusted list, in a debug build only.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
mod unix {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::collections::HashMap;
    use std::net::TcpListener;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const NEWER: &str = "v99.0.0";
    const MARKER: &str = "semlith 99.0.0 from the s06 fixture";

    /// A loopback HTTP server over a fixed route table, counting requests.
    /// One thread, one request per connection: `upgrade` asks for three files.
    fn serve(routes: HashMap<String, Vec<u8>>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&hits);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                counter.fetch_add(1, Ordering::SeqCst);
                let mut stream = stream;
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).is_err() {
                    continue;
                }
                let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
                // Drain the head first: answering mid-write is a reset on macOS.
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
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        (format!("http://127.0.0.1:{port}"), hits)
    }

    fn target() -> &'static str {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
            ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
            ("macos", "aarch64") => "aarch64-apple-darwin",
            (os, arch) => panic!("no semlith release target for {os}/{arch}"),
        }
    }

    /// Whether `ssh-keygen` can sign, which is what generating the fixture
    /// needs. Absent, the test says so and passes rather than failing on a
    /// tool this machine lacks.
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

    /// A throwaway ed25519 key: the private key's path and the public line.
    fn keygen(dir: &Path, name: &str) -> (PathBuf, String) {
        let key = dir.join(name);
        let out = Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
            .arg(&key)
            .output()
            .unwrap();
        assert!(out.status.success(), "ssh-keygen failed: {out:?}");
        let public = std::fs::read_to_string(key.with_extension("pub")).unwrap();
        (key, public.trim().to_string())
    }

    /// `ssh-keygen -Y sign`, exactly as `release.yml` runs it.
    fn sign(key: &Path, message: &[u8], namespace: &str, dir: &Path) -> Vec<u8> {
        let file = dir.join(format!("message-{namespace}"));
        std::fs::write(&file, message).unwrap();
        let sig = dir.join(format!("message-{namespace}.sig"));
        let _ = std::fs::remove_file(&sig);
        let out = Command::new("ssh-keygen")
            .args(["-Y", "sign", "-n", namespace, "-f"])
            .arg(key)
            .arg(&file)
            .output()
            .unwrap();
        assert!(out.status.success(), "ssh-keygen -Y sign failed: {out:?}");
        std::fs::read(sig).unwrap()
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        dir: PathBuf,
        archive_name: String,
        archive: Vec<u8>,
        sums: Vec<u8>,
        project: (PathBuf, String),
        stranger: (PathBuf, String),
        home: PathBuf,
        binary: PathBuf,
    }

    /// A release shaped like `release.yml`'s, two keys, and the binary under
    /// test installed where `upgrade` agrees to replace it.
    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();
        let name = format!("semlith-{NEWER}-{}", target());
        let staged = dir.join("stage").join(&name);
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(
            staged.join("semlith"),
            format!("#!/bin/sh\necho '{MARKER}'\n"),
        )
        .unwrap();
        Command::new("chmod")
            .arg("755")
            .arg(staged.join("semlith"))
            .status()
            .unwrap();
        let archive_name = format!("{name}.tar.gz");
        let tar = Command::new("tar")
            .arg("czf")
            .arg(dir.join(&archive_name))
            .arg("-C")
            .arg(dir.join("stage"))
            .arg(&name)
            .status()
            .unwrap();
        assert!(tar.success());
        let archive = std::fs::read(dir.join(&archive_name)).unwrap();
        let digest: String = Sha256::digest(&archive)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let sums = format!("{digest}  {archive_name}\n").into_bytes();

        let project = keygen(&dir, "project");
        let stranger = keygen(&dir, "stranger");

        let home = dir.join("home");
        let bin_dir = home.join(".semlith").join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let binary = bin_dir.join("semlith");
        // `cp`, not `fs::copy`: a write handle this process held could leak
        // into a parallel test's fork and make the exec ETXTBSY (#135).
        let cp = Command::new("cp").arg(bin()).arg(&binary).status().unwrap();
        assert!(cp.success());
        Fixture {
            _dir: tmp,
            dir,
            archive_name,
            archive,
            sums,
            project,
            stranger,
            home,
            binary,
        }
    }

    impl Fixture {
        fn routes(&self, sums: &[u8], sig: Option<Vec<u8>>) -> HashMap<String, Vec<u8>> {
            let base = format!("/semlith/semlith/releases/download/{NEWER}");
            let mut routes = HashMap::from([
                (
                    format!("{base}/{}", self.archive_name),
                    self.archive.clone(),
                ),
                (format!("{base}/SHA256SUMS"), sums.to_vec()),
            ]);
            if let Some(sig) = sig {
                routes.insert(format!("{base}/SHA256SUMS.sig"), sig);
            }
            routes
        }

        /// `semlith upgrade --version v99.0.0` against `routes`, trusting the
        /// fixture's "project" key on top of the real one.
        fn upgrade(&self, routes: HashMap<String, Vec<u8>>) -> (Output, usize) {
            let (origin, hits) = serve(routes);
            let mut cmd = Command::new(&self.binary);
            cmd.args(["upgrade", "--version", NEWER])
                .env("HOME", &self.home)
                .env("USERPROFILE", &self.home)
                .env("SEMLITH_HOME", self.home.join(".semlith"))
                .env("SEMLITH_RELEASES_ORIGIN", &origin)
                .env("SEMLITH_EXTRA_RELEASE_KEY", &self.project.1)
                .env_remove("SEMLITH_AIRGAP");
            let out = cmd.output().expect("semlith upgrade runs");
            let hits = hits.load(Ordering::SeqCst);
            assert!(
                hits > 0,
                "the fixture was never asked, so this binary ignored SEMLITH_RELEASES_ORIGIN \
                 (a release build): run this test against a debug build.\nstderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
            (out, hits)
        }

        fn refused(&self, routes: HashMap<String, Vec<u8>>, why: &str, says: &[&str]) {
            let before = std::fs::read(&self.binary).unwrap();
            let (out, _) = self.upgrade(routes);
            let said = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            assert!(
                !out.status.success(),
                "{why}: the upgrade was accepted\n{said}"
            );
            assert!(
                says.iter().any(|s| said.contains(s)),
                "{why}: refused, but not for the signature ({says:?}):\n{said}"
            );
            assert_eq!(
                std::fs::read(&self.binary).unwrap(),
                before,
                "{why}: the binary was replaced"
            );
            assert!(
                !self.binary.with_extension("old").exists(),
                "{why}: the binary was moved aside"
            );
            // Nothing was unpacked beside the binary either.
            let left: Vec<_> = std::fs::read_dir(self.binary.parent().unwrap())
                .unwrap()
                .flatten()
                .map(|e| e.file_name())
                .collect();
            assert_eq!(left, vec!["semlith"], "{why}: files were left behind");
        }
    }

    /// The v0.39.0 behaviour: a matching SHA256SUMS with no signature at all
    /// was enough to replace the binary.
    #[test]
    fn an_unsigned_release_is_refused() {
        if !have_ssh_keygen() {
            return;
        }
        let f = fixture();
        f.refused(
            f.routes(&f.sums, None),
            "no SHA256SUMS.sig",
            &["SHA256SUMS.sig"],
        );
    }

    #[test]
    fn a_release_signed_by_another_key_is_refused() {
        if !have_ssh_keygen() {
            return;
        }
        let f = fixture();
        let sig = sign(&f.stranger.0, &f.sums, "file", &f.dir);
        f.refused(
            f.routes(&f.sums, Some(sig)),
            "signed by an untrusted key",
            &["not signed by a trusted", "untrusted key"],
        );
    }

    /// The signature is the project key's, but over a different SHA256SUMS:
    /// exactly what an attacker who swapped the archive and the sums but not
    /// the signature serves.
    #[test]
    fn a_signature_over_other_sums_is_refused() {
        if !have_ssh_keygen() {
            return;
        }
        let f = fixture();
        let sig = sign(
            &f.project.0,
            b"0000  something-else.tar.gz\n",
            "file",
            &f.dir,
        );
        f.refused(
            f.routes(&f.sums, Some(sig)),
            "signature over other bytes",
            &["signature does not match"],
        );
    }

    /// A signature made for another purpose (`-n git`, say) is not a release
    /// signature, even from the right key.
    #[test]
    fn a_signature_in_another_namespace_is_refused() {
        if !have_ssh_keygen() {
            return;
        }
        let f = fixture();
        let sig = sign(&f.project.0, &f.sums, "git", &f.dir);
        f.refused(
            f.routes(&f.sums, Some(sig)),
            "namespace git",
            &["namespace"],
        );
    }

    #[test]
    fn garbage_in_place_of_a_signature_is_refused() {
        if !have_ssh_keygen() {
            return;
        }
        let f = fixture();
        f.refused(
            f.routes(
                &f.sums,
                Some(b"-----BEGIN SSH SIGNATURE-----\nAAAA\n".to_vec()),
            ),
            "unparseable signature",
            &["SHA256SUMS.sig"],
        );
    }

    /// The accept path, so the refusals above are refusals of the signature and
    /// not of everything: signed by a trusted key, the release installs and the
    /// signer's key id is said.
    #[test]
    fn a_release_signed_by_a_trusted_key_installs() {
        if !have_ssh_keygen() {
            return;
        }
        let f = fixture();
        let sig = sign(&f.project.0, &f.sums, "file", &f.dir);
        let (out, _) = f.upgrade(f.routes(&f.sums, Some(sig)));
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            out.status.success(),
            "a signed release was refused:\n{said}"
        );
        let fingerprint = Command::new("ssh-keygen")
            .arg("-lf")
            .arg(f.project.0.with_extension("pub"))
            .output()
            .unwrap();
        let id = String::from_utf8_lossy(&fingerprint.stdout)
            .split_whitespace()
            .nth(1)
            .unwrap()
            .to_string();
        assert!(
            said.contains(&id),
            "the upgrade did not log the signer's key id {id}:\n{said}"
        );
        let ran = Command::new(&f.binary).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&ran.stdout).trim(), MARKER);
    }
}

/// `semlith --version` names the key this binary trusts, as the Security page
/// says it does, and its first line is still the bare version scripts read.
#[test]
fn the_version_names_the_signing_key() {
    let out = Command::new(bin()).arg("--version").output().unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    let mut lines = said.lines();
    assert_eq!(
        lines.next(),
        Some(format!("semlith {}", env!("CARGO_PKG_VERSION")).as_str())
    );
    assert!(
        said.contains(semlith::upgrade::SIGNING_KEY_ID),
        "--version does not name the signing key:\n{said}"
    );
}
