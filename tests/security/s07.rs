//! S07: indexing a repository never runs a program its git config names.

use super::*;

/// A repository whose config would make any `git log` run `gpg.program`: the
/// head commit carries a signature, `log.showSignature` asks for it to be
/// checked, and the "gpg" is a script that leaves a marker file.
#[cfg(unix)]
fn planted_repo(repo: &Path, marker: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(repo).unwrap();
    std::fs::write(
        repo.join("README.md"),
        "# A repository\n\nWith a planted config.\n",
    )
    .unwrap();
    let git = |args: &[&str], input: Option<&str>| {
        let mut cmd = Command::new("git");
        cmd.arg("-C")
            .arg(repo)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if input.is_some() {
            cmd.stdin(Stdio::piped());
        }
        let mut child = cmd.spawn().ok()?;
        if let Some(text) = input {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(text.as_bytes())
                .ok()?;
        }
        let out = child.wait_with_output().ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    if git(&["init", "-q"], None).is_none() {
        return false;
    }
    git(&["add", "."], None);
    let tree = git(&["write-tree"], None).unwrap();
    let commit = format!(
        "tree {tree}\nauthor t <t@t> 0 +0000\ncommitter t <t@t> 0 +0000\n\
         gpgsig -----BEGIN PGP SIGNATURE-----\n \n iQEzBAABCAAdFiEE\n -----END PGP SIGNATURE-----\n\nsigned\n"
    );
    let id = git(
        &["hash-object", "-t", "commit", "-w", "--stdin"],
        Some(&commit),
    )
    .unwrap();
    git(&["update-ref", "HEAD", &id], None).unwrap();
    let gpg = repo.join("fake-gpg.sh");
    std::fs::write(
        &gpg,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&gpg, std::fs::Permissions::from_mode(0o755)).unwrap();
    git(&["config", "log.showSignature", "true"], None);
    git(&["config", "gpg.program", gpg.to_str().unwrap()], None);
    // The plant works: a plain `git log` runs the program.
    let _ = git(&["log", "-n", "1"], None);
    let armed = marker.exists();
    let _ = std::fs::remove_file(marker);
    armed
}

#[cfg(unix)]
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s07_a_repo_config_cannot_make_indexing_run_a_program() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    let marker = dir.path().join("ran");
    std::fs::create_dir_all(&home).unwrap();
    if !planted_repo(&repo, &marker) {
        eprintln!("no git that honours the plant on this machine; nothing to prove");
        return;
    }
    let out = run(&home, &["index", repo.to_str().unwrap()], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // A second pass takes the recency path as well as the first-import one.
    std::fs::write(repo.join("README.md"), "# Changed\n").unwrap();
    let _ = run(&home, &["index", repo.to_str().unwrap()], &[]);
    assert!(
        !marker.exists(),
        "indexing ran the repository's gpg.program"
    );
}
