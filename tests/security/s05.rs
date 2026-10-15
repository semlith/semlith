//! S05: read never serves a file outside the store through a symlink.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s05_a_file_swapped_for_a_symlink_is_not_served() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::create_dir_all(&outside).unwrap();
    let file = corpus.join("notes.md");
    std::fs::write(&file, "# Notes\n\nNothing to see here, line three.\n").unwrap();
    std::fs::write(
        outside.join("config"),
        "OUTSIDE-THE-STORE kube token line\n",
    )
    .unwrap();
    let out = run(&home, &["index", corpus.to_str().unwrap()], &[]);
    assert!(out.status.success());

    // After indexing, the file becomes a link to something the store never held.
    std::fs::remove_file(&file).unwrap();
    std::os::unix::fs::symlink(outside.join("config"), &file).unwrap();

    let out = run(&home, &["read", &format!("{}:1-3", file.display())], &[]);
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        !said.contains("OUTSIDE-THE-STORE"),
        "read served the link's target:\n{said}"
    );
}
