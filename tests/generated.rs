//! What a corpus is, and what it is not.
//!
//! `.gitignore` is a statement about what is committed, and semlith needs one
//! about what was generated. The two agree in a clean checkout and part company
//! in every folder somebody downloaded, every directory where `npm install` ran
//! outside a repository, and every machine whose `node_modules` lives in a
//! global gitignore the walk cannot see. A corpus that swallows a dependency
//! tree is wrong twice: it holds chunks nobody asked about, and every one of
//! its symbols is a graph node with no edge into the project.
//!
//! No model is needed to ask what the walk yields, so none of this is ignored.

use semlith::is_generated_dir;
use std::fs;
use std::path::Path;

fn tree(dir: &Path, files: &[&str]) {
    for file in files {
        let at = dir.join(file);
        fs::create_dir_all(at.parent().unwrap()).unwrap();
        fs::write(&at, "fn placeholder() {}\n").unwrap();
    }
}

/// The names that are never somebody's own source, in every ecosystem semlith
/// supports. A Node project with no `.gitignore` at all is the reported case.
#[test]
fn a_generated_directory_is_not_part_of_the_corpus() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    tree(
        root,
        &[
            "src/index.js",
            "node_modules/left-pad/index.js",
            "node_modules/.bin/tsc",
            "app/__pycache__/thing.cpython-312.pyc",
            "app/.venv/lib/python3.12/site-packages/requests/api.py",
            "android/.gradle/8.5/checksums.bin",
            "ios/DerivedData/Build/Products/thing.swiftmodule",
            "flutter/.dart_tool/package_config.json",
        ],
    );

    for generated in [
        "node_modules",
        "app/__pycache__",
        "app/.venv",
        "android/.gradle",
        "ios/DerivedData",
        "flutter/.dart_tool",
    ] {
        assert!(
            is_generated_dir(&root.join(generated)),
            "{generated} was treated as somebody's own source"
        );
    }
    assert!(
        !is_generated_dir(&root.join("src")),
        "a source directory was treated as generated"
    );
}

/// The names that are sometimes somebody's own. `target` beside a `Cargo.toml`
/// is cargo's; `target` on its own is whatever the person meant by it, and
/// deleting it from their corpus on a name match alone would be semlith
/// deciding something it has no evidence for.
#[test]
fn a_conditional_directory_needs_its_manifest_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();

    // With the manifest: generated.
    tree(root, &["rust/Cargo.toml", "rust/target/debug/thing"]);
    tree(root, &["go/go.mod", "go/vendor/github.com/x/y.go"]);
    tree(root, &["jvm/build.gradle", "jvm/build/classes/Thing.class"]);
    tree(root, &["node/package.json", "node/dist/bundle.js"]);
    for generated in ["rust/target", "go/vendor", "jvm/build", "node/dist"] {
        assert!(
            is_generated_dir(&root.join(generated)),
            "{generated} sits beside its manifest and is still indexed"
        );
    }

    // Without it: somebody's own, and kept.
    tree(root, &["plain/build/make.sh", "plain/vendor/theirs.rb"]);
    tree(root, &["notes/target/audience.md"]);
    for kept in ["plain/build", "plain/vendor", "notes/target"] {
        assert!(
            !is_generated_dir(&root.join(kept)),
            "{kept} has no manifest beside it and was dropped from the corpus anyway"
        );
    }
}

/// The dotnet ecosystem names its project file after the project, so the
/// manifest is a pattern rather than a name.
#[test]
fn a_dotnet_project_file_is_matched_by_its_extension() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    tree(root, &["api/Api.csproj", "api/bin/Debug/Api.dll"]);
    tree(root, &["scripts/bin/deploy.sh"]);

    assert!(is_generated_dir(&root.join("api/bin")));
    assert!(
        !is_generated_dir(&root.join("scripts/bin")),
        "a bin directory with no project file beside it is somebody's scripts"
    );
}

/// The table is an opinion, and a user whose corpus really is a vendored tree
/// has to be able to overrule it.
///
/// Read through the pure half rather than by setting the variable: a test that
/// mutates the process environment races every other test in the binary, and
/// this one did exactly that before it was written this way.
#[test]
fn the_table_can_be_switched_off() {
    use semlith::default_ignores_on;
    for off in ["0", "off", "false"] {
        assert!(
            !default_ignores_on(Some(off)),
            "SEMLITH_DEFAULT_IGNORES={off} did not turn the table off"
        );
    }
    for on in [None, Some("1"), Some(""), Some("yes")] {
        assert!(
            default_ignores_on(on),
            "SEMLITH_DEFAULT_IGNORES={on:?} turned the table off and should not have"
        );
    }
}

/// The whole point, end to end: a Node project with no `.gitignore` at all is
/// indexed, and nothing under `node_modules` is in the store afterwards.
///
/// The unit tests above prove the table; this proves the walk uses it, which is
/// the part that was actually broken.
#[test]
#[ignore = "indexes, so it downloads an embedding model on first run"]
fn a_node_project_with_no_gitignore_indexes_without_its_dependencies() {
    let dir = tempfile::tempdir().unwrap();
    let corpus = dir.path().join("app");
    let store_dir = dir.path().join("store");
    fs::create_dir_all(&corpus).unwrap();
    fs::write(corpus.join("package.json"), "{ \"name\": \"app\" }\n").unwrap();
    tree(
        &corpus,
        &[
            "src/index.js",
            "src/util.js",
            "node_modules/left-pad/index.js",
            "node_modules/react/react.js",
            "dist/bundle.js",
        ],
    );
    assert!(
        !corpus.join(".gitignore").exists(),
        "the fixture must have no .gitignore: that is the case that was broken"
    );

    let mut s = semlith::Semlith::open(&store_dir, None).unwrap();
    s.quiet = true;
    let report = s
        .index_paths(std::slice::from_ref(&corpus), |_, _| {})
        .unwrap();

    let held = semlith::store::filtered_paths(s.db(), &[]).unwrap();
    assert!(
        held.iter().any(|p| p.ends_with("src/index.js")),
        "the project's own source was not indexed: {held:?}"
    );
    assert!(
        !held.iter().any(|p| p.contains("node_modules")),
        "node_modules is in the store: {held:?}"
    );
    assert!(
        !held.iter().any(|p| p.contains("dist")),
        "a dist beside a package.json is generated output: {held:?}"
    );
    assert!(
        report.generated.iter().any(|p| p.contains("node_modules")),
        "the run did not say it had stepped over node_modules: {:?}",
        report.generated
    );
}
