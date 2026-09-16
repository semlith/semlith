//! The counts the README states, asserted against the code that defines them.
//!
//! An audit of the README before 0.17.2 found six numbers that had drifted
//! from their source — six languages where there were forty-six, ten agent
//! tools where there were twelve, a portal page that had been deleted, a binary
//! size two and a half times too small. Every one of them had been true when it
//! was written, and none of them failed anything when it stopped being true.
//!
//! So the README states its counts in one table and this file reads them back
//! out of it. A count that drifts fails the build, in the release that moved it,
//! rather than in an audit somebody gets round to.
//!
//! ```sh
//! cargo test --test readme
//! ```

use std::process::Command;

const README: &str = include_str!("../README.md");

/// The private modules' counts cannot be read through the library — `formats`
/// is private on purpose, and making it public to satisfy a test would be the
/// test changing the product. They are parsed out of the source instead, which
/// is the same file a reader would check.
const FORMATS_RS: &str = include_str!("../src/formats.rs");

/// The release workflow's build matrix, which is where "prebuilt targets" is a
/// fact rather than an intention.
const RELEASE_YML: &str = include_str!("../.github/workflows/release.yml");

/// The heading the table lives under, and the label each row is found by.
///
/// Matched on the row's text rather than on its position, so reordering the
/// table or rewording the prose around it changes nothing here. A label that no
/// longer appears fails loudly: a deleted row is how a count stops being
/// checked.
const TABLE_SECTION: &str = "## The numbers";

/// One row of the stated table: the text that identifies it, and the number it
/// must agree with.
fn stated(label: &str) -> usize {
    let section = README
        .split_once(TABLE_SECTION)
        .unwrap_or_else(|| panic!("the README has no {TABLE_SECTION:?} heading"))
        .1;
    // The table ends where the next top-level heading begins; a number further
    // down the document is not this table's and must not be read as it.
    let section = section
        .split_once("\n## ")
        .map_or(section, |(head, _)| head);

    let row = section
        .lines()
        .find(|line| line.starts_with('|') && line.contains(label))
        .unwrap_or_else(|| {
            panic!("no row of the README's counts table is labelled {label:?}; if the row was renamed, rename it here too, and if it was deleted, this is the count going unchecked")
        });

    let cell = row
        .split('|')
        .find_map(|cell| cell.trim().strip_prefix("**")?.strip_suffix("**"))
        .unwrap_or_else(|| panic!("the {label:?} row states no bold number: {row}"));

    cell.replace(['\u{a0}', ' '], "")
        .parse()
        .unwrap_or_else(|e| panic!("the {label:?} row's number does not parse: {cell:?} ({e})"))
}

/// Entries in a `const NAME: ... = [ ... ];` block, counted by quoted strings.
fn quoted_entries(source: &str, decl: &str) -> usize {
    let start = source
        .find(decl)
        .unwrap_or_else(|| panic!("the source has no {decl:?}"));
    let rest = &source[start + decl.len()..];
    let end = rest
        .find("\n];")
        .unwrap_or_else(|| panic!("{decl:?} has no closing bracket on its own line"));
    rest[..end].matches('"').count() / 2
}

#[test]
fn the_readme_states_the_number_of_languages_the_filter_holds() {
    assert_eq!(
        stated("languages searchable and graphed"),
        semlith::filter::LANGUAGES.len(),
        "the README's language count and filter::LANGUAGES disagree"
    );
}

#[test]
fn the_readme_states_the_number_of_edge_kinds_the_graph_stores() {
    assert_eq!(
        stated("edge kinds"),
        semlith::graph::KINDS.len(),
        "the README's edge-kind count and graph::KINDS disagree"
    );
}

#[test]
fn the_readme_states_the_number_of_document_formats_with_a_reader() {
    assert_eq!(
        stated("document formats with a reader"),
        quoted_entries(FORMATS_RS, "const HANDLED: &[&str] = &["),
        "the README's format count and formats::HANDLED disagree"
    );
}

#[test]
fn the_readme_states_the_number_of_image_types() {
    assert_eq!(
        stated("image types"),
        semlith::image::EXTENSIONS.len(),
        "the README's image-type count and image::EXTENSIONS disagree"
    );
}

#[test]
fn the_readme_states_the_number_of_mcp_tools() {
    let tools = semlith::mcp::tool_names();
    assert_eq!(
        stated("MCP tools"),
        tools.len(),
        "the README's tool count and mcp::tool_names disagree: {tools:?}"
    );
}

#[test]
fn the_readme_states_the_number_of_clients_with_a_stanza() {
    assert_eq!(
        stated("agent clients"),
        semlith::clients::clients().len(),
        "the README's client count and the stanzas parsed out of docs/clients.md disagree"
    );
}

/// The one count read off the built binary rather than out of a constant.
///
/// `--help` is what a user sees, and it is what `tests/portal.rs` reads for the
/// parity gate, so a command added without a README row fails in both places
/// for the same reason.
#[test]
fn the_readme_states_the_number_of_cli_commands() {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--help")
        .output()
        .expect("semlith --help runs");
    let help = String::from_utf8_lossy(&out.stdout).into_owned();

    let mut names = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        if line.trim_end().ends_with("Commands:") {
            in_commands = true;
            continue;
        }
        if in_commands {
            if line.trim().is_empty() {
                continue;
            }
            if !line.starts_with("  ") || line.trim_start().starts_with('-') {
                break;
            }
            if let Some(name) = line.split_whitespace().next() {
                names.push(name.to_string());
            }
        }
    }
    names.retain(|n| n != "help");
    assert!(
        names.len() >= 10,
        "only parsed {names:?} out of --help; the parser, not the CLI, is probably wrong"
    );

    assert_eq!(
        stated("CLI commands"),
        names.len(),
        "the README's command count and `semlith --help` disagree: {names:?}"
    );
}

#[test]
fn the_readme_states_the_number_of_prebuilt_targets() {
    let built = RELEASE_YML.matches("            target: ").count();
    assert!(
        built > 0,
        "the release workflow's matrix has no target rows"
    );
    assert_eq!(
        stated("prebuilt targets"),
        built,
        "the README's target count and release.yml's build matrix disagree"
    );
}

/// The release-shaped claims a grep can settle, kept here so they fail at the
/// same moment the counts do.
#[test]
fn the_readme_carries_no_release_specific_content() {
    for phrase in ["from 0.", "through 0.", "before 0.", "as of 0.", "coming"] {
        assert!(
            !README.contains(phrase),
            "the README contains {phrase:?}; a release named in it is a sentence that goes stale \
             on its own"
        );
    }
    assert!(
        !README.contains("cookie"),
        "the README mentions a cookie; the portal's token is a Semlith-Token request header"
    );
    assert!(
        README.contains("`Semlith-Token` request header"),
        "the README no longer says how the portal's session token travels"
    );
}

/// Under 500 lines, because the document that has to convert a first-time
/// reader was 1 816 of them and mostly a reference table.
#[test]
fn the_readme_is_short() {
    let lines = README.lines().count();
    assert!(
        lines < 500,
        "the README is {lines} lines; the ceiling is 500"
    );
}
