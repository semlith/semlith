//! S22: `add` refuses credentials in a URL and understands an IPv6 literal.

#[allow(unused_imports)]
use super::*;

fn add(url: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = run(&home, &["add", url, "--name", "web"], &[]);
    assert!(!out.status.success(), "add accepted {url}");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn s22_credentials_in_a_url_are_refused_before_anything_is_fetched() {
    let said = add("https://zquser:zqpass@example.invalid/notes.md");
    assert!(said.contains("credentials"), "{said}");
    assert!(!said.contains("zqpass"), "the password was echoed: {said}");
}

#[test]
fn s22_an_ipv6_literal_is_checked_as_an_address() {
    if std::env::var_os("SEMLITH_ADD_ALLOW_PRIVATE").is_some() {
        return;
    }
    let said = add("https://[::1]:9/notes.md");
    assert!(said.contains("not on the public internet"), "{said}");
}
