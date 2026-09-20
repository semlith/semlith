//! The five reports, the four formats, and the one rule that matters: a file
//! saved from the portal and a file written by `semlith report` are the same
//! bytes, because they come from the same generator.
//!
//! ```sh
//! cargo test --test report -- --ignored
//! ```

use semlith::Semlith;
use semlith::fleet::Fleet;
use std::path::{Path, PathBuf};
use std::process::Command;

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

fn cli(store: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store)
        .args(args)
        .output()
        .expect("the binary runs");
    assert!(
        out.status.success(),
        "semlith {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A stamp differs between two runs a second apart, so it is not what byte
/// equality is about. Everything else is.
fn without_the_stamp(text: &str) -> String {
    text.lines()
        .filter(|line| !line.contains("Generated 2") && !line.contains("\"generated\""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn corpus() -> (tempfile::TempDir, tempfile::TempDir) {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    write(
        corpus.path(),
        "a.rs",
        "fn start() { middle(); outside(); }\n",
    );
    write(corpus.path(), "b.rs", "fn middle() { finish(); }\n");
    write(corpus.path(), "c.rs", "fn finish() {}\n");
    write(
        corpus.path(),
        "notes.md",
        "# Notes\n\nThe lock is held by one writer.\n",
    );
    (corpus, store)
}

/// Every report generates in every format, and the CLI's bytes are the
/// generator's bytes.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn every_report_generates_in_every_format_and_the_cli_matches() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
        // A row, so the savings report has something to be over.
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client: "claude-code",
                session: "s1",
                tool: "search",
                query: "who holds the lock",
                hits: 2,
                micros: 5_000,
                excerpt_tokens: 120,
                whole_file_tokens: 3_000,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "q1",
            },
        )
        .unwrap();
    }

    let dirs: Vec<PathBuf> = vec![store.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    for (kind, title) in semlith::report::KINDS {
        for format in semlith::report::FORMATS {
            let generated = semlith::report::generate(&fleet, kind, "Sonnet 5")
                .unwrap()
                .render(format)
                .unwrap();
            assert!(generated.contains(title), "{kind}/{format} lost its title");
            let printed = cli(store.path(), &["report", kind, "--format", format]);
            assert_eq!(
                without_the_stamp(&printed),
                without_the_stamp(&generated),
                "`semlith report {kind} --format {format}` is not what the generator produced"
            );
        }
    }
}

/// The savings report keeps every field the design names, and never sums the
/// three counterfactual lines.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn the_savings_report_states_coverage_tier_and_the_trust_strip() {
    let (corpus, store) = corpus();
    {
        let mut s = Semlith::open(store.path(), None).unwrap();
        s.quiet = true;
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap();
        for (query, hits, excerpt, whole) in [
            ("who holds the lock", 2, 120, 3_000),
            ("nothing at all", 0, 40, 0),
        ] {
            semlith::store::record_retrieval(
                s.db(),
                &semlith::store::NewRetrieval {
                    client: "claude-code",
                    session: "s1",
                    tool: "search",
                    query,
                    hits,
                    micros: 5_000,
                    excerpt_tokens: excerpt,
                    whole_file_tokens: whole,
                    stale_hits: 0,
                    tokenizer: semlith::ledger::CHARS4,
                    query_id: query,
                },
            )
            .unwrap();
        }
    }
    let dirs: Vec<PathBuf> = vec![store.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    let text = semlith::report::generate(&fleet, "savings", "Sonnet 5")
        .unwrap()
        .render("markdown")
        .unwrap();

    for wanted in [
        "Whole files not read",
        "Excerpts read instead",
        "Refunds",
        "Net",
        "Coverage",
        "Tier",
        "Zero-hit",
        "Refund rate",
        "p50",
        "p95",
        "Ledger chain",
        "Sonnet 5",
    ] {
        assert!(
            text.contains(wanted),
            "the savings report is missing {wanted:?}"
        );
    }
    assert!(
        text.contains("never \nsummed")
            || text.contains("never summed")
            || text.contains("never\n                   summed"),
        "the report must say the counterfactuals are not summed: {text}"
    );

    // A different model is a different price on the same tokens, and the
    // report says which model it priced at rather than leaving a bare dollar
    // figure on the page.
    let dearer = semlith::report::generate(&fleet, "savings", "Opus 5")
        .unwrap()
        .render("markdown")
        .unwrap();
    assert!(dearer.contains("Opus 5"), "{dearer}");
    assert_ne!(
        text.lines().find(|l| l.contains("Net cost avoided")),
        dearer.lines().find(|l| l.contains("Net cost avoided")),
        "the same tokens cost the same at two different prices"
    );
}

/// An unknown report names the five that exist rather than failing blankly.
#[test]
fn an_unknown_report_names_the_ones_there_are() {
    let store = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_semlith"))
        .arg("--store")
        .arg(store.path())
        .args(["report", "team-rollup"])
        .output()
        .expect("the binary runs");
    assert!(!out.status.success());
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("savings"), "{said}");
    assert!(said.contains("gaps"), "{said}");
}
