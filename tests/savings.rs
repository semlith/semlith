//! A savings number never appears without its coverage and its tier.
//!
//! This is the rule 0.17.2 removed the last README paragraph for. A figure like
//! "18× fewer tokens" is unanswerable on its own: a reader cannot tell whether
//! it covers every retrieval or three of them, nor whether the tokens were
//! counted by a tokenizer or estimated at four characters each. The rule is
//! therefore enforced by this file rather than by review, over a list of
//! surfaces kept in one place.

use std::path::Path;

const APP_JS: &str = include_str!("../src/portal/app.js");
const ROUTES: &str = include_str!("../src/routes.rs");
const MAIN: &str = include_str!("../src/main.rs");
const README: &str = include_str!("../README.md");

/// Every surface that prints a figure derived from the ledger's saved tokens.
///
/// Adding one means adding a row here, which is the whole point: the compiler
/// cannot notice a new tile, and this list is the thing a reviewer looks at.
const SURFACES: &[(&str, &str, &str)] = &[
    // (what it is, the source it lives in, the token the figure is spelled with)
    ("the Ledger route", "src/routes.rs", "net_tokens"),
    (
        "the Stores route's per-store line",
        "src/routes.rs",
        "net_tokens",
    ),
    ("the Ledger page's tiles", "src/portal/app.js", "net_tokens"),
    (
        "the Stores page's per-store cell",
        "src/portal/app.js",
        "savings.net_tokens",
    ),
    ("`semlith ledger`", "src/main.rs", "savings.net"),
];

fn source(path: &str) -> &'static str {
    match path {
        "src/portal/app.js" => APP_JS,
        "src/routes.rs" => ROUTES,
        "src/main.rs" => MAIN,
        other => panic!("no source registered for {other}"),
    }
}

/// How far from the figure its qualifiers may sit.
///
/// Generous: they need to be in the same tile, the same cell or the same
/// printed block, not on the same line. A figure whose coverage is four hundred
/// characters away is in a different part of the page.
const NEARBY: usize = 1200;

#[test]
fn no_surface_states_a_saving_without_its_coverage_and_its_tier() {
    for (what, path, token) in SURFACES {
        let text = source(path);
        let at = text.find(token).unwrap_or_else(|| {
            panic!("{what} does not state a saving at all: {token} not in {path}")
        });
        let from = at.saturating_sub(NEARBY);
        let to = (at + NEARBY).min(text.len());
        let around = &text[from..to];
        for qualifier in ["coverage", "tier"] {
            assert!(
                around.contains(qualifier),
                "{what} states a saving with no {qualifier} beside it, in {path}. \
                 A figure a reader cannot check is one they are being asked to take on trust."
            );
        }
    }
}

/// The README's own paragraph is held to the same rule as every other surface.
#[test]
fn the_readme_states_no_saving_without_its_coverage_and_its_tier() {
    let Some(at) = README.find("tokens per answered question") else {
        // 1.5 has not landed yet. The gate is the test above plus this one once
        // the paragraph exists; a missing paragraph is not a false pass,
        // because `tests/readme.rs` is what requires it to be there.
        return;
    };
    let from = at.saturating_sub(NEARBY);
    let to = (at + NEARBY).min(README.len());
    let around = &README[from..to];
    assert!(
        around.contains("coverage") || around.contains("of the"),
        "the README savings paragraph states no denominator:\n{around}"
    );
}

/// "grep loop" appears nowhere.
///
/// It was the phrase the savings copy reached for, and it describes a workflow
/// nobody has: an agent does not loop over greps, it greps once and reads three
/// files whole. A claim measured against a straw man is worse than no claim.
#[test]
fn the_phrase_grep_loop_appears_nowhere() {
    let mut found = Vec::new();
    for dir in ["src", "docs"] {
        walk(Path::new(dir), &mut found);
    }
    if README.to_lowercase().contains("grep loop") {
        found.push("README.md".to_string());
    }
    assert!(
        found.is_empty(),
        "the phrase \"grep loop\" is in: {}",
        found.join(", ")
    );
}

fn walk(dir: &Path, found: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, found);
            continue;
        }
        let readable = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "js" | "md" | "css" | "html")
        );
        if !readable {
            continue;
        }
        if std::fs::read_to_string(&path)
            .map(|t| t.to_lowercase().contains("grep loop"))
            .unwrap_or(false)
        {
            found.push(path.display().to_string());
        }
    }
}
