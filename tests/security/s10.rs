//! S10: a refused secret leaves no searchable trace.

use super::*;

/// Every byte of every file in the store directory.
fn store_bytes(home: &Path) -> Vec<u8> {
    let mut all = Vec::new();
    for path in walk(&home.join(".semlith")) {
        if let Ok(bytes) = std::fs::read(path) {
            all.extend(bytes);
        }
    }
    all
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s10_a_file_refused_for_a_secret_leaves_no_trace_in_the_store() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    let file = corpus.join("deploy.md");
    // A credential already in the store: indexed with the scan off, as a
    // store indexed before a rule widened holds one.
    let token = "ghp_R8x2Kq9LmP4vTz7Wn3Yb6Hc1Jd5Fg0Se2Uaq";
    std::fs::write(
        &file,
        format!("# Deploy\n\nzqmarker7 uses {token} to push.\n"),
    )
    .unwrap();
    let out = run(
        &home,
        &["index", "--include-secrets", corpus.to_str().unwrap()],
        &[],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        contains(&store_bytes(&home), b"zqmarker7"),
        "the plant did not reach the store"
    );

    // Edited and indexed again under the rules: the file is refused and its
    // earlier contents leave.
    std::fs::write(
        &file,
        format!("# Deploy\n\nzqmarker7 uses {token} to push.\nAnd a new line.\n"),
    )
    .unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(&home, &["refused", "--json"], &[]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("deploy.md"),
        "the file was not refused"
    );

    let bytes = store_bytes(&home);
    assert!(
        !contains(&bytes, b"zqmarker7"),
        "a term of the refused file is still in the store's files"
    );
    assert!(
        !contains(&bytes, token.as_bytes()),
        "the secret is still in the store's files"
    );
}
