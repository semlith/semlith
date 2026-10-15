//! S20: the developer overrides are not read by a release binary.
//!
//! Needs a release binary under test, since a debug build reads them by
//! design: `SEMLITH_TEST_BIN=target/release/semlith`.

#[allow(unused_imports)]
use super::*;

#[test]
#[ignore = "needs a release binary: SEMLITH_TEST_BIN=target/release/semlith"]
fn s20_a_release_binary_ignores_the_add_origin_override() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    // A plain-http URL the override would have let through. Nothing listens
    // there: the refusal has to come before any connection.
    let out = run(
        &home,
        &["add", "http://127.0.0.1:9/zq.md", "--name", "web"],
        &[("SEMLITH_ADD_ORIGIN", "http://127.0.0.1:9/")],
    );
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out.status.success(), "add accepted plain http:\n{said}");
    assert!(
        said.contains("https only"),
        "refused for another reason:\n{said}"
    );
}
