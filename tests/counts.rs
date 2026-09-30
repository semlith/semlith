//! What 0.34.0 promises about `Inside the index`: PDF pages, slides,
//! spreadsheet cells and notebook cells are counted as each file is read, a
//! store written by an older binary gains its counts on the next index pass
//! without anything being embedded again, and a PDF's text is unchanged by
//! the counting.
//!
//! The expected figures below were taken from the fixtures by Python's
//! `zipfile` and `json`, not by semlith:
//!
//! ```sh
//! cargo test --test counts
//! cargo test --test counts -- --ignored   # the index round trip
//! ```

use pdf_extract::content::{Content, Operation};
use pdf_extract::{Document, Object, Stream, dictionary};
use semlith::chunk;
use std::fs;
use std::path::{Path, PathBuf};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// A PDF of `pages` pages, each carrying `page <n>`, written by lopdf.
fn pdf(pages: usize) -> Vec<u8> {
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let font = doc.add_object(dictionary! {
        "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
    });
    let resources = doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font } });
    let mut kids: Vec<Object> = Vec::new();
    for n in 1..=pages {
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 24.into()]),
                Operation::new("Td", vec![72.into(), 720.into()]),
                Operation::new("Tj", vec![Object::string_literal(format!("page {n}"))]),
                Operation::new("ET", vec![]),
            ],
        };
        let stream = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page = doc.add_object(dictionary! {
            "Type" => "Page", "Parent" => pages_id, "Contents" => stream,
        });
        kids.push(page.into());
    }
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => pages as i64,
            "Resources" => resources,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
        }),
    );
    let catalog = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
    doc.trailer.set("Root", catalog);
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

fn units_of(name: &str, bytes: &[u8]) -> Option<i64> {
    chunk::extract_counted(Path::new(name), bytes)
        .unwrap_or_else(|why| panic!("{name} was skipped: {}", why.as_str()))
        .1
}

const CSV: &str = "name,amount,note\nalpha,1,\"a, quoted\"\nbeta,,\n";

#[test]
fn every_counted_format_counts_what_the_file_holds() {
    let expected = [
        ("deck.pptx", 12),
        ("deck.odp", 3),
        ("sheet.xlsx", 18),
        ("sheet.ods", 8),
        ("analysis.ipynb", 3),
    ];
    for (name, units) in expected {
        let bytes = fs::read(fixtures().join(name)).unwrap();
        assert_eq!(units_of(name, &bytes), Some(units), "{name}");
    }
    assert_eq!(units_of("book.pdf", &pdf(3)), Some(3));
    assert_eq!(units_of("t.csv", CSV.as_bytes()), Some(7));
    assert_eq!(units_of("t.tsv", b"a\tb\n\tc\n"), Some(3));
    // A format with no unit has no count, and says so rather than zero.
    let docx = fs::read(fixtures().join("notes.docx")).unwrap();
    assert_eq!(units_of("notes.docx", &docx), None);
    assert_eq!(units_of("main.rs", b"fn main() {}\n"), None);
}

#[test]
fn counting_pages_leaves_a_pdfs_text_as_it_was() {
    for pages in [1, 2, 7] {
        let bytes = pdf(pages);
        let before = pdf_extract::extract_text_from_mem(&bytes).unwrap();
        let after = chunk::extract(Path::new("x.pdf"), &bytes).unwrap();
        assert_eq!(after, before, "{pages} pages");
        assert!(after.contains(&format!("page {pages}")));
    }
}

/// The upgrade path: files counted on first index; the counts removed, as a
/// store an older binary wrote has none; one more pass puts them back with
/// every chunk, vector and FTS row exactly where it was.
#[test]
#[ignore = "downloads an embedding model on first run"]
fn an_older_store_gains_its_counts_without_re_embedding() {
    let corpus = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    for name in [
        "deck.pptx",
        "deck.odp",
        "sheet.xlsx",
        "sheet.ods",
        "analysis.ipynb",
    ] {
        fs::copy(fixtures().join(name), corpus.path().join(name)).unwrap();
    }
    fs::write(corpus.path().join("book.pdf"), pdf(4)).unwrap();
    fs::write(corpus.path().join("table.csv"), CSV).unwrap();
    fs::write(corpus.path().join("notes.md"), "# Notes\n\nplain prose\n").unwrap();

    let index = |s: &mut semlith::Semlith| {
        s.index_paths(&[corpus.path().to_path_buf()], |_, _| {})
            .unwrap()
    };
    let mut s = semlith::Semlith::open(store.path(), None).unwrap();
    s.quiet = true;
    let first = index(&mut s);
    assert_eq!(first.indexed, 8, "{first:?}");

    let db = rusqlite::Connection::open(store.path().join("store.db")).unwrap();
    let kinds = |db: &rusqlite::Connection| {
        let corpus = semlith::store::corpus(db, |_| String::new()).unwrap();
        let mut rows: Vec<(String, i64, i64, i64)> = corpus
            .kinds
            .into_iter()
            .map(|k| (k.name, k.count, k.units, k.uncounted))
            .collect();
        rows.sort();
        rows
    };
    let counted = vec![
        ("Notebook".to_string(), 1, 3, 0),
        ("PDF".to_string(), 1, 4, 0),
        ("Slide deck".to_string(), 2, 15, 0),
        ("Spreadsheet".to_string(), 3, 33, 0),
        ("Text and code".to_string(), 1, 0, 0),
    ];
    assert_eq!(kinds(&db), counted);

    let snapshot = |db: &rusqlite::Connection| -> (Vec<i64>, i64) {
        let mut q = db.prepare("SELECT id FROM chunks ORDER BY id").unwrap();
        let ids = q
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<i64>, _>>()
            .unwrap();
        let fts: i64 = db
            .query_row("SELECT COUNT(*) FROM chunks_fts", [], |r| r.get(0))
            .unwrap();
        (ids, fts)
    };
    let before = snapshot(&db);

    // What a 0.33.1 store looks like: the column exists (added on open) and
    // holds nothing.
    db.execute("UPDATE files SET units = NULL", []).unwrap();
    let waiting = kinds(&db);
    assert!(
        waiting
            .iter()
            .filter(|k| k.0 != "Text and code")
            .all(|k| k.3 == k.1 && k.2 == 0),
        "{waiting:?}"
    );

    // Planned first, as the daemon and the portal run it: the scan phase's
    // hashes are what let an unchanged file skip being read at all.
    s.plan(&[corpus.path().to_path_buf()]).unwrap();
    let again = index(&mut s);
    assert_eq!(again.indexed, 0, "a count re-embedded a file: {again:?}");
    assert_eq!(kinds(&db), counted);
    assert_eq!(snapshot(&db), before, "chunks or FTS rows moved");
}
