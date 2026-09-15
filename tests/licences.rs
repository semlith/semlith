//! Forty-six parsers are compiled into this binary, and every one of them
//! carries a licence.
//!
//! Two things have to stay true for that to be safe. The licence has to be
//! permissive — a GPL or LGPL grammar would take the whole Apache-2.0 crate
//! with it, which is a licensing change nobody would notice in a dependency
//! bump. And the notices those licences require have to actually ship, which
//! means `THIRD-PARTY-NOTICES` has to still describe the grammars that are in
//! the tree rather than the ones that were in it when somebody last looked.
//!
//! Both are checked here rather than trusted.
//!
//! ```sh
//! SEMLITH_WRITE_NOTICES=1 cargo test --test licences
//! ```
//!
//! rewrites the file instead of comparing against it, which is how it is
//! regenerated after a grammar is added, removed or bumped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Licences a grammar may ship under.
///
/// Permissive only: each of these permits use in a closed or commercially
/// licensed work, and asks for nothing back beyond the notice this file
/// generates. Anything outside the list — copyleft, source-available, or a
/// licence nobody can identify — is refused rather than researched, because
/// the cost of being wrong is the licence of the whole binary.
const PERMISSIVE: &[&str] = &[
    "0BSD",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC0-1.0",
    "ISC",
    "MIT",
    "Unlicense",
    "Zlib",
];

struct Grammar {
    name: String,
    version: String,
    licence: String,
    source: String,
}

#[test]
fn every_grammar_is_permissively_licensed_and_its_notice_ships() {
    let grammars = grammars();
    assert!(
        grammars.len() >= 46,
        "only {} grammar crates found; the graph covers forty-six languages",
        grammars.len()
    );

    let mut refused: Vec<String> = Vec::new();
    for grammar in &grammars {
        if !grammar
            .licence
            .split(|c: char| !(c.is_alphanumeric() || c == '-' || c == '.'))
            .filter(|token| !token.is_empty())
            .any(|token| PERMISSIVE.contains(&token))
        {
            refused.push(format!(
                "{} {} is licensed {}, which is not in the permissive allowlist",
                grammar.name, grammar.version, grammar.licence
            ));
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));

    let path = repository_root().join("THIRD-PARTY-NOTICES");
    let expected = notices(&grammars);
    if std::env::var_os("SEMLITH_WRITE_NOTICES").is_some() {
        std::fs::write(&path, &expected).unwrap();
        return;
    }
    // Normalised, because git checks this file out with CRLF on Windows and the
    // generated string is built with LF. The bytes differ, the notice does not,
    // and a release is not going to be held up over a line ending.
    let actual = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert_eq!(
        actual, expected,
        "THIRD-PARTY-NOTICES no longer describes the grammars in the tree. \
         Regenerate it with SEMLITH_WRITE_NOTICES=1 cargo test --test licences"
    );
}

/// Every grammar crate in the dependency graph, from cargo's own metadata.
///
/// `tree-sitter` itself and `tree-sitter-language` are the runtime and the ABI
/// shim rather than grammars, so they are named in the file's preamble instead
/// of being listed as languages.
fn grammars() -> Vec<Grammar> {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--all-features"])
        .current_dir(repository_root())
        .output()
        .expect("cargo metadata runs");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();

    let mut found: BTreeMap<String, Grammar> = BTreeMap::new();
    for package in metadata["packages"].as_array().unwrap() {
        let name = package["name"].as_str().unwrap().to_string();
        let is_grammar = (name.starts_with("tree-sitter-") && name != "tree-sitter-language")
            || name == "ts-parser-perl";
        if !is_grammar {
            continue;
        }
        found.insert(
            name.clone(),
            Grammar {
                name,
                version: package["version"].as_str().unwrap().to_string(),
                licence: package["license"].as_str().unwrap_or("UNKNOWN").to_string(),
                source: package["repository"].as_str().unwrap_or("").to_string(),
            },
        );
    }
    found.into_values().collect()
}

fn notices(grammars: &[Grammar]) -> String {
    let mut out = String::from(
        "Third-party notices
===================

semlith is licensed under Apache-2.0. It links the tree-sitter parsing runtime
and one grammar per language its code graph covers, and each grammar's parser is
compiled into the binary. Those grammars are listed below with the licence they
are distributed under and the repository they come from.

Every grammar here is under a permissive licence. `tests/licences.rs` refuses
any that is not, so a grammar added or bumped in future cannot quietly change
what this binary may be used for.

The tree-sitter runtime itself (`tree-sitter`) and the ABI shim every grammar
exposes its parser through (`tree-sitter-language`) are MIT licensed and are
covered by the same terms.

Grammars
--------

",
    );
    for grammar in grammars {
        out.push_str(&format!("{} {}\n", grammar.name, grammar.version));
        out.push_str(&format!("    Licence: {}\n", grammar.licence));
        if !grammar.source.is_empty() {
            out.push_str(&format!("    Source:  {}\n", grammar.source));
        }
        out.push('\n');
    }
    out
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}
