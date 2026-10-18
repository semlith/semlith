//! S24: a store outside the semlith home is private, checked before it is
//! written, and keeps no token beside it; its ledger chain is keyed.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s24_a_store_outside_the_home_is_private_and_its_ledger_is_keyed() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let corpus = dir.path().join("corpus");
    let store = corpus.join(".semlith");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&corpus).unwrap();
    std::fs::write(
        corpus.join("notes.md"),
        "# Notes\n\nThe deploy runs nightly.\n",
    )
    .unwrap();
    let s = store.to_str().unwrap();
    let out = run(
        &home,
        &["--store", s, "index", corpus.to_str().unwrap()],
        &[],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = run(&home, &["--store", s, "search", "deploy"], &[]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    for path in walk(&store) {
        assert_eq!(
            mode(&path) & 0o077,
            0,
            "{} is {:o}",
            path.display(),
            mode(&path)
        );
    }
    for sub in [store.clone(), store.join("index")] {
        if sub.is_dir() {
            assert_eq!(
                mode(&sub) & 0o077,
                0,
                "{} is {:o}",
                sub.display(),
                mode(&sub)
            );
        }
    }
    let key = store.join("ledger.key");
    assert!(key.is_file(), "no ledger key beside the store");
    let ledger = run(&home, &["--store", s, "ledger", "--json"], &[]);
    let ledger = String::from_utf8_lossy(&ledger.stdout).into_owned();
    // Chain v2 (0.42.0, 3b61a5d) writes keyed rows as `k2:`; `k1:` is the
    // 0.41.0 formula, which still verifies on rows it wrote but is no longer
    // written.
    assert!(
        ledger.contains("\"k2:"),
        "the ledger's rows are not keyed:\n{ledger}"
    );

    // Damage the database where its pages are, and forget the check: the
    // next open refuses it before writing anything.
    std::fs::remove_file(store.join(".integrity-checked")).unwrap();
    let db = store.join("store.db");
    let mut bytes = std::fs::read(&db).unwrap();
    for b in bytes.iter_mut().skip(4096).take(4096) {
        *b = 0xA5;
    }
    std::fs::write(&db, bytes).unwrap();
    let _ = std::fs::remove_file(store.join("store.db-wal"));
    let _ = std::fs::remove_file(store.join("store.db-shm"));
    let out = run(&home, &["--store", s, "search", "deploy"], &[]);
    let said = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(!out.status.success(), "a damaged foreign store opened");
    assert!(
        said.contains("integrity"),
        "refused for another reason: {said}"
    );
}
