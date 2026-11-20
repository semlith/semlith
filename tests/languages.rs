//! `semlith languages --json` prints the same list the portal's `/api/languages`
//! serves, so a script can read it without parsing padded columns.
//!
//! No model and no store: the command reads only the built-in language table.

use std::process::Command;

#[test]
fn languages_json_lists_every_language_with_its_filenames() {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .args(["languages", "--json"])
        .output()
        .expect("semlith runs");
    assert!(out.status.success(), "exit: {:?}", out.status);

    let doc: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is one JSON document");
    let languages = doc["languages"].as_array().expect("a `languages` array");
    assert!(!languages.is_empty());

    for lang in languages {
        assert!(lang["name"].is_string(), "name in {lang}");
        assert!(lang["extensions"].is_array(), "extensions in {lang}");
        assert!(lang["filenames"].is_array(), "filenames in {lang}");
        assert!(lang["graph"].is_boolean(), "graph in {lang}");
    }

    let dockerfile = languages
        .iter()
        .find(|l| l["name"] == "dockerfile")
        .expect("dockerfile is listed");
    let filenames: Vec<&str> = dockerfile["filenames"]
        .as_array()
        .expect("filenames")
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    assert!(
        filenames.contains(&"dockerfile"),
        "dockerfile filenames: {filenames:?}"
    );
}
