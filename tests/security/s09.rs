//! S09: rewriting a client config keeps its mode and writes through a symlink.
//!
//! Driven through the portal's Register all (`POST /api/agents/register`) on a
//! scratch daemon, which rewrites every client file present in the scratch
//! home: the same writer `setup --register-all`, the hooks, the rules and the
//! shell rc go through. Unix only: modes and symlinks are what it is about.

#[allow(unused_imports)]
use super::*;

#[cfg(unix)]
#[test]
fn register_all_keeps_modes_and_links() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = tempfile::tempdir().unwrap();
    let home = scratch.path();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let private = |p: &Path, text: &str| {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600)).unwrap();
    };

    // Narrowed to 0600 because it holds another server's token.
    let opencode = home.join(".config/opencode/opencode.json");
    private(
        &opencode,
        r#"{"mcp":{"other":{"headers":{"Authorization":"Bearer theirs"}}}}"#,
    );
    // A dotfiles user's link into a repository.
    let real = home.join("dotfiles/cursor-mcp.json");
    private(&real, "{}");
    let cursor = home.join(".cursor/mcp.json");
    std::fs::create_dir_all(cursor.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, &cursor).unwrap();
    // Present, with no file yet: one is created.
    let zed = home.join(".config/zed/settings.json");
    std::fs::create_dir_all(zed.parent().unwrap()).unwrap();

    let daemon = Daemon::start(home, &[]);
    let (status, body) = daemon.send(
        "POST",
        "/api/agents/register",
        &format!("Semlith-Token: {}\r\n", daemon.token),
        r#"{"confirm":true}"#,
    );
    assert_eq!(status, 200, "{body}");

    // Every failure at once, so a red run shows each defect, not the first.
    let mut wrong = Vec::new();
    if !std::fs::symlink_metadata(&cursor)
        .unwrap()
        .file_type()
        .is_symlink()
    {
        wrong.push("the link was replaced by a regular file".to_string());
    }
    for (path, what) in [
        (&opencode, "a 0600 config"),
        (&real, "the link's target"),
        (&zed, "a new config"),
    ] {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        if !text.contains("semlith") {
            wrong.push(format!("{what} was not registered: {text}"));
        } else if mode(path) != 0o600 {
            wrong.push(format!("{what} is {:o}, not 600", mode(path)));
        }
    }
    if !std::fs::read_to_string(&opencode)
        .unwrap()
        .contains("theirs")
    {
        wrong.push("the other server's entry is gone".to_string());
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
