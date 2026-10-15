//! S12: an ignore file cannot whitelist `.git` or a credential directory into
//! a store, and those directories are refused wherever they sit.

#[allow(unused_imports)]
use super::*;

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s12_a_whitelist_cannot_reinclude_git_or_a_credential_directory() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&home).unwrap();
    for sub in [".git", ".aws", ".github/workflows", "src"] {
        std::fs::create_dir_all(repo.join(sub)).unwrap();
    }
    // What a hostile or careless repository ships: a whitelist for git's own
    // state and a copied credentials folder, beside an ordinary dotfile the
    // 0.19.0 whitelist is for.
    std::fs::write(repo.join(".gitignore"), "!.git/\n!.aws/\n!.github/\n").unwrap();
    std::fs::write(
        repo.join(".git").join("config"),
        "[remote \"origin\"]\n\turl = https://zqgitmarker@example.invalid/x.git\n",
    )
    .unwrap();
    std::fs::write(repo.join(".aws").join("notes.md"), "# zqawsmarker\n").unwrap();
    std::fs::write(
        repo.join(".github").join("workflows").join("ci.yml"),
        "name: zqcimarker\non: push\n",
    )
    .unwrap();
    std::fs::write(repo.join("src").join("main.rs"), "fn main() {}\n").unwrap();

    let out = run(&home, &["index", repo.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let files = run(&home, &["files"], &[]);
    let files = String::from_utf8_lossy(&files.stdout).into_owned();
    assert!(files.contains("main.rs"), "the repository was not indexed:\n{files}");
    assert!(
        files.contains("ci.yml"),
        "a whitelisted ordinary dotfile stopped being indexed:\n{files}"
    );
    assert!(!files.contains("config"), "a file under .git was indexed:\n{files}");
    assert!(!files.contains("notes.md"), "a file under .aws was indexed:\n{files}");
}

#[test]
#[ignore = "indexes, so it needs the embedding model"]
fn s12_a_named_hidden_root_is_walked_without_its_credential_directories() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let config = home.join(".config");
    std::fs::create_dir_all(config.join("gh")).unwrap();
    std::fs::create_dir_all(config.join("tool")).unwrap();
    std::fs::write(
        config.join("gh").join("hosts.yml"),
        "github.com:\n  user: zqghmarker\n",
    )
    .unwrap();
    std::fs::write(config.join("tool").join("settings.toml"), "theme = \"dark\"\n").unwrap();

    let out = run(&home, &["index", config.to_str().unwrap()], &[]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let files = run(&home, &["files"], &[]);
    let files = String::from_utf8_lossy(&files.stdout).into_owned();
    assert!(files.contains("settings.toml"), "the named root was not walked:\n{files}");
    assert!(!files.contains("hosts.yml"), "~/.config/gh was indexed:\n{files}");
}
