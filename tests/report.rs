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
        .filter(|line| {
            !line.contains("Generated 2")
                && !line.contains("\"generated")
                && !line.contains("\"to\":")
        })
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
                client_version: "",
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
                    client_version: "",
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
    let cheaper = semlith::report::generate(&fleet, "savings", "Sonnet 5").unwrap();
    let dearer = semlith::report::generate(&fleet, "savings", "Opus 5").unwrap();
    assert!(dearer.render("markdown").unwrap().contains("Opus 5"));
    assert_ne!(
        cheaper.kpis[1].figure.value, dearer.kpis[1].figure.value,
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

// --------------------------------------------- the window and the scope

use semlith::report::{Block, Cell, Kind, Report, Table, Window};

/// A store with one recorded retrieval and nothing indexed.
///
/// No embedding model is downloaded: a report reads the ledger, and the ledger
/// is written by `record_retrieval` under its own write permission. That is
/// what lets these run in the default set rather than behind `--ignored`.
fn ledger_store(dir: &Path, session: &str, days_ago: i64) {
    let s = Semlith::open(dir, None).unwrap();
    semlith::store::record_retrieval(
        s.db(),
        &semlith::store::NewRetrieval {
            client_version: "",
            client: "claude-code",
            session,
            tool: "search",
            query: session,
            hits: 2,
            micros: 5_000,
            excerpt_tokens: 120,
            whole_file_tokens: 3_000,
            stale_hits: 0,
            tokenizer: semlith::ledger::CHARS4,
            query_id: session,
        },
    )
    .unwrap();
    if days_ago > 0 {
        // Backdated in place because `NewRetrieval` has no `at` — the ledger
        // stamps its own rows, which is the point of it. This breaks the row's
        // hash, so nothing below asserts the chain verifies.
        semlith::store::read_only(s.db(), false).unwrap();
        s.db()
            .execute("UPDATE retrievals SET at = at - ?1", [days_ago * 86_400])
            .unwrap();
        semlith::store::read_only(s.db(), true).unwrap();
    }
}

fn labels(fleet: &Fleet) -> Vec<String> {
    fleet.each().map(|(label, _)| label.to_string()).collect()
}

/// A caller that names neither gets the default window over every store.
#[test]
fn a_report_with_no_window_and_no_scope_is_the_default() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    ledger_store(one.path(), "recent", 0);
    ledger_store(two.path(), "older", 40);
    let dirs = vec![one.path().to_path_buf(), two.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();

    for (kind, _) in semlith::report::KINDS {
        let old = semlith::report::generate(&fleet, kind, "Sonnet 5").unwrap();
        let default = semlith::report::window_of(None).unwrap();
        let new = semlith::report::generate_over(&fleet, kind, "Sonnet 5", default, &[]).unwrap();
        assert_eq!(
            without_the_stamp(&old.render("markdown").unwrap()),
            without_the_stamp(&new.render("markdown").unwrap()),
            "{kind} changed when nothing was asked for"
        );
        assert_eq!(old.stores, labels(&fleet), "{kind} narrowed on its own");
    }
}

/// Every window value says something different, and the two reports whose
/// rows carry a date actually lose rows to it.
#[test]
fn each_window_changes_the_report() {
    let dir = tempfile::tempdir().unwrap();
    ledger_store(dir.path(), "recent", 0);
    {
        // A second session, old enough that only the widest windows hold it.
        let s = Semlith::open(dir.path(), None).unwrap();
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client_version: "",
                client: "cursor",
                session: "ancient",
                tool: "search",
                query: "ancient",
                hits: 1,
                micros: 1_000,
                excerpt_tokens: 10,
                whole_file_tokens: 100,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "ancient",
            },
        )
        .unwrap();
        semlith::store::read_only(s.db(), false).unwrap();
        s.db()
            .execute(
                "UPDATE retrievals SET at = at - ?1 WHERE query = 'ancient'",
                [40 * 86_400i64],
            )
            .unwrap();
        semlith::store::read_only(s.db(), true).unwrap();
    }
    let dirs = vec![dir.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();

    let audit = |window| {
        semlith::report::generate_over(&fleet, "access", "Sonnet 5", window, &[])
            .unwrap()
            .render("markdown")
            .unwrap()
    };
    // A day and a week hold the recent session and not the one forty days old;
    // a month and a quarter differ from each other only in what they say, and
    // `all` holds both.
    assert!(audit(Window::Days(1)).contains("recent"));
    assert!(!audit(Window::Days(1)).contains("ancient"));
    assert!(!audit(Window::Days(7)).contains("ancient"));
    assert!(audit(Window::Days(90)).contains("ancient"));
    assert!(audit(Window::All).contains("ancient"));

    // And every value is distinguishable in every report, including the three
    // that cannot narrow: a reader can tell from the document which was asked
    // for, rather than having to trust the caller.
    for (kind, _) in semlith::report::KINDS {
        let said: std::collections::BTreeSet<String> = semlith::report::WINDOWS
            .iter()
            .map(|(name, _)| {
                let window = semlith::report::window_of(Some(name)).unwrap();
                let report =
                    semlith::report::generate_over(&fleet, kind, "Sonnet 5", window, &[]).unwrap();
                report.window.label
            })
            .collect();
        assert_eq!(
            said.len(),
            semlith::report::WINDOWS.len(),
            "{kind} says the same thing about two different windows: {said:?}"
        );
    }
}

/// The scope is the store filter the route used to read and discard.
#[test]
fn each_scope_changes_the_report() {
    let one = tempfile::tempdir().unwrap();
    let two = tempfile::tempdir().unwrap();
    ledger_store(one.path(), "first-session", 0);
    ledger_store(two.path(), "second-session", 0);
    let dirs = vec![one.path().to_path_buf(), two.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    let open = labels(&fleet);
    assert_eq!(open.len(), 2, "two stores did not open: {open:?}");

    let audit = |only: &[String]| {
        semlith::report::generate_over(&fleet, "access", "Sonnet 5", Window::All, only).unwrap()
    };
    let both = audit(&[]);
    assert_eq!(both.stores, open);
    let both = both.render("markdown").unwrap();
    assert!(both.contains("first-session") && both.contains("second-session"));

    for (i, label) in open.iter().enumerate() {
        let narrowed = audit(std::slice::from_ref(label));
        assert_eq!(narrowed.stores, vec![label.clone()]);
        let text = narrowed.render("markdown").unwrap();
        let (mine, theirs) = if i == 0 {
            ("first-session", "second-session")
        } else {
            ("second-session", "first-session")
        };
        assert!(text.contains(mine), "{label} lost its own session");
        assert!(
            !text.contains(theirs),
            "{label} answered for the other store"
        );
        // The narrowing is printed rather than implied.
        assert!(text.contains(label), "the report does not name its scope");
    }

    // A store nobody opened is refused rather than dropped: a report headed
    // with one store when two were asked for is a figure with the wrong
    // subject, and nothing in the output would say so.
    let e = semlith::report::generate_over(
        &fleet,
        "access",
        "Sonnet 5",
        Window::All,
        &["not-a-store".to_string()],
    )
    .unwrap_err()
    .to_string();
    assert!(e.contains("not-a-store"), "{e}");
    assert!(
        e.contains(&open[0]),
        "{e} does not say what would have worked"
    );
}

// ------------------------------------------------------- the fifth format

/// A report long enough to need more than one page.
fn long_report() -> Report {
    let rows: Vec<Vec<Cell>> = (0..240)
        .map(|i| {
            vec![
                Cell::text(format!("2026-09-{:02} 10:00:00", (i % 28) + 1)),
                Cell::text(format!(
                    "/a/very/long/path/without/spaces/module-{i}/generated-source-file.rs"
                )),
                Cell::text("semlith"),
            ]
        })
        .collect();
    let column = |key: &'static str, label: &str, kind: Kind| semlith::report::Column {
        key,
        label: label.into(),
        kind,
    };
    let mut report = Report::new("change", "Change brief");
    report.stores = vec!["semlith".into()];
    report.window.label = "Last 7 days".into();
    report.kpis = vec![semlith::report::Kpi {
        label: "Files re-read".into(),
        figure: semlith::report::Figure::count(240, "files", "Last 7 days"),
        lead: true,
    }];
    report.blocks = vec![
        Block::Note {
            text: "Files this machine re-read, newest first — re-read, not rewritten.".into(),
        },
        Block::Table(Table {
            id: "re_read",
            title: "Re-read".into(),
            meta: "newest first".into(),
            columns: vec![
                column("when", "When", Kind::Id),
                column("file", "File", Kind::Id),
                column("store", "Store", Kind::Text),
            ],
            rows,
            total: None,
            more: 0,
            note: None,
            empty: "Nothing.".into(),
        }),
    ];
    report
}

/// The number of pages the document's own page tree claims.
fn page_count(pdf: &[u8]) -> usize {
    let text = String::from_utf8_lossy(pdf);
    let at = text.find("/Count ").expect("a page tree with a count");
    text[at + "/Count ".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .expect("a page count")
}

/// All five formats, one structure, and a PDF a person can actually read.
#[test]
fn all_five_formats_render_from_the_one_structure() {
    let report = long_report();
    for format in semlith::report::ALL_FORMATS {
        let bytes = report.render_bytes(format).unwrap();
        assert!(!bytes.is_empty(), "{format} rendered nothing");
        if format == semlith::report::PDF || format == semlith::report::CSV {
            continue;
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Change brief"), "{format} lost the title");
        assert!(text.contains("module-239"), "{format} lost the last row");
        assert!(text.contains("Last 7 days"), "{format} lost the window");
    }
    let csv = report.render_table("re_read").unwrap();
    assert!(csv.starts_with("when,file,store\n"), "{csv}");
    assert!(csv.contains("module-239"));
}

#[test]
fn the_pdf_is_valid_paginated_and_its_text_comes_back_out() {
    let pdf = long_report().render_bytes(semlith::report::PDF).unwrap();
    assert!(pdf.starts_with(b"%PDF-"), "no PDF header");
    assert!(
        String::from_utf8_lossy(&pdf).contains("%%EOF"),
        "no trailer"
    );

    let pages = page_count(&pdf);
    assert!(pages > 1, "240 rows fitted on {pages} page(s)");
    assert_eq!(
        String::from_utf8_lossy(&pdf)
            .matches("/Type /Page\n")
            .count(),
        pages,
        "the page tree's count is not the number of page objects"
    );

    // Read back with the extractor this binary already ships: a report semlith
    // writes has to be a report semlith can index.
    let text = pdf_extract::extract_text_from_mem(&pdf).expect("the PDF parses");
    for wanted in [
        "Change brief",
        "FILES RE-READ",
        "module-0/",
        "module-239/",
        "Re-read",
        "page 1 of",
    ] {
        assert!(
            text.contains(wanted),
            "the PDF does not say {wanted:?}:\n{}",
            &text[..text.len().min(1200)]
        );
    }
}

/// The builder's two content toggles change the document, not only the request.
///
/// The design draws three. Signing is not one of them here: it needs a key this
/// product has not decided anything about, so the page draws that switch
/// disabled and says what it is waiting for rather than offering a promise the
/// file would not keep.
#[test]
fn the_two_content_toggles_each_change_the_generated_document() {
    let dir = tempfile::tempdir().unwrap();
    // The session id and the query text are deliberately different strings.
    // `ledger_store` sets both from one argument, which would make this test
    // pass or fail on the session column rather than on the query one — and a
    // session id is an id, not something redaction promises to hide.
    {
        let s = Semlith::open(dir.path(), None).unwrap();
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client_version: "",
                client: "claude-code",
                session: "sess-1",
                tool: "search",
                query: "what the ledger records",
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
    let dirs = vec![dir.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();

    let audit = |options| {
        semlith::report::generate_with(&fleet, "access", "Sonnet 5", Window::All, &[], options)
            .unwrap()
            .render("markdown")
            .unwrap()
    };

    let salt = Some([7u8; 32]);
    let plain = audit(semlith::report::Options::default());
    let attached = audit(semlith::report::Options {
        excerpts: true,
        redact: false,
        salt,
    });
    let hidden = audit(semlith::report::Options {
        excerpts: true,
        redact: true,
        salt,
    });

    // Off is the 0.26.x document: the session totals and nothing under them.
    assert!(
        !plain.contains("## What each agent asked"),
        "an access report with no toggles set grew a Retrievals table:\n{plain}"
    );
    assert!(
        attached.contains("## What each agent asked"),
        "`Attach retrieved excerpts` did not attach anything:\n{attached}"
    );
    assert!(
        attached.contains("what the ledger records"),
        "the attachment does not carry the query it recorded:\n{attached}"
    );

    // Redaction is what it says: the query goes, everything around it stays.
    assert!(
        !hidden.contains("what the ledger records"),
        "`Hash the query text` left the query text in the document:\n{hidden}"
    );
    assert!(
        hidden.contains("hash:"),
        "the redacted document carries no digest where the query was:\n{hidden}"
    );
    for kept in ["claude-code", "search", "sess-1"] {
        assert!(
            hidden.contains(kept),
            "redaction dropped {kept:?}, which it promises to keep:\n{hidden}"
        );
    }

    // Wherever a query reaches paper, not only in the attachment. A toggle that
    // hid it in one table and printed it in another would be a promise broken
    // by the document that made it.
    let zero = tempfile::tempdir().unwrap();
    {
        let s = Semlith::open(zero.path(), None).unwrap();
        semlith::store::record_retrieval(
            s.db(),
            &semlith::store::NewRetrieval {
                client_version: "",
                client: "claude-code",
                session: "s",
                tool: "search",
                query: "a question nothing answers",
                hits: 0,
                micros: 1_000,
                excerpt_tokens: 0,
                whole_file_tokens: 0,
                stale_hits: 0,
                tokenizer: semlith::ledger::CHARS4,
                query_id: "zero",
            },
        )
        .unwrap();
    }
    let dirs = vec![zero.path().to_path_buf()];
    let fleet = Fleet::open(&dirs).unwrap();
    let gaps = |options| {
        semlith::report::generate_with(&fleet, "gaps", "Sonnet 5", Window::All, &[], options)
            .unwrap()
            .render("markdown")
            .unwrap()
    };
    assert!(
        gaps(semlith::report::Options::default()).contains("a question nothing answers"),
        "the gaps report stopped naming the questions that found nothing"
    );
    assert!(
        !gaps(semlith::report::Options {
            excerpts: false,
            redact: true,
            salt: Some([7u8; 32]),
        })
        .contains("a question nothing answers"),
        "redaction reaches the access report and not the gaps report, so a query text \
         survives in the second document a reader opens"
    );
}

// ------------------------------------------- 0.42.0: what the reports claim

/// One ledger row with every field named, backdated by `days_ago`.
#[allow(clippy::too_many_arguments)]
fn row(
    dir: &Path,
    days_ago: i64,
    client: &str,
    session: &str,
    tool: &str,
    query: &str,
    hits: i64,
    excerpt: i64,
    whole: i64,
    micros: i64,
) {
    let s = Semlith::open(dir, None).unwrap();
    semlith::store::record_retrieval(
        s.db(),
        &semlith::store::NewRetrieval {
            client_version: "1.0.0",
            client,
            session,
            tool,
            query,
            hits,
            micros,
            excerpt_tokens: excerpt,
            whole_file_tokens: whole,
            stale_hits: 0,
            tokenizer: semlith::ledger::MODEL,
            query_id: &format!("{session}:{query}:{days_ago}"),
        },
    )
    .unwrap();
    if days_ago > 0 {
        semlith::store::read_only(s.db(), false).unwrap();
        s.db()
            .execute(
                "UPDATE retrievals SET at = at - ?1 WHERE id = (SELECT MAX(id) FROM retrievals)",
                [days_ago * 86_400],
            )
            .unwrap();
        semlith::store::read_only(s.db(), true).unwrap();
    }
}

/// The audit's aged-store shape (R5): inside a week, one credited query and
/// one that found nothing; forty days back, a credited query, a refund and a
/// slow call. A savings report over a week counts the first two and nothing
/// else, on every line.
fn aged_store(dir: &Path) {
    row(
        dir,
        0,
        "claude-code",
        "s1",
        "search",
        "recent",
        2,
        120,
        3_000,
        5_000,
    );
    row(
        dir,
        0,
        "claude-code",
        "s1",
        "search",
        "nothing",
        0,
        0,
        0,
        7_000,
    );
    row(
        dir,
        40,
        "claude-code",
        "s0",
        "search",
        "ancient",
        1,
        50,
        9_000,
        900_000,
    );
    let refund = semlith::ledger::RAW_READ;
    row(
        dir,
        40,
        "claude-code",
        "s0",
        refund,
        "src/old.rs",
        0,
        0,
        4_000,
        0,
    );
}

/// A headline or secondary figure's value, by label.
fn figure(json: &serde_json::Value, label: &str) -> serde_json::Value {
    let kpi = json["kpis"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["label"] == label);
    let fact = json["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|b| b["facts"].as_array().cloned().unwrap_or_default())
        .find(|f| f["label"] == label);
    kpi.cloned()
        .or(fact)
        .unwrap_or_else(|| panic!("no figure {label} in {json}"))["value"]
        .clone()
}

/// One cell of a table, by the table's id, the row and the column's key.
fn cell(json: &serde_json::Value, table: &str, row: usize, key: &str) -> serde_json::Value {
    let t = json["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|b| b["id"] == table)
        .unwrap_or_else(|| panic!("no table {table} in {json}"));
    let at = t["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c["key"] == key)
        .unwrap_or_else(|| panic!("no column {key} in {t}"));
    t["rows"][row][at].clone()
}

#[test]
fn a_windowed_savings_report_counts_only_rows_inside_the_window() {
    let dir = tempfile::tempdir().unwrap();
    aged_store(dir.path());
    let fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let report =
        semlith::report::generate_over(&fleet, "savings", "Sonnet 5", Window::Days(7), &[])
            .unwrap();
    let json: serde_json::Value = serde_json::to_value(&report).unwrap();
    // Whole less excerpt is net, on the rows the window holds: 3 000 - 120.
    assert_eq!(cell(&json, "not_read", 0, "tokens"), 3000, "{json}");
    assert_eq!(cell(&json, "not_read", 1, "tokens"), 120, "{json}");
    assert_eq!(figure(&json, "Net tokens not read"), 2880, "{json}");
    assert_eq!(
        figure(&json, "Refunds"),
        0,
        "the refund is 40 days old: {json}"
    );
    assert_eq!(figure(&json, "Zero-hit"), 1, "{json}");
    // The 900 ms call is outside the window, so it is not the p95.
    assert_eq!(figure(&json, "Latency p95"), 7.0, "{json}");
}

#[test]
fn a_windowed_access_report_counts_rows_by_their_own_time() {
    // One session, one row today and one three days ago: a day's window holds
    // one read, not the session's lifetime two (R4, L12).
    let dir = tempfile::tempdir().unwrap();
    row(
        dir.path(),
        0,
        "claude-code",
        "s1",
        "search",
        "today",
        1,
        10,
        100,
        1_000,
    );
    row(
        dir.path(),
        3,
        "claude-code",
        "s1",
        "search",
        "earlier",
        1,
        10,
        900,
        1_000,
    );
    let fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let report =
        semlith::report::generate_over(&fleet, "access", "Sonnet 5", Window::Days(1), &[]).unwrap();
    let json: serde_json::Value = serde_json::to_value(&report).unwrap();
    assert_eq!(cell(&json, "sessions", 0, "queries"), 1, "{json}");
    assert_eq!(cell(&json, "sessions", 0, "net_tokens"), 90, "{json}");
}

#[test]
fn a_redacted_query_is_not_its_unsalted_digest() {
    // `detect_binary` was reversed by guessing (R7): an unkeyed digest of a
    // symbol name is a dictionary lookup away from the name.
    let dir = tempfile::tempdir().unwrap();
    row(
        dir.path(),
        0,
        "claude-code",
        "s1",
        "search",
        "detect_binary",
        0,
        0,
        0,
        1_000,
    );
    let fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let options = semlith::report::Options {
        excerpts: true,
        redact: true,
        salt: Some([9u8; 32]),
    };
    let text =
        semlith::report::generate_with(&fleet, "access", "Sonnet 5", Window::All, &[], options)
            .unwrap()
            .render("json")
            .unwrap();
    let unsalted = blake3::hash(b"detect_binary").to_hex()[..16].to_string();
    assert!(!text.contains("detect_binary"), "{text}");
    assert!(
        !text.contains(&unsalted),
        "the digest is the unsalted blake3 of the query: {text}"
    );
}

#[test]
fn the_change_brief_counts_every_file_and_a_first_index_is_not_a_re_read() {
    // 476 = 200 + 200 + 58 + 14 + 4 per-store caps over 511 files (R3).
    let dir = tempfile::tempdir().unwrap();
    {
        let s = Semlith::open(dir.path(), None).unwrap();
        semlith::store::read_only(s.db(), false).unwrap();
        for i in 0..250 {
            s.db()
                .execute(
                    "INSERT INTO files (path, hash, bytes, indexed_at) \
                     VALUES (?1, 'h', 10, CAST(strftime('%s','now') AS INTEGER))",
                    [format!("/corpus/f{i}.md")],
                )
                .unwrap();
        }
        semlith::store::read_only(s.db(), true).unwrap();
    }
    let fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let report =
        semlith::report::generate_over(&fleet, "change", "Sonnet 5", Window::Days(7), &[]).unwrap();
    let json: serde_json::Value = serde_json::to_value(&report).unwrap();
    assert_eq!(
        figure(&json, "Files re-read"),
        0,
        "250 files indexed for the first time are not re-reads: {json}"
    );
    assert!(
        report
            .lede_plain()
            .contains("250 files read for the first time"),
        "{}",
        report.lede_plain()
    );
}

#[test]
fn a_figure_prints_with_the_portals_grouping() {
    // The portal prints 736,397; the reports printed 736 397.
    let dir = tempfile::tempdir().unwrap();
    row(
        dir.path(),
        0,
        "claude-code",
        "s1",
        "search",
        "q",
        1,
        0,
        736_397,
        1_000,
    );
    let fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let text = semlith::report::generate_over(&fleet, "savings", "Sonnet 5", Window::All, &[])
        .unwrap()
        .render("markdown")
        .unwrap();
    assert!(text.contains("736,397"), "{text}");
    assert!(!text.contains("736 397"), "{text}");
}

#[test]
fn csv_and_json_carry_numbers_as_numbers() {
    let dir = tempfile::tempdir().unwrap();
    row(
        dir.path(),
        0,
        "claude-code",
        "s1",
        "search",
        "q",
        1,
        3,
        736_397,
        1_000,
    );
    let fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let report =
        semlith::report::generate_over(&fleet, "savings", "Sonnet 5", Window::All, &[]).unwrap();
    let json: serde_json::Value = serde_json::from_str(&report.render("json").unwrap()).unwrap();
    assert_eq!(json["schema"], "semlith.report/2", "{json}");
    // One table per file: the zip holds summary.csv and each table, and every
    // file has one header row.
    let zip = report.render_bytes("csv").unwrap();
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
    let names: Vec<String> = archive.file_names().map(str::to_string).collect();
    for wanted in [
        "savings-summary.csv",
        "savings-not_read.csv",
        "savings-by_agent.csv",
    ] {
        assert!(names.iter().any(|n| n == wanted), "{names:?}");
    }
    let mut summary = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_name("savings-summary.csv").unwrap(),
        &mut summary,
    )
    .unwrap();
    assert_eq!(summary, report.render_table("summary").unwrap());
    assert!(
        summary
            .lines()
            .any(|l| l.split(',').any(|cell| cell == "736394")),
        "no unformatted net figure in the CSV:\n{summary}"
    );
    let not_read = report.render_table("not_read").unwrap();
    assert!(
        not_read.starts_with("line,tokens,cost_usd,counts\n"),
        "{not_read}"
    );
    assert!(
        not_read.contains("Whole files not read,736397,"),
        "{not_read}"
    );
}

#[test]
fn the_pdf_text_layer_is_the_text() {
    // "Résumé café 東京" printed "R?sum? caf? ??" in 0.41.
    let mut report = long_report();
    report.blocks.insert(
        0,
        Block::Note {
            text: "Résumé café 東京".into(),
        },
    );
    let pdf = report.render_bytes(semlith::report::PDF).unwrap();
    let text = pdf_extract::extract_text_from_mem(&pdf).expect("the PDF parses");
    assert!(
        text.contains("Résumé café 東京"),
        "{}",
        &text[..text.len().min(400)]
    );
}

#[test]
fn the_cli_and_the_portal_share_one_default_window() {
    // The portal opened at 30 days and the CLI at everything (R11).
    assert_eq!(
        semlith::report::window_of(None).unwrap(),
        Window::Days(30),
        "no window named is not the portal's default"
    );
}

#[test]
fn semlith_report_takes_the_window_scope_and_options() {
    // semlith_report took no window or scope, so "the same document over
    // MCP" held only at the defaults (R11).
    let dir = tempfile::tempdir().unwrap();
    aged_store(dir.path());
    let mut fleet = Fleet::open(&[dir.path().to_path_buf()]).unwrap();
    let label = labels(&fleet)[0].clone();
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "semlith_report", "arguments": {
            "kind": "savings", "format": "json", "window": "week", "scope": [label],
        }}
    });
    let mut session = semlith::mcp::Session::new("t");
    let reply = semlith::mcp::answer(&mut fleet, None, &call, &mut session).unwrap();
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    let json: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|_| panic!("{reply}"));
    assert_eq!(json["window"]["name"], "week", "{json}");
    assert_eq!(figure(&json, "Net tokens not read"), 2880, "{json}");
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": { "name": "semlith_report", "arguments": {
            "kind": "savings", "format": "csv", "table": "not_read", "window": "all",
        }}
    });
    let reply = semlith::mcp::answer(&mut fleet, None, &call, &mut session).unwrap();
    let text = reply["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("line,tokens,cost_usd,counts\n"), "{text}");
}

#[test]
fn the_pdf_embeds_plex_and_is_tagged_titled_and_bookmarked() {
    // 0.41: base-14 Courier, not embedded, US Letter, no page numbers, no
    // metadata, no tags (R2, item 16).
    let pdf = long_report().render_bytes(semlith::report::PDF).unwrap();
    let raw = String::from_utf8_lossy(&pdf);
    assert!(!raw.contains("/Courier"), "a base-14 face");
    for wanted in [
        "IBMPlexSans",
        "IBMPlexMono",
        "/FontFile2",
        "/ToUnicode",
        "/Identity-H",
        "/StructTreeRoot",
        "/ParentTree",
        "/MarkInfo",
        "/Outlines",
        "/Title ",
        "/Author (Semlith)",
        "/Producer (semlith ",
        "/CreationDate",
        "/Lang (en)",
    ] {
        assert!(raw.contains(wanted), "the PDF lacks {wanted}");
    }
    // Every font is a subset: a six-letter tag before the name.
    let tagged = raw.match_indices("/BaseFont /").all(|(at, _)| {
        raw[at + 11..]
            .chars()
            .take(7)
            .collect::<String>()
            .ends_with('+')
    });
    assert!(tagged, "a font is embedded whole rather than as a subset");
    let text = pdf_extract::extract_text_from_mem(&pdf).unwrap();
    let pages = page_count(&pdf);
    assert!(
        text.contains(&format!("page {pages} of {pages}")),
        "no page x of y"
    );
}
